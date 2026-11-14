//! Isolated stdin/stdout signing helper: bounded DTOs, native crypto and PDF work.
//!
//! PATH: src/signing/service.rs
//! CONSUMED BY: main --signature-service and client-owned child processes.
//! No subprocess commands, network or final-path publication. Parser aborts stay
//! in this child; private output is create_new, input hashes are checked first.
//! Versioned reports keep CMS integrity and cache-only certificate trust separate;
//! publication signing/rechecking never evaluates trust. Only ordinary VerifyPdf
//! assesses current-time trust after valid CMS; no chain work delays publication.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};
use super::{identity, pdf, HelperRequest, HelperResponse, HelperSuccess, HelperTask, MAX_PDF_BYTES, MAX_PFX_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION};

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("Could not open signing input: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 { return Err("Signing input is not a regular file or exceeds its size limit.".into()); }
    let mut bytes = Vec::new();
    (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > limit { return Err("Signing input grew beyond its size limit.".into()); }
    Ok(bytes)
}

fn input(path: &Path, expected: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("Invalid immutable-input digest.".into()); }
    let bytes = Zeroizing::new(read_limited(path, MAX_PDF_BYTES)?);
    if digest(&bytes) != expected { return Err("Immutable signing input changed; operation refused.".into()); }
    Ok(bytes)
}

fn private_output(path: &Path, bytes: &[u8], protected: &[&Path]) -> Result<(), String> {
    if bytes.is_empty() { return Err("Helper produced empty output.".into()); }
    let parent = path.parent().ok_or("Private output has no parent.")?.canonicalize().map_err(|e| e.to_string())?;
    let temporary_root = std::env::temp_dir().canonicalize().map_err(|e| e.to_string())?;
    if parent.parent() != Some(temporary_root.as_path()) || !parent.file_name().is_some_and(|n| n.to_string_lossy().starts_with("kinetic-signing-")) {
        return Err("Helper output must be inside its private kinetic-signing-* temporary directory, never a final destination.".into());
    }
    let location = parent.join(path.file_name().ok_or("Private output has no file name.")?);
    for source in protected {
        if let Ok(source) = source.canonicalize() { if source == location { return Err("Helper output aliases an input.".into()); } }
    }
    // Existing files, hard links and symlinks all fail create_new; this helper
    // never writes the final user destination supplied to the parent job.
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("Could not create private signing output: {e}"))?;
    file.write_all(bytes).and_then(|_| file.flush()).map_err(|e| format!("Could not write private signing output: {e}"))
}

fn execute(task: HelperTask) -> Result<HelperSuccess, String> {
    match task {
        HelperTask::CreatePfx { spec, password, output } => {
            let (bytes, info) = identity::create_pfx(&spec, password.expose())?;
            let bytes = Zeroizing::new(bytes);
            if bytes.len() > MAX_PFX_BYTES { return Err("Generated identity exceeds PFX size limit.".into()); }
            private_output(&output, &bytes, &[])?;
            Ok(HelperSuccess::IdentityCreated { info })
        }
        HelperTask::InspectPfx { pfx, password } => {
            let bytes = Zeroizing::new(read_limited(&pfx, MAX_PFX_BYTES)?);
            let identity = identity::Identity::from_pfx(&bytes, password.expose())?;
            Ok(HelperSuccess::IdentityInspected { info: identity.info().clone() })
        }
        HelperTask::SignPdf { input: source, input_sha256, output, pfx, password, options } => {
            let bytes = input(&source, &input_sha256)?;
            let pfx_bytes = Zeroizing::new(read_limited(&pfx, MAX_PFX_BYTES)?);
            let identity = identity::Identity::from_pfx(&pfx_bytes, password.expose())?;
            let (signed, report) = pdf::sign(&bytes, &identity, &options)?;
            private_output(&output, &signed, &[&source, &pfx])?;
            Ok(HelperSuccess::PdfSigned { report })
        }
        HelperTask::VerifyPdf { input: source, input_sha256 } => {
            let bytes = input(&source, &input_sha256)?;
            Ok(HelperSuccess::PdfVerified { report: pdf::verify(&bytes)? })
        }
        HelperTask::VerifyPublicationPdf { input: source, input_sha256 } => {
            let bytes = input(&source, &input_sha256)?;
            Ok(HelperSuccess::PdfPublicationVerified { report: pdf::verify_publication(&bytes)? })
        }
    }
}

/// Call before GUI/updater/PDFium initialization. Passwords are never formatted
/// into diagnostics. Own transport/input buffers zeroize on every normal exit;
/// no guarantee is made for serializer/native/OS copies or process-abort cleanup.
pub fn run() -> i32 {
    let mut raw = Zeroizing::new(Vec::new());
    if std::io::stdin().lock().take(MAX_REQUEST_BYTES as u64 + 1).read_to_end(&mut raw).is_err() || raw.len() > MAX_REQUEST_BYTES {
        return 2;
    }
    let request: HelperRequest = match serde_json::from_slice(&raw) { Ok(r) => r, Err(_) => return 2 };
    raw.zeroize(); // DTO owns its fields; do not retain a duplicate password during crypto.
    let id = request.id;
    let result = if request.version == PROTOCOL_VERSION { execute(request.task) }
        else { Err("Unsupported signing helper protocol version.".into()) };
    let response = HelperResponse { version: PROTOCOL_VERSION, id, result };
    let encoded = match serde_json::to_vec(&response) { Ok(v) if v.len() <= MAX_RESPONSE_BYTES => Zeroizing::new(v), _ => {
        match serde_json::to_vec(&HelperResponse { version: PROTOCOL_VERSION, id, result: Err("Signing helper response exceeded its limit.".into()) }) {
            Ok(v) => Zeroizing::new(v), Err(_) => return 3,
        }
    } };
    if std::io::stdout().lock().write_all(&encoded).is_err() { 3 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_outputs_never_replace_inputs_or_existing_targets() {
        let dir = tempfile::Builder::new().prefix("kinetic-signing-").tempdir().unwrap();
        let path = dir.path().join("existing");
        std::fs::write(&path, b"original").unwrap();
        assert!(private_output(&path, b"replacement", &[&path]).is_err());
        assert!(private_output(&path, b"replacement", &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let new = dir.path().join("new");
        private_output(&new, b"new", &[&path]).unwrap();
        assert_eq!(std::fs::read(new).unwrap(), b"new");
    }
    #[cfg(windows)]
    #[test]
    fn signing_and_publication_recheck_never_call_trust_but_normal_verify_does() {
        use super::super::{CreateIdentitySpec, IdentityTrust, Secret, SignOptions, SignatureMode};
        let dir = tempfile::Builder::new().prefix("kinetic-signing-").tempdir().unwrap();
        let pfx = dir.path().join("synthetic.pfx");
        let password = || Secret::new("Synthetic publication boundary test 94!".into());
        execute(HelperTask::CreatePfx { spec: CreateIdentitySpec { name: "Synthetic publication boundary".into(), organization: "Kinetic test".into(), email: "test@example.invalid".into(), valid_years: 1 }, password: password(), output: pfx.clone() }).unwrap();
        let bytes = pdf_content::fixtures::stamped_pdf();
        let source = dir.path().join("source.pdf"); std::fs::write(&source, &bytes).unwrap();
        let output = dir.path().join("signed.pdf");
        pdf::TRUST_EVALUATIONS.with(|count| count.set(0));
        let HelperSuccess::PdfSigned { report } = execute(HelperTask::SignPdf { input: source, input_sha256: digest(&bytes), output: output.clone(), pfx, password: password(), options: SignOptions { mode: SignatureMode::CertifyNoChanges, ..Default::default() } }).unwrap() else { panic!("signing receipt") };
        assert_eq!(pdf::TRUST_EVALUATIONS.with(|count| count.get()), 0, "pdf::sign must not enter native trust evaluation");
        assert_eq!(report.signatures[0].identity_trust, IdentityTrust::NotChecked);
        assert!(report.signatures[0].trust_report.is_none());
        let signed = std::fs::read(&output).unwrap();
        let HelperSuccess::PdfPublicationVerified { report } = execute(HelperTask::VerifyPublicationPdf { input: output.clone(), input_sha256: digest(&signed) }).unwrap() else { panic!("publication receipt") };
        assert_eq!(pdf::TRUST_EVALUATIONS.with(|count| count.get()), 0, "parent publication recheck must not enter native trust evaluation");
        assert!(report.signatures[0].integrity_valid && !report.signatures[0].later_changes);
        assert_eq!(report.signatures[0].certification, Some(1));
        assert_eq!(report.signatures[0].identity_trust, IdentityTrust::NotChecked);
        assert!(report.signatures[0].trust_report.is_none());
        let HelperSuccess::PdfVerified { report } = execute(HelperTask::VerifyPdf { input: output, input_sha256: digest(&signed) }).unwrap() else { panic!("ordinary verification") };
        assert_eq!(pdf::TRUST_EVALUATIONS.with(|count| count.get()), 1, "ordinary verify must actually enter the trust evaluator");
        assert!(report.signatures[0].integrity_valid && report.signatures[0].trust_report.is_some());
        assert_ne!(report.signatures[0].identity_trust, IdentityTrust::NotChecked);
    }
    #[test]
    fn mismatched_digest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input");
        std::fs::write(&path, b"changed").unwrap();
        assert!(input(&path, &digest(b"original")).is_err());
    }
}
