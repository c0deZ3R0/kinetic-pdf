# Experimental Rust native-key broker

Status: **isolated Windows feasibility checks passed after coordinator CPU handoff.**
Only `experiments/signature-key-broker/` is involved; its manifest has `[workspace]`
and no relationship to the production dependency graph. No root Cargo files changed.
Its own generated `Cargo.lock` pins 33 dependency packages and is intentionally retained.
No binaries, keys, PDFs or caches are placed in the repository.

## Question and bounded answer

Can a portable PDF engine request a precisely specified signing operation from Rust
without receiving a private key? The prototype provides a platform-neutral trait and
versioned stdio contract, with **only** a Windows CNG adapter. This is not a PDF/CMS,
PAdES, certificate-trust or revision validator; no backend selection follows from it.
The [pyHanko spike](signature-backend-qualification.md) supplies read-only helpers
for the optional Python test-oracle bridge below, not a shipping dependency.
No existing Python author/production files are modified.

## Key lifecycle: safety basis before running

Microsoft explicitly documents that `NCryptCreatePersistedKey` with **NULL
`pszKeyName` creates an ephemeral key that is not persisted** [1]. The function's
name alone is misleading; the NULL argument is essential.

- Fixed `MS_KEY_STORAGE_PROVIDER` (Microsoft Software KSP), RSA-2048, NULL name,
  zero creation/legacy flags. No named key, machine, overwrite, VBS/per-boot flags.
- Set length 2048, signing-only usage, export policy zero; finalize silently without
  `NCRYPT_WRITE_KEY_TO_LEGACY_STORE_FLAG`. Read back properties; fail if unsupported.
- Export **only** `BCRYPT_RSAPUBLIC_BLOB`, checked for public magic, zero private-prime
  lengths, exact bounded shape. Convert public n/e to DER SPKI in Rust. No private,
  opaque or archival export call exists, including negative-test probes.
- RAII frees key before provider with `NCryptFreeObject`; no persisted-key deletion
  operation is needed. Failure does not trigger a named-key/export workaround.
- Never call `NCryptOpenKey`, enumeration, certificate-store/import or user-identity
  APIs. The adapter has no provider/key-name/path parameters and creates no certificates.
  The optional Python bridge issues synthetic certificates only; no Windows store import.
- This relies on Microsoft's documented software-provider contract, not independent
  filesystem forensics or hardware attestation. It does not guarantee physical RAM
  residency, pagefile/dump exclusion or secure erasure. If the API/policy fails, stop.

## Exact portable contract

`TestKeyBroker::public_key() -> &PublicKey` exposes DER SPKI and its lowercase
SHA-256 hex key ID. `sign_sha256_prehashed(&[u8; 32]) -> Result<Vec<u8>, CryptoError>`
uses RSA PKCS#1 v1.5 with SHA-256 DigestInfo. CNG receives the digest, not the message.
The public DER encoder is not crypto; independent OpenSSL decoding passed below.

Requests are UTF-8 JSON lines, with unknown/duplicate fields rejected by serde:

```json
{"version":1,"operation_id":"pub-1","body":{"kind":"public_key"}}
{"version":1,"operation_id":"sig-1","body":{"kind":"sign","expected_key_id":"<SPKI SHA256 hex>","algorithm":"rsa_pkcs1_sha256","input":{"kind":"message","bytes_b64":"YWJj"}}}
{"version":1,"operation_id":"sig-2","body":{"kind":"sign","expected_key_id":"<same ID>","algorithm":"rsa_pkcs1_sha256","input":{"kind":"sha256_prehashed","digest_b64":"<exactly 32 bytes, base64>"}}}
```

- Response has `version`, correlated `operation_id`, and `body.kind` = `public_key`,
  `signature` or `error`. Public/signature responses include algorithm, key ID,
  `spki_der_b64`/`signature_b64`, and `test_only=true`; never private key/handle data.
- `message` hashes decoded bytes **once**; `sha256_prehashed` validates length and
  does **not** hash again. No implicit digest/message guessing or algorithm defaults.
- Maximum frame 16 KiB, decoded message 4096 bytes, operation ID 1–64 ASCII token
  characters, 128 frames/operations per process. Incremental frame reading is bounded;
  oversized frames close the session. Standard base64 only. IDs are consumed when an
  envelope is admitted, even if the operation subsequently fails; duplicates refused.
- Sign requests must match this process's public key ID. Errors are controlled codes,
  not parser/native input dumps. No arguments, arbitrary command paths, network,
  provider selection or message logging. Key initialization happens on explicit launch.
- Stdio is **not authenticated IPC**, user consent or a production capability boundary.
  Anyone controlling this experiment's stdin can request signatures by its synthetic
  test key. Never adapt that policy directly to real user keys.
- For future PDF integration, sign DER CMS signed attributes (or their explicit SHA-256
  prehash), not the PDF digest in place of signed attributes. Document/revision/consent
  binding and CMS/PDF policy remain the engine/transaction layer's responsibility.

## Executed verification (Windows x64, 2026-11-13)

After explicit CPU handoff, these Git Bash commands were run (not the root workspace):

```bash
target=$(mktemp -d /tmp/kinetic-native-broker-build.XXXXXX)
cargo generate-lockfile --manifest-path experiments/signature-key-broker/Cargo.toml
cargo test --manifest-path experiments/signature-key-broker/Cargo.toml --locked \
  --target-dir "$(cygpath -m "$target")" -j2 -- --test-threads=1
```

The target was the OS-temp directory `kinetic-native-broker-build.pw4ggq`; the final
rerun used its same absolute path. Cargo/rustc **1.100.0**, OpenSSL **3.5.7**.
Initial build succeeded but a unit test caught serde accepting unknown fields on a
unit `public_key` variant. It was changed to an empty struct variant; the test then
passed. The final rerun passed **5 unit tests + 1 Windows/OpenSSL integration test**,
with no failures. The native test took ~0.15 s; this is not a benchmark.

- Track only the experiment's lockfile. Build output goes to OS temp; local `/target/`
  is also ignored by the experiment. No binary, key, PDF or cache belongs in the repo.
- Pure tests cover protocol shape, IDs/replay, limits, message/prehash equivalence,
  public DER lengths/sign encoding and refusal of private/malformed blobs.
- The Windows integration test generated **one unnamed test key**. Property readback
  confirmed RSA-2048, signing-only use and zero export-policy flags. Message and prehash
  modes returned identical 256-byte signatures; OpenSSL decoded the public SPKI and
  verified the correct message. Wrong-message and mutated-signature checks failed.
- A genuine CNG signature over a deliberately wrong digest failed verification for
  the original message, but OpenSSL `pkeyutl` verified it against exactly that wrong
  digest with `rsa_padding_mode:pkcs1` and `digest:sha256`. This checks the prehash
  boundary, not merely corrupt-signature rejection.
- Version/shape/ID/input bounds, replay and wrong-key checks passed. The framed service
  returned errors for unknown algorithms and oversized frames; no unsupported algorithm
  reaches CNG. The test used the real service over in-memory I/O, not an external
  interactive stdio client. Authentication/process supervision remain unqualified.
- OpenSSL verification files contain only public data and a synthetic message in an
  OS temporary directory. They are removed by `tempfile`; the private key never leaves
  CNG. Do not run this test during performance measurement. Crash cleanup is not proven.
- No fake macOS/Linux implementation. Portable tests may eventually compile there;
  the executable currently exits unsupported without a Windows adapter.

## Optional Python TEST-ORACLE bridge (executed)

`bridge_pyhanko.py` connects the real executable through stdin/stdout pipes. **PDF/CMS
assembly is PYTHON TEST ORACLE, not our Rust PDF signing engine.** Existing pyHanko
scripts/lock/provenance helpers are imported read-only, with bytecode writing disabled.
No Rust rebuild or dependency change was needed for this follow-up.

Actual commands (Git Bash, Python **3.12.15**, pyHanko **0.38.1**, OpenSSL **3.5.7**):

```bash
uv run --no-project --isolated --python 3.12.15 python -B -m unittest discover \
  -s experiments/signature-key-broker/tests -p 'test_bridge.py' -v
uv run --no-project --isolated --python 3.12.15 python -B \
  experiments/signature-key-broker/bridge_pyhanko.py --broker \
  'C:/Users/flowcost-home/AppData/Local/Temp/kinetic-native-broker-build.pw4ggq/debug/signature-key-broker-experiment.exe'
```

The launcher reuses the official versioned-PyPI/source provenance checks and all 22
hash-locked oracle dependencies in a fresh private OS-temp uv venv. It copies the
already-built executable into that private tree and checks its hash again before launch.
Recorded provenance (measurement, not signed-artifact/reproducible-build attestation):

```text
Broker executable SHA-256:
b81daa43d656c4c2de2a2e40dddf9b13d54f70fe8e9237a4c3f5497955078c06
Experiment Cargo.lock SHA-256:
cf0f3ff4057532910eeb32fb03d23286b4260b94fd37ffd269711ba245db49c5
```

Results: **4 bridge transport tests passed; end-to-end command exited 0.**

- One broker process/key supplies PUBLIC SPKI. Python generates a synthetic RSA root
  and issues a leaf certificate for exactly that public key, without any Windows
  certificate-store import. Only the synthetic Python root key is serialized, inside
  the ACL-private temp directory; the Windows private key never leaves CNG.
- A custom pyHanko signer receives DER CMS signed attributes, checks their form,
  hashes once and sends explicit `sha256_prehashed` input to Rust. Each returned raw
  RSA signature is verified with that SPKI and PyCA `Prehashed(SHA256)` **before**
  returning it for embedding. Two reservation callbacks return dummy 256-byte buffers
  without IPC. Total: **3 broker requests = 1 public-key + 2 actual signing requests**.
- Approval B-B (8,736 bytes) and **fresh unsigned-base** P=1 certification B-B (8,883
  bytes) passed. Approval has no `/Perms`; certification catalog references the actual
  signature with `/P=1`. Each original base remains an exact prefix. ByteRange covers
  the exact final revision, excluding only hex Contents containing CMS plus zero padding.
- OpenSSL independently accepted both detached CMS signatures and rejected **2 wrong
  contents + 2 changed CMS signatures**. It uses `cms -verify -binary -noverify`:
  crypto integrity only, not trust. Validation with the synthetic root passed; identical
  CMS with explicit empty roots remained intact/valid but untrusted. Revocation is
  **soft-fail**, not qualified; no real identity, timestamp or LTV claim.
- Parent checks bounded 4 KiB replies, version/operation/key IDs, allowed fields and
  strict base64. Send/read waits have 10-second timeouts; cleanup closes stdin and
  waits, then kills only its own Popen child if required. Tests cover mismatches,
  duplicate/oversized frames, empty-channel timeout and mocked own-child kill fallback.
  **Normal-success evidence only:** actual broker exited **0 after EOF**; venv,
  synthetic keys, PDFs and verification data were removed before
  `temp_cleanup=completed` was reported. This does not qualify forced-timeout cleanup.
- **Production IPC blocker:** the launcher's outer 180-second timeout kills the
  Python worker, bypassing its `finally`. Descendant broker termination and ephemeral
  key release are not strictly supervised/bounded; a surviving broker may retain its
  unnamed ephemeral key and copied executable, and Windows temp cleanup may fail.
  No such leak was observed, and no forced-timeout experiment was performed. There
  are still no persistent/named or user keys involved. Explicit descendant lifecycle
  supervision needs separate qualification before production IPC use.
- The report code now declares `forced_timeout_cleanup_qualified=false` with
  `cleanup_scope` limited to normal success. These are static unperformed-status
  declarations added after the recorded run; **no crypto/timeout tests were rerun**.
- No TCP signing transport or wire-controlled executable/file paths. Python's Windows
  asyncio implementation may use internal loopback wakeup sockets. No external fetch
  is enabled for signature validation. No real hung-native-process, forced-parent-death,
  malicious helper, authenticated IPC, external PAdES profile-validator or hostile-PDF
  qualification was performed. Crash secure erasure/process-tree cleanup is not proven.

## Release position / next decision

**Native-key-to-PDF test boundary demonstrated; production backend still undecided.**
Rust owns the bounded CNG key operation without exporting a private key; Python alone
assembles the fixture PDF/CMS. This does not qualify a complete Rust PDF/CMS/PAdES/
revision-validation engine. Real identities/hardware/PKCS#11, cancellation, authenticated
transport, real certificate policy, other RSA padding/EC algorithms, Rust PDF/CMS
assembly and hostile-input/sandbox qualification remain open. The app's unsigned
startup/view/save performance target is not measured by either experiment.

Sources (official documentation inspected before writing adapter):
[1] [CreatePersistedKey: NULL means ephemeral](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptcreatepersistedkey),
[2] [FinalizeKey flags](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptfinalizekey),
[3] [FreeObject ownership](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptfreeobject),
[4] [SignHash prehash/padding](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptsignhash),
[5] [public export](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptexportkey),
[6] [RSA public blob layout](https://learn.microsoft.com/en-us/windows/win32/api/bcrypt/ns-bcrypt-bcrypt_rsakey_blob),
[7] [key-storage properties](https://learn.microsoft.com/en-us/windows/win32/seccng/key-storage-property-identifiers).
