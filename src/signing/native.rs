//! Lazy System32-only Windows crypto bindings and handle ownership.
//!
//! PATH: src/signing/native.rs
//! CONSUMED BY: signing identity/CMS and cache-only certificate-trust helper jobs.
//! No global initialization or static ncrypt/crypt32 calls. Every native owner
//! keeps its DLL table alive.

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::sync::Arc;
    use libloading::os::windows::Library;
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32;
    pub(crate) use windows_sys::Win32::Security::Cryptography as c;

    macro_rules! api {
        ($($name:ident: $library:ident ($($arg:ty),*) -> $result:ty;)*) => {
            #[allow(non_snake_case)]
            pub(crate) struct Api {
                $(pub $name: unsafe extern "system" fn($($arg),*) -> $result,)*
                _crypt32: Library,
                _ncrypt: Library,
            }
            impl Api {
                pub fn load() -> Result<Arc<Self>, String> {
                    // SAFETY: fixed basenames plus SYSTEM32-only search, never a
                    // caller path. Called only by explicit helper crypto work.
                    let crypt32 = unsafe { Library::load_with_flags("crypt32.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
                        .map_err(|_| "Windows crypt32.dll could not be loaded from System32.")?;
                    let ncrypt = unsafe { Library::load_with_flags("ncrypt.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
                        .map_err(|_| "Windows ncrypt.dll could not be loaded from System32.")?;
                    Ok(Arc::new(Self {
                        $($name: {
                            let library = if stringify!($library) == "crypt32" { &crypt32 } else { &ncrypt };
                            let symbol = unsafe { library.get::<unsafe extern "system" fn($($arg),*) -> $result>(concat!(stringify!($name), "\0").as_bytes()) }
                                .map_err(|_| concat!("Windows crypto API unavailable: ", stringify!($name)))?;
                            *symbol
                        },)*
                        _crypt32: crypt32, _ncrypt: ncrypt,
                    }))
                }
            }
        }
    }
    api! {
        NCryptOpenStorageProvider: ncrypt (*mut usize, *const u16, u32) -> i32;
        NCryptCreatePersistedKey: ncrypt (usize, *mut usize, *const u16, *const u16, u32, u32) -> i32;
        NCryptSetProperty: ncrypt (usize, *const u16, *const u8, u32, u32) -> i32;
        NCryptGetProperty: ncrypt (usize, *const u16, *mut u8, u32, *mut u32, u32) -> i32;
        NCryptFinalizeKey: ncrypt (usize, u32) -> i32;
        NCryptFreeObject: ncrypt (usize) -> i32;
        CertCreateSelfSignCertificate: crypt32 (usize, *const c::CRYPT_INTEGER_BLOB, u32, *const c::CRYPT_KEY_PROV_INFO, *const c::CRYPT_ALGORITHM_IDENTIFIER, *const SYSTEMTIME, *const SYSTEMTIME, *const c::CERT_EXTENSIONS) -> *mut c::CERT_CONTEXT;
        CertSetCertificateContextProperty: crypt32 (*const c::CERT_CONTEXT, u32, u32, *const c_void) -> i32;
        CertGetCertificateContextProperty: crypt32 (*const c::CERT_CONTEXT, u32, *mut c_void, *mut u32) -> i32;
        CertStrToNameW: crypt32 (u32, *const u16, u32, *const c_void, *mut u8, *mut u32, *mut *const u16) -> i32;
        CertNameToStrW: crypt32 (u32, *const c::CRYPT_INTEGER_BLOB, u32, *mut u16, u32) -> u32;
        CertOpenStore: crypt32 (*const u8, u32, usize, u32, *const c_void) -> c::HCERTSTORE;
        CertCloseStore: crypt32 (c::HCERTSTORE, u32) -> i32;
        CertAddCertificateLinkToStore: crypt32 (c::HCERTSTORE, *const c::CERT_CONTEXT, u32, *mut *mut c::CERT_CONTEXT) -> i32;
        CertEnumCertificatesInStore: crypt32 (c::HCERTSTORE, *const c::CERT_CONTEXT) -> *mut c::CERT_CONTEXT;
        CertDuplicateCertificateContext: crypt32 (*const c::CERT_CONTEXT) -> *mut c::CERT_CONTEXT;
        CertFreeCertificateContext: crypt32 (*const c::CERT_CONTEXT) -> i32;
        CertCreateCertificateContext: crypt32 (u32, *const u8, u32) -> *mut c::CERT_CONTEXT;
        CertCreateCertificateChainEngine: crypt32 (*const c::CERT_CHAIN_ENGINE_CONFIG, *mut c::HCERTCHAINENGINE) -> i32;
        CertFreeCertificateChainEngine: crypt32 (c::HCERTCHAINENGINE) -> ();
        CertGetCertificateChain: crypt32 (c::HCERTCHAINENGINE, *const c::CERT_CONTEXT, *const FILETIME, c::HCERTSTORE, *const c::CERT_CHAIN_PARA, u32, *const c_void, *mut *mut c::CERT_CHAIN_CONTEXT) -> i32;
        CertFreeCertificateChain: crypt32 (*const c::CERT_CHAIN_CONTEXT) -> ();
        CertVerifyCertificateChainPolicy: crypt32 (*const u8, *const c::CERT_CHAIN_CONTEXT, *const c::CERT_CHAIN_POLICY_PARA, *mut c::CERT_CHAIN_POLICY_STATUS) -> i32;
        CertGetPublicKeyLength: crypt32 (u32, *const c::CERT_PUBLIC_KEY_INFO) -> u32;
        CertGetIntendedKeyUsage: crypt32 (u32, *const c::CERT_INFO, *mut u8, u32) -> i32;
        CertGetEnhancedKeyUsage: crypt32 (*const c::CERT_CONTEXT, u32, *mut c::CTL_USAGE, *mut u32) -> i32;
        CertVerifyTimeValidity: crypt32 (*const FILETIME, *const c::CERT_INFO) -> i32;
        CertCompareCertificateName: crypt32 (u32, *const c::CRYPT_INTEGER_BLOB, *const c::CRYPT_INTEGER_BLOB) -> i32;
        CertFindExtension: crypt32 (*const u8, u32, *const c::CERT_EXTENSION) -> *mut c::CERT_EXTENSION;
        CryptDecodeObjectEx: crypt32 (u32, *const u8, *const u8, u32, u32, *const c::CRYPT_DECODE_PARA, *mut c_void, *mut u32) -> i32;
        CryptVerifyCertificateSignatureEx: crypt32 (usize, u32, u32, *const c_void, u32, *const c_void, u32, *mut c_void) -> i32;
        PFXExportCertStoreEx: crypt32 (c::HCERTSTORE, *mut c::CRYPT_INTEGER_BLOB, *const u16, *const c_void, u32) -> i32;
        PFXImportCertStore: crypt32 (*const c::CRYPT_INTEGER_BLOB, *const u16, u32) -> c::HCERTSTORE;
        CryptSignMessage: crypt32 (*const c::CRYPT_SIGN_MESSAGE_PARA, i32, u32, *const *const u8, *const u32, *mut u8, *mut u32) -> i32;
        CryptVerifyDetachedMessageSignature: crypt32 (*const c::CRYPT_VERIFY_MESSAGE_PARA, u32, *const u8, u32, u32, *const *const u8, *const u32, *mut *mut c::CERT_CONTEXT) -> i32;
        CryptGetMessageSignerCount: crypt32 (u32, *const u8, u32) -> i32;
        CryptQueryObject: crypt32 (u32, *const c_void, u32, u32, u32, *mut u32, *mut u32, *mut u32, *mut c::HCERTSTORE, *mut *mut c_void, *mut *mut c_void) -> i32;
        CryptMsgGetParam: crypt32 (*const c_void, u32, u32, *mut c_void, *mut u32) -> i32;
        CryptMsgClose: crypt32 (*const c_void) -> i32;
        CryptSignAndEncodeCertificate: crypt32 (usize, u32, u32, *const u8, *const c_void, *const c::CRYPT_ALGORITHM_IDENTIFIER, *const c_void, *mut u8, *mut u32) -> i32;
        CertAddEncodedCRLToStore: crypt32 (c::HCERTSTORE, u32, *const u8, u32, u32, *mut *mut c::CRL_CONTEXT) -> i32;
    }

    pub(crate) fn error(operation: &str) -> String {
        // Kernel32 only; never report password, file contents, subject or key data.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        format!("{operation} failed (Windows error 0x{code:08x}).")
    }
    pub(crate) fn ok(value: i32, operation: &str) -> Result<(), String> {
        if value != 0 { Ok(()) } else { Err(error(operation)) }
    }
    pub(crate) fn status(value: i32, operation: &str) -> Result<(), String> {
        if value == 0 { Ok(()) } else { Err(format!("{operation} failed (Windows status 0x{:08x}).", value as u32)) }
    }

    pub(crate) struct Key { pub raw: usize, pub api: Arc<Api> }
    impl Drop for Key { fn drop(&mut self) { if self.raw != 0 { unsafe { (self.api.NCryptFreeObject)(self.raw); } } } }
    pub(crate) struct Cert { pub raw: *const c::CERT_CONTEXT, pub api: Arc<Api> }
    impl Drop for Cert { fn drop(&mut self) { if !self.raw.is_null() { unsafe { (self.api.CertFreeCertificateContext)(self.raw); } } } }
    pub(crate) struct Store { pub raw: c::HCERTSTORE, pub api: Arc<Api> }
    impl Drop for Store { fn drop(&mut self) { if !self.raw.is_null() { unsafe { (self.api.CertCloseStore)(self.raw, 0); } } } }

    pub(crate) fn key_u32(api: &Api, key: usize, property: *const u16) -> Result<u32, String> {
        let mut bytes = [0u8; 4]; let mut written = 0;
        status(unsafe { (api.NCryptGetProperty)(key, property, bytes.as_mut_ptr(), 4, &mut written, 0) }, "Read key policy")?;
        if written != 4 { return Err("Windows returned an invalid key property.".into()); }
        Ok(u32::from_le_bytes(bytes))
    }
}
#[cfg(windows)]
pub(crate) use platform::*;
