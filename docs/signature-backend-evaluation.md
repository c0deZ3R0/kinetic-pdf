# PDF signature backend evaluation

Research snapshot: 2026-11-13. Scope: Kinetic PDF (Rust/Windows; future macOS/Linux).
Owner scope: this document only; no production dependencies or code added.
Public upstream sources only; no proprietary Bluebeam code used.

## Recommendation

**Prototype an on-demand, out-of-process pyHanko backend behind a versioned Rust
interface; do not approve any evaluated open-source backend for production yet.**
Use pyHanko's high-level signing/validation APIs, not just its CMS embedder. Keep
Digitorus pdfsign as a secondary candidate and independent interoperability check,
not the sole authority for document safety. Do not build a new PDF security
validator from Rust CMS primitives as the first implementation.

Two material qualification blockers:
- pyHanko calls itself beta/not production-ready and does not support difference
  analysis for post-signing comments/annotations (DocMDP P=3). This directly affects
  a takeoff app. Initially block such signed-document edits; do not report them as
  safe. Full P=3 support requires a separately reviewed implementation/backend.
- No measured performance, hardware identity interoperability, hostile-document
  qualification or redistribution/Store packaging proof exists from this research.

If certified PDFs must remain annotatable in the first release, evaluate a supported
commercial cross-platform SDK against the same gates before choosing a backend.
An open-source license or successful CMS check is not a production assurance claim.

## Evidence and candidates

“Verified” below means inspected upstream source/docs, **not** executed, audited,
or independently demonstrated compliant. Upstream HEADs inspected:
Digitorus `9483e7201db2d13dba9a835bc849c7d19f280a16`;
pyHanko `0b27455d49ecde3d2853f598558f76d1af38cbf1`.
These are research references, not proposed release pins. Other linked upstream
branches/docs are mutable; freeze versions and hashes for any qualification build.

| Candidate | Verified capability | Limit / production impact |
| --- | --- | --- |
| **pyHanko** [P1–P4] | CMS/PAdES B-B, B-T, B-LT, B-LTA; RFC 3161; multiple signatures via incremental updates; certification/field-lock APIs; cryptographic, X.509, AdES and historical difference validation. PKCS#12 loading, PKCS#11 signer, custom `Signer.async_sign_raw` and interrupted signing. | Best coverage reviewed, not a production endorsement. Explicit beta warning. Standard difference policy cannot approve annotation changes under P=3. PDF/A and PDF/UA preservation not enforced. No turnkey Windows CNG adapter verified. |
| **Digitorus pdfsign (Go)** [D1–D5] | Detached CMS, explicit PAdES B-B/B-T; approval/certification/document timestamps, DocMDP P=1/2/3, incremental signing. Current verification reads signed dictionaries from their signed revision, discovers removed historical signatures, and compares changed objects for DocMDP. Certificate/TSA/revocation options; external `crypto.Signer`. | `PAdES_B_LT` and `PAdES_B_LTA` constants exist but are explicitly unsupported/rejected: enum names are not capabilities. Default legacy format is **not PAdES**. PKCS#11 module is best-effort. No native CNG integration or built-in PKCS#12 path verified; inspected CLI reads PEM keys/certs. |
| **Rust `pdf_signing`** [R1] | `lopdf`-based signature construction, CMS through `cryptographic-message-syntax`, image appearance. MIT OR Apache-2.0. | README says work in progress. Inspected signing source contains disabled DocMDP setup, fixed placeholder matching, `unwrap`/panic paths and whole-file copies. No complete PAdES/LTV or trustworthy revision-validation system demonstrated. Reject as production backend now. |
| **Rust building blocks** [R2–R4] | `lopdf::IncrementalDocument` writes incremental PDFs; RustCrypto `cms` supplies CMS ASN.1/building blocks; `cryptoki` wraps PKCS#11; Windows bindings expose CNG. | These are components, not a PAdES/certification validator. Application would own signed-attribute rules, TSA, chain/revocation/time policy, PDF historical object semantics, locks and attack resistance. Largest engineering/security burden. |

### Important source-level qualifications

- Digitorus is **not merely a hash checker** at this snapshot. Its DocMDP checker
  compares signed/current xref entries, expands changed object streams, classifies
  changes by object role and protects page/content/resources [D3]. However, xref
  access uses reflection over private parser fields (fallback: compare all objects).
  Policy exceptions include metadata and validation-data objects. These require
  adversarial review; source/tests are not proof against shadow/wrapping attacks.
  General approval-signature modification classification and FieldMDP coverage
  were not established here; do not infer them from DocMDP support.
- pyHanko's historical resolver/difference policy is substantially more developed
  than assembling a new Rust validator, but it deliberately reports some legitimate
  changes unsafe [P2]. Conservative rejection is preferable to false assurance.
- PAdES baseline levels are different deliverables: B-B basic signature; B-T trusted
  signature timestamp; B-LT embedded validation material (DSS); B-LTA adds archival
  document timestamps and requires later renewal. A timestamp token without TSA
  trust validation does not establish trusted signing time. LT/LTA support is not
  automatic long-term validity or a qualified/legal signature guarantee.

## Identity integration: Windows first, portable contract

**Verified Microsoft API behavior** [W1–W3]: enumerate/select certificate contexts
from the Windows certificate store; `CryptAcquireCertificatePrivateKey` can prefer
or require CNG and returns `CERT_NCRYPT_KEY_SPEC` for an `NCRYPT_KEY_HANDLE`.
`NCryptSignHash` signs using that key without requiring private-key export, supports
RSA PKCS#1/PSS padding parameters and a size-query call. Providers can require UI;
silent acquisition/signing can fail with `NTE_SILENT_CONTEXT`. Handle ownership is
returned separately: preserve certificate lifetime and free only when instructed.

**Proposed adapter, not yet verified end to end:** a dedicated Rust identity broker
owns certificate/key handles and exposes certificate chain, supported algorithms,
and an explicitly authorized signing operation to the PDF backend. For RSA/ECDSA,
pyHanko supplies the DER CMS signed-attributes bytes; the adapter hashes those once
and invokes CNG. Do **not** sign the PDF digest directly in place of signed attributes
or double-hash a supplied prehash. Bind every request to document digest, operation
ID, certificate, algorithm and user approval. Test PSS salt/algorithm parameters;
convert CNG ECDSA fixed-width `r || s` to CMS DER `(r,s)` with tested vectors [W3].
Return raw signature bytes; backend constructs CMS and locally verifies the result.

- Do not export non-exportable keys to PFX to accommodate a backend. Legacy CSP
  identities need a separate CryptoAPI adapter or an explicit unsupported message;
  never reinterpret a CSP handle as CNG. Smart-card/KSP UI, cancellation, token
  removal and user/machine-store access need real-device testing. CNG alone supplies
  neither PAdES nor PDF revision validation.
- PKCS#12: pyHanko supports software key loading [P3]. Password/PIN via protected
  local IPC only, not argv/environment/logs; minimize lifetime and acknowledge that
  Python/crypto libraries may copy secrets (zeroization is not guaranteed).
- PKCS#11: pyHanko optional `python-pkcs11` or broker-side Rust `cryptoki`; load only
  explicitly selected trusted provider paths. Match certificate/key by ID/public key,
  not label alone. Vendor DLL architecture, redistribution terms and mechanisms vary.
- Future adapters: macOS Keychain/SecKey; Linux PKCS#11/p11-kit plus PKCS#12.
  These are architectural extension points, **not verified native integrations**.
  Windows trust stores and EU trusted lists are different trust policies; do not
  assume a Python validator automatically mirrors the user's OS trust decisions.

## Trustworthy revision and save architecture (proposed)

1. **Separate service, lazy activation.** An optional signed, version-pinned backend
   package runs only for signature operations. No interpreter import, identity-store
   enumeration, token loading or network checks on unsigned startup/view. Local
   framed IPC (not an unauthenticated HTTP server), private temp files/read-only
   handles, schema/size limits, timeout, crash recovery and cancellation. Run parser/
   validator at reduced privilege; keep the key broker separately capability-limited.
   Revocation/TSA fetching needs explicit network policy: bound time/size/redirects,
   block local/private-network SSRF targets, and distinguish offline/unknown status.
2. **Bind analysis to exact immutable bytes.** Existing `src/document.rs::Snapshot`
   is suitable input, but its `u64` cache fingerprint (`DefaultHasher` in
   `src/cache.rs`) is not cryptographic evidence. On the activated path bind requests
   and results to document generation, SHA-256 of exact bytes, signature/revision
   identity, backend version, trust policy and validation time. Reject stale replies;
   invalidate trust results on policy changes and evidence expiry, not only edits.
3. **Validate structure as well as cryptography.** Check bounded/nonoverlapping
   `/ByteRange`, exact `/Contents` exclusion and full signed-revision coverage;
   parse CMS/signer binding, algorithms, attributes, chains, TSA and revocation.
   Resolve signature dictionaries, catalog `/Perms /DocMDP`, field locks and seed
   constraints in the authenticated historical revision, not only latest objects.
   Walk historical xref/trailer chains and signatures, including removed fields;
   classify later revisions, object reuse/aliasing, streams and changed reachability.
   Ambiguity, unsupported transforms, malformed history or parser disagreement is
   **indeterminate/unsafe**, never valid-by-default. Pixel comparison or append-only
   bytes alone cannot prove permitted modifications; test PDFium rendering agreement.
4. **Keep separate result dimensions.** UI shows cryptographic integrity, revision
   coverage/later modifications, DocMDP/FieldMDP permission result, chain trust,
   revocation status, trusted timestamp and unsupported/indeterminate reasons.
   “Signature valid on revision N” must not imply the current visible document is
   authenticated. Self-signed/embedded roots are not automatically trusted; claimed
   PDF/CMS signing time is not trusted time. Define roots, intermediates, EKU/key
   usage, algorithm cutoffs, historical revocation and offline policy explicitly.
5. **Signing is a transaction.** Save/reconcile ordinary pending edits first; then
   sign one immutable clean revision under an exclusive document-edit gate. Freeze
   field/appearance, reserve CMS space, append an incremental revision, fill only
   reserved bytes. Underestimation retries must not silently repeat hardware signing
   or user consent. Independently validate staged output before commit; confirm input
   generation/digest and preserve external-change checks and atomic replacement.
   Reload readers/render caches from the committed revision. Sign-to-copy initially;
   failure leaves source untouched. Keep PDFium on existing worker/helper owners.
6. **Signed saves need permission-aware routing.** Existing ordinary markup writes
   use `IncrementalDocument` (`crates/pdf-io/src/write.rs`); incremental does not mean
   DocMDP-safe: layers, metadata, page resources and custom measurement data can
   exceed allowed changes. Audit each command's actual object changes. Structural
   saves/flattening/content edits must not silently rewrite signed input. Initially
   permit validated additional signatures/LTV operations only; otherwise block or
   offer an explicitly unsigned derived copy. Preserve session undo and concurrent
   ordinary-save reconciliation; do not graft signing into its editable-save path.

Cheap current-field inspection is **not proof that a document has no historical
signatures**. Piggyback a bounded metadata hint on existing worker parsing, labelled
unknown until analyzed; do not scan entire history on every startup/view. Before
claiming any signature-preserving save, require full analysis. The policy for unknown
ordinary-save inputs (explicit warning/derived copy versus mandatory analysis) is a
product/security gate: do not silently trade away revision checking to meet a latency
claim. No unsigned hot-path regression guarantee is established by this design alone.

## Dependency, licensing and performance consequences

- Digitorus: BSD-2-Clause [D4]; Go runtime/toolchain and transitive modules, plus
  native PKCS#11 vendor libraries when enabled. Inspected `go.mod` requires Go 1.27
  and specific PDF/PKCS7/timestamp/x/crypto modules [D5]. A helper avoids linking the
  Go runtime into the UI executable, but adds a distributable binary and maintenance.
- pyHanko: MIT, retains PyPDF2 notices; Python >=3.11. Inspected manifest includes
  `asn1crypto`, `tzlocal`, `pyhanko-certvalidator`, `aiohttp`, `cryptography`, `lxml`;
  PKCS#11/fonts/images/reporting add optional dependencies [P4]. Bundling entails a
  Python runtime, native crypto/XML libraries (and their notices), platform-specific
  packaging, updates and a larger download. `python-pkcs11` is MIT. Audit the exact
  wheels/runtime/OpenSSL/native library SBOM; the top-level MIT license is not enough.
- Rust pieces: `lopdf` MIT, RustCrypto `cms` MIT OR Apache-2.0, `cryptoki` Apache-2.0
  [R2–R4]; bindings do not grant redistribution rights to vendor token drivers.
  None eliminates trust-policy or security-review costs. No copyleft requirement was
  identified in the inspected top-level candidates; transitive license clearance is
  still open. Separating a process does not erase license obligations.
- **Target: no measurable unsigned startup/view/save regression, not literal zero
  cost.** Optional packaging/lazy launch prevents eager backend overhead, not all
  dispatch, IPC, disk, scheduler, memory or antivirus costs. Measure installer size
  separately from unsigned runtime. A strict single-executable distribution conflicts
  with the proposed Python package unless packaging requirements change.
- Activated operations require O(file bytes) hashing/I/O; history/difference checks
  scale with revisions/objects and may repeat work per signature. Digitorus's inspected
  signing path buffers/copies the original and may retry placeholder sizing; its
  verification loads signed byte ranges into memory [D2]. Python worker cold start
  and runtime/native libraries add latency/RSS. Hardware PIN prompts and TSA/OCSP/CRL
  latency may dominate (expectation, **not benchmarked**). Bound CPU/memory and network;
  avoid unnecessary whole-file IPC copies; do not budget security by skipping checks.

## Release gates / blockers

- **G0 — scope/legal:** choose supported PAdES levels (start B-B/B-T), signed-save
  behavior, P=3 requirement, roots/offline/time policy and packaging/Store constraints.
  Pin released versions, SBOM, notices, provenance and update/support ownership.
- **G1 — isolated spike:** PKCS#12, software CNG RSA/ECDSA, non-exportable KSP and
  real smart-card/PKCS#11 identities; PSS/encoding vectors, consent, cancellation,
  token removal, crash handling. Windows Store and macOS/Linux packaging trials.
- **G2 — security/interoperability:** independent DSS plus Acrobat verification and
  cross-backend fixtures; B-B/T and later LT/LTA with offline evidence/renewal.
  Test P=1/2/3, FieldMDP, multiple/removed signatures, permitted and forbidden updates,
  metadata/resource aliasing, malformed/overlapping byte ranges, duplicate dictionaries,
  xref/object streams, wrapping/shadow attacks, encrypted/repaired PDFs, untrusted
  roots, expired/revoked certs, forged TSA and unavailable revocation. Fuzz and obtain
  independent security review. Agreement between validators alone is not a proof.
- **G3 — app correctness:** snapshot races, edits during ordinary save, signing gate,
  external replacement, rollback, undo, cache invalidation and current/revision UI.
  Unknown/unsupported must never become green; PDFium visual parser disagreement
  blocks an authoritative current-document safety claim.
- **G4 — performance:** paired release-build baseline/candidate runs on the same
  machines: cold/warm unsigned startup, first-page/scroll latency, ordinary and
  structural saves, peak RSS/CPU/I/O; large files, many revisions/signatures, offline
  network and idle-after-signing cases. Define noise/equivalence budgets and p50/p95
  before testing; require sufficient repeats to detect regressions. Do not ship until
  unsigned overhead is within the agreed detection/budget limits.

No builds, signing trials, benchmarks or interactive checks were run: documentation
research only. Production selection remains conditional on these gates, especially
P=3 validation, hostile-revision review and native-identity/packaging qualification.

## Sources

- **D1:** [Digitorus README](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/README.md), [formats/API](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/types.go).
- **D2:** [Signing implementation](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/sign/sign.go), [signature verification](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/verify/signature.go).
- **D3:** [DocMDP change checker](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/verify/docmdp_changes.go), [historical signature discovery](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/verify/signatures.go).
- **D4:** [BSD license](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/LICENSE), [PKCS#11 source](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/signers/pkcs11/pkcs11.go), [CLI key loading](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/cli/sign.go).
- **D5:** [Go dependencies](https://github.com/digitorus/pdfsign/blob/9483e7201db2d13dba9a835bc849c7d19f280a16/go.mod).
- **P1:** [pyHanko README/features and beta warning](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/README.md).
- **P2:** [Known issues](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/docs/known-issues.rst), [historical difference policies](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/pkgs/pyhanko/src/pyhanko/sign/diff_analysis/policies.py).
- **P3:** [Signing/integration guide](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/docs/lib-guide/signing.rst), [CMS/key loader and external signer source](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/pkgs/pyhanko/src/pyhanko/sign/signers/pdf_cms.py).
- **P4:** [Package dependencies/licenses](https://github.com/MatthiasValvekens/pyHanko/blob/0b27455d49ecde3d2853f598558f76d1af38cbf1/pkgs/pyhanko/pyproject.toml), [python-pkcs11 license](https://github.com/pyauth/python-pkcs11/blob/master/LICENSE).
- **R1:** [Rust pdf_signing README](https://github.com/ralpha/pdf_signing/blob/master/README.md), [signing source](https://github.com/ralpha/pdf_signing/blob/master/src/digitally_sign.rs), [dependencies](https://github.com/ralpha/pdf_signing/blob/master/Cargo.toml).
- **R2:** [lopdf incremental API](https://docs.rs/lopdf/latest/lopdf/struct.IncrementalDocument.html), [repository/license](https://github.com/J-F-Liu/lopdf).
- **R3:** [RustCrypto CMS scope/license](https://github.com/RustCrypto/formats/tree/master/cms), [cryptographic-message-syntax audit caveat](https://github.com/indygreg/cryptography-rs).
- **R4:** [Rust cryptoki dependencies/license](https://github.com/parallaxsecond/rust-cryptoki/blob/main/cryptoki/Cargo.toml).
- **W1:** [Certificate-store enumeration](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-certenumcertificatesinstore), [CryptAcquireCertificatePrivateKey](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-cryptacquirecertificateprivatekey).
- **W2:** [NCryptSignHash](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptsignhash).
- **W3:** [CNG signing example](https://learn.microsoft.com/en-us/windows/win32/seccng/signing-data-with-cng), [P1363 versus DER signature formats](https://learn.microsoft.com/en-us/dotnet/api/system.security.cryptography.dsasignatureformat).
- **Standards:** [CMS RFC 5652](https://www.rfc-editor.org/rfc/rfc5652), [RFC 3161](https://www.rfc-editor.org/rfc/rfc3161), [ETSI PAdES EN 319 142-1 publications](https://www.etsi.org/deliver/etsi_en/319100_319199/31914201/), [PDF 1.7 / ISO 32000-1 §12.8](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf).
