# Isolated pyHanko backend qualification

Run: 2026-11-13, Windows x64. **Developer spike only; no production integration.**
See [backend evaluation](signature-backend-evaluation.md) and [roadmap](SIGNATURES.md).
All PDFs, keys, root/leaf/TSA certificates and CRL were freshly synthetic. No user
PDFs, private identities, proprietary implementation or system Python install used.

## Decision

**Conditional GO for another isolated identity/IPC prototype, NOT production selection.**
The released backend demonstrates the basic signing/certification paths. However,
**P=3 annotation validation remains a blocking feature gap for certified takeoff
editing**, and native identity, external-validator, hostile-revision, packaging and
performance qualification remain open. Do not bypass difference analysis to get a pass.
No signing backend is added to the app by this experiment.

## Reproduce / commands actually run

Requires already available uv and OpenSSL; internet is used for official package
metadata/artifacts and temporary environment setup, not for document validation.
Python 3.12.15 was already present in uv's managed interpreter cache.

```bash
uv --version
openssl version
uv pip compile --no-config --index-url https://pypi.org/simple \
  --python-version 3.12 --generate-hashes --no-annotate --no-header \
  scripts/signatures/pyhanko-qualification/requirements.in \
  --output-file scripts/signatures/pyhanko-qualification/requirements.txt
uv run --no-project --isolated --python 3.12.15 python -B -m unittest discover \
  -s scripts/signatures/pyhanko-qualification -p 'test_*.py' -v
uv run --no-project --isolated --python 3.12.15 python -B \
  scripts/signatures/qualify_pyhanko.py
```

The launcher creates a private OS temp directory, then executes these subprocesses
(with generated paths replacing the placeholders):

```text
uv venv --no-config --python <launcher-python> <temp>/venv
uv pip install --no-config --python <venv-python> --index-url https://pypi.org/simple
  --only-binary :all: --require-hashes --requirements <qualification>/requirements.txt
<venv-python> qualify_pyhanko.py --worker <temp>
```

- Versions used: uv **0.13.5**, CPython **3.12.15**, OpenSSL **3.5.7**.
- Windows temp ACL inheritance is removed and current SID granted full control;
  POSIX code uses directory mode 0700 and exclusive file creation mode 0600.
  Only Windows was executed. Test private keys are unencrypted **temporary fixtures**,
  never arguments, environment variables or log contents.
- The launcher removes the complete venv and fixture tree before reporting success;
  observed `temp_cleanup=completed`. Failure paths also use `TemporaryDirectory`.
  Forced termination/crash can still leave private temp material: this is not a
  production secret-management or secure-erasure design.
- Public uv interpreter/wheel caches remain outside the repo. No global install,
  shell activation, shipping dependency or Rust Cargo command was used.
- Python validation has explicit roots, CRLs and `allow_fetching=False`; DummyTimeStamper
  is entirely in-process. The worker blocks non-loopback Python socket connections;
  Windows asyncio needs literal loopback for its wakeup channel. This is not an OS
  network sandbox. OpenSSL CMS verification does not perform revocation fetching.

## Release provenance and licensing

Selected **pyHanko 0.38.1**, released wheel uploaded **2026-11-03T22:15:28Z**.
Official [versioned PyPI JSON](https://pypi.org/pypi/pyHanko/0.38.1/json) supplies
artifact URLs/hashes; both live metadata and downloaded source hash are checked.
The wheel is pinned by official `files.pythonhosted.org` URL plus SHA-256 in the
input/installation lock. All 22 resolved distributions have pinned versions/hashes.
The 92 KB generated lock intentionally includes hashes for multiple wheel artifacts.

```text
wheel (485,133 bytes) SHA-256:
723fdf1a6e27be72d0d7fbf65deccf03aeaa6fafbdddc5c698b6b2eba06c543d
sdist (8,004,631 bytes) SHA-256:
27348b2bb5c6756402102a930b3404a1ccce4d1102f81e56399df268dd90f70d
```

The downloaded sdist's `LICENSE` is MIT and
`src/pyhanko/pdf_utils/LICENSE.PyPDF2` retains BSD-3-Clause-style notices.
Release metadata labels development status **Beta**; its README does not repeat
all warnings found in the earlier upstream-HEAD evaluation. Neither observation
constitutes production assurance. Source APIs inspected in this release:
`sign/signers/pdf_signer.py`, `sign/timestamps/dummy_client.py`,
`sign/validation/pdf_embedded.py`, and `tests/test_diff_analysis.py` under `src/pyhanko`
(or the sdist test directory).

Key installed versions: `pyhanko-certvalidator 0.33.0`, `cryptography 50.0.2`,
`asn1crypto 1.5.1`, `aiohttp 3.14.5`, `lxml 6.1.3`, `oscrypto 1.3.0`.
The lock and JSON report list all dependencies. MIT at the top level is not a
complete redistribution clearance: Python, crypto/OpenSSL, XML native libraries,
transitive notices, exact wheels and update provenance still need an SBOM/license
audit. PKCS#11, font/image extras and the separate pyHanko CLI are not installed.

## Executed results

**4 launcher/helper regression tests passed. Full qualification command exited 0.**
Fresh random identities/times mean output bytes are not deterministic; sizes below
are one observed run. All claimed prefixes were checked by actual byte equality.

| Case | Observed result |
| --- | --- |
| Approval B-B construction | 8,874-byte PDF. `/ETSI.CAdES.detached`, SHA-256; `content_type`, `message_digest`, `signing_certificate_v2` signed attributes; no CMS signing-time attribute. Original unsigned prefix preserved. Integrity and fixture-root validation passed; no trusted timestamp. |
| Certification P=1, P=2, P=3 | Each 9,021 bytes. Catalog `/Perms /DocMDP` exists and transform `/P` equals requested permission. Original prefix preserved; fixture validation passed. |
| Additional approval, P=1 | Actual catalog-linked DocMDP `/P=1` asserted/recorded. Rejected only if both `SigningError` type and exact pinned-release message `Author signature forbids all changes` match; unrelated signing failures do not count as a pass. No signed source mutated. |
| Additional approval, P=2/P=3 | Each 16,657 bytes. Prior signed PDF remains exact prefix. Both signatures validate against fixture root; original reports permitted `FORM_FILLING`. |
| Text annotation appended to P=3 | 9,688 bytes. Exact signed prefix preserved; CMS intact/valid and test-root trusted, but `modification_level=OTHER`, `docmdp_ok=false`, `bottom_line=false`. **Unsupported by default policy.** No `skip_diff` or replacement policy was used. |
| B-T local fixture | 16,473 bytes. Actual RFC 3161 signature timestamp, validated against ephemeral test TSA/root; no DSS. B-B without a token was not relabelled B-T. |
| B-LT local fixture | 20,420 bytes. Signature timestamp plus DSS containing embedded CRL. Basic signature/timestamp checks passed using supplied fixture evidence; no document timestamp. |
| B-LTA local fixture | 29,193 bytes. DSS plus one document timestamp. Timestamp CMS/imprint and pyHanko document-timestamp integrity/trust checks passed. |

**Trust separation:** the same approval remains cryptographically intact/valid with
`trust_roots=[]`, but reports `trusted=false` and `bottom_line=false`. The positive
context trusts only the generated root and requires the generated CRL; it does not
use OS roots. These are test fixtures, not a trusted human identity, qualified
certificate, independent wall-clock proof or a real timestamp service.

The permission-change matrix covers **co-signing only**, plus the P=3 annotation
limitation. P=2 permitted form filling and forbidden page-content changes were **not
exercised**. This is not broad DocMDP policy qualification.

### Independent CMS checks and negative controls

OpenSSL performed **13 PDF CMS checks** (12 detached signatures and one document
timestamp), plus **3 signature-timestamp CMS checks**. All passed. Verification of
12 deliberately changed signed-content inputs and 13 changed CMS signature values
failed as required. Four timestamp message imprints were separately recomputed
using `hashlib`: three signature timestamps and one document timestamp.

Effective detached verification command, with temporary paths:

```text
openssl cms -verify -binary -inform DER -in <cms> -noverify
  -out <verified> -content <exact-ByteRange-concatenation>
```

`-noverify` skips certificate-chain trust, **not** signature, content or signed-attribute
verification. Output must equal the reconstructed signed bytes. Encapsulated timestamp
CMS is verified without `-content`, then its imprint is bound separately. OpenSSL
reads only synthetic CMS/data; it receives no private key. This independently checks
CMS crypto, not PDF object discovery, DocMDP semantics or full PAdES conformance:
ByteRange/CMS discovery still uses pyHanko's parser on generated fixtures.
No Acrobat or DSS profile-validator run was performed. Negative controls do **not**
include a cryptographically valid, deliberately re-signed timestamp token with the
wrong message imprint. Positive imprint comparisons and corrupt-signature controls
do not establish that missing adversarial-token qualification.

### Timestamp/LTV limits

The local TSA uses a newly generated timestamp-only EKU certificate and RSA key;
the local root issues a fresh empty CRL. **Actual token/evidence generation was
exercised**, not just presence of Python option names. However, LT/LTA results are
fixture emission/basic verification, not a passed comprehensive AdES archival-policy
test. **Embedded-only LT/LTA evidence validation was not performed**: validation
contexts explicitly supply fixture certificates and CRLs outside the PDF. A DSS CRL
presence check does not establish that embedded evidence alone covers either chain.
The JSON reports `embedded_only_evidence_qualified=false` and lists unperformed cases.
No external TSA, OCSP service, expired/revoked historical path, timestamp
renewal, clock rollback, offline-after-expiry or qualified-signature policy was tested.
Future implementation must validate the real TSA and revocation policy; it must not
ship this DummyTimeStamper or test root as trusted production infrastructure.

## Packaging/performance observations and next gate

- Temporary venv logical file size: **32,531,247 bytes (~31.0 MiB)** across 22
  distributions, excluding the managed Python base runtime, uv caches, OpenSSL CLI,
  symbols and optional extras. uv may hardlink cached public artifacts: logical size
  is not installer size or unique disk allocation.
- Final warm-cache measured run: setup **1.181 s**, worker **5.802 s**, including imports,
  RSA key generation, all signatures, validation and OpenSSL subprocesses. Not a
  cold-start benchmark or per-signature performance claim. No app startup/view/save
  measurements were taken; no-measurable-unsigned-regression remains an acceptance
  target, not a result of this spike.
- Early development runs failed on blocked Windows asyncio loopback, an unresolved
  PDF indirect reference in the harness, and assuming `DocumentTimestampStatus` had
  `bottom_line`. Harness fixes were applied; final checks use timestamp
  `intact/valid/trusted`. These were not silently recorded as backend successes.
- Next isolated gate: prove a non-exportable CNG/custom-signer contract and cancellation,
  then PKCS#11/PKCS#12 and macOS/Linux packaging; qualify real TSA/revocation services,
  independent profile validators, hostile revisions and resource limits. P=3 support
  requires a reviewed alternative/extension or must remain blocked.
- This prototype does not test native keys, remote signing, encrypted PDFs, field-lock
  variations, appearances/PDF-A/PDF-UA preservation, large documents, save lifecycle,
  crashes, sandboxing or Store redistribution. **No production backend approval.**
