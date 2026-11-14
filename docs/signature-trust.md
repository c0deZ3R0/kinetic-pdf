# Windows certificate-trust checks

Current-time, cache-only certificate assessment for the Signatures panel.
Implemented in `src/signing/trust.rs`, consumed by PDF verification in the isolated
signing helper. It is separate from CMS integrity and PDF modification policy.

## Results

- **Trusted:** chain, validity, document-signing usage and algorithm checks all
  passed; revocation evidence was affirmatively good for every non-anchor
  certificate; no unresolved issues remain.
- **Untrusted:** a required check failed, or revocation was established.
- **Indeterminate:** incomplete chain, missing/stale revocation evidence,
  unsupported policy/algorithm or an operational failure prevents a conclusion.
- **Not checked:** no certificate assessment was made. Invalid CMS/ranges never
  receive a trusted certificate result.

The UI keeps certificate assessment, signed-byte integrity and later changes
separate. A trusted certificate does not authenticate bytes added after signing.
A genuine self-signed signature can still be saved and mathematically verified;
it does not become trusted merely because its certificate is embedded.

## Policy and privacy

- Uses Windows' current-user configured chain engine, including applicable
  machine roots. Embedded CMS certificates may supply issuers, never trust anchors.
- Checks the current system time. PDF/CMS claimed signing dates do not rescue an
  expired certificate or establish historical trust. Trusted signing time is not
  established; no trusted timestamp/LTV/QES claim is made.
- Recognizes RFC 9336 document signing, Microsoft document signing and Adobe
  authentic-document trust EKUs. Absent usage restrictions are unconstrained;
  explicit unrelated-only usages are rejected, unknown-only usages remain
  indeterminate. Leaf key usage must permit digital signature/content commitment
  when a key-usage extension exists.
- The initial explicit algorithm policy supports RSA 2048–8192 with SHA-2.
  Known weak algorithms fail; unsupported algorithms are indeterminate rather
  than silently accepted. Windows' strong-signature policy also applies.
- Chain and revocation URL retrieval are cache-only. AIA fetching, automatic root
  updates and MY-store peer trust are disabled. Revocation excludes the anchor.
  No online verification mode is exposed, so offline/cache misses commonly remain
  indeterminate rather than turning green.
- No root installation, trust-store modification, private-key access or credential
  enumeration. Certificate material, chain sizes, diagnostics and helper lifetime
  are bounded. The existing owned helper process supplies the hard deadline.

## Qualification

Focused checks passed: 13 native/CMS/importer tests, 3 PDF integrity/trust
separation tests, 2 real helper-service tests and 27 Signatures UI tests. Existing
native raster, OpenSSL and pyHanko checks passed for nine generated PDF outputs.
After review fixes, final validation passed: 749 workspace tests (7 ignored),
separate Store and all-targets checks, and 28 Signatures UI tests. The user confirmed
manual signing checks passed. See `SIGNATURES.md` for qualification limits.

Pre-publication verification checks integrity without invoking certificate trust;
creation receipts report trust as not checked. Normal verification, including the
automatically reopened copy, separately assesses trust. Tests prove neither
pre-publication path invokes trust while normal verification does.

Native trust fixtures use ephemeral synthetic keys, exclusive in-memory roots
and CRLs. They do not install certificates in Windows stores. Cases cover missing
issuers, untrusted roots, usage restrictions, weak/bad signatures, expiry/future
validity, missing/stale/invalid/revoked evidence and unknown errors. Initial test
engine failures were fixed by using valid exclusive-memory engine configuration;
production engine configuration was unchanged.

This is not a complete hostile-PDF, historical-revision, permitted-change or
network-revocation qualification. Online checks need a separate constrained fetch
design, including destination/redirect/DNS rules, authentication privacy and limits.

## References

- [RFC 9336: Document-signing EKU](https://www.rfc-editor.org/rfc/rfc9336.html)
- [CertGetCertificateChain](https://learn.microsoft.com/windows/win32/api/wincrypt/nf-wincrypt-certgetcertificatechain)
- [CertVerifyCertificateChainPolicy](https://learn.microsoft.com/windows/win32/api/wincrypt/nf-wincrypt-certverifycertificatechainpolicy)
- [Adobe security OIDs](https://www.adobe.com/devnet-docs/acrobatetk/tools/DigSigDC/oids.html#security-oids)
