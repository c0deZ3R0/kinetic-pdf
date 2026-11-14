//! Nonblocking UI jobs for the same-executable, isolated signing helper.
//!
//! PATH: src/signing/client.rs
//! CONSUMED BY: app Signatures panel; child service owns all PDF/native-key work.
//! Passwords use zeroizing stdin buffers, never argv/env/logs. Drop/cancel never
//! joins parsing: an unnamed, noninheritable kill-on-close Windows Job contains
//! the helper before stdin receives secrets; job threads kill/reap and clean temps.
//! Publication is new-file/no-clobber; cancellation is checked at the commit point.
//! Fresh pre-publication verification checks CMS/ranges only. Native trust work
//! belongs to ordinary explicit/reopened verification, outside this deadline.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use eframe::egui;
use zeroize::Zeroizing;
use crate::document::Snapshot;
use super::{CreateIdentitySpec, HelperRequest, HelperResponse, HelperSuccess, HelperTask, JobOutcome, Secret, SignOptions,
    HELPER_FLAG, PROTOCOL_VERSION, MAX_PDF_BYTES, MAX_PFX_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
use super::service::{digest, read_limited};

const JOB_TIMEOUT: Duration = Duration::from_secs(120);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit { fn drop(&mut self) { ACTIVE.fetch_sub(1, Ordering::AcqRel); } }

pub struct Job {
    result: mpsc::Receiver<Result<JobOutcome, String>>,
    cancelled: Arc<AtomicBool>,
    consumed: AtomicBool,
}
impl Job {
    pub fn poll(&self) -> Option<Result<JobOutcome, String>> {
        if self.consumed.load(Ordering::Acquire) { return None; }
        match self.result.try_recv() {
            Ok(result) => { self.consumed.store(true, Ordering::Release); Some(result) }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.consumed.store(true, Ordering::Release);
                Some(Err("Signing job stopped without a result.".into()))
            }
        }
    }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Release); }
}
impl Drop for Job { fn drop(&mut self) { self.cancel(); } }

fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) { Err("Signing operation cancelled before publication.".into()) }
    else if Instant::now() >= deadline { Err("Signing operation timed out before publication.".into()) }
    else { Ok(()) }
}

fn start(ctx: egui::Context, work: impl FnOnce(&AtomicBool, Instant, u64) -> Result<JobOutcome, String> + Send + 'static) -> Result<Job, String> {
    ACTIVE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1)).map_err(|_| "Two signing jobs are already active; wait or cancel one.")?;
    let permit = Permit;
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_thread = cancelled.clone();
    let (send, result) = mpsc::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::thread::Builder::new().name("signing-job".into()).spawn(move || {
        let _permit = permit;
        let deadline = Instant::now() + JOB_TIMEOUT;
        let result = check(&cancel_thread, deadline).and_then(|_| work(&cancel_thread, deadline, id));
        let _ = send.send(result);
        ctx.request_repaint();
    }).map_err(|e| format!("Could not start signing job: {e}"))?;
    Ok(Job { result, cancelled, consumed: AtomicBool::new(false) })
}

#[derive(Clone)]
enum HelperProgram {
    Current,
    #[cfg(all(test, windows))]
    Test { exe: PathBuf, on_spawn: Arc<dyn Fn(u32) + Send + Sync>, on_task: Arc<dyn Fn(&HelperTask) + Send + Sync> },
}
impl HelperProgram {
    fn invoke(&self, request: HelperRequest, cancelled: &AtomicBool, deadline: Instant) -> Result<HelperSuccess, String> {
        match self {
            Self::Current => child(request, cancelled, deadline),
            #[cfg(all(test, windows))]
            Self::Test { exe, on_spawn, on_task } => {
                on_task(&request.task);
                child_at(exe, request, cancelled, deadline, |owned| {
                use std::os::windows::io::AsRawHandle;
                let mut member = 0;
                assert_ne!(unsafe { windows_sys::Win32::System::JobObjects::IsProcessInJob(
                    owned.process.as_raw_handle().cast(), owned.job.as_ref().unwrap().0, &mut member) }, 0);
                assert_ne!(member, 0, "private job assignment must precede every secret write");
                on_spawn(owned.id());
                })
            },
        }
    }
}

pub fn create_identity(spec: CreateIdentitySpec, password: Secret, destination: PathBuf, ctx: egui::Context) -> Result<Job, String> {
    create_identity_job(spec, password, destination, ctx, HelperProgram::Current)
}
fn create_identity_job(spec: CreateIdentitySpec, password: Secret, destination: PathBuf, ctx: egui::Context, program: HelperProgram) -> Result<Job, String> {
    start(ctx, move |cancelled, deadline, id| {
        let guard = NewOutput::capture(&destination, &[])?;
        let private = tempfile::Builder::new().prefix("kinetic-signing-").tempdir().map_err(|e| e.to_string())?;
        let output = private.path().join("identity.pfx");
        let success = program.invoke(HelperRequest { version: PROTOCOL_VERSION, id, task: HelperTask::CreatePfx { spec, password, output: output.clone() } }, cancelled, deadline)?;
        let HelperSuccess::IdentityCreated { info } = success else { return Err("Unexpected helper response for identity creation.".into()); };
        let bytes = Zeroizing::new(read_limited(&output, MAX_PFX_BYTES)?);
        guard.commit(&bytes, cancelled, deadline)?;
        Ok(JobOutcome::IdentityCreated { path: destination, info })
    })
}

pub fn inspect_identity(pfx: PathBuf, password: Secret, ctx: egui::Context) -> Result<Job, String> {
    inspect_identity_job(pfx, password, ctx, HelperProgram::Current)
}
fn inspect_identity_job(pfx: PathBuf, password: Secret, ctx: egui::Context, program: HelperProgram) -> Result<Job, String> {
    start(ctx, move |cancelled, deadline, id| {
        let success = program.invoke(HelperRequest { version: PROTOCOL_VERSION, id, task: HelperTask::InspectPfx { pfx: pfx.clone(), password } }, cancelled, deadline)?;
        let HelperSuccess::IdentityInspected { info } = success else { return Err("Unexpected helper response for identity inspection.".into()); };
        Ok(JobOutcome::IdentityInspected { path: pfx, info })
    })
}

pub fn sign_copy(snapshot: Snapshot, source_path: PathBuf, destination: PathBuf, pfx: PathBuf, password: Secret, options: SignOptions, ctx: egui::Context) -> Result<Job, String> {
    sign_copy_job(snapshot, source_path, destination, pfx, password, options, ctx, HelperProgram::Current)
}
fn sign_copy_job(snapshot: Snapshot, source_path: PathBuf, destination: PathBuf, pfx: PathBuf, password: Secret, options: SignOptions, ctx: egui::Context, program: HelperProgram) -> Result<Job, String> {
    start(ctx, move |cancelled, deadline, id| {
        let guard = NewOutput::capture(&destination, &[&source_path, &pfx])?;
        let private = tempfile::Builder::new().prefix("kinetic-signing-").tempdir().map_err(|e| e.to_string())?;
        let input = private.path().join("input.pdf");
        let output = private.path().join("signed.pdf");
        snapshot_file(&input, &snapshot)?;
        let expected_identity = options.expected_identity_fingerprint.clone();
        let require_p1 = options.mode == super::SignatureMode::CertifyNoChanges;
        let success = program.invoke(HelperRequest { version: PROTOCOL_VERSION, id, task: HelperTask::SignPdf { input, input_sha256: digest(snapshot.bytes()),
            output: output.clone(), pfx, password, options } }, cancelled, deadline)?;
        let HelperSuccess::PdfSigned { report: prepared_report } = success else { return Err("Unexpected helper response for PDF signing.".into()); };
        let bytes = read_limited(&output, MAX_PDF_BYTES)?;
        let normalized = prepared_report.signatures.iter().any(|s| s.message.contains(super::pdf::NORMALIZED_NOTE));
        if !bytes.starts_with(snapshot.bytes()) && !normalized {
            return Err("Signed candidate neither preserves the original prefix nor discloses a prepared unsigned normalization.".into());
        }
        // Re-verify the actual output file under the parent-captured digest in a
        // second isolated child, without native chain work under this deadline.
        // The exact retained buffer is then published;
        // no PDF/CMS parsing or native-library initialization occurs in parent.
        let verified = program.invoke(HelperRequest { version: PROTOCOL_VERSION, id, task: HelperTask::VerifyPublicationPdf { input: output, input_sha256: digest(&bytes) } }, cancelled, deadline)?;
        let HelperSuccess::PdfPublicationVerified { report } = verified else { return Err("Unexpected final PDF verification response.".into()); };
        if report.signatures.len() != 1 || !report.signatures[0].integrity_valid || report.signatures[0].later_changes
            || (require_p1 && report.signatures[0].certification != Some(1)) {
            return Err("Actual signed output did not pass final byte-range/CMS verification.".into());
        }
        if let Some(expected) = expected_identity {
            if !report.signatures[0].signer.as_ref().is_some_and(|info| expected.eq_ignore_ascii_case(&info.sha256_fingerprint)) {
                return Err("Identity file changed; reopen it to confirm".into());
            }
        }
        guard.commit(&bytes, cancelled, deadline)?;
        Ok(JobOutcome::PdfSigned { path: destination, report })
    })
}

pub fn verify_pdf(snapshot: Snapshot, ctx: egui::Context) -> Result<Job, String> {
    verify_pdf_job(snapshot, ctx, HelperProgram::Current)
}
fn verify_pdf_job(snapshot: Snapshot, ctx: egui::Context, program: HelperProgram) -> Result<Job, String> {
    start(ctx, move |cancelled, deadline, id| {
        let private = tempfile::Builder::new().prefix("kinetic-signing-").tempdir().map_err(|e| e.to_string())?;
        let input = private.path().join("input.pdf");
        snapshot_file(&input, &snapshot)?;
        let success = program.invoke(HelperRequest { version: PROTOCOL_VERSION, id, task: HelperTask::VerifyPdf { input, input_sha256: digest(snapshot.bytes()) } }, cancelled, deadline)?;
        let HelperSuccess::PdfVerified { report } = success else { return Err("Unexpected helper response for PDF verification.".into()); };
        check(cancelled, deadline)?;
        Ok(JobOutcome::PdfVerified { report })
    })
}

fn snapshot_file(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.bytes().len() > MAX_PDF_BYTES { return Err("PDF exceeds signing size limit.".into()); }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| e.to_string())?;
    file.write_all(snapshot.bytes()).and_then(|_| file.flush()).map_err(|e| e.to_string())
}

#[cfg(windows)]
struct ChildJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl ChildJob {
    fn assign(process: &Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{CreateJobObjectW, SetInformationJobObject, AssignProcessToJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation};
        // Null attributes => noninheritable handle; null name => private unnamed
        // job. Created after child spawn, so the child cannot inherit the handle.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() { return Err(format!("Could not create signing isolation job: {}", std::io::Error::last_os_error())); }
        let job = Self(raw);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe { SetInformationJobObject(job.0, JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32) };
        if configured == 0 { return Err(format!("Could not configure signing isolation job: {}", std::io::Error::last_os_error())); }
        if unsafe { AssignProcessToJobObject(job.0, process.as_raw_handle().cast()) } == 0 {
            return Err(format!("Could not assign owned signing helper to isolation job: {}", std::io::Error::last_os_error()));
        }
        Ok(job)
    }
}
#[cfg(windows)]
impl Drop for ChildJob { fn drop(&mut self) { unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0); } } }

struct OwnedChild {
    process: Child,
    #[cfg(windows)]
    job: Option<ChildJob>,
}
impl std::ops::Deref for OwnedChild { type Target = Child; fn deref(&self) -> &Child { &self.process } }
impl std::ops::DerefMut for OwnedChild { fn deref_mut(&mut self) -> &mut Child { &mut self.process } }
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.process.try_wait(), Ok(Some(_))) { let _ = self.process.kill(); }
        let _ = self.process.wait();
    }
}

fn spawn_owned(exe: &Path) -> Result<OwnedChild, String> {
    #[cfg(not(windows))] { let _ = exe; return Err("Signing helper isolation requires Windows Job Objects.".into()); }
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new(exe);
        command.arg(HELPER_FLAG).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(0x08000000);
        let mut owned = OwnedChild { process: command.spawn().map_err(|e| format!("Could not start owned signing helper: {e}"))?, job: None };
        owned.job = Some(ChildJob::assign(&owned.process)?);
        Ok(owned)
    }
}

fn child(request: HelperRequest, cancelled: &AtomicBool, deadline: Instant) -> Result<HelperSuccess, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    child_at(&exe, request, cancelled, deadline, |_| {})
}

// Private transport seam for developer tests; shipped callers always use this
// process's executable, never an argv/environment-selected crypto program.
fn child_at(exe: &Path, request: HelperRequest, cancelled: &AtomicBool, deadline: Instant, on_spawn: impl FnOnce(&OwnedChild)) -> Result<HelperSuccess, String> {
    check(cancelled, deadline)?;
    let id = request.id;
    let payload = Zeroizing::new(serde_json::to_vec(&request).map_err(|_| "Could not encode signing request.")?);
    drop(request); // Secret's owned string clears before native work begins.
    if payload.len() > MAX_REQUEST_BYTES { return Err("Signing request exceeds the transport limit.".into()); }
    let mut process = spawn_owned(exe)?; // Assigned to private kill-on-close job BEFORE any stdin write.
    let mut stdin = process.stdin.take().ok_or("Missing helper stdin.")?;
    let mut stdout = process.stdout.take().ok_or("Missing helper stdout.")?;
    on_spawn(&process);
    let writer = std::thread::Builder::new().name("signing-stdin".into())
        .spawn(move || stdin.write_all(&payload).map_err(|_| "Could not send signing request.".to_owned()))
        .map_err(|_| "Could not start bounded signing transport writer.")?;
    let reader = std::thread::Builder::new().name("signing-stdout".into()).spawn(move || -> Result<Zeroizing<Vec<u8>>, String> {
        let mut bytes = Zeroizing::new(Vec::new());
        (&mut stdout).take(MAX_RESPONSE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| "Could not read helper response.".to_owned())?;
        if bytes.len() > MAX_RESPONSE_BYTES { return Err("Signing response exceeds the transport limit.".into()); }
        Ok(bytes)
    }).map_err(|_| "Could not start bounded signing transport reader.")?;
    let status = loop {
        if let Err(error) = check(cancelled, deadline) {
            let _ = process.kill();
            let _ = process.wait();
            let _ = writer.join();
            let _ = reader.join();
            return Err(error);
        }
        match process.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = process.kill(); let _ = process.wait();
                let _ = writer.join(); let _ = reader.join();
                return Err(format!("Owned signing helper failed: {error}"));
            }
        }
    };
    let _ = process.wait();
    writer.join().map_err(|_| "Signing transport writer stopped.")??;
    let raw = reader.join().map_err(|_| "Signing transport reader stopped.")??;
    if !status.success() { return Err("Signing helper failed or aborted; original and existing destinations were not replaced.".into()); }
    let response: HelperResponse = serde_json::from_slice(&raw).map_err(|_| "Invalid signing helper response.")?;
    if response.version != PROTOCOL_VERSION { return Err("Signing helper protocol version mismatch.".into()); }
    if response.id != id { return Err("Signing helper operation ID mismatch.".into()); }
    check(cancelled, deadline)?;
    response.result
}

/// New-file-only guard: unlike replacement exports, this never parses an
/// existing destination in the parent. File identities/paths and absence are
/// rechecked; persist_noclobber refuses a racing creator. Not filesystem CAS.
struct NewOutput { path: PathBuf, parent: PathBuf, protected: Vec<(PathBuf, PathBuf, Option<same_file::Handle>)> }
fn location(path: &Path) -> Result<PathBuf, String> {
    match path.canonicalize() {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(path.parent().ok_or("Missing output parent.")?.canonicalize().map_err(|e| e.to_string())?.join(path.file_name().ok_or("Missing file name.")?))
        }
        Err(e) => Err(e.to_string()),
    }
}
fn same_location(a: &Path, b: &Path) -> bool {
    #[cfg(windows)] { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() }
    #[cfg(not(windows))] { a == b }
}
impl NewOutput {
    fn capture(path: &Path, protected: &[&Path]) -> Result<Self, String> {
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        let parent = path.parent().ok_or("Destination needs a directory.")?.canonicalize().map_err(|e| e.to_string())?;
        let mut sources = vec![];
        for p in protected { sources.push((p.to_path_buf(), location(p)?, same_file::Handle::from_path(p).ok())); }
        let guard = Self { path, parent, protected: sources };
        guard.recheck()?;
        Ok(guard)
    }
    fn recheck(&self) -> Result<(), String> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => return Err("Choose a NEW file: signing never replaces an existing destination, symlink or hard link.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("Could not inspect signing destination: {e}")),
        }
        if self.path.parent().unwrap().canonicalize().map_err(|e| e.to_string())? != self.parent { return Err("Signing destination directory changed.".into()); }
        let target = location(&self.path)?;
        for (path, original, identity) in &self.protected {
            if same_location(&target, original) || same_location(&target, &location(path)?) { return Err("Signed-copy destination aliases a protected input.".into()); }
            if let (Some(original), Ok(target)) = (identity, same_file::Handle::from_path(&self.path)) {
                if *original == target { return Err("Signed-copy destination aliases a protected input.".into()); }
            }
        }
        Ok(())
    }
    fn commit(&self, bytes: &[u8], cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
        if bytes.is_empty() { return Err("Refusing empty signing output.".into()); }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.parent).map_err(|e| e.to_string())?;
        temporary.write_all(bytes).and_then(|_| temporary.flush()).map_err(|e| e.to_string())?;
        self.recheck()?;
        check(cancelled, deadline)?; // Last cancellation check before atomic publication.
        temporary.persist_noclobber(&self.path).map_err(|_| "Signing destination appeared or could not be created; no existing file was replaced.".to_owned())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    fn fresh_helper() -> Option<PathBuf> {
        let Some(path) = std::env::var_os("KINETIC_SIGNING_TEST_EXE") else {
            eprintln!("Real client lifecycle test not run: set KINETIC_SIGNING_TEST_EXE to the freshly built app (test-only injection)");
            return None;
        };
        let path = PathBuf::from(path);
        assert!(path.is_file(), "fresh test helper binary must exist");
        Some(path)
    }

    #[cfg(windows)]
    #[test]
    fn owned_job_close_terminates_helper_waiting_for_stdin_without_keys() {
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
        let Some(exe) = fresh_helper() else { return };
        let mut owned = spawn_owned(&exe).unwrap();
        let mut flags = 0;
        assert_ne!(unsafe { GetHandleInformation(owned.job.as_ref().unwrap().0, &mut flags) }, 0);
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0, "job handle must remain parent-owned/noninheritable");
        assert!(owned.try_wait().unwrap().is_none());
        // Leave stdin open and send no DTO/secret. Closing JUST this unnamed job
        // must terminate its assigned child independently of stdin EOF/cancel.
        drop(owned.job.take());
        let start = Instant::now();
        loop {
            if owned.try_wait().unwrap().is_some() { break; }
            assert!(start.elapsed() < Duration::from_secs(5), "kill-on-job-close did not terminate owned helper");
            std::thread::sleep(Duration::from_millis(5));
        }
        owned.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn real_high_level_jobs_create_inspect_sign_verify_publish_and_cancel() {
        let Some(exe) = fresh_helper() else { return };
        let ctx = egui::Context::default();
        let tasks = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = tasks.clone();
        let program = HelperProgram::Test { exe: exe.clone(), on_spawn: Arc::new(|_| {}), on_task: Arc::new(move |task| {
            observed.lock().unwrap().push(match task {
                HelperTask::SignPdf { .. } => "sign",
                HelperTask::VerifyPublicationPdf { .. } => "publication",
                HelperTask::VerifyPdf { .. } => "verify",
                _ => "identity",
            });
        }) };
        let dir = tempfile::tempdir().unwrap();
        let pfx = dir.path().join("created.pfx");
        let spec = || CreateIdentitySpec { name: "High-level client test".into(), organization: "Synthetic".into(), email: "test@example.invalid".into(), valid_years: 1 };
        let password = || Secret::new("Runtime high-level identity password 93!".into());
        let wait = |job: &Job| -> Result<JobOutcome, String> {
            let started = Instant::now();
            loop {
                if let Some(result) = job.poll() { return result; }
                if started.elapsed() > Duration::from_secs(30) { job.cancel(); return Err("high-level client test timed out".into()); }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let job = create_identity_job(spec(), password(), pfx.clone(), ctx.clone(), program.clone()).unwrap();
        let JobOutcome::IdentityCreated { info: approved_identity, .. } = wait(&job).unwrap() else { panic!("wrong creation outcome") };
        assert!(pfx.exists());
        let inspect = inspect_identity_job(pfx.clone(), password(), ctx.clone(), program.clone()).unwrap();
        assert!(matches!(wait(&inspect).unwrap(), JobOutcome::IdentityInspected { .. }));
        let wrong = inspect_identity_job(pfx.clone(), Secret::new("wrong".into()), ctx.clone(), program.clone()).unwrap();
        assert!(wait(&wrong).is_err());
        let bytes = pdf_content::fixtures::stamped_pdf();
        let source = dir.path().join("source.pdf"); fs::write(&source, &bytes).unwrap();
        let signed = dir.path().join("signed.pdf");
        let options = SignOptions { mode: super::super::SignatureMode::CertifyNoChanges, expected_identity_fingerprint: Some(approved_identity.sha256_fingerprint.clone()), ..Default::default() };
        let wrong_output = dir.path().join("wrong-fingerprint.pdf");
        let wrong_identity = sign_copy_job(Snapshot::new(bytes.clone()), source.clone(), wrong_output.clone(), pfx.clone(), password(),
            SignOptions { expected_identity_fingerprint: Some("00".repeat(32)), ..Default::default() }, ctx.clone(), program.clone()).unwrap();
        assert!(wait(&wrong_identity).unwrap_err().contains("Identity file changed; reopen it to confirm"));
        assert!(!wrong_output.exists());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        tasks.lock().unwrap().clear();
        let job = sign_copy_job(Snapshot::new(bytes.clone()), source.clone(), signed.clone(), pfx.clone(), password(),
            options, ctx.clone(), program.clone()).unwrap();
        let JobOutcome::PdfSigned { report, .. } = wait(&job).unwrap() else { panic!("wrong high-level outcome") };
        assert!(report.signatures[0].integrity_valid && !report.signatures[0].later_changes);
        assert_eq!(tasks.lock().unwrap().as_slice(), &["sign", "publication"], "parent must never request ordinary trust verification before commit");
        assert_eq!(report.signatures[0].certification, Some(1));
        assert_eq!(report.signatures[0].identity_trust, super::super::IdentityTrust::NotChecked);
        assert!(report.signatures[0].trust_report.is_none());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let signed_bytes = fs::read(&signed).unwrap();
        let verify = verify_pdf_job(Snapshot::new(signed_bytes.clone()), ctx.clone(), program.clone()).unwrap();
        let JobOutcome::PdfVerified { report } = wait(&verify).unwrap() else { panic!("wrong high-level verify outcome") };
        assert!(report.signatures[0].integrity_valid);
        assert!(report.signatures[0].trust_report.is_some());
        assert_eq!(tasks.lock().unwrap().last(), Some(&"verify"));
        // Same high-level public workflow must refuse any existing final target.
        let refused = sign_copy_job(Snapshot::new(bytes), source.clone(), signed.clone(), pfx, password(),
            SignOptions::default(), ctx.clone(), program).unwrap();
        assert!(wait(&refused).is_err());
        assert_eq!(fs::read(&signed).unwrap(), signed_bytes);
        let (spawned, spawned_rx) = mpsc::channel();
        let cancel_program = HelperProgram::Test { exe: exe.clone(), on_spawn: Arc::new(move |pid| { let _ = spawned.send(pid); }), on_task: Arc::new(|_| {}) };
        let cancelled_output = dir.path().join("cancelled.pfx");
        let cancelled = create_identity_job(spec(), password(), cancelled_output.clone(), ctx, cancel_program).unwrap();
        spawned_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cancelled.cancel();
        assert!(wait(&cancelled).unwrap_err().contains("cancelled"));
        assert!(!cancelled_output.exists());
        // Deterministic real Job::drop: hold the background callback after job
        // assignment but before stdin. Drop must not join this held worker.
        let (spawned, spawned_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = std::sync::Mutex::new(resume_rx);
        let drop_program = HelperProgram::Test { exe, on_task: Arc::new(|_| {}),
            on_spawn: Arc::new(move |pid| { let _ = spawned.send(pid); let _ = resume_rx.lock().unwrap().recv(); }) };
        let dropped_output = dir.path().join("dropped.pfx");
        let dropped = create_identity_job(spec(), password(), dropped_output.clone(), egui::Context::default(), drop_program).unwrap();
        let pid = spawned_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let process = unsafe { windows_sys::Win32::System::Threading::OpenProcess(0x00100000, 0, pid) };
        assert!(!process.is_null());
        drop(dropped);
        resume.send(()).unwrap();
        let wait_result = unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(process, 5000) };
        unsafe { windows_sys::Win32::Foundation::CloseHandle(process); }
        assert_eq!(wait_result, 0, "dropped high-level job must terminate its owned helper");
        assert!(!dropped_output.exists());
    }

    #[test]
    fn guarded_publication_refuses_existing_racing_alias_and_cancelled_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pdf"); fs::write(&source, b"original").unwrap();
        assert!(NewOutput::capture(&source, &[&source]).is_err());
        let alias = dir.path().join("alias.pdf"); fs::hard_link(&source, &alias).unwrap();
        assert!(NewOutput::capture(&alias, &[&source]).is_err());
        let output = dir.path().join("new.pdf");
        let guard = NewOutput::capture(&output, &[&source]).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(guard.commit(b"signed", &cancelled, Instant::now() + JOB_TIMEOUT).is_err());
        assert!(!output.exists());
        fs::write(&output, b"racing creator").unwrap();
        assert!(guard.commit(b"signed", &AtomicBool::new(false), Instant::now() + JOB_TIMEOUT).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"racing creator");
        assert_eq!(fs::read(&source).unwrap(), b"original");
    }
}
