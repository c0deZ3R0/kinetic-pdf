# Signature interoperability: Bluebeam samples

## Scope and result

Independent, read-only inspection of the user-supplied Desktop `Signed.pdf` and
`Signed_marked.pdf`. No samples, identities, or raw certificate dumps were added
to the repository. No packages were downloaded. Only the new developer scripts
under `scripts/signatures/` and this document belong to this investigation.

**Result:** the marked file preserves the entire original file byte-for-byte.
Its original detached CMS signature still verifies cryptographically, but does
**not** sign the appended markup. Neither sample is a DocMDP certification, and
neither contains an RFC 3161 timestamp. This is not a certificate-trust verdict,
a comprehensive PDF security assessment, or an observed Bluebeam UI verdict.

## Reproduce

Run from the repository root, using already-installed Node and OpenSSL:

```powershell
node --version
openssl version
node --test scripts/signatures/inspect.test.mjs
node scripts/signatures/inspect.mjs "$env:USERPROFILE/Desktop/Signed.pdf" "$env:USERPROFILE/Desktop/Signed_marked.pdf"
```

Commands actually run here (Git Bash):

```bash
node --version
openssl version
node --test scripts/signatures/inspect.test.mjs
node scripts/signatures/inspect.mjs \
  'C:/Users/flowcost-home/Desktop/Signed.pdf' \
  'C:/Users/flowcost-home/Desktop/Signed_marked.pdf'
```

- Node `v24.19.0`; OpenSSL `3.5.7` (library and executable).
- Final unit run: **30 tests passed**, no failures. Tests use synthetic bytes,
  not private sample fixtures. An initial test had an incorrect hard-coded
  synthetic offset; it was corrected to derive the offset from the fixture.
- Both sample CMS checks passed, including four negative controls total:
  changed signed content and changed CMS rejected for each sample.
- `OPENSSL` may select a different locally installed executable. No npm install,
  Python environment, `uv` download, application build, or shipping dependency
  is needed. `qpdf` and `pdfsig` were not available on PATH here.

### What the optional probe does

`inspect.mjs` opens input PDFs only for reading. It validates one literal
four-integer ByteRange before slicing, checks that the excluded gap is exactly
a literal hex `/Contents` value, removes only zero padding after a definite-length
DER SEQUENCE, and reconstructs the exact two signed byte segments. It compares
actual bytes, not just hashes. A maximum 64 MiB input size and a 30-second timeout
per OpenSSL invocation bound this small developer tool; these are not production
resource-safety guarantees.

It uses a unique OS temporary directory for extracted CMS, signed content, and
OpenSSL output, and deletes it in `finally`. Those files can contain private
PDF content and public certificates; use a private local temp directory. Node
file modes are not a Windows ACL guarantee, and forced termination/crashes may
leave the directory behind. Nothing is written beside the samples or to the repo.
It prints no certificate subject, serial number, author, signature name, or raw
PDF/CMS dump. Raw OpenSSL diagnostics are kept internal.

For each sample the effective positive verification command is:

```text
openssl cms -verify -binary -inform DER -in signature.der \
  -content signed-content.bin -noverify -out verified.bin
```

The output must exactly equal the reconstructed signed content. `-binary`
prevents newline canonicalization; **`-noverify` skips certificate trust/chain
validation, not the CMS signature/content check**. The probe does not pass
`-nosigs`, `-no_content_verify`, or `-no_attr_verify`. It also invokes
`openssl cms -cmsout -print -inform DER` internally to summarize algorithms and
attributes without exposing identities. Changing the first signed content byte,
then separately the last unpadded CMS byte, must make verification fail. These
mutations affect temporary buffers/files only, never the samples.

## Observations and evidence

### Byte preservation and revisions

| Observation | `Signed.pdf` | `Signed_marked.pdf` |
| --- | --- | --- |
| File size | 5,056 bytes | 8,100 bytes |
| Literal ByteRange | `[0 788 2544 2512]` | Same |
| Signed segments (half-open) | `[0,788)` + `[2544,5056)` | Same |
| Excluded Contents gap | `[788,2544)` (1,756 bytes) | Same |
| Bytes after signed revision | 0 | 3,044 |
| CMS DER size (without padding) | 868 bytes | Same |
| CMS integrity, no trust check | PASS | PASS |

```text
Signed.pdf SHA-256:
6ac1f03870639c343b884f12e56897194bb8887b4a156e271bca7f90f9f6da50
Signed_marked.pdf SHA-256:
89a20898b5c0d07bb1dcbc5bb4375ba1fc6d7b9bd9f47dec71498a0cd5a4084c
Concatenated signed-content SHA-256 (both):
40538d09fafaa45ac67eb973ddbd41c7ce880f2a9413255a04610f5746b78dd8
Unpadded CMS SHA-256 (both):
277c1875cedf2db554ffc885ee72d158df5d397790a9d8168109860b13460fd6
```

The comparison reports `originalIsExactPrefix`, `signedContentIdentical`, and
`cmsIdentical` all `true`; `appendedBytes` is `3044`. Thus even the excluded
signature gap and original xref/trailer are preserved, not merely the digest.
The 868-byte DER occupies a hex Contents slot with 9 zero padding bytes.

Additional read-only Node inspection enumerated direct object dictionaries and
classic xref/trailer sections, redacting all literal strings and long hex values
before output. These small files have direct, uncompressed catalog/signature
objects (no object-stream resolution was needed):

- Original header `%PDF-1.4`, xref at `4670`, `/Size 12`, `/Root 2 0 R`.
- Marked final xref at `7836`, `/Size 14`, `/Root 2 0 R`, `/Prev 4670`.
- Appended xref updates objects `1`, `2`, `8`, `12`, `13`; no new signature.
- Original catalog `2 0`: `/Type /Catalog /Pages 3 0 R /AcroForm 6 0 R`.
  Appended catalog keeps these references and adds `/Version /1.5`.
- AcroForm `6 0`: `/Fields [7 0 R] /SigFlags 3`.
  Signature widget `7 0` has `/FT /Sig /V 9 0 R`.
- Annotation array `8 0` changes from `[7 0 R]` to `[7 0 R 12 0 R]`.
- New `12 0` is `/Type /Annot /Subtype /Polygon /IT /PolygonCloud`, with
  `/AP /N 13 0 R`; `13 0` is a Flate-compressed appearance Form XObject.
  Metadata object `1 0` is also replaced. Appearance stream contents were not
  rendered or semantically validated.

These changes are consistent with an incremental annotation save, not evidence
that *every* possible incremental update is safe or permitted.

### Signature type, certification, and time

- Signature object `9 0` has `/Type /Sig`, `/Filter /Adobe.PPKLite`, and
  `/SubFilter /adbe.pkcs7.detached`: a detached CMS **approval signature**.
  This sample is not `ETSI.CAdES.detached` or a document timestamp signature.
- Both catalog versions lack `/Perms`; signature `9 0` lacks `/Reference` and
  DocMDP/FieldMDP transforms. No literal `/DocMDP` or `/TransformMethod` was found.
  In these inspected direct-object structures there is **no DocMDP certification**
  and therefore no certification `/P` value to report. `/SigFlags 3` is not
  certification permission `P=3`.
- CMS SignerInfo digest is **SHA-256** and signature algorithm is
  **rsaEncryption** (RSA PKCS#1 v1.5 with that digest). The algorithm inventory
  also contains `sha1WithRSAEncryption` in the embedded certificate; this does
  **not** mean the PDF content signature uses SHA-1. It is not a model for new
  certificate issuance. Certificate identity/trust was intentionally not assessed.
- CMS signed attributes and unsigned attributes are both absent. There is no
  CMS `signingTime`, RFC 3161 signature timestamp token
  (`1.2.840.113549.1.9.16.2.14`), TSTInfo (`1.2.840.113549.1.9.16.1.4`), or
  countersignature (`1.2.840.113549.1.9.6`). No PDF `/DocTimeStamp` was found.
- PDF signature `/M` exists within signed bytes. It is a signer-asserted date,
  not independent proof of signing time. **No trusted timestamp is established.**
  If future input contains timestamp tokens, this probe only reports presence;
  it does not validate TSA CMS, message imprint, policy, time, or TSA trust.

A correct UI must distinguish “original revision's cryptographic signature
intact; later changes exist” from “entire current document signed and unchanged.”
Approval without DocMDP is not a blanket permission verdict. Trust, allowed
changes, and cryptographic integrity need separate states.

## Malformed input and validation boundary

The unit tests reject negative, fractional, exponential, nonnumeric, unsafe-size,
missing/extra, overlapping, zero-length, nonzero-origin, out-of-bounds, and
oversized ByteRange inputs before any Buffer slicing. PDF whitespace is explicit;
JavaScript-only whitespace such as NBSP/vertical tab is rejected. BigInt arithmetic
prevents overflow before conversion to safe JS numbers. Independent `signedBytes`
calls revalidate ranges, so Buffer's permissive clamping cannot hide bad offsets.
Tests also cover duplicate literal ByteRanges, non-array values, incorrect gap
boundaries, invalid/odd hex, truncated/indefinite/nonminimal DER lengths, and
nonzero trailing padding. Unsupported inputs produce a nonzero CLI exit, not a
“signature valid” result.

**Structural inspection is not comprehensive PDF security validation.** This
probe's regex discovery is intentionally sample-oriented, not a conforming PDF
parser. It cannot resolve escaped names, stream-contained decoys, encryption,
compressed objects, hybrid xrefs, active versus superseded signatures, duplicate
dictionary keys, or hostile object graphs. A misleading PDF could pass lexical
checks without having the claimed active signature field. Multiple signatures,
indirect Contents/ByteRange, more than two segments, and some otherwise legal
PDF syntax are unsupported. Absence of a raw token is not a general absence
proof; the stronger sample conclusions above also use direct-object inspection.

A CMS pass establishes integrity of the supplied byte sequence with its embedded
signer key, not safe rendering, a trusted identity, current-revision coverage,
DocMDP compliance, or resistance to shadow/signature-wrapping attacks. No
revocation, chain policy, expiration, key usage, trust anchor, DSS/LTV, JS/actions,
attachments, visual signature spoofing, or adversarial revision analysis was
performed. No interactive checks in Bluebeam, Acrobat, or Kinetic PDF were run.

## Future backend regression matrix

Use synthetic keys/certificates and redistributable fixtures, not these private
PDFs, in CI. Retain private interoperability samples only externally with consent.
For every signing/certifying output, compare signed bytes and independently check
CMS, resolve the active PDF object graph, and record reader versions/verdicts in
Bluebeam and Acrobat. A cryptographic result alone must never assert trust.

| Case | Required oracle / expected result |
| --- | --- |
| Unsigned PDF; empty signature field | No signed-document warning or verification work; ordinary open/edit/save unchanged. Empty widget is not a signature. |
| First approval signature | Correct field/widget/appearance; exact ByteRange excludes only Contents; CMS independently passes; no false DocMDP status. |
| Approval followed by markup | Append only; all original bytes and original CMS retained; mark current revision as modified, classify change separately. These samples cover this positive crypto case. |
| Second/third approval or counter-signing revision | Each signature verifies against its own revision; previous bytes preserved; active fields, signer order, and coverage resolved, not raw-token counts. Probe does not support this today. |
| Certification `P=1` | Catalog `/Perms /DocMDP` points to certifying signature with matching transform; subsequent edits cannot be advertised as allowed. Include attempted annotation, form, signing, and structural changes. |
| Certification `P=2` | Test permitted form filling/signing versus disallowed annotations/content/structural edits; verify actual transformations under applicable PDF rules and independent readers. |
| Certification `P=3` | Test annotations plus permitted form/signing operations; reject content/structural edits as allowed. Incremental preservation alone does not establish permission. |
| FieldMDP / signature field locks | All/Include/Exclude and locked fields honored; keep separate from DocMDP. Certification must be first signature, not retrofitted after approvals. |
| Timestamped approval / document timestamp | RFC 3161 CMS passes, message imprint binds to correct signature value or document bytes, respectively; validate TSA chain/policy/time separately. Wrong imprint, stale/failed TSA, malformed token must not become trusted time. |
| SHA-256+ RSA / RSA-PSS / ECDSA | Verify supported algorithms and parameters independently; explicit unsupported state otherwise. Legacy SHA-1 is not a new-signing default. |
| Trust states | Trusted/untrusted/self-signed/expired/not-yet-valid/revoked/unknown-offline certificates; integrity and trust stay separate, including timestamp-assisted historical validation and DSS/LTV cases. |
| Signed-content and CMS tampering | One-byte changes fail integrity. Changes solely after signed revision leave old CMS intact but flag later revision; tampered Contents padding/ByteRange cannot silently pass. |
| Hostile PDF structure | Escaped/duplicate keys, stream decoys, shadow objects, xref streams/hybrids, truncated revisions, enormous/overlapping ranges, encryption, object cycles; fail safely or report unsupported/indeterminate. Use full-parser and security-specific tests, not this probe as oracle. |
| Rewrite / arrange / optimize | Rotate, delete, duplicate, reorder, blank insertion, flatten, redact, and full rewrite must not silently claim old signatures preserved or permitted. Explicit invalidation/removal policy and user confirmation required. |
| Save lifecycle | Save/Save As, no-op save, cancel/failure/out-of-space, incremental save after undo/redo, concurrent edits and external replacement; never corrupt source or drop edits. Gate prohibited structural changes; test immutable revision/stale-result handling. |
| Cross-reader appearance | Widget visibility, rotations/crop boxes, field selection, page changes, and final-reader signature status match semantics; screenshots without private identities. |

## Unsigned performance baseline (defined, not measured here)

There is no signing implementation change in this task. Application benchmarks
and interactive checks were **not run**: this investigation owns only docs and
optional probe files, and the probe timings are not an application baseline.
Before integrating a backend, capture pre-feature and candidate release builds
on the same machine with the same PDFium, corpus hashes, settings, and power mode.
Use an external scratch worktree/output location to avoid unrelated shared-tree
artifacts. Never feed the private signed samples into a rewriting benchmark.

The existing benchmark entry point is `src/bin/bench/main.rs` (requires `bench`),
not the older path mentioned in parts of `docs/DEVELOPMENT.md`. Example baseline:

```powershell
cargo run --release --features bench --bin bench -- --label unsigned-before --pages 300 --runs 10
# Repeat after integration, same inputs/settings:
cargo run --release --features bench --bin bench -- --label unsigned-after --pages 300 --runs 10
# Separately pass approved unsigned vector/scanned/mixed PDFs as positional args.
```

Use small 1-page, generated 300-page, large vector/takeoff, scanned-image, and
mixed unsigned documents; include an empty signature widget and a large AcroForm
without signatures. Record startup, open-to-first-page stages, render at normal
and high zoom, extraction/search, annotation save round trip, memory per stage,
and output size. The benchmark produces Markdown under `bench-results/` and
may create repeated-page/generated documents. Inspect its output behavior before
using private inputs. Retain exact before/after commit IDs and commands.

Run repeated cold-process and warm-cache trials; distinguish process-cold from
OS-cache-cold and first PDFium extraction. Report medians and tail latency across
runs; supplement benchmark averages with instrumented p95 UI frame/open/save
latencies in a real window (pan/zoom/edit), because the benchmark has no UI frames.

Acceptance: unsigned documents launch no OpenSSL/sidecar, perform no certificate,
revocation, or TSA network calls, and do not read/hash the entire file merely to
discover whether signatures exist. Discovery belongs off the UI thread with
bounded work and immutable-revision caching. Treat >5% repeatable median slowdown
or peak-memory growth above established noise as an investigation trigger, not a
claimed measured regression; agree an absolute small-file latency budget before
release. Confirm UI p95 and save output remain within their recorded baseline
budgets. Signed-document verification/signing latency needs separate measurements
and must not be charged to the unsigned fast path.

## Production safety integration coverage and handoff

New `tests/signature_safety.rs` uses `tests/common` and synthetic AcroForm
widgets, never the private samples. Its populated values deliberately contain
invalid CMS and ByteRange data: finding these signatures is not validating them.
Only worker threads load PDFium; fixture construction uses lopdf, not PDFium.

Coverage written:

- Ordinary unsigned file, missing/null empty signature values, one populated
  value, and two populated values mixed with an empty field. Direct and indirect
  `/V` dictionaries, plus two levels of `/Kids` with `/FT` inherited from the
  root field, cover both populated and empty values.
- Malformed non-null `/V` (string, integer, empty dictionary), flat and nested:
  discovery must be Unknown or Present, never unsigned. Direct Save/SaveAs/Extract
  must refuse without changing source/target bytes; no artificial native failure
  injection is used.
- `Opened` precedes `SignatureDiscovery`, with matching generation and exact
  snapshot fingerprint/bytes.
- Fourteen protected-source refusals: empty/edited Save, SaveAs, and Extract,
  including existing/new/source destinations, changes, reorder/rotation/duplicate
  sheets, and a new blank page. Assert the signature-policy error, no SaveTarget
  or Saved success, source/target bytes unchanged, no output or leftover temp file.
- Read original page text after refusal, then reopen unsigned and successfully
  SaveAs an edit using the same worker. The positive target is replaced with a
  valid unsigned PDF, not the non-PDF sentinel used in refusal assertions. Positive
  saves compare against the actual initial unsigned bytes and check the first
  page's sole annotation is a Highlight with the exact requested decoded Contents.
- Separate unsigned-source / detected-signed-destination SaveAs and Extract
  cases, with both empty and edited/arranged requests; refusal preserves both
  files, and a subsequent ordinary Save must still work. These require the
  backend's shared output-safety hook.
- Unknown/Present session policy (including zero and large Present counts),
  command/merged-command/save/undo/redo guards, preserved dirty work/history,
  and an independent editing block remaining effective after discovery changes.

After the coordinator released Cargo, these commands were run:

```powershell
cargo test --test signature_safety -j2 --locked unknown_and_present_policy
cargo test --test signature_safety -j2 --locked signature_discovery_does_not_clear
cargo test --test signature_safety -j2 --locked worker_discovers_signatures
cargo test -j2 --locked --test save_as --test extract --test session --test worker
```

Both session tests passed, as did all four existing integration binaries. The
initial worker test **failed**: an empty signature field was reported as Present.
An independent stdlib-only Python ctypes probe of the already-installed PDFium
DLL, using synthetic PDFs constructed in memory, confirmed raw
`FPDF_GetSignatureCount = 1` for each of missing `/V`, null `/V`, and populated
`/V`. Native count alone therefore does not distinguish empty fields. The
required unsigned expectation was retained and reported to the production owners.
Production now refines native hints via `hinted_signatures(&bytes)`; the final
passing run below confirms empty-field expectations were fixed, not waived.

Discovery, source barriers, and destination barriers are now separate tests to
allow independent execution. Subsequent filtered retries (`worker_discovery_distinguishes`
and `worker_refuses_direct`) were blocked by `--locked` while backend edits added
a dependency before updating Cargo.lock. No lockfile/production changes were
made by this investigation.

After the source discovery fix and destination hook landed, actual runs were:

```powershell
cargo test -j2 --locked --test signature_safety
cargo test -j2 --workspace --locked
```

- `signature_safety`: **6 passed, 0 failed**, including nested/direct/empty values,
  malformed values, both source/destination barriers, and exact positive-save
  content assertions. No test weakening or correction was required.
- Workspace command: **FAILED (exit 101)**. `gpu-lines` ran 99 passing tests;
  `kinetic-pdf --lib` ran 351 passing, 20 failing, 2 ignored tests. Aggregate
  actually executed: **450 passed, 20 failed, 2 ignored**. Cargo stopped at the
  library failure; later workspace targets were not all executed. Compilation
  finished in 2m 53s; this is not a performance benchmark.
- The 20 failures were in `app::layers` (4), `app::pins` (8), `app::text` (6),
  and `app::tool_panel` (2): missing-edit/index assertions and layer/edit state
  mismatches. The coordinator traced these to the shared synthetic Table fixture
  missing the new discovery reply; sig-save corrected that fixture. No production
  files were changed by this investigation.
- Logs in OS temp:
  `C:\\Users\\flowcost-home\\AppData\\Local\\Temp\\kinetic-signature-safety-final.log`
  and `C:\\Users\\flowcost-home\\AppData\\Local\\Temp\\kinetic-signature-workspace-final.log`.
- Existing builds emitted egui deprecation and an unrelated unused-assignment
  warning. An initial shell log-path quoting error prevented the first invocation
  from starting Cargo; the commands/results above are the subsequent actual runs.

### Pre-async frozen-source validation

After production owners completed the canonical SaveAs alias-retarget fix, mixed
empty/missing-FT discovery fix, inherited destination `/V` fix, shared Table
fixture correction, and missing-source SaveAs recovery compatibility, the
coordinator froze production edits and authorized these final commands:

```powershell
cargo test -j2 --workspace --locked
cargo check -j2 --workspace --all-targets --features bench --locked
cargo check -j2 --bin kinetic-pdf --features store --locked
```

All three commands exited **0**:

- Full workspace: **675 passed, 0 failed, 5 ignored**, summed across all 46 test
  result groups, with no filtered/measured tests. This supersedes the incomplete
  450/20/2 run above. All six `signature_safety` tests passed again; the workspace
  also exercised worker/output/overlay/app tests and the broader integration
  targets. Compilation finished in 4m 05s; no performance baseline is inferred.
- Workspace/all-targets with `bench`: **PASS**, finished in 18.75s.
- Store binary checked separately: **PASS**, finished in 6.83s. This is a compile
  check, not Store packaging or interactive runtime validation.
- Five ignored tests were two manual timing tests, a network update test, manual
  erase performance measurement, and installed Windows PDF-driver printing.
  Ignored cases were not executed or counted as passing.
- Existing egui deprecation/unused-assignment warnings remain; no tests or locks
  were weakened. Cargo was released to the coordinator after all three checks.

Final logs (OS temp, not repository artifacts):

```text
C:\Users\flowcost-home\AppData\Local\Temp\kinetic-signature-workspace-frozen.log
C:\Users\flowcost-home\AppData\Local\Temp\kinetic-signature-all-targets-frozen.log
C:\Users\flowcost-home\AppData\Local\Temp\kinetic-signature-store-frozen.log
```

### Deferred-discovery integration update

The **675 passed / 0 failed / 5 ignored** workspace result above is the
**pre-async baseline**, not validation of the subsequent deferred-discovery patch.
The release performance experiment also found measurable unsigned empty-form
open/first-render overhead; that gate is not waived by the correctness baseline.

`tests/signature_safety.rs` now waits for matching generation AND snapshot
fingerprint discovery after `Opened`, tolerating interleaved Highlights/render
replies and ignoring obsolete completions. A deterministic channel-only test
queues wrong-generation/wrong-fingerprint unsigned replies before the matching
protected result; these must never complete the waiter or imply unlock.

Added real-worker cases: populated ordinary text/button/choice AcroForm fields
resolve unsigned; untyped top-level containers with child-typed populated Sig
fields resolve protected even when the native top-level signature count is zero.
Those protected cases also attempt a direct Save and assert refusal and byte
preservation. Direct/indirect `/V`, inherited `/FT`, empty values, and existing
source/destination/readability checks remain. The file now has seven tests.

After sig-save's async API handoff, `cargo test -j2 --locked --test signature_safety`
passed **7/7**. Deterministically delayed service/pending-write protection unit
tests belong to sig-save; no timing-race integration test was added here.
Deferred analysis is not a historical/orphan-signature absence proof.

Initial async validation runs (before the final freeze):

- First `cargo test -j2 --workspace --locked`: incomplete **481 passed / 1 failed /
  2 ignored** aggregate. Main library reported 382/1/2; the single failure was
  `app::gpu::tests::the_budget_learns_how_fast_this_gpu_really_is_within_a_few_draws`
  at `gpu.rs:1718`. The original failure log is preserved; GPU production/tests
  were not changed.
- Full workspace retry, started before the coordinator requested isolation:
  **685 passed / 0 failed / 5 ignored**, including signature safety 7/7 and both
  helper-render scheduling tests. Treat as provisional until the final UI/worker
  patch freeze and quiet build window are confirmed.
- The named GPU test rerun alone with `cargo test -j2 --locked --lib
  app::gpu::tests::the_budget_learns_how_fast_this_gpu_really_is_within_a_few_draws
  -- --exact` passed (1 passed, 386 filtered). Failure did not reproduce alone;
  timing sensitivity is plausible but not established.
- `cargo check -j2 --workspace --all-targets --features bench --locked` and
  separate `cargo check -j2 --bin kinetic-pdf --features store --locked` both
  passed. Final reruns await coordinator confirmation of settled source.

OS-temp logs: `kinetic-signature-async-integration.log`,
`kinetic-signature-async-workspace.log` (failed),
`kinetic-signature-async-workspace-retry.log` (passed),
`kinetic-signature-async-gpu-isolated.log`,
`kinetic-signature-async-all-targets.log`, and `kinetic-signature-async-store.log`.
Read-only service/scheduling findings and snapshot hashes are recorded externally
in `kinetic-signature-async-review.md` in the same OS temp directory. No performance
collection was performed; correctness passes do not waive the latency gate.

### Final async frozen-source validation

After the coordinator confirmed the app/helper hook in `src/app/mod.rs` was
finished, captured SHA-256 values for **21 production/dependency files**, including
that hook, the service, worker, protocol, and the unchanged GPU source. The
snapshot is `kinetic-signature-async-final-freeze.json` in OS temp. Rehashing after
all commands found **zero changes** to those captured files.

```powershell
cargo test -j2 --locked --lib
cargo test -j2 --workspace --locked
cargo check -j2 --workspace --all-targets --features bench --locked
cargo check -j2 --bin kinetic-pdf --features store --locked
```

All four commands exited **0**:

- App/library: **385 passed / 0 failed / 2 ignored**.
- Full workspace: **685 passed / 0 failed / 5 ignored**, summed across 46 result
  groups. Includes all **7 signature_safety** tests, worker/service/app scheduling
  cases, and the existing integration targets. This supersedes the pre-async
  675/0/5 result; it is not added to that older total.
- Bench/workspace/all-targets check and separate Store binary check: **PASS**.
- Tests used **default libtest threading**, with `RUST_TEST_THREADS` unset and
  no `--test-threads` override. `-j2` limits Cargo build jobs, not test threads.
  No serialized retry was needed in this final frozen-source sequence. The
  earlier GPU assertion failure and isolated passing rerun remain recorded above;
  the GPU source/test logic was not changed to obtain a pass.
- Existing deprecation/unused-assignment warnings remain; no tests or locks were
  weakened. Cargo was released after all checks and hash verification completed.

Final OS-temp logs: `kinetic-signature-async-final-lib.log`,
`kinetic-signature-async-final-workspace.log`,
`kinetic-signature-async-final-all-targets.log`, and
`kinetic-signature-async-final-store.log`.

Read-only scheduling qualification remains: helper render handlers release
analysis before determining whether a completed render is prediction-only and
stored in spares. Such completion need not mean the visible page is usable.
The deliberate 100 ms fallback can also overlap a slow first render. These
schedule work but do not unlock edits; no private-result authority bypass was
identified. First-visible-render latency and time-to-editable still require
performance qualification. Correctness checks do not waive the unsigned-forms
no-measurable-overhead gate.

Known gaps, not claimed solved by these tests:

- Source protection must not be confused with destination protection. Both
  integration barriers passed again in the final frozen-source workspace run.
  Hardlink/alias, target-byte recheck, and conservative malformed/non-PDF target
  rejection helper tests belong to the backend owner and ran as part of the
  workspace; they are not implemented in this integration file. Passing tests
  do not turn filesystem byte rechecks into an atomic compare-and-swap.
- PDFium's active signature count is not a scan of all historical revisions.
  Removed/hidden historical fields, malformed structures, and adversarial detection
  bypasses are not absence-proven or covered here. A zero count means only
  `NoSignaturesDetected`, never a security guarantee.
- Native discovery failure cannot be injected through the public worker API;
  Unknown is covered at the public policy/session boundary, not with a flaky
  malformed-PDF attempt to force a particular PDFium failure.
- App-side stale generation/fingerprint rejection is covered separately by
  sig-save's `src/app/mod.rs::tests::pending_signature_discovery_is_revision_bound_and_failure_cannot_unlock`
  (inspected read-only). It checks pending lock, wrong generation/fingerprint,
  matching unlock, and SaveFailed not clearing a signature lock. This integration
  file checks worker reply identity/order; it does not duplicate that app test.

### Signature panel workflow

- **Sign** places session-only, revision-bound drafts, not PDF edits or approval
  signatures. Ordinary Save does not save drafts; Open/New/Quit ask before loss.
- **Sign & Certify** draws a new final block. **Certify** uses existing drafts,
  promoting the last block without adding another, or explicitly certifies with
  no visible block when there are no drafts. Both require password confirmation
  and create one P1-certified copy covering the whole PDF and all blocks.
- Cancelling Certify preserves existing drafts. Cancelling Sign & Certify removes
  its new final box. A failed certification retains drafts for retry.
- Box text uses the backend's preferred PDF-point size and current page zoom.
  The final backend additionally shrinks text to fit readable certificate details.
- After accepted, revision-matched success, the normal worker opens the saved copy
  in the same window. The original remains unchanged. The creation receipt stays
  separately labelled; verification of the current PDF starts again from the
  newly opened immutable snapshot, never from matching paths or byte lengths.
- Remembering passwords remains explicit Windows-account consent, persisted only
  after successful authentication. Only bounded public ID metadata enters prefs.
  Accepted credential-save work can finish across the automatic open transition.
