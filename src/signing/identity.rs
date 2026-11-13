//! Portable PFX creation and memory-only identity import for explicit helper jobs.
//!
//! PATH: src/signing/identity.rs
//! CONSUMED BY: signing helper service and CMS. New identities alone permit private
//! export, always encrypted PBES2/AES256-SHA256. No MY/system store installation.

use super::{CreateIdentitySpec, IdentityInfo};

pub struct Identity {
    info: IdentityInfo,
    #[cfg(windows)]
    pub(super) native: platform::Imported,
}

impl Identity {
    pub fn from_pfx(bytes: &[u8], password: &str) -> Result<Self, String> {
        #[cfg(windows)] { platform::import(bytes, password) }
        #[cfg(not(windows))] { let _ = (bytes, password); Err("Digital identity operations require Windows.".into()) }
    }
    pub fn info(&self) -> IdentityInfo { self.info.clone() }
    #[cfg(windows)]
    pub(super) fn ensure_signing_allowed(&self) -> Result<(), String> {
        platform::signing_usage(&self.native.api, self.native.cert.raw)
    }
}

pub fn create_pfx(spec: &CreateIdentitySpec, password: &str) -> Result<(Vec<u8>, IdentityInfo), String> {
    #[cfg(windows)] { platform::create(spec, password) }
    #[cfg(not(windows))] { let _ = (spec, password); Err("Digital identity operations require Windows.".into()) }
}

#[cfg(windows)]
pub(super) fn describe_certificate(api: &super::native::Api, cert: *const super::native::c::CERT_CONTEXT) -> Result<IdentityInfo, String> {
    platform::describe(api, cert)
}

#[cfg(windows)]
mod platform {
    use std::ffi::CStr;
    use std::mem::{size_of, replace};
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use chrono::{Datelike, Timelike, Utc};
    use sha2::{Digest, Sha256};
    use zeroize::{Zeroize, Zeroizing};
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use super::*;
    use crate::signing::native::{self, c, Api, Cert, Key, Store};

    const ENCODING: u32 = c::X509_ASN_ENCODING | c::PKCS_7_ASN_ENCODING;
    const MAX_CERTS: usize = 64;
    pub(crate) struct Imported {
        pub api: Arc<Api>,
        pub cert: Cert,
        pub certificates: Vec<Cert>,
        _store: Store,
    }

    fn password_wide(password: &str, creating: bool) -> Result<Zeroizing<Vec<u16>>, String> {
        let units = password.encode_utf16().count();
        if password.contains('\0') || units > 1024 {
            return Err("Password contains an invalid character or exceeds 1024 UTF-16 units.".into());
        }
        if creating && password.chars().count() < 8 {
            return Err("Use at least 8 password characters; 12 or more are recommended.".into());
        }
        // Exact capacity avoids secret-bearing intermediate reallocations.
        let mut wide = Zeroizing::new(Vec::with_capacity(units + 1));
        wide.extend(password.encode_utf16()); wide.push(0);
        Ok(wide)
    }

    fn dn_text(spec: &CreateIdentitySpec) -> Result<Vec<u16>, String> {
        if !(1..=10).contains(&spec.valid_years) { return Err("Identity validity must be 1 through 10 years.".into()); }
        for (value, limit) in [(&spec.name, 256), (&spec.organization, 256), (&spec.email, 320)] {
            if value.chars().count() > limit || value.chars().any(char::is_control) {
                return Err("Identity text is too long or contains control characters.".into());
            }
        }
        if spec.name.trim().is_empty() { return Err("Enter a name for the digital identity.".into()); }
        // CertStrToName's quoted RDN syntax: embedded quotes are doubled. No
        // certificate syntax can be injected through commas/plus/equal characters.
        let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
        let mut text = format!("CN={}", quote(spec.name.trim()));
        if !spec.organization.trim().is_empty() { text += &format!(", O={}", quote(spec.organization.trim())); }
        if !spec.email.trim().is_empty() { text += &format!(", E={}", quote(spec.email.trim())); }
        Ok(text.encode_utf16().chain(Some(0)).collect())
    }

    fn system_time(date: chrono::DateTime<Utc>) -> SYSTEMTIME {
        SYSTEMTIME { wYear: date.year() as u16, wMonth: date.month() as u16,
            wDayOfWeek: date.weekday().num_days_from_sunday() as u16, wDay: date.day() as u16,
            wHour: date.hour() as u16, wMinute: date.minute() as u16, wSecond: date.second() as u16, wMilliseconds: 0 }
    }
    fn date_string(time: FILETIME) -> Result<String, String> {
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        let seconds = (ticks / 10_000_000) as i64 - 11_644_473_600;
        chrono::DateTime::<Utc>::from_timestamp(seconds, ((ticks % 10_000_000) * 100) as u32)
            .map(|d| d.to_rfc3339()).ok_or_else(|| "Certificate contains an invalid date.".into())
    }
    fn name(api: &Api, blob: &c::CRYPT_INTEGER_BLOB) -> Result<String, String> {
        let length = unsafe { (api.CertNameToStrW)(ENCODING, blob, c::CERT_X500_NAME_STR, null_mut(), 0) };
        if length == 0 || length > 16_384 { return Err("Certificate name is invalid or too long.".into()); }
        let mut text = vec![0u16; length as usize];
        let written = unsafe { (api.CertNameToStrW)(ENCODING, blob, c::CERT_X500_NAME_STR, text.as_mut_ptr(), length) };
        if written == 0 || written > length { return Err(native::error("Read certificate name")); }
        Ok(String::from_utf16_lossy(&text[..written.saturating_sub(1) as usize]))
    }

    pub(super) fn describe(api: &Api, cert: *const c::CERT_CONTEXT) -> Result<IdentityInfo, String> {
        if cert.is_null() { return Err("No signer certificate was returned.".into()); }
        // SAFETY: contexts come only from crypt32, retained by their owning Cert.
        let context = unsafe { &*cert };
        if context.pCertInfo.is_null() || context.pbCertEncoded.is_null() || context.cbCertEncoded == 0 || context.cbCertEncoded > 1024 * 1024 {
            return Err("Signer certificate is invalid or exceeds the size limit.".into());
        }
        let info = unsafe { &*context.pCertInfo };
        let oid = if info.SubjectPublicKeyInfo.Algorithm.pszObjId.is_null() { "".into() }
            else { unsafe { CStr::from_ptr(info.SubjectPublicKeyInfo.Algorithm.pszObjId.cast()) }.to_string_lossy().into_owned() };
        let bits = unsafe { (api.CertGetPublicKeyLength)(ENCODING, &info.SubjectPublicKeyInfo) };
        let der = unsafe { std::slice::from_raw_parts(context.pbCertEncoded, context.cbCertEncoded as usize) };
        let self_signed = unsafe {
            (api.CertCompareCertificateName)(ENCODING, &info.Subject, &info.Issuer) != 0
                && (api.CryptVerifyCertificateSignatureEx)(0, ENCODING, c::CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT,
                    cert.cast(), c::CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT, cert.cast(), 0, null_mut()) != 0
        };
        Ok(IdentityInfo { subject: name(api, &info.Subject)?, issuer: name(api, &info.Issuer)?,
            sha256_fingerprint: Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect(),
            not_before: date_string(info.NotBefore)?, not_after: date_string(info.NotAfter)?,
            algorithm: if oid == "1.2.840.113549.1.1.1" { "RSA".into() } else { format!("Unsupported ({oid})") },
            key_bits: bits, self_signed })
    }

    // Read only a cached, memory-imported key context. Never acquire/open a named
    // container or heal an association using a user/system store.
    fn private_context(api: &Api, cert: *const c::CERT_CONTEXT) -> Result<Option<c::CERT_KEY_CONTEXT>, String> {
        let mut value = c::CERT_KEY_CONTEXT::default();
        value.cbSize = size_of::<c::CERT_KEY_CONTEXT>() as u32;
        let mut size = value.cbSize;
        let result = unsafe { (api.CertGetCertificateContextProperty)(cert, c::CERT_KEY_CONTEXT_PROP_ID, (&mut value as *mut c::CERT_KEY_CONTEXT).cast(), &mut size) };
        if result == 0 {
            let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if code == 0x80092004 { return Ok(None); } // CRYPT_E_NOT_FOUND
            return Err(format!("Read imported private-key association failed (0x{code:08x})."));
        }
        if size != size_of::<c::CERT_KEY_CONTEXT>() as u32 || value.cbSize != size || value.dwKeySpec != c::CERT_NCRYPT_KEY_SPEC {
            return Err("This PFX does not contain a supported memory-only CNG identity.".into());
        }
        if unsafe { value.Anonymous.hNCryptKey } == 0 { return Err("PFX private-key association is invalid.".into()); }
        Ok(Some(value)) // borrowed handle; crypt32 owns it through the context.
    }

    pub(super) fn signing_usage(api: &Api, cert: *const c::CERT_CONTEXT) -> Result<(), String> {
        let info = unsafe { &*(*cert).pCertInfo };
        if unsafe { (api.CertVerifyTimeValidity)(null(), info) } != 0 {
            return Err("The selected signing certificate is expired or not yet valid.".into());
        }
        let mut usage = [0u8; 2];
        if !unsafe { (api.CertFindExtension)(c::szOID_KEY_USAGE, info.cExtension, info.rgExtension) }.is_null() {
            native::ok(unsafe { (api.CertGetIntendedKeyUsage)(ENCODING, info, usage.as_mut_ptr(), 2) }, "Read certificate key usage")?;
            if usage[0] & (c::CERT_DIGITAL_SIGNATURE_KEY_USAGE | c::CERT_NON_REPUDIATION_KEY_USAGE) as u8 == 0 {
                return Err("Certificate key usage does not permit signing.".into());
            }
            if usage[0] & c::CERT_KEY_CERT_SIGN_KEY_USAGE as u8 != 0 {
                return Err("Choose a personal signing identity, not a certificate-authority key.".into());
            }
        }
        let basic = unsafe { (api.CertFindExtension)(c::szOID_BASIC_CONSTRAINTS2, info.cExtension, info.rgExtension) };
        if !basic.is_null() {
            let extension = unsafe { &*basic };
            let mut constraints = c::CERT_BASIC_CONSTRAINTS2_INFO::default();
            let mut size = size_of::<c::CERT_BASIC_CONSTRAINTS2_INFO>() as u32;
            native::ok(unsafe { (api.CryptDecodeObjectEx)(ENCODING, c::X509_BASIC_CONSTRAINTS2, extension.Value.pbData,
                extension.Value.cbData, 0, null(), (&mut constraints as *mut c::CERT_BASIC_CONSTRAINTS2_INFO).cast(), &mut size) }, "Decode certificate constraints")?;
            if constraints.fCA != 0 { return Err("Choose a personal signing identity, not a certificate-authority key.".into()); }
        }
        if unsafe { (api.CertFindExtension)(c::szOID_ENHANCED_KEY_USAGE, info.cExtension, info.rgExtension) }.is_null() {
            return Ok(()); // absent EKU is unrestricted, unlike an explicit empty EKU
        }
        let mut size = 0;
        if unsafe { (api.CertGetEnhancedKeyUsage)(cert, c::CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, null_mut(), &mut size) } == 0 {
            let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return Err(format!("Read certificate extended usage failed (0x{code:08x})."));
        }
        if size < size_of::<c::CTL_USAGE>() as u32 || size > 64 * 1024 { return Err("Certificate extended usage is invalid.".into()); }
        let mut storage = vec![0usize; (size as usize + size_of::<usize>() - 1) / size_of::<usize>()];
        let pointer = storage.as_mut_ptr().cast::<c::CTL_USAGE>();
        native::ok(unsafe { (api.CertGetEnhancedKeyUsage)(cert, c::CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, pointer, &mut size) }, "Read certificate extended usage")?;
        let eku = unsafe { &*pointer };
        if eku.cUsageIdentifier > 128 || eku.cUsageIdentifier == 0 || eku.rgpszUsageIdentifier.is_null() { return Err("Certificate extended usage is ambiguous.".into()); }
        let allowed = unsafe { std::slice::from_raw_parts(eku.rgpszUsageIdentifier, eku.cUsageIdentifier as usize) }.iter().any(|oid| {
            if oid.is_null() { return false; }
            matches!(unsafe { CStr::from_ptr((*oid).cast()) }.to_bytes(), b"1.3.6.1.4.1.311.10.3.12" | b"1.2.840.113583.1.1.5" | b"2.5.29.37.0")
        });
        if !allowed { return Err("Certificate extended usage does not permit document signing.".into()); }
        Ok(())
    }

    pub(super) fn import(bytes: &[u8], password: &str) -> Result<Identity, String> {
        if bytes.is_empty() || bytes.len() > crate::signing::MAX_PFX_BYTES { return Err("PFX file is empty or exceeds 16 MiB.".into()); }
        let password = password_wide(password, false)?;
        let api = Api::load()?;
        let blob = c::CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr().cast_mut() };
        let raw = unsafe { (api.PFXImportCertStore)(&blob, password.as_ptr(), c::PKCS12_NO_PERSIST_KEY | c::PKCS12_ALWAYS_CNG_KSP) };
        if raw.is_null() { return Err("Could not open PFX. Check the password and that the file contains a supported identity.".into()); }
        let store = Store { raw, api: api.clone() };
        let mut cursor = Cert { raw: null(), api: api.clone() };
        let mut certificates = Vec::new(); let mut chosen = None;
        loop {
            // Enum consumes the previous reference, including on end-of-store.
            let previous = replace(&mut cursor.raw, null());
            cursor.raw = unsafe { (api.CertEnumCertificatesInStore)(store.raw, previous) };
            if cursor.raw.is_null() {
                let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                if code != 0x80092004 { return Err(format!("Read PFX certificates failed (0x{code:08x}).")); }
                break;
            }
            if certificates.len() >= MAX_CERTS { return Err("PFX contains too many certificates.".into()); }
            if private_context(&api, cursor.raw)?.is_some() {
                if chosen.is_some() { return Err("PFX contains multiple private signing identities. Export one identity per file.".into()); }
                chosen = Some(certificates.len());
            }
            let duplicate = unsafe { (api.CertDuplicateCertificateContext)(cursor.raw) };
            if duplicate.is_null() { return Err(native::error("Retain imported certificate")); }
            certificates.push(Cert { raw: duplicate, api: api.clone() });
        }
        let selected = chosen.ok_or("PFX contains no supported private signing identity.")?;
        let info = describe(&api, certificates[selected].raw)?;
        if info.algorithm != "RSA" || !(2048..=8192).contains(&info.key_bits) {
            return Err("Only RSA signing identities from 2048 through 8192 bits are supported.".into());
        }
        signing_usage(&api, certificates[selected].raw)?;
        let key = private_context(&api, certificates[selected].raw)?.ok_or("PFX private key is unavailable.")?;
        let handle = unsafe { key.Anonymous.hNCryptKey };
        if native::key_u32(&api, handle, c::NCRYPT_LENGTH_PROPERTY)? != info.key_bits
            || native::key_u32(&api, handle, c::NCRYPT_KEY_USAGE_PROPERTY)? & c::NCRYPT_ALLOW_SIGNING_FLAG == 0
        { return Err("PFX private key does not support the certificate's signing algorithm.".into()); }
        let duplicate = unsafe { (api.CertDuplicateCertificateContext)(certificates[selected].raw) };
        if duplicate.is_null() { return Err(native::error("Retain signing identity")); }
        let cert = Cert { raw: duplicate, api: api.clone() };
        Ok(Identity { info, native: Imported { api, cert, certificates, _store: store } })
    }

    pub(super) fn create(spec: &CreateIdentitySpec, password: &str) -> Result<(Vec<u8>, IdentityInfo), String> {
        let dn = dn_text(spec)?;
        let password_utf16 = password_wide(password, true)?;
        let api = Api::load()?;
        let mut provider = Key { raw: 0, api: api.clone() };
        native::status(unsafe { (api.NCryptOpenStorageProvider)(&mut provider.raw, c::MS_KEY_STORAGE_PROVIDER, 0) }, "Open signing provider")?;
        if provider.raw == 0 { return Err("Windows returned an invalid signing provider.".into()); }
        let mut key = Key { raw: 0, api: api.clone() };
        // NULL name is explicitly ephemeral; exportable only for this NEW-PFX job.
        native::status(unsafe { (api.NCryptCreatePersistedKey)(provider.raw, &mut key.raw, c::BCRYPT_RSA_ALGORITHM, null(), 0, 0) }, "Create ephemeral identity key")?;
        if key.raw == 0 { return Err("Windows returned an invalid ephemeral signing key.".into()); }
        for (property, value) in [(c::NCRYPT_LENGTH_PROPERTY, 2048u32), (c::NCRYPT_KEY_USAGE_PROPERTY, c::NCRYPT_ALLOW_SIGNING_FLAG),
            (c::NCRYPT_EXPORT_POLICY_PROPERTY, c::NCRYPT_ALLOW_EXPORT_FLAG | c::NCRYPT_ALLOW_PLAINTEXT_EXPORT_FLAG)] {
            native::status(unsafe { (api.NCryptSetProperty)(key.raw, property, value.to_le_bytes().as_ptr(), 4, 0) }, "Set new identity policy")?;
        }
        native::status(unsafe { (api.NCryptFinalizeKey)(key.raw, c::NCRYPT_SILENT_FLAG) }, "Generate signing key")?;
        let mut dn_size = 0;
        native::ok(unsafe { (api.CertStrToNameW)(ENCODING, dn.as_ptr(), c::CERT_X500_NAME_STR, null(), null_mut(), &mut dn_size, null_mut()) }, "Encode identity name")?;
        if dn_size == 0 || dn_size > 16_384 { return Err("Identity name encoding is too large.".into()); }
        let mut encoded_dn = vec![0u8; dn_size as usize];
        native::ok(unsafe { (api.CertStrToNameW)(ENCODING, dn.as_ptr(), c::CERT_X500_NAME_STR, null(), encoded_dn.as_mut_ptr(), &mut dn_size, null_mut()) }, "Encode identity name")?;
        let dn_blob = c::CRYPT_INTEGER_BLOB { cbData: dn_size, pbData: encoded_dn.as_mut_ptr() };
        let mut basic = [0x30, 0x00]; // BasicConstraints CA=false.
        let mut usage = [0x03, 0x02, 0x06, 0xc0]; // digitalSignature + contentCommitment only.
        let mut eku = [0x30, 0x0c, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x0a, 0x03, 0x0c];
        let extension = |oid: *const u8, critical, data: &mut [u8]| c::CERT_EXTENSION { pszObjId: oid.cast_mut(), fCritical: critical,
            Value: c::CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_mut_ptr() } };
        let mut extensions = [extension(c::szOID_BASIC_CONSTRAINTS2, 1, &mut basic), extension(c::szOID_KEY_USAGE, 1, &mut usage),
            extension(c::szOID_ENHANCED_KEY_USAGE, 0, &mut eku)];
        let extension_list = c::CERT_EXTENSIONS { cExtension: extensions.len() as u32, rgExtension: extensions.as_mut_ptr() };
        let now = Utc::now();
        let end = now.with_year(now.year() + i32::from(spec.valid_years))
            .or_else(|| now.with_day(28).and_then(|d| d.with_year(now.year() + i32::from(spec.valid_years)))).ok_or("Invalid identity expiry date.")?;
        let start = system_time(now - chrono::Duration::minutes(5)); let end = system_time(end);
        let algorithm = c::CRYPT_ALGORITHM_IDENTIFIER { pszObjId: c::szOID_RSA_SHA256RSA.cast_mut(), ..Default::default() };
        let raw = unsafe { (api.CertCreateSelfSignCertificate)(key.raw, &dn_blob, c::CERT_CREATE_SELFSIGN_NO_KEY_INFO,
            null(), &algorithm, &start, &end, &extension_list) };
        if raw.is_null() { return Err(native::error("Create SHA256 self-signed identity")); }
        let cert = Cert { raw, api: api.clone() };
        // The certificate BORROWS this scoped key; NO_CRYPT_RELEASE prevents a
        // double free. Memory-store links retain the context, not another key owner.
        let context = c::CERT_KEY_CONTEXT { cbSize: size_of::<c::CERT_KEY_CONTEXT>() as u32,
            Anonymous: c::CERT_KEY_CONTEXT_0 { hNCryptKey: key.raw }, dwKeySpec: c::CERT_NCRYPT_KEY_SPEC };
        native::ok(unsafe { (api.CertSetCertificateContextProperty)(cert.raw, c::CERT_KEY_CONTEXT_PROP_ID,
            c::CERT_STORE_NO_CRYPT_RELEASE_FLAG | c::CERT_SET_PROPERTY_INHIBIT_PERSIST_FLAG, (&context as *const c::CERT_KEY_CONTEXT).cast()) }, "Bind new ephemeral identity")?;
        let info = describe(&api, cert.raw)?;
        if !info.self_signed { return Err("New certificate self-signature could not be verified.".into()); }
        let raw = unsafe { (api.CertOpenStore)(c::CERT_STORE_PROV_MEMORY, 0, 0, c::CERT_STORE_CREATE_NEW_FLAG, null()) };
        if raw.is_null() { return Err(native::error("Create temporary identity store")); }
        let store = Store { raw, api: api.clone() };
        native::ok(unsafe { (api.CertAddCertificateLinkToStore)(store.raw, cert.raw, c::CERT_STORE_ADD_ALWAYS, null_mut()) }, "Add new identity to memory store")?;
        let params = c::PKCS12_PBES2_EXPORT_PARAMS { dwSize: size_of::<c::PKCS12_PBES2_EXPORT_PARAMS>() as u32,
            hNcryptDescriptor: null_mut(), pwszPbes2Alg: c::PKCS12_PBES2_ALG_AES256_SHA256.cast_mut() };
        let flags = c::EXPORT_PRIVATE_KEYS | c::REPORT_NO_PRIVATE_KEY | c::REPORT_NOT_ABLE_TO_EXPORT_PRIVATE_KEY
            | c::PKCS12_EXPORT_PBES2_PARAMS | c::PKCS12_ENCRYPT_CERTIFICATES;
        let mut blob = c::CRYPT_INTEGER_BLOB::default();
        let export_error = || format!("{} Windows 10 1709 or newer with PBES2 support is required; no legacy encryption fallback was used.",
            native::error("Secure AES256/SHA256 PFX export"));
        if unsafe { (api.PFXExportCertStoreEx)(store.raw, &mut blob, password_utf16.as_ptr(), (&params as *const c::PKCS12_PBES2_EXPORT_PARAMS).cast(), flags) } == 0 {
            return Err(export_error());
        }
        if blob.cbData == 0 || blob.cbData as usize > crate::signing::MAX_PFX_BYTES { return Err("Exported identity exceeds the PFX size limit.".into()); }
        let mut encrypted = vec![0u8; blob.cbData as usize]; blob.pbData = encrypted.as_mut_ptr();
        if unsafe { (api.PFXExportCertStoreEx)(store.raw, &mut blob, password_utf16.as_ptr(), (&params as *const c::PKCS12_PBES2_EXPORT_PARAMS).cast(), flags) } == 0 {
            encrypted.zeroize(); return Err(export_error());
        }
        if blob.cbData as usize > encrypted.len() { encrypted.zeroize(); return Err("Windows returned an invalid PFX size.".into()); }
        encrypted.truncate(blob.cbData as usize);
        // Verify the portable artifact really reopens as this one memory-only ID.
        match import(&encrypted, password) {
            Ok(imported) if imported.info.sha256_fingerprint == info.sha256_fingerprint => Ok((encrypted, info)),
            _ => { encrypted.zeroize(); Err("New PFX could not be reopened as the intended signing identity.".into()) }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::signing::cms;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn spec() -> CreateIdentitySpec {
        CreateIdentitySpec { name: "Native QA, \"Identity\"".into(), organization: "Local + Tests".into(),
            email: "synthetic@example.invalid".into(), valid_years: 1 }
    }

    #[test]
    fn native_create_import_sign_verify_and_reject_bad_inputs() {
        let password = "Synthetic only 密码 42!";
        let (pfx, info) = create_pfx(&spec(), password).unwrap();
        assert!(info.self_signed && info.algorithm == "RSA" && info.key_bits >= 2048);
        assert_eq!(info.sha256_fingerprint.len(), 64);
        assert!(Identity::from_pfx(&pfx, "wrong password").is_err());
        let identity = Identity::from_pfx(&pfx, password).unwrap();
        assert_eq!(identity.info().sha256_fingerprint, info.sha256_fingerprint);
        let data = b"synthetic detached signing content\r\n";
        let signature = cms::sign_detached(&identity, data).unwrap();
        assert_eq!(cms::verify_detached(&signature, data).unwrap().sha256_fingerprint, info.sha256_fingerprint);
        assert!(cms::verify_detached(&signature, b"changed content").is_err());
        let mut changed = signature; let last = changed.len() - 1; changed[last] ^= 1;
        assert!(cms::verify_detached(&changed, data).is_err());
        assert!(create_pfx(&spec(), "short").is_err());
        assert!(create_pfx(&spec(), "embedded\0NUL password").is_err());
        let mut invalid = spec(); invalid.valid_years = 0;
        assert!(create_pfx(&invalid, password).is_err());
        invalid = spec(); invalid.name = "bad\0name".into();
        assert!(create_pfx(&invalid, password).is_err());
    }

    fn private_temp() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let who = Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().unwrap();
        assert!(who.status.success());
        let text = String::from_utf8_lossy(&who.stdout);
        let sid = text.trim().rsplit(',').next().unwrap().trim_matches('"');
        assert!(sid.starts_with("S-1-"));
        let result = Command::new("icacls").arg(directory.path()).args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{sid}:(OI)(CI)F")).output().unwrap();
        assert!(result.status.success());
        directory
    }
    fn openssl(args: &[&str], input: Option<&[u8]>) -> std::process::Output {
        let mut child = Command::new("openssl").args(args).env("OPENSSL_CONF", "NUL")
            .env_remove("OPENSSL_MODULES").env_remove("OPENSSL_ENGINES")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        if let Some(input) = input { child.stdin.take().unwrap().write_all(input).unwrap(); }
        else { drop(child.stdin.take()); }
        child.wait_with_output().unwrap()
    }

    #[test]
    #[ignore = "Developer-only OpenSSL PFX/CMS interoperability; synthetic private material in ACL-private OS temp"]
    fn developer_openssl_pfx_and_cms_interop_including_legacy_verification() {
        let directory = private_temp();
        let path = |name: &str| directory.path().join(name).to_str().unwrap().to_owned();
        let password = "Developer synthetic password 42!";
        let (pfx, info) = create_pfx(&spec(), password).unwrap();
        std::fs::write(path("new.pfx"), &pfx).unwrap();
        let metadata = openssl(&["pkcs12", "-in", &path("new.pfx"), "-info", "-noout", "-passin", "stdin"], Some(format!("{password}\n").as_bytes()));
        assert!(metadata.status.success(), "Modern PFX must open independently");
        let encryption = String::from_utf8_lossy(&metadata.stderr);
        assert!(encryption.contains("MAC: sha256") && encryption.contains("PBES2") && encryption.contains("AES-256-CBC") && encryption.contains("hmacWithSHA256"));
        assert!(!encryption.contains("TripleDES") && !encryption.contains("pbeWithSHA1And3-KeyTripleDES"));
        assert!(!openssl(&["pkcs12", "-in", &path("new.pfx"), "-noout", "-passin", "stdin"], Some(b"wrong password\n")).status.success());
        let identity = Identity::from_pfx(&pfx, password).unwrap();
        let data = b"synthetic native CMS interoperability";
        let signature = cms::sign_detached(&identity, data).unwrap();
        std::fs::write(path("data.bin"), data).unwrap(); std::fs::write(path("signature.der"), &signature).unwrap();
        let verify = || openssl(&["cms", "-verify", "-binary", "-inform", "DER", "-in", &path("signature.der"),
            "-content", &path("data.bin"), "-noverify", "-out", &path("verified.bin")], None);
        assert!(verify().status.success());
        assert_eq!(std::fs::read(path("verified.bin")).unwrap(), data);
        std::fs::write(path("data.bin"), b"tampered").unwrap(); assert!(!verify().status.success());
        std::fs::write(path("data.bin"), data).unwrap();
        let mut corrupt = signature; let last = corrupt.len() - 1; corrupt[last] ^= 1;
        std::fs::write(path("signature.der"), corrupt).unwrap(); assert!(!verify().status.success());
        // Verification must not apply the NEW/import signing key-size policy to
        // historical, mathematically valid CMS. This key is synthetic, not a user ID.
        assert!(openssl(&["req", "-x509", "-newkey", "rsa:1024", "-nodes", "-sha256", "-days", "1", "-subj", "/CN=SyntheticLegacy",
            "-keyout", &path("legacy-key.pem"), "-out", &path("legacy-cert.pem")], None).status.success());
        assert!(openssl(&["cms", "-sign", "-binary", "-in", &path("data.bin"), "-signer", &path("legacy-cert.pem"),
            "-inkey", &path("legacy-key.pem"), "-md", "sha256", "-outform", "DER", "-out", &path("legacy.der")], None).status.success());
        let legacy = cms::verify_detached(&std::fs::read(path("legacy.der")).unwrap(), data).unwrap();
        assert_eq!(legacy.key_bits, 1024);
        assert_eq!(identity.info().sha256_fingerprint, info.sha256_fingerprint);
    }
}
