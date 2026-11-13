//! Fixed Microsoft Software KSP adapter for one unnamed ephemeral RSA TEST key.
//!
//! PATH: experiments/signature-key-broker/src/windows.rs
//! CONSUMED BY: experimental stdio broker and explicitly gated Windows tests.
//! NCryptCreatePersistedKey gets NULL name and zero flags: Microsoft documents
//! this as NOT persisted. No named-key/store APIs, private export or fallback.

use std::ptr::{null, null_mut};

use sha2::{Digest, Sha256};
use windows_sys::core::PCWSTR;
use windows_sys::Win32::Security::Cryptography::{
    NCryptCreatePersistedKey, NCryptExportKey, NCryptFinalizeKey, NCryptFreeObject,
    NCryptGetProperty, NCryptOpenStorageProvider, NCryptSetProperty, NCryptSignHash,
    BCRYPT_PKCS1_PADDING_INFO, BCRYPT_RSAPUBLIC_BLOB, BCRYPT_RSA_ALGORITHM, BCRYPT_SHA256_ALGORITHM,
    MS_KEY_STORAGE_PROVIDER, NCRYPT_ALLOW_SIGNING_FLAG, NCRYPT_EXPORT_POLICY_PROPERTY,
    NCRYPT_KEY_USAGE_PROPERTY, NCRYPT_LENGTH_PROPERTY, NCRYPT_PAD_PKCS1_FLAG, NCRYPT_SILENT_FLAG,
};

use crate::{CryptoError, PublicKey, TestKeyBroker};

struct NativeHandle(usize);
impl Drop for NativeHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: each successful handle is owned once; the key is unnamed,
            // so freeing it needs no NCryptDeleteKey/disk-key deletion operation.
            unsafe { NCryptFreeObject(self.0); }
        }
    }
}

fn check(status: i32, operation: &'static str) -> Result<(), CryptoError> {
    if status == 0 { Ok(()) } else { Err(CryptoError::Native { operation, status }) }
}

fn set_u32(key: usize, property: PCWSTR, value: u32) -> Result<(), CryptoError> {
    let bytes = value.to_le_bytes();
    // SAFETY: key is live, property is a static Windows API constant, and the
    // 4-byte DWORD buffer lives through the call. No persistence flags are set.
    check(unsafe { NCryptSetProperty(key, property, bytes.as_ptr(), 4, 0) }, "set_property")
}

fn get_u32(key: usize, property: PCWSTR) -> Result<u32, CryptoError> {
    let mut bytes = [0u8; 4];
    let mut written = 0;
    // SAFETY: live key, static property, writable four-byte buffer/result count.
    check(unsafe { NCryptGetProperty(key, property, bytes.as_mut_ptr(), 4, &mut written, 0) }, "get_property")?;
    if written != 4 { return Err(CryptoError::UnexpectedNativeOutput); }
    Ok(u32::from_le_bytes(bytes))
}

/// No parameters: callers cannot substitute a provider, existing key or key name.
/// Key field is declared first so it is freed before the owning provider.
pub struct EphemeralTestKey {
    key: NativeHandle,
    _provider: NativeHandle,
    public: PublicKey,
}

impl EphemeralTestKey {
    /// Creates a new in-memory TEST key only. Never call during quiet measurement.
    /// Any unsupported policy/property fails closed; no named-key workaround.
    pub fn new() -> Result<Self, CryptoError> {
        let mut provider = NativeHandle(0);
        // SAFETY: output handle starts null; fixed built-in software provider only.
        check(unsafe { NCryptOpenStorageProvider(&mut provider.0, MS_KEY_STORAGE_PROVIDER, 0) }, "open_test_provider")?;
        if provider.0 == 0 { return Err(CryptoError::UnexpectedNativeOutput); }
        let mut key = NativeHandle(0);
        // SAFETY/CRITICAL: NULL pszKeyName explicitly requests an ephemeral,
        // non-persisted key. Zero legacy spec/creation flags: no machine, overwrite,
        // VBS, per-boot or other persistence modes. Never replace NULL with a name.
        check(unsafe { NCryptCreatePersistedKey(provider.0, &mut key.0, BCRYPT_RSA_ALGORITHM, null(), 0, 0) }, "create_unnamed_test_key")?;
        if key.0 == 0 { return Err(CryptoError::UnexpectedNativeOutput); }
        set_u32(key.0, NCRYPT_LENGTH_PROPERTY, 2048)?;
        set_u32(key.0, NCRYPT_KEY_USAGE_PROPERTY, NCRYPT_ALLOW_SIGNING_FLAG)?;
        set_u32(key.0, NCRYPT_EXPORT_POLICY_PROPERTY, 0)?;
        // SAFETY: configured unnamed key, silent operation only. In particular,
        // NCRYPT_WRITE_KEY_TO_LEGACY_STORE_FLAG is NEVER used.
        check(unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) }, "finalize_test_key")?;
        if get_u32(key.0, NCRYPT_EXPORT_POLICY_PROPERTY)? != 0
            || get_u32(key.0, NCRYPT_LENGTH_PROPERTY)? != 2048
            || get_u32(key.0, NCRYPT_KEY_USAGE_PROPERTY)? != NCRYPT_ALLOW_SIGNING_FLAG
        { return Err(CryptoError::UnexpectedNativeOutput); }

        let mut size = 0;
        // SAFETY: exports PUBLIC material only; null output requests size.
        check(unsafe { NCryptExportKey(key.0, 0, BCRYPT_RSAPUBLIC_BLOB, null(), null_mut(), 0, &mut size, NCRYPT_SILENT_FLAG) }, "public_blob_size")?;
        if !(24..=512).contains(&size) { return Err(CryptoError::UnexpectedNativeOutput); }
        let mut blob = vec![0; size as usize];
        let mut written = 0;
        // SAFETY: buffer has the queried capacity. Blob type is a fixed PUBLIC
        // constant; there is no private/opaque/archival export branch or probe.
        check(unsafe { NCryptExportKey(key.0, 0, BCRYPT_RSAPUBLIC_BLOB, null(), blob.as_mut_ptr(), size, &mut written, NCRYPT_SILENT_FLAG) }, "export_public_blob")?;
        if written != size { return Err(CryptoError::UnexpectedNativeOutput); }
        let spki_der = crate::spki::from_rsa_public_blob(&blob)?;
        let key_id = Sha256::digest(&spki_der).iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self { key, _provider: provider, public: PublicKey { spki_der, key_id } })
    }
}

impl TestKeyBroker for EphemeralTestKey {
    fn public_key(&self) -> &PublicKey { &self.public }

    fn sign_sha256_prehashed(&self, digest: &[u8; 32]) -> Result<Vec<u8>, CryptoError> {
        let padding = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: BCRYPT_SHA256_ALGORITHM };
        let flags = NCRYPT_PAD_PKCS1_FLAG | NCRYPT_SILENT_FLAG;
        let mut size = 0;
        // SAFETY: live key, RSA PKCS#1 padding info, exactly 32 prehashed bytes;
        // CNG performs encoding/signing, not another SHA-256 over this digest.
        check(unsafe { NCryptSignHash(self.key.0, (&padding as *const BCRYPT_PKCS1_PADDING_INFO).cast(),
            digest.as_ptr(), 32, null_mut(), 0, &mut size, flags) }, "signature_size")?;
        if size != 256 { return Err(CryptoError::UnexpectedNativeOutput); }
        let mut signature = vec![0; size as usize];
        let mut written = 0;
        // SAFETY: same input/algorithm and a buffer of the queried size.
        check(unsafe { NCryptSignHash(self.key.0, (&padding as *const BCRYPT_PKCS1_PADDING_INFO).cast(),
            digest.as_ptr(), 32, signature.as_mut_ptr(), size, &mut written, flags) }, "sign_sha256_prehash")?;
        if written != size { return Err(CryptoError::UnexpectedNativeOutput); }
        Ok(signature)
    }
}
