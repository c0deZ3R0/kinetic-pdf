//! Native SHA-256 detached CMS for standard PDF adbe.pkcs7.detached signatures.
//!
//! PATH: src/signing/cms.rs
//! CONSUMED BY: signing PDF helper and current-time certificate-trust evaluator.
//! Integrity uses embedded certs only; bounded public material can separately feed
//! cache-only trust checks. No PAdES, trusted timestamps, LTV or QES claim.

use super::{identity::Identity, IdentityInfo};

pub(crate) const MAX_CERTIFICATES: usize = 64;
pub(crate) const MAX_CERTIFICATE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_MATERIAL_BYTES: usize = 4 * 1024 * 1024;

/// Public certificate material for the signer whose CMS mathematics passed.
/// This is not a trust result and contains neither private keys nor passwords.
pub(crate) struct VerifiedSignerMaterial {
    pub info: IdentityInfo,
    pub signer_der: Vec<u8>,
    pub supporting_certificates_der: Vec<Vec<u8>>,
    pub digest_algorithm_oid: String,
    pub signature_algorithm_oid: String,
}

pub(crate) fn verify_detached_with_material(cms: &[u8], data: &[u8]) -> Result<VerifiedSignerMaterial, String> {
    #[cfg(windows)] { platform::verify_material(cms, data) }
    #[cfg(not(windows))] { let _ = (cms, data); Err("Digital signature verification requires Windows.".into()) }
}

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
    use crate::signing::native::{self, c, Api, Cert, Store};

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
        let certificate = verified_certificate(cms, data)?;
        crate::signing::identity::describe_certificate(&certificate.api, certificate.raw)
    }

    fn verified_certificate(cms: &[u8], data: &[u8]) -> Result<Cert, String> {
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
        Ok(certificate)
    }

    fn certificate_der(certificate: *const c::CERT_CONTEXT) -> Result<Vec<u8>, String> {
        if certificate.is_null() { return Err("CMS certificate is missing.".into()); }
        let certificate = unsafe { &*certificate };
        if certificate.pbCertEncoded.is_null() || certificate.cbCertEncoded == 0 || certificate.cbCertEncoded as usize > MAX_CERTIFICATE_BYTES {
            return Err("CMS certificate exceeds its public-material bound.".into());
        }
        Ok(unsafe { std::slice::from_raw_parts(certificate.pbCertEncoded, certificate.cbCertEncoded as usize) }.to_vec())
    }

    struct Message { raw: *mut std::ffi::c_void, api: std::sync::Arc<Api> }
    impl Drop for Message {
        fn drop(&mut self) { if !self.raw.is_null() { unsafe { (self.api.CryptMsgClose)(self.raw); } } }
    }

    fn oid(pointer: *const u8) -> Result<String, String> {
        if pointer.is_null() { return Err("CMS algorithm identifier is missing.".into()); }
        let mut bytes = Vec::new();
        for index in 0..128 {
            let byte = unsafe { *pointer.add(index) };
            if byte == 0 {
                if bytes.is_empty() { break; }
                return String::from_utf8(bytes).map_err(|_| "CMS algorithm identifier is invalid.".into());
            }
            if !byte.is_ascii_digit() && byte != b'.' { break; }
            bytes.push(byte);
        }
        Err("CMS algorithm identifier is invalid or too long.".into())
    }

    pub(super) fn verify_material(cms: &[u8], data: &[u8]) -> Result<VerifiedSignerMaterial, String> {
        let certificate = verified_certificate(cms, data)?;
        let api = &certificate.api;
        let info = crate::signing::identity::describe_certificate(api, certificate.raw)?;
        let signer_der = certificate_der(certificate.raw)?;
        let blob = c::CRYPT_INTEGER_BLOB { cbData: cms.len() as u32, pbData: cms.as_ptr().cast_mut() };
        let mut store = Store { raw: null_mut(), api: api.clone() };
        let mut message = Message { raw: null_mut(), api: api.clone() };
        // Query only the already mathematically verified, bounded binary CMS.
        // CTLs/CRLs in its store are never transferred into the trust evaluator.
        native::ok(unsafe { (api.CryptQueryObject)(c::CERT_QUERY_OBJECT_BLOB, (&blob as *const c::CRYPT_INTEGER_BLOB).cast(),
            c::CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED, c::CERT_QUERY_FORMAT_FLAG_BINARY, 0, null_mut(), null_mut(), null_mut(),
            &mut store.raw, &mut message.raw, null_mut()) }, "Read verified CMS public certificates")?;
        if store.raw.is_null() || message.raw.is_null() { return Err("Windows returned incomplete CMS public material.".into()); }
        let mut size = 0;
        native::ok(unsafe { (api.CryptMsgGetParam)(message.raw, c::CMSG_SIGNER_INFO_PARAM, 0, null_mut(), &mut size) }, "Size CMS signer algorithms")?;
        if size < size_of::<c::CMSG_SIGNER_INFO>() as u32 || size as usize > MAX_CMS_BYTES { return Err("CMS signer information exceeds its bound.".into()); }
        let capacity = size as usize;
        let mut storage = vec![0usize; capacity.div_ceil(size_of::<usize>())];
        native::ok(unsafe { (api.CryptMsgGetParam)(message.raw, c::CMSG_SIGNER_INFO_PARAM, 0, storage.as_mut_ptr().cast(), &mut size) }, "Read CMS signer algorithms")?;
        if size < size_of::<c::CMSG_SIGNER_INFO>() as u32 || size as usize > capacity { return Err("Windows returned invalid CMS signer information.".into()); }
        let signer = unsafe { &*storage.as_ptr().cast::<c::CMSG_SIGNER_INFO>() };
        let digest_algorithm_oid = oid(signer.HashAlgorithm.pszObjId)?;
        let signature_algorithm_oid = oid(signer.HashEncryptionAlgorithm.pszObjId)?;
        let mut cursor = Cert { raw: null(), api: api.clone() };
        let mut supporting_certificates_der = Vec::new();
        let mut total = signer_der.len();
        let mut count = 0;
        loop {
            let previous = std::mem::replace(&mut cursor.raw, null());
            cursor.raw = unsafe { (api.CertEnumCertificatesInStore)(store.raw, previous) };
            if cursor.raw.is_null() {
                let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                if code != 0x80092004 { return Err(format!("Read CMS certificates failed (0x{code:08x}).")); }
                break;
            }
            count += 1;
            if count > MAX_CERTIFICATES { return Err("CMS contains too many certificates.".into()); }
            let der = certificate_der(cursor.raw)?;
            if der == signer_der || supporting_certificates_der.contains(&der) { continue; }
            total = total.checked_add(der.len()).ok_or("CMS public material size overflow.")?;
            if total > MAX_MATERIAL_BYTES { return Err("CMS public material exceeds its bound.".into()); }
            supporting_certificates_der.push(der);
        }
        Ok(VerifiedSignerMaterial { info, signer_der, supporting_certificates_der, digest_algorithm_oid, signature_algorithm_oid })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use sha2::{Digest, Sha256};

        #[test]
        fn actual_verified_material_is_the_signing_certificate_and_has_no_private_properties() {
            let spec = crate::signing::CreateIdentitySpec { name: "Synthetic CMS material".into(), organization: String::new(), email: String::new(), valid_years: 1 };
            let password = "Synthetic memory-only password";
            let (pfx, info) = crate::signing::identity::create_pfx(&spec, password).unwrap();
            let identity = Identity::from_pfx(&pfx, password).unwrap();
            let data = b"Synthetic material extraction only";
            let cms = sign(&identity, data).unwrap();
            let material = verify_material(&cms, data).unwrap();
            let fingerprint: String = Sha256::digest(&material.signer_der).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(fingerprint, info.sha256_fingerprint);
            assert_eq!(material.info, info);
            assert_eq!(material.digest_algorithm_oid, "2.16.840.1.101.3.4.2.1");
            assert!(matches!(material.signature_algorithm_oid.as_str(), "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.11"));
            assert!(material.supporting_certificates_der.is_empty(), "The signer must not be duplicated as a supporting certificate");
            assert_eq!(verify(&cms, data).unwrap(), info, "Integrity-only API is unchanged");
            assert!(verify_material(&cms, b"tampered synthetic data").is_err());
            let certificate = unsafe { (identity.native.api.CertCreateCertificateContext)(c::X509_ASN_ENCODING, material.signer_der.as_ptr(), material.signer_der.len() as u32) };
            assert!(!certificate.is_null());
            let certificate = Cert { raw: certificate, api: identity.native.api.clone() };
            let mut size = 0;
            assert_eq!(unsafe { (certificate.api.CertGetCertificateContextProperty)(certificate.raw, c::CERT_KEY_CONTEXT_PROP_ID, null_mut(), &mut size) }, 0,
                "Reconstructed DER must carry no PFX private-key context");
        }
    }
}
