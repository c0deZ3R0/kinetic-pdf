//! Native SHA-256 detached CMS for standard PDF adbe.pkcs7.detached signatures.
//!
//! PATH: src/signing/cms.rs
//! CONSUMED BY: signing PDF helper. Integrity verification uses embedded certificates
//! only, never chain/network trust. No PAdES, timestamps, LTV or QES claim.

use super::{identity::Identity, IdentityInfo};

pub fn sign_detached(identity: &Identity, data: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)] { platform::sign(identity, data) }
    #[cfg(not(windows))] { let _ = (identity, data); Err("Digital signing requires Windows.".into()) }
}

pub fn verify_detached(cms: &[u8], data: &[u8]) -> Result<IdentityInfo, String> {
    #[cfg(windows)] { platform::verify(cms, data) }
    #[cfg(not(windows))] { let _ = (cms, data); Err("Digital signature verification requires Windows.".into()) }
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;
    use std::ptr::{null, null_mut};
    use super::*;
    use crate::signing::native::{self, c, Api, Cert};

    const MAX_CMS_BYTES: usize = 4 * 1024 * 1024;
    const ENCODING: u32 = c::X509_ASN_ENCODING | c::PKCS_7_ASN_ENCODING;

    fn bounds(data: &[u8]) -> Result<(), String> {
        if data.len() > crate::signing::MAX_PDF_BYTES { return Err("Signed content exceeds the supported 256 MiB bound.".into()); }
        Ok(())
    }

    pub(super) fn sign(identity: &Identity, data: &[u8]) -> Result<Vec<u8>, String> {
        bounds(data)?;
        identity.ensure_signing_allowed()?;
        let native = &identity.native;
        let api = &native.api;
        let mut certificates: Vec<*mut c::CERT_CONTEXT> = native.certificates.iter().map(|c| c.raw.cast_mut()).collect();
        let params = c::CRYPT_SIGN_MESSAGE_PARA {
            cbSize: size_of::<c::CRYPT_SIGN_MESSAGE_PARA>() as u32, dwMsgEncodingType: ENCODING,
            pSigningCert: native.cert.raw,
            HashAlgorithm: c::CRYPT_ALGORITHM_IDENTIFIER { pszObjId: c::szOID_NIST_sha256.cast_mut(), ..Default::default() },
            cMsgCert: certificates.len() as u32, rgpMsgCert: certificates.as_mut_ptr(),
            dwFlags: c::CRYPT_MESSAGE_SILENT_KEYSET_FLAG, ..Default::default()
        };
        let pointers = [data.as_ptr()]; let lengths = [data.len() as u32];
        let mut size = 0;
        // SAFETY: identity retains the in-memory cert/key/store and DLL table;
        // both source arrays/data remain immutable for the synchronous calls.
        native::ok(unsafe { (api.CryptSignMessage)(&params, 1, 1, pointers.as_ptr(), lengths.as_ptr(), null_mut(), &mut size) }, "Size detached CMS signature")?;
        if size == 0 || size as usize > MAX_CMS_BYTES { return Err("CMS signature exceeds its supported size bound.".into()); }
        let mut output = vec![0u8; size as usize];
        native::ok(unsafe { (api.CryptSignMessage)(&params, 1, 1, pointers.as_ptr(), lengths.as_ptr(), output.as_mut_ptr(), &mut size) }, "Sign detached CMS")?;
        if size == 0 || size as usize > output.len() { return Err("Windows returned an invalid CMS length.".into()); }
        output.truncate(size as usize);
        // Cached PFX key associations are not assumed correct: verify against
        // the embedded certificate before allowing this CMS to leave the job.
        let verified = verify(&output, data)?;
        if verified.sha256_fingerprint != identity.info().sha256_fingerprint {
            return Err("The private key did not sign as the selected PFX identity.".into());
        }
        Ok(output)
    }

    pub(super) fn verify(cms: &[u8], data: &[u8]) -> Result<IdentityInfo, String> {
        bounds(data)?;
        if cms.is_empty() || cms.len() > MAX_CMS_BYTES { return Err("CMS signature is empty or exceeds 4 MiB.".into()); }
        let api = Api::load()?;
        // Reject ambiguous multi-signer envelopes, not just a valid signer zero
        // hiding other invalid signers. No certificate stores are searched.
        if unsafe { (api.CryptGetMessageSignerCount)(ENCODING, cms.as_ptr(), cms.len() as u32) } != 1 {
            return Err("Expected exactly one signer in the detached CMS signature.".into());
        }
        let params = c::CRYPT_VERIFY_MESSAGE_PARA { cbSize: size_of::<c::CRYPT_VERIFY_MESSAGE_PARA>() as u32,
            dwMsgAndCertEncodingType: ENCODING, ..Default::default() };
        let pointers = [data.as_ptr()]; let lengths = [data.len() as u32];
        let mut certificate = Cert { raw: null(), api: api.clone() };
        let mut returned = null_mut();
        // Default signer callback searches only the message's embedded certs.
        // This checks CMS math, not expiry, key size policy, revocation or trust.
        let result = unsafe { (api.CryptVerifyDetachedMessageSignature)(&params, 0, cms.as_ptr(), cms.len() as u32,
            1, pointers.as_ptr(), lengths.as_ptr(), &mut returned) };
        certificate.raw = returned;
        native::ok(result, "Verify detached CMS integrity")?;
        crate::signing::identity::describe_certificate(&api, certificate.raw)
    }
}
