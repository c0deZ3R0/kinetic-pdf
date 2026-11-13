//! Explicit opt-in PFX password storage in the current user's Windows credentials.
//!
//! PATH: src/signing/credentials.rs
//! CONSUMED BY: signing UI after successful PFX authentication and explicit consent.
//! No automatic startup/signing, enumeration, own encryption or plaintext files.
//! Generic LOCAL_MACHINE persistence means this user on this computer, not all users.
//! Targets bind the full SHA-256 of a lexical absolute path + certificate hash;
//! filesystem aliases are separate. Forget never requires the file to exist.
//! Same-user processes are not isolated from one another by Credential Manager.

use std::path::Path;
use super::Secret;

/// Retrieve only a password bound to this selected lexical file path and certificate.
/// The caller must still authenticate the PFX; a cached value never authorizes signing.
pub fn load(pfx: &Path, fingerprint: &str) -> Result<Option<Secret>, String> {
    #[cfg(windows)] { platform::load(pfx, fingerprint) }
    #[cfg(not(windows))] { let _ = (pfx, fingerprint); Err("Remembering PFX passwords requires Windows Credential Manager.".into()) }
}

/// Call ONLY after correct PFX authentication and explicit user opt-in.
pub fn save(pfx: &Path, fingerprint: &str, password: &Secret) -> Result<(), String> {
    #[cfg(windows)] { platform::save(pfx, fingerprint, password) }
    #[cfg(not(windows))] { let _ = (pfx, fingerprint, password); Err("Remembering PFX passwords requires Windows Credential Manager.".into()) }
}

/// Idempotent deletion of this exact app target; no other credentials are enumerated.
/// No filesystem lookup is needed, including for missing files/parents. A moved
/// file or filesystem alias is a different binding: use the original selected path.
pub fn forget(pfx: &Path, fingerprint: &str) -> Result<(), String> {
    #[cfg(windows)] { platform::forget(pfx, fingerprint) }
    #[cfg(not(windows))] { let _ = (pfx, fingerprint); Err("Remembering PFX passwords requires Windows Credential Manager.".into()) }
}

#[cfg(any(windows, test))]
const MAGIC: &[u8] = b"KPDF-PFX-PW\x01";
#[cfg(any(windows, test))]
const MAX_BLOB: usize = 2560; // CRED_MAX_CREDENTIAL_BLOB_SIZE, includes our format header.
#[cfg(any(windows, test))]
fn fingerprint_bytes(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("A full 64-digit SHA-256 certificate fingerprint is required.".into());
    }
    let mut bytes = [0; 32];
    for (out, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let digit = |b: u8| if b.is_ascii_digit() { b - b'0' } else { b.to_ascii_lowercase() - b'a' + 10 };
        *out = (digit(pair[0]) << 4) | digit(pair[1]);
    }
    Ok(bytes)
}
#[cfg(any(windows, test))]
fn valid_password(value: &str) -> Result<(), String> {
    // No CreatePfx minimum here: imported legacy PFX passwords can be short/blank.
    if value.contains('\0') || value.encode_utf16().take(1025).count() > 1024 {
        return Err("PFX password contains NUL or exceeds 1024 UTF-16 units.".into());
    }
    if value.len() > MAX_BLOB - MAGIC.len() {
        return Err("This password is too long for Windows Credential Manager; it was not remembered.".into());
    }
    Ok(())
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::path::{Component, Prefix};
    use std::ptr::null_mut;
    use std::os::windows::ffi::OsStrExt;
    use libloading::os::windows::Library;
    use sha2::{Digest, Sha256};
    use zeroize::{Zeroize, Zeroizing};
    use windows_sys::Win32::Security::Credentials as c;
    use windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32;
    use super::*;

    const MAX_PATH_UNITS: usize = 32760;
    const PREFIX: &str = "KineticPDF/PFXPassword/v1/";
    type Read = unsafe extern "system" fn(*const u16, u32, u32, *mut *mut c::CREDENTIALW) -> i32;
    type Write = unsafe extern "system" fn(*const c::CREDENTIALW, u32) -> i32;
    type Delete = unsafe extern "system" fn(*const u16, u32, u32) -> i32;
    type Free = unsafe extern "system" fn(*const c_void);
    struct Api { read: Read, write: Write, delete: Delete, free: Free, _library: Library }
    impl Api {
        fn load() -> Result<Self, String> {
            // SAFETY: fixed basename, SYSTEM32 only. No eager/static advapi32 import.
            let library = unsafe { Library::load_with_flags("advapi32.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(|_| "Windows Credential Manager could not be loaded from System32.")?;
            unsafe fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
                let symbol = unsafe { library.get::<T>(name) }
                    .map_err(|_| "Windows Credential Manager APIs are unavailable.")?;
                Ok(*symbol)
            }
            Ok(Self { read: unsafe { symbol(&library, b"CredReadW\0")? }, write: unsafe { symbol(&library, b"CredWriteW\0")? },
                delete: unsafe { symbol(&library, b"CredDeleteW\0")? }, free: unsafe { symbol(&library, b"CredFree\0")? }, _library: library })
        }
    }
    fn last_error() -> u32 { unsafe { windows_sys::Win32::Foundation::GetLastError() } }
    fn failure(operation: &str, code: u32) -> String {
        format!("Windows Credential Manager {operation} failed (0x{code:08x}).")
    }

    fn bounded_path(path: &Path) -> Result<(), String> {
        let mut count = 0;
        for unit in path.as_os_str().encode_wide().take(MAX_PATH_UNITS + 1) {
            count += 1;
            if unit == 0 { return Err("PFX path contains NUL.".into()); }
        }
        if count == 0 || count > MAX_PATH_UNITS { return Err("PFX path is empty or too long.".into()); }
        Ok(())
    }

    // Pure lexical binding: no symlink resolution or filesystem access. File
    // dialogs supply absolute paths. Preserve filename case/non-Unicode UTF-16
    // units (Windows directories can be case-sensitive); normalize drive case only.
    fn lexical_path(path: &Path) -> Result<Vec<u16>, String> {
        bounded_path(path)?;
        if !path.is_absolute() { return Err("Select an absolute PFX path for remembered passwords.".into()); }
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else { return Err("PFX path needs a Windows drive or UNC prefix.".into()); };
        let (drive_position, verbatim) = match prefix.kind() {
            Prefix::Disk(_) => (Some(0), false),
            Prefix::VerbatimDisk(_) => (Some(4), true),
            Prefix::UNC(_, _) => (None, false),
            Prefix::VerbatimUNC(_, _) => (None, true),
            _ => return Err("Device-namespace PFX paths cannot have remembered passwords.".into()),
        };
        if verbatim {
            // Stripping an extended prefix or rewriting its dots/trailing spaces
            // could change which file Windows opens. Keep that spelling separate.
            if path.file_name().is_none() { return Err("PFX path has no filename.".into()); }
            let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
            if let Some(position) = drive_position { units[position] = (units[position] as u8).to_ascii_uppercase() as u16; }
            return Ok(units);
        }
        let mut names: Vec<Vec<u16>> = Vec::new();
        for component in components {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::ParentDir => {
                    if names.pop().is_none() { return Err("PFX path traverses above its Windows root.".into()); }
                }
                Component::Normal(name) => names.push(name.encode_wide().collect()),
                _ => return Err("PFX path has an unsupported prefix.".into()),
            }
        }
        if names.is_empty() { return Err("PFX path has no filename.".into()); }
        let mut units: Vec<u16> = prefix.as_os_str().encode_wide().collect();
        if let Some(position) = drive_position { units[position] = (units[position] as u8).to_ascii_uppercase() as u16; }
        for name in names { units.push(b'\\' as u16); units.extend(name); }
        if units.len() > MAX_PATH_UNITS { return Err("PFX path is too long.".into()); }
        Ok(units)
    }

    fn target(pfx: &Path, fingerprint: &str) -> Result<Vec<u16>, String> {
        let certificate = fingerprint_bytes(fingerprint)?;
        let path = lexical_path(pfx)?;
        let mut digest = Sha256::new();
        digest.update(b"Kinetic PDF explicit lexical PFX credential binding v1\0");
        digest.update((path.len() as u64).to_le_bytes());
        for unit in path { digest.update(unit.to_le_bytes()); }
        digest.update(certificate);
        let hex: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let target = format!("{PREFIX}{hex}"); // full SHA-256; never path/password text
        Ok(target.encode_utf16().chain(Some(0)).collect())
    }

    struct Allocation<'a> { raw: *mut c::CREDENTIALW, api: &'a Api }
    impl Drop for Allocation<'_> {
        fn drop(&mut self) {
            if self.raw.is_null() { return; }
            // SAFETY: CredRead returns one OS allocation; its documented blob
            // pointer is writable for the reported size. Clear the native copy
            // before CredFree on EVERY path, including invalid format/metadata.
            unsafe {
                let credential = &mut *self.raw;
                if !credential.CredentialBlob.is_null() && credential.CredentialBlobSize != 0 {
                    std::slice::from_raw_parts_mut(credential.CredentialBlob, credential.CredentialBlobSize as usize).zeroize();
                }
                (self.api.free)(self.raw.cast());
            }
        }
    }

    pub(super) fn load(pfx: &Path, fingerprint: &str) -> Result<Option<Secret>, String> {
        let target = target(pfx, fingerprint)?;
        if !pfx.is_file() { return Err("The selected PFX file is missing or is not a regular file. Its saved password was not deleted.".into()); }
        let api = Api::load()?;
        let mut raw = null_mut();
        if unsafe { (api.read)(target.as_ptr(), c::CRED_TYPE_GENERIC, 0, &mut raw) } == 0 {
            let code = last_error();
            if code == 1168 { return Ok(None); } // ERROR_NOT_FOUND, not a logon/access failure
            return Err(failure("read", code));
        }
        let allocation = Allocation { raw, api: &api };
        if raw.is_null() { return Err("Windows returned an invalid credential record.".into()); }
        let record = unsafe { &*raw };
        if record.Type != c::CRED_TYPE_GENERIC || record.Persist != c::CRED_PERSIST_LOCAL_MACHINE
            || record.CredentialBlobSize as usize > MAX_BLOB || record.CredentialBlobSize < MAGIC.len() as u32
            || record.CredentialBlob.is_null()
        { return Err("Saved password data is unsupported. Forget it and enter the password again.".into()); }
        let blob = unsafe { std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize) };
        let mut owned = Zeroizing::new(Vec::with_capacity(blob.len()));
        owned.extend_from_slice(blob);
        drop(allocation); // wipe/free native plaintext BEFORE decoding our owned copy
        if !owned.starts_with(MAGIC) { return Err("Saved password format is unsupported. Forget it and re-enter it.".into()); }
        let text = std::str::from_utf8(&owned[MAGIC.len()..]).map_err(|_| "Saved password encoding is invalid. Forget it and re-enter it.")?;
        valid_password(text)?;
        let mut password = String::with_capacity(text.len());
        password.push_str(text); // exactly sized; final owned buffer is cleared by Secret
        Ok(Some(Secret::new(password)))
    }

    pub(super) fn save(pfx: &Path, fingerprint: &str, password: &Secret) -> Result<(), String> {
        valid_password(password.expose())?;
        let target = target(pfx, fingerprint)?;
        if !pfx.is_file() { return Err("The selected PFX file is missing or is not a regular file; its password was not saved.".into()); }
        let mut blob = Zeroizing::new(Vec::with_capacity(MAGIC.len() + password.expose().len()));
        blob.extend_from_slice(MAGIC); blob.extend_from_slice(password.expose().as_bytes());
        let record = c::CREDENTIALW { Type: c::CRED_TYPE_GENERIC, TargetName: target.as_ptr().cast_mut(),
            CredentialBlobSize: blob.len() as u32, CredentialBlob: blob.as_mut_ptr(),
            Persist: c::CRED_PERSIST_LOCAL_MACHINE, ..Default::default() };
        let api = Api::load()?;
        // This stores only through the current user's OS credential service.
        // LOCAL_MACHINE is local persistence for THIS user, not a machine credential.
        if unsafe { (api.write)(&record, 0) } == 0 { return Err(failure("write", last_error())); }
        Ok(()) // zeroize our UTF-8 blob on drop, regardless of native success/failure
    }

    pub(super) fn forget(pfx: &Path, fingerprint: &str) -> Result<(), String> {
        let target = target(pfx, fingerprint)?;
        let api = Api::load()?;
        if unsafe { (api.delete)(target.as_ptr(), c::CRED_TYPE_GENERIC, 0) } == 0 {
            let code = last_error();
            if code != 1168 { return Err(failure("delete", code)); }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn binding_is_full_hash_of_lexical_path_and_certificate() {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("a.pfx"); let b = dir.path().join("b.pfx");
            std::fs::write(&a, b"synthetic, not a private identity").unwrap();
            std::fs::write(&b, b"synthetic, not a private identity").unwrap();
            let one = "ab".repeat(32); let two = "ac".repeat(32);
            let first = target(&a, &one).unwrap();
            assert_eq!(first, target(&dir.path().join(".").join("a.pfx"), &one.to_uppercase()).unwrap());
            assert_eq!(first, target(&dir.path().join("unused").join("..").join("a.pfx"), &one).unwrap());
            assert_ne!(first, target(&b, &one).unwrap());
            assert_ne!(first, target(&a, &two).unwrap());
            std::fs::hard_link(&a, dir.path().join("alias.pfx")).unwrap();
            assert_ne!(first, target(&dir.path().join("alias.pfx"), &one).unwrap());
            assert_eq!(first.len(), PREFIX.len() + 64 + 1);
            let display = String::from_utf16_lossy(&first[..first.len() - 1]);
            assert!(display.starts_with(PREFIX));
            assert!(!display.contains("a.pfx"));
            std::fs::remove_file(&a).unwrap();
            assert_eq!(first, target(&a, &one).unwrap());
            assert!(target(Path::new("relative.pfx"), &one).is_err());
            assert!(target(Path::new("bad\0path"), &one).is_err());
            let missing = dir.path().join("missing-parent").join("file.pfx");
            assert!(target(&missing, &one).is_ok(), "Forget binding requires no filesystem");
        }

        #[test]
        #[ignore = "Explicit developer opt-in: creates/deletes only a unique synthetic current-user credential"]
        fn native_unique_synthetic_credential_roundtrip_and_forget() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("synthetic-credential-only.pfx");
            std::fs::write(&path, b"synthetic placeholder, no certificate or private key").unwrap();
            let fingerprint = "d7".repeat(32);
            // Never overwrite/delete an unexpected existing record, even in the
            // astronomically unlikely event of a fresh temp-path/hash collision.
            assert!(load(&path, &fingerprint).unwrap().is_none());
            struct Cleanup { path: std::path::PathBuf, fingerprint: String }
            impl Drop for Cleanup { fn drop(&mut self) { let _ = forget(&self.path, &self.fingerprint); } }
            let _cleanup = Cleanup { path: path.clone(), fingerprint: fingerprint.clone() };
            let password = Secret::new("Synthetic only 密码 42!".into());
            save(&path, &fingerprint, &password).unwrap();
            assert!(load(&path, &fingerprint).unwrap().unwrap().expose() == password.expose(), "Synthetic saved password mismatch");
            assert!(load(&path, &"d8".repeat(32)).unwrap().is_none());
            save(&path, &fingerprint, &Secret::new(String::new())).unwrap();
            assert!(load(&path, &fingerprint).unwrap().unwrap().expose().is_empty());
            std::fs::remove_file(&path).unwrap();
            assert!(load(&path, &fingerprint).is_err(), "A missing PFX is not silently authenticated");
            std::fs::write(&path, b"synthetic placeholder").unwrap();
            assert!(load(&path, &fingerprint).unwrap().unwrap().expose().is_empty(), "Missing file must not auto-delete its credential");
            std::fs::remove_file(&path).unwrap();
            forget(&path, &fingerprint).unwrap(); // missing file is still forgettable
            forget(&path, &fingerprint).unwrap(); // idempotent, no enumeration
            std::fs::write(&path, b"synthetic placeholder").unwrap();
            assert!(load(&path, &fingerprint).unwrap().is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprint_and_password_bounds_reject_truncation_without_minimum() {
        assert_eq!(fingerprint_bytes(&"aB".repeat(32)).unwrap(), [0xab; 32]);
        for value in ["".to_owned(), "ab".repeat(31), "ab".repeat(33), "zz".repeat(32), format!("{}\0", "ab".repeat(32))] {
            assert!(fingerprint_bytes(&value).is_err());
        }
        assert!(valid_password("").is_ok());
        assert!(valid_password("x").is_ok());
        assert!(valid_password("Valid synthetic 密码").is_ok());
        assert!(valid_password("bad\0password").is_err());
        assert!(valid_password(&"x".repeat(1025)).is_err());
        assert!(valid_password(&"密".repeat(1024)).is_err(), "UTF-8 blob limit applies independently of UTF-16 PFX limit");
    }
}
