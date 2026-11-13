# Digital signatures: Windows preview and roadmap

Status: a native Windows signing preview is implemented, not release-qualified.
It adds a Signatures panel, encrypted PFX creation/import, Windows-protected opt-in
password storage, visible signature appearances and P=1 certification to a new copy.
The multi-page draft-then-certify workflow, scaled text, Certify-existing-drafts
and automatic-open polish are implemented. The lower-level API
retains invisible approval signing and explicit CMS integrity results. Native/client
lifecycle tests and OpenSSL/pyHanko interoperability checks passed. The preview
window was launched and inspected; the toolbar gap is removed and the panel is
compact, with a certificate/seal icon and modal certificate details. The updated
UI/palette/layout checks passed (35 tests). Full GUI creation/signing interaction
is not yet qualified.
The earlier safety-only frozen source passed 685 workspace tests (5 ignored);
those counts and its all-targets/Store checks do not cover the new signing preview.
The final milestone snapshot passed **730 workspace tests (7 ignored)**, the
separate Store check and the workspace all-targets check. Native vault save/load/
forget, multi-page raster/rotation/crop, scalable-text and OpenSSL/pyHanko checks
passed. After headless-test texture cleanup was made unwind-safe, 40 baseline
parallel/serial runs and five final parallel runs passed without reproducing the
earlier test-process abort. This does not establish general parser crash isolation.
Focused UI/prefs/palette/layout checks passed. Preview windows were launched and
the toolbar-gap fix was visually inspected; full manual signing interaction remains
an explicit qualification limit.
Earlier release measurements still show ~1.5 ms extra form render-plus-edit readiness;
no-regression is not accepted. Full trust/revision validation remains gated.
Scope: Windows now; portable contracts for future macOS/Linux.
Target: **no measurable unsigned startup/view/save regression**, not literally zero cost.

## Windows preview workflow and limits

- Open **Signatures** from the tool rail or Tools menu, even without a PDF.
- Choose **Create new…** in the Digital ID dropdown. The modal generates a new
  non-CA, self-signed RSA-2048/SHA-256
  identity and password-protected PBES2/AES-256 PFX. Use at least eight password
  characters (twelve recommended), keep a backup, and never share the PFX/private key.
  A self-created identity is not independently trusted; share the signed PDF instead.
- Alternatively open an existing supported RSA PFX. Imports are memory-only and
  do not install certificates/keys in certificate stores or export imported private keys.
- **Remember password on this device** is opt-in after successful authentication.
  Windows Credential Manager protects it for the current account on this computer;
  other processes/people using that account can use it. Preferences store only bounded
  public identity metadata. **Forget saved password** removes that exact credential,
  not the PFX file. File-path aliases have separate bindings.
- Save the unsigned PDF and finish edits first. **Sign** places draft watermark/name/
  reason blocks across pages; they are previews, not digital signatures. **Certify**
  seals existing drafts without another block (or certifies invisibly when no drafts
  exist). **Sign & Certify**
  draws the final block first. Both create one document-wide P=1 certification
  covering every finalized appearance. Certification must be the first digital
  signature; repeated visual blocks are not separate approvals or co-signatures.
- Unsealed drafts are session-only and must be certified or explicitly discarded;
  an ordinary PDF save does not preserve them. The original file stays unchanged,
  existing output paths are not overwritten, and a successfully certified copy
  opens in the same window. The newly opened immutable snapshot is verified again;
  the creation receipt is not treated as proof of bytes read later from a path.
  Unsigned revision history may be
  normalized in the new copy; earlier signature history is not preserved or validated.
  Signed, encrypted, malformed or ambiguous sources are refused rather than stripped.
- The operation binds exact snapshot bytes and the inspected certificate fingerprint.
  Results for the output copy are labelled separately from verification of the original.
- **Verify** separates CMS integrity, later changes and identity trust. Trust is
  **NotChecked**. Intact earlier signed bytes do not authenticate later modifications.
- This is standard detached CMS (`adbe.pkcs7.detached`), not a PAdES/QES/LTV claim.
  No co-signing, P=2/P=3 editing, chain/revocation validation, trusted timestamps,
  hardware/certificate-store identity selection, or macOS/Linux signing yet.
  Visible appearances use `assets/signature-watermark.svg`, actual certificate
  text and embedded Unicode fonts. Preferred text size scales with displayed box
  area (10 pt at 220×80 pt, bounded to 6–48 pt), then fits the actual metadata;
  unreadably small text is refused. Appearance bytes are prepared before CMS hashing;
  the displayed signing date is signer-asserted device time, not a trusted timestamp.
- `src/signing/` uses lazy System32 Windows crypto in an owned helper process;
  passwords travel through bounded stdin, never arguments, logs or preferences.
  The parent assigns a kill-on-close Windows Job before sending requests. Heavy
  signing/verification parsing is isolated; the older passive inventory still has
  the separate release-panic caveat below. Python/OpenSSL are developer oracles only.

## Design references

- [Backend evaluation and sources](signature-backend-evaluation.md): candidates,
  licenses, native identities, trust/revision analysis and qualification limits.
- [Save-path safety audit](signature-save-audit.md): rewriting paths, bypasses,
  discovery limitations and focused test cases. Verify line references against source.
- [Interoperability evidence](signature-interoperability.md): supplied approval-signature
  samples preserve signed bytes after incremental markup; this is not certification,
  trust validation or proof that arbitrary later changes are permitted.
- [Isolated pyHanko qualification](signature-backend-qualification.md): synthetic
  signing/certification evidence and remaining qualification gaps; not a runtime dependency.
- [Release worker measurements](signature-performance.md): ordinary PDF comparisons,
  unsigned-form regression, controls, and limitations.
- [Experimental Rust/CNG broker](signature-native-broker.md): ephemeral-key and PDF bridge
  evidence. That earlier experiment used a Python assembler; the current Windows
  preview in `src/signing/` assembles PDFs in Rust and CMS through Windows crypto.

## Architecture and backend decision

- Keep PDF policy, transaction/lifecycle and result models UI-independent; put costly
  parsing, cryptography and network work in an on-demand isolated backend.
- The **Windows preview** uses Rust/lopdf PDF assembly and Windows CNG/CryptoAPI
  CMS/PFX operations. It is not a qualified complete historical revision validator.
  The full portable verification/trust/LTV backend remains undecided. pyHanko is an
  isolated developer oracle, not a shipping dependency; upstream labels it beta/not
  production-ready. Inspected Digitorus source rejects PAdES LT/LTA.
- **DocMDP P=3 is a qualification blocker:** pyHanko cannot currently approve later
  annotation changes through its standard difference policy. Do not equate signature
  integrity or prefix preservation with permission to annotate a certified document.
  Qualify a reviewed alternative/extension (including commercial SDKs if needed).
- A portable provider contract separates identities from PDF/CMS policy: Windows
  memory-only PKCS#12/CNG now; future Windows store, PKCS#11, macOS Keychain/SecKey
  and Linux providers.
  Keep non-exportable keys in their provider; define message versus prehash precisely.
- Bind operations to exact immutable snapshot bytes, cryptographic digest, generation
  and operation ID. Cache fingerprints are routing hints, not cryptographic evidence.

## Phases and acceptance gates
### 1. Protect existing signed documents (code/tests complete; UI/performance gates pending)

- Persistent session/UI read-only policy and worker Save/SaveAs/Extract guards.
  Checked native form/signature hints keep no-form inputs on the no-parser fast path.
  AcroForms get bounded, deferred V/inheritance inventory, including nested signatures
  missed by PDFium's top-level count. Missing/null V is unsigned; ambiguity locks.
- One lazy service keeps one running job plus one latest pending job; stale work is
  cancelled. Worker authority becomes ready before the revision-bound UI result.
  Rendering can proceed while edits stay locked. Render release/headless fallback
  moves parsing cost and delays editability; it does not eliminate work.
- Present/unknown stays locked. Discovery is not validation or proof that historical,
  orphaned, malformed or unsupported signatures are absent.
- Gate: direct worker/UI tests; failure/unblock cannot clear protection; viewing/search/
  print remain usable. Refusal leaves signed sources and existing targets unchanged.
- `output_safety::inspect_destination` captures exact destination bytes/identity (or
  absence). Overlay protects both source aliases and publishes via guard.commit; worker
  distinct exports use the same guard. Ordinary same-source saves must not invoke it.
- Gate: populated/ambiguous destination and source-alias refusals, final byte/identity
  recheck, atomic replacement, absent-target no-clobber and Windows live-handle tests.
  Existing targets >256 MiB, unreadable/encrypted PDFs and recognizable history are refused.
- Output inspection uses strict lopdf/current objects plus conservative history hints,
  not a full malformed/escaped-history validator. Final-check-to-rename writes/path swaps
  remain possible (not compare-and-swap); no crash-durability guarantee. New-file no-clobber
  may leave a temporary hard link on cleanup failures/crashes. These are explicit
  limitations; universal signature-safe saving remains blocked pending stronger analysis.

### 2. Qualify backend, packaging and identity adapters
- Isolated spike only: pin releases, SBOM/notices, provenance, update/support ownership;
  prove Windows Store/distribution feasibility and macOS/Linux packaging strategy.
- Gate: PKCS#12, CNG RSA/ECDSA/PSS and non-exportable smart-card/KSP plus PKCS#11 trials;
  encoding vectors, provider UI/consent, cancellation, removal and crash recovery.
- Release approval still requires feature/security and P=3 decisions. The current
  Windows preview is intentionally narrower, not evidence this full gate passed.
  Document supported profiles/unsupported operations; keep developer oracles unshipped.

### 3. Revision-aware verification and honest status UI
- Validate CMS/PAdES, exact ByteRange/Contents coverage, historical xref/object graphs,
  removed signatures, DocMDP/FieldMDP, certificate chains, timestamps and revocation.
- Gate: adversarial fixtures/fuzzing and independent review; cross-check DSS/Acrobat;
  cover malformed ranges, wrapping/shadow attacks, object streams, encrypted/repaired
  PDFs and PDFium rendering disagreement. Ambiguous/unsupported stays indeterminate.
- Report integrity, signed revision/later changes, allowed modifications, identity trust,
  revocation and trusted time separately. Never label current content signed merely
  because an earlier revision's CMS verifies. Network fetching must be bounded/SSRF-safe.

### 4. Approval and certification signing transactions
- The preview implements standard detached CMS approval/P=1 sign-to-copy, not PAdES.
  Future PAdES B-B/B-T must freeze edits/arrangement on a clean snapshot, prepare an incremental
  revision/appearance, finalize ranges, authorize provider signing and fill reserved CMS.
- Gate: independent staged-output verification, unchanged signed prefix, atomic commit,
  external-change/stale-result rejection and failure rollback. Preserve unsigned ordinary
  save concurrency/undo and existing worker/helper PDFium ownership. Sign-to-copy first.
- Certification requires authenticated catalog/transform semantics, not a visual stamp.
  Test multiple signatures and field locks; reservation retries cannot silently repeat consent.

### 5. Permitted signed edits, co-signing and long-term evidence
- Enable only command/object changes qualified against DocMDP/FieldMDP and prior signatures;
  layers, measurement metadata, resources and structural edits are not automatically safe.
- Gate P=3 annotation support independently; otherwise keep those edits blocked. Explicit
  unsigned derivatives must remove misleading signature/certification claims and protect sources.
- Add B-LT DSS/revocation material and B-LTA timestamps/renewal only after independent
  offline/time-policy tests. Historical validity and legal qualification are separate claims.

## Gates applying to every phase / current non-goals
- **Passive-inventory release recovery blocker:** release/quick use `panic=abort`.
  The older discovery thread's `catch_unwind` cannot recover parser panics there.
  The new signing/verification helper is process-isolated, but that does not fix the
  separate passive inventory path. Do not claim panic-to-Unknown recovery there.
- Paired release-build cold/warm unsigned startup, view/scroll, normal/structural save,
  RSS/CPU/I/O benchmarks; predeclare noise/budgets and p50/p95 criteria. Lazy activation
  is a design mechanism, not evidence of no regression. Interactive UI checks are required.
- No automatic trusted/valid badge from discovery, embedded roots or signer-asserted time;
  no silent signature-breaking save, permissive P=3 shortcut or proprietary-code copying.
- The initial lock milestone's evidence must not be confused with the later Windows
  signing preview. Neither establishes robust historical absence proof, complete
  modification-policy/trust validation, permitted signed editing, or LTV.
