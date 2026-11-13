//! Compact Signatures panel with real isolated jobs and password/detail dialogs.
//!
//! PATH: src/app/signatures.rs
//! CONSUMED BY: app rail, menus, palette, frame polling and lifecycle transitions.
//! Passwords are never preferences; explicit remembering uses Windows credentials.
//! Draft previews/results are revision-bound; certification also binds the
//! selected certificate fingerprint and exact immutable document bytes.
//! CMS integrity is not identity trust. Sign adds session-only drafts. Certify
//! reuses drafts (or signs invisibly); Sign & Certify adds a final widget. Both seal one P1
//! copy, then open and reverify its new immutable snapshot in this window.
//! Draft loss has a separate discard guard, never ordinary PDF dirty state.

use super::*;
use crate::signing::{client, CreateIdentitySpec, IdentityInfo, IdentityTrust, JobOutcome, Secret,
    SignOptions, SignatureMode, VerificationReport};

pub(super) mod placement;
mod remember;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Binding { pub(super) generation: u64, pub(super) file: u64 }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation { Create, Inspect, Sign, Verify }

struct Active {
    job: client::Job,
    operation: Operation,
    serial: u64,
    binding: Option<Binding>,
    remember: Option<remember::Pending>,
}

struct CreateForm {
    name: String,
    organization: String,
    email: String,
    years: u16,
    password: Secret,
    confirmation: Secret,
    remember: bool,
}
impl Default for CreateForm {
    fn default() -> Self {
        Self { name: String::new(), organization: String::new(), email: String::new(), years: 3,
            password: Secret::default(), confirmation: Secret::default(), remember: false }
    }
}
impl CreateForm {
    fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() { return Err("Enter a name for the digital ID."); }
        if self.name.len() > 256 || self.organization.len() > 256 || self.email.len() > 256
            || [&self.name, &self.organization, &self.email].iter().any(|s| s.chars().any(char::is_control)) {
            return Err("Identity fields must be short and contain no control characters.");
        }
        if !self.email.is_empty() && (!self.email.contains('@') || self.email.contains(' ')) {
            return Err("Enter a valid email address, or leave email blank.");
        }
        if !(1..=10).contains(&self.years) { return Err("Choose a validity of 1–10 years."); }
        if self.password.expose().contains('\0') || self.confirmation.expose().contains('\0') {
            return Err("Passwords cannot contain a NUL character.");
        }
        if self.password.expose().chars().count() < 8 { return Err("Use at least 8 password characters; 12 or more is recommended."); }
        if self.password.expose() != self.confirmation.expose() { return Err("The passwords do not match."); }
        Ok(())
    }
}
struct ImportForm { path: PathBuf, password: Secret, remember: bool }
struct OpeningCertifiedCopy { generation: u64, path: PathBuf }

#[derive(Default)]
pub(super) struct State {
    identity_path: Option<PathBuf>,
    identity: Option<IdentityInfo>,
    identity_restored: bool,
    password: Secret,
    remember_password: bool,
    credential_epoch: u64,
    credential: Option<remember::Task>,
    credential_notice: Option<String>,
    credential_warning: Option<String>,
    pub(super) placement: placement::Placement,
    placement_lock: Option<Binding>,
    discard_drafts: Option<Discarding>,
    reason: String,
    create: Option<CreateForm>,
    import: Option<ImportForm>,
    confirm_sign: Option<Binding>,
    identity_details: bool,
    report_details: Option<(String, VerificationReport)>,
    active: Option<Active>,
    serial: u64,
    document_binding: Option<Binding>,
    pub(super) discovery: Option<(Binding, crate::domain::SignatureDiscovery)>,
    verification: Option<(Binding, VerificationReport)>,
    output: Option<PathBuf>,
    output_binding: Option<Binding>,
    output_report: Option<VerificationReport>,
    opening_copy: Option<OpeningCertifiedCopy>,
    message: Option<String>,
    error: Option<String>,
}
impl State {
    pub(super) fn restore_public(remembered: Option<prefs::RememberedIdentity>) -> Self {
        let mut state = Self::default();
        if let Some(remembered) = remembered {
            state.identity_path = Some(remembered.path); state.identity = Some(remembered.info); state.identity_restored = true;
        }
        state
    }
    fn identity_key(&self) -> Option<remember::Key> {
        Some(remember::Key { path: self.identity_path.clone()?, fingerprint: self.identity.as_ref()?.sha256_fingerprint.clone() })
    }
    fn accepts_credential_load(&self, epoch: u64, key: &remember::Key, current: Option<Binding>) -> bool {
        epoch == self.credential_epoch && self.identity_key().as_ref() == Some(key)
            && self.confirm_sign.is_some() && self.confirm_sign == current
    }
    fn has_inspected_identity(&self) -> bool {
        self.identity_path.is_some() && self.identity.as_ref().is_some_and(|info| !info.sha256_fingerprint.is_empty())
    }
    fn can_open_signed_outcome(&self, serial: u64, operation: Operation, binding: Option<Binding>, current: Option<Binding>, outcome: &JobOutcome) -> bool {
        serial == self.serial && operation == Operation::Sign && binding.is_some() && binding == current
            && matches!(outcome, JobOutcome::PdfSigned { .. })
    }
    fn selected_sign_options(&self) -> Result<SignOptions, &'static str> {
        if !self.has_inspected_identity() { return Err("Create or reopen/import the digital ID before signing."); }
        let info = self.identity.as_ref().unwrap(); // Checked above; no native identity work here.
        let appearances = self.placement.appearances(self.document_binding);
        if !self.placement.final_pending() || appearances.len() > 256 {
            return Err("Choose Certify for existing drafts, or Sign & Certify for a new final box.");
        }
        Ok(SignOptions { mode: SignatureMode::CertifyNoChanges, reason: self.reason.clone(),
            expected_identity_fingerprint: Some(info.sha256_fingerprint.clone()), appearance: None, appearances })
    }
    fn credential_failure(&mut self, loading: bool, error: String) {
        if loading { self.credential_notice = Some(format!("Could not load saved password; enter it manually: {error}")); }
        else { self.credential_warning = Some(format!("Could not update protected saved password: {error}")); }
    }
    fn fail(&mut self, operation: Operation, error: String) {
        self.message = None;
        if operation == Operation::Sign {
            self.placement.certification_failed(); // Keep all boxes available for a Certify retry.
            // A prior creation receipt is historical, never this failed attempt's result.
            self.error = Some(format!("Signing failed: {error} If the ID file changed, reopen/import the digital ID and try again."));
        } else { self.error = Some(error); }
    }
    fn clear_passwords(&mut self) {
        self.password.clear();
        self.remember_password = false;
        if let Some(form) = &mut self.create { form.password.clear(); form.confirmation.clear(); }
        if let Some(form) = &mut self.import { form.password.clear(); }
    }
    fn accept(&mut self, serial: u64, operation: Operation, binding: Option<Binding>, current: Option<Binding>, outcome: JobOutcome) {
        if serial != self.serial || (binding.is_some() && binding != current) { return; }
        match (operation, outcome) {
            (Operation::Create, JobOutcome::IdentityCreated { path, info })
            | (Operation::Inspect, JobOutcome::IdentityInspected { path, info }) => {
                self.identity_path = Some(path);
                self.identity = Some(info);
                self.identity_restored = false;
                self.message = Some("Digital ID selected.".into());
            }
            (Operation::Sign, JobOutcome::PdfSigned { path, report }) => {
                if let Some(info) = report.signatures.first().and_then(|s| s.signer.as_ref()) {
                    if self.identity.as_ref().is_some_and(|selected| selected.sha256_fingerprint == info.sha256_fingerprint) {
                        self.identity = Some(info.clone()); self.identity_restored = false;
                    }
                }
                self.output = Some(path);
                self.output_binding = binding;
                self.message = Some("Signed copy saved; original unchanged.".into());
                self.output_report = Some(report);
                self.placement.clear();
            }
            (Operation::Verify, JobOutcome::PdfVerified { report }) => {
                if let Some(binding) = binding { self.verification = Some((binding, report)); }
                self.message = Some("Verification complete.".into());
            }
            _ => self.error = Some("The signing helper returned an unexpected operation result.".into()),
        }
    }
}

fn password_field(ui: &mut Ui, label: &str, value: &mut Secret, salt: &'static str) {
    ui.label(label);
    let id = Id::unique(salt);
    ui.add(TextEdit::singleline(value.edit()).password(true).id(id).char_limit(1024).desired_width(f32::INFINITY));
    // Password text must not be retained in egui's text-edit undo history.
    if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
        state.clear_undoer();
        state.store(ui.ctx(), id);
    }
}
fn reset_password_widgets(ctx: &egui::Context) {
    for salt in ["signature-password", "identity-create-password", "identity-confirm-password", "identity-import-password"] {
        let id = Id::unique(salt);
        if egui::text_edit::TextEditState::load(ctx, id).is_some() {
            egui::text_edit::TextEditState::default().store(ctx, id);
        }
    }
}

impl App {
    pub(super) fn signature_binding(&self) -> Option<Binding> {
        self.doc.as_ref().filter(|d| d.generation == self.generation)
            .map(|d| Binding { generation: d.generation, file: d.snapshot.file() })
    }
    pub(super) fn signature_preview_caption(&self) -> String {
        if !self.signature_drawing() && self.signatures.placement.appearance(self.signature_binding()).is_none() { return String::new(); }
        let signer = self.signatures.identity.as_ref().map_or("choose a digital ID", |i| i.subject.as_str());
        let mut caption = format!("Digitally certified (no changes)\nSigned by: {signer}\nDate (UTC): when signed");
        if !self.signatures.reason.is_empty() { caption.push_str(&format!("\nReason: {}", self.signatures.reason)); }
        caption
    }
    pub(super) fn signature_working(&self) -> bool { self.signatures.active.is_some() || self.signatures.credential.is_some() }
    pub(super) fn signature_drawing(&self) -> bool { self.signatures.placement.active() }
    pub(super) fn cancel_signature_placement(&mut self) {
        let lock = self.signatures.placement_lock.take();
        self.signatures.placement.cancel_current();
        self.unlock_signature_job(lock, Operation::Sign);
    }
    pub(super) fn clear_signature_placements(&mut self) {
        self.cancel_signature_placement(); self.signatures.placement.clear();
    }
    pub(super) fn has_unsealed_drafts(&self) -> bool { self.signatures.placement.count() != 0 }
    pub(super) fn request_draft_discard(&mut self, then: Discarding) {
        self.cancel_signature_placement(); self.clear_signature_forms(); self.signatures.discard_drafts = Some(then);
    }
    fn resolve_draft_discard(&mut self, discard: bool) {
        let Some(then) = self.signatures.discard_drafts.take() else { return; };
        if discard { self.clear_signature_placements(); self.unless_unsaved(then); }
    }
    fn certify_existing_drafts(&mut self) {
        if self.sign_block_reason().is_some() || self.signature_working() || !self.signatures.has_inspected_identity() { return; }
        let Some(binding) = self.signature_binding() else { return; };
        self.cancel_signature_placement();
        if self.signatures.placement.certify_existing(binding) { self.prepare_sign_dialog(); }
    }
    fn open_certified_copy(&mut self, source: Binding) {
        if self.signature_binding() != Some(source) || self.has_unsaved_work() || self.lifecycle.status() != Status::Idle { return; }
        let (Some(path), Some(receipt)) = (self.signatures.output.clone(), self.signatures.output_report.clone()) else { return; };
        self.clear_signature_placements(); // These blocks are already sealed; no draft-loss prompt.
        self.open(path.clone()); // Normal worker/lifecycle ownership; never PDFium on the UI thread.
        self.signatures.output = Some(path.clone()); self.signatures.output_report = Some(receipt);
        self.signatures.output_binding = None; // Creation receipt, NOT verification of the reopened bytes.
        self.signatures.message = Some("Certified copy saved; original unchanged. Opening the copy…".into());
        self.signatures.opening_copy = Some(OpeningCertifiedCopy { generation: self.generation, path });
    }
    fn opened_signature_copy_for_verification(&mut self) -> Option<(Binding, crate::document::Snapshot)> {
        let pending = self.signatures.opening_copy.take()?;
        if pending.generation != self.generation { return None; }
        if self.lifecycle.status() == Status::Opening {
            self.signatures.opening_copy = Some(pending); return None;
        }
        if self.lifecycle.status() == Status::Idle {
            if let Some(doc) = &self.doc {
                if doc.generation == pending.generation && doc.path == pending.path {
                    self.signatures.message = Some("Certified copy opened. Verifying its current bytes…".into());
                    return Some((Binding { generation: doc.generation, file: doc.snapshot.file() }, doc.snapshot.clone()));
                }
            }
        }
        self.signatures.error = Some("Certified copy was saved, but could not be opened. The creation receipt is retained below.".into());
        None
    }
    fn start_signature_placement(&mut self, final_box: bool) {
        if self.sign_block_reason().is_some() || self.signatures.active.is_some() || self.signatures.credential.is_some() { return; }
        let Some(binding) = self.signature_binding() else { return; };
        self.cancel_signature_placement();
        self.signatures.document_binding = Some(binding);
        self.signatures.placement.arm(binding, final_box);
        if !self.signature_drawing() { return; }
        self.signatures.placement_lock = Some(binding);
        if let Some(doc) = &mut self.doc { doc.session.block_editing(true); }
        self.signatures.message = Some("Drag a signature box on a page. Esc cancels.".into());
        if self.sheet_mode() { self.request_fit(ZoomMode::FitPage); }
    }
    pub(super) fn finish_signature_placement(&mut self) {
        if self.signatures.placement.take_completed() {
            let lock = self.signatures.placement_lock.take();
            self.unlock_signature_job(lock, Operation::Sign);
            self.signatures.message = Some("Unsealed draft added; original PDF unchanged.".into());
            if self.signatures.placement.final_pending() {
                if self.sign_block_reason().is_none() && self.signatures.has_inspected_identity() { self.prepare_sign_dialog(); }
                else { self.cancel_signature_placement(); }
            }
        }
    }
    pub(super) fn signature_signing(&self) -> bool {
        self.signatures.active.as_ref().is_some_and(|a| a.operation == Operation::Sign)
    }
    pub(super) fn signature_modal(&self) -> bool {
        self.signatures.create.is_some() || self.signatures.import.is_some() || self.signatures.confirm_sign.is_some()
            || self.signatures.identity_details || self.signatures.report_details.is_some() || self.signatures.discard_drafts.is_some()
    }
    pub(super) fn clear_signature_forms(&mut self) {
        if self.signatures.confirm_sign.is_some() { self.cancel_signature_placement(); }
        self.invalidate_credential_load();
        self.signatures.clear_passwords();
        self.signatures.create = None;
        self.signatures.import = None;
        self.signatures.confirm_sign = None;
        self.signatures.identity_details = false;
        self.signatures.report_details = None;
        reset_password_widgets(&self.ctx);
    }
    pub(super) fn record_signature_discovery(&mut self, generation: u64, file: u64, discovery: crate::domain::SignatureDiscovery) {
        self.signatures.discovery = Some((Binding { generation, file }, discovery));
    }
    pub(super) fn shutdown_signatures(&mut self) {
        self.cancel_signature_job();
        self.signatures.create = None;
        self.signatures.import = None;
        self.signatures.confirm_sign = None;
    }

    fn sign_block_reason(&self) -> Option<&'static str> {
        let Some(doc) = &self.doc else { return Some("Open a saved PDF to sign a copy."); };
        if self.compare.is_some() { return Some("Exit Compare before signing."); }
        if self.lifecycle.status() != Status::Idle || doc.generation != self.generation {
            return Some("Wait for the document to finish opening or saving.");
        }
        if doc.session.is_signature_read_only() {
            return Some("Read-only input — adding signatures is not supported.");
        }
        if self.popup.is_some() || self.text_editing.is_some() || self.quantity_edit.is_some()
            || self.layer_rename.is_some() || self.pins.renaming.is_some() || self.placing.is_some()
            || self.callout_placing.is_some() || self.drag.is_some() || self.insert_sheet.is_some() {
            return Some("Finish or cancel the active edit, then save.");
        }
        if self.has_unsaved_work() { return Some("Save first to include your edits."); }
        None
    }
    fn start_signature_job(&mut self, result: Result<client::Job, String>, operation: Operation, binding: Option<Binding>, remember: Option<remember::Pending>) {
        self.signatures.clear_passwords();
        reset_password_widgets(&self.ctx);
        self.signatures.error = None;
        self.signatures.message = None;
        match result {
            Ok(job) => {
                self.signatures.serial = self.signatures.serial.wrapping_add(1);
                self.signatures.active = Some(Active { job, operation, serial: self.signatures.serial, binding, remember });
                if operation == Operation::Verify { self.signatures.verification = None; }
                if operation == Operation::Sign {
                    if let Some(doc) = &mut self.doc { doc.session.block_editing(true); }
                    self.placing = None;
                    self.drag = None;
                    self.sheet_drag = None;
                }
                self.ctx.request_repaint();
            }
            Err(error) => self.signatures.fail(operation, error),
        }
    }
    fn unlock_signature_job(&mut self, binding: Option<Binding>, operation: Operation) {
        // A new saved revision can retain the same Session and our temporary
        // editing block. Release that block for this open generation only;
        // persistent signature read-only policy is independent and untouched.
        let same_open = binding.zip(self.signature_binding()).is_some_and(|(a, b)| a.generation == b.generation);
        if operation == Operation::Sign && same_open && self.lifecycle.status() == Status::Idle {
            if let Some(doc) = &mut self.doc { doc.session.block_editing(false); }
        }
    }
    pub(super) fn cancel_signature_job(&mut self) {
        self.cancel_signature_placement();
        if let Some(active) = self.signatures.active.take() {
            active.job.cancel();
            self.unlock_signature_job(active.binding, active.operation);
            self.signatures.serial = self.signatures.serial.wrapping_add(1);
            self.signatures.message = Some("Cancellation requested. The original PDF is unchanged; a completed output may already exist at the chosen path.".into());
        }
        self.signatures.clear_passwords();
        reset_password_widgets(&self.ctx);
    }
    pub(super) fn cancel_document_signatures(&mut self) {
        self.clear_signature_placements();
        if self.signatures.active.as_ref().is_some_and(|a| a.binding.is_some()) { self.cancel_signature_job(); }
        self.invalidate_credential_load();
        self.signatures.password.clear();
        self.signatures.placement.clear();
        self.signatures.confirm_sign = None;
        self.signatures.report_details = None;
        self.signatures.verification = None;
        self.signatures.output = None;
        self.signatures.output_binding = None;
        self.signatures.output_report = None;
        self.signatures.opening_copy = None;
        self.signatures.error = None;
        self.signatures.message = None;
        reset_password_widgets(&self.ctx);
    }
    fn invalidate_credential_load(&mut self) {
        if self.signatures.credential.as_ref().is_some_and(|task| task.loading) { self.signatures.credential = None; }
        self.signatures.credential_epoch = self.signatures.credential_epoch.wrapping_add(1);
        self.signatures.credential_notice = None;
    }
    fn start_credential(&mut self, key: remember::Key, work: remember::Work) {
        self.signatures.credential_warning = None;
        let loading = matches!(&work, remember::Work::Load);
        self.signatures.credential_epoch = self.signatures.credential_epoch.wrapping_add(1);
        match remember::Task::start(self.signatures.credential_epoch, key, work, self.ctx.clone()) {
            Ok(task) => self.signatures.credential = Some(task),
            Err(error) => self.signatures.credential_failure(loading, error),
        }
    }
    fn prepare_sign_dialog(&mut self) {
        self.signatures.password.clear(); self.signatures.remember_password = false;
        reset_password_widgets(&self.ctx);
        self.signatures.confirm_sign = self.signature_binding();
        self.signatures.document_binding = self.signature_binding();
        self.signatures.error = None;
        self.invalidate_credential_load();
        if let Some(key) = self.signatures.identity_key() { self.start_credential(key, remember::Work::Load); }
    }
    fn poll_signature_credentials(&mut self) {
        let Some(task) = self.signatures.credential.take() else { return; };
        let Some(result) = task.poll() else { self.signatures.credential = Some(task); self.ctx.request_repaint_after(Duration::from_millis(100)); return; };
        if task.loading {
            if !self.signatures.accepts_credential_load(task.epoch, &task.key, self.signature_binding()) { return; }
        } else if self.signatures.identity_key().as_ref() != Some(&task.key) { return; }
        match result {
            Ok(remember::Reply::Loaded(password)) => {
                if self.signatures.confirm_sign != self.signature_binding() || self.signatures.confirm_sign.is_none() { return; }
                if let Some(password) = password {
                    self.signatures.password = password;
                    self.signatures.credential_notice = Some("Saved password loaded. Use Forget saved password to remove it.".into());
                }
            }
            Ok(remember::Reply::Saved) => {
                if let Some(info) = &self.signatures.identity {
                    let metadata = prefs::RememberedIdentity { path: task.key.path.clone(), info: info.clone() };
                    if metadata.bounded() {
                        let mut prefs = prefs::Prefs::load(); prefs.remembered_identity = Some(metadata); prefs.save();
                        self.signatures.message = Some(format!("{} Password saved on this device.", self.signatures.message.as_deref().unwrap_or("Operation succeeded.")));
                    }
                }
            }
            Ok(remember::Reply::Forgotten) => {
                let mut prefs = prefs::Prefs::load();
                if prefs.remembered_identity.as_ref().is_some_and(|id| id.matches(&task.key.path, &task.key.fingerprint)) {
                    prefs.remembered_identity = None; prefs.save();
                }
                self.signatures.password.clear(); self.signatures.remember_password = false; reset_password_widgets(&self.ctx);
                self.signatures.message = Some("Saved password and remembered selection removed; PFX file unchanged.".into());
            }
            Err(error) => self.signatures.credential_failure(task.loading, error),
        }
    }
    pub(super) fn poll_signatures(&mut self) {
        self.poll_signature_credentials();
        let current = self.signature_binding();
        if self.signatures.document_binding != current {
            self.clear_signature_placements();
            self.signatures.document_binding = current;
            self.invalidate_credential_load();
            self.signatures.placement.clear();
            self.signatures.report_details = None;
            self.signatures.password.clear();
            self.signatures.confirm_sign = None;
            self.signatures.error = None;
            self.signatures.message = None;
            reset_password_widgets(&self.ctx);
        }
        if self.doc.as_ref().is_some_and(|doc| doc.arrange.edited()) { self.clear_signature_placements(); }
        if self.signatures.output_binding.is_some() && self.signatures.output_binding != current {
            self.signatures.output = None;
            self.signatures.output_binding = None;
            self.signatures.output_report = None;
            self.signatures.message = None;
        }
        if self.signatures.discovery.as_ref().is_some_and(|(b, _)| Some(*b) != current) {
            self.signatures.discovery = None;
        }
        if self.signatures.verification.as_ref().is_some_and(|(b, _)| Some(*b) != current) {
            self.signatures.verification = None;
        }
        if self.signatures.active.is_none() {
            if let Some((binding, snapshot)) = self.opened_signature_copy_for_verification() {
                let result = client::verify_pdf(snapshot, self.ctx.clone());
                self.start_signature_job(result, Operation::Verify, Some(binding), None);
            }
        }
        let Some(active) = self.signatures.active.take() else { return; };
        if active.binding.is_some() && (active.binding != current || (active.operation == Operation::Sign && self.has_unsaved_work())) {
            active.job.cancel();
            self.unlock_signature_job(active.binding, active.operation);
            self.signatures.clear_passwords();
            self.signatures.message = Some("Document changed; the previous signing/verification operation was cancelled.".into());
            return;
        }
        match active.job.poll() {
            None => {
                self.signatures.active = Some(active);
                self.ctx.request_repaint_after(Duration::from_millis(100));
            }
            Some(result) => {
                self.unlock_signature_job(active.binding, active.operation);
                self.signatures.clear_passwords();
                reset_password_widgets(&self.ctx);
                match result {
                    Ok(outcome) => {
                        let fresh = active.serial == self.signatures.serial && (active.binding.is_none() || active.binding == current);
                        let open_copy = self.signatures.can_open_signed_outcome(active.serial, active.operation, active.binding, current, &outcome);
                        let pending = active.remember.filter(|_| fresh);
                        let key = pending.as_ref().and_then(|p| p.authenticated_key(active.operation, &outcome));
                        self.signatures.accept(active.serial, active.operation, active.binding, current, outcome);
                        if let (Some(pending), Some(key)) = (pending, key) {
                            if self.signatures.identity.as_ref().is_some_and(|info| prefs::RememberedIdentity { path: key.path.clone(), info: info.clone() }.bounded()) {
                                self.start_credential(key, remember::Work::Save(pending.password));
                            } else { self.signatures.credential_warning = Some("Operation succeeded, but identity metadata cannot be remembered.".into()); }
                        }
                        if open_copy { if let Some(source) = active.binding { self.open_certified_copy(source); } }
                    }
                    Err(error) => self.signatures.fail(active.operation, error),
                }
            }
        }
    }
    pub(super) fn signatures_body(&mut self, ui: &mut Ui) {
        ui.heading("Signatures");
        let busy = self.signatures.active.is_some() || self.signatures.credential.is_some();
        ui.label("Digital ID");
        let selected = self.signatures.identity.as_ref().map_or_else(|| "Choose digital ID…".to_owned(), |info| info.subject.clone());
        let (mut create, mut import, mut forget, mut forget_password) = (false, false, false, false);
        ui.add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt("signature-digital-id").width(ui.available_width()).selected_text(&selected).show_ui(ui, |ui| {
                if self.signatures.has_inspected_identity() {
                    let _ = ui.selectable_label(true, &selected).on_hover_text("Currently inspected digital ID");
                    if ui.button("Details…").clicked() { self.signatures.identity_details = true; ui.close(); }
                    forget_password = ui.button("Forget saved password").clicked();
                    forget = ui.button("Clear selection").clicked();
                    ui.separator();
                }
                if ui.button("Create new…").clicked() { create = true; ui.close(); }
                if ui.button("Import .pfx…").clicked() { import = true; ui.close(); }
            });
        });
        if create {
            self.clear_signature_forms();
            self.signatures.create = Some(CreateForm::default());
            self.signatures.error = None;
        }
        if import {
            self.clear_signature_forms();
            if let Some(path) = rfd::FileDialog::new().add_filter("Digital ID", &["pfx", "p12"]).pick_file() {
                self.signatures.import = Some(ImportForm { path, password: Secret::default(), remember: false });
                self.signatures.error = None;
            }
        }
        if forget_password {
            self.clear_signature_forms();
            if let Some(key) = self.signatures.identity_key() { self.start_credential(key, remember::Work::Forget); }
        }
        if forget {
            self.clear_signature_forms();
            self.signatures.identity = None;
            self.signatures.identity_path = None;
        }
        ui.add_space(6.0);
        ui.add_enabled_ui(!busy, |ui| {
            ui.label("Description / reason");
            ui.add(TextEdit::singleline(&mut self.signatures.reason).char_limit(256).desired_width(f32::INFINITY));
        });
        let reason = self.sign_block_reason();
        let can_draw = !busy && reason.is_none();
        let count = self.signatures.placement.count();
        let (mut sign, mut sign_certify, mut certify) = (false, false, false);
        ui.horizontal(|ui| {
            sign = ui.add_enabled_ui(can_draw && count < 255, |ui| styled_button(ui, "Sign", Tone::Primary, false)).inner.clicked();
            sign_certify = ui.add_enabled_ui(can_draw && count < 256 && self.signatures.has_inspected_identity(),
                |ui| styled_button(ui, "Sign & Certify", Tone::Primary, false)).inner.clicked();
            certify = ui.add_enabled_ui(can_draw && self.signatures.has_inspected_identity(),
                |ui| styled_button(ui, "Certify", Tone::Primary, false)).inner.clicked();
        });
        if sign { self.start_signature_placement(false); }
        if sign_certify { self.start_signature_placement(true); }
        if certify { self.certify_existing_drafts(); }
        if self.signature_drawing() && ui.button("Cancel box (Esc)").clicked() { self.cancel_signature_placement(); }
        if let Some(notice) = &self.signatures.placement.notice { ui.small(notice); }
        if count > 0 {
            ui.small(format!("{count} unsealed draft block(s)"));
            ui.small("Drafts are not saved until certification; ordinary Save does not save them.");
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Undo last draft")).clicked() {
                    self.cancel_signature_placement(); self.signatures.placement.undo_last();
                }
                if ui.add_enabled(!busy, egui::Button::new("Clear drafts")).clicked() { self.clear_signature_placements(); }
            });
        }
        if let Some(reason) = reason { ui.label(reason); }
        if self.has_unsaved_work() && self.doc.as_ref().is_some_and(|d| !d.session.is_signature_read_only()) {
            if ui.add_enabled(!busy && self.lifecycle.status() == Status::Idle, egui::Button::new("Save first…")).clicked() { self.save(); }
        }
        ui.separator();
        if let Some(doc) = &self.doc {
            let binding = self.signature_binding();
            let discovery = self.signatures.discovery.filter(|(b, _)| Some(*b) == binding).map(|(_, d)| d);
            if let Some(crate::domain::SignatureDiscovery::Present { count }) = discovery {
                ui.label(format!("{count} signature(s) detected"));
            }
            if ui.add_enabled(!busy && self.lifecycle.status() == Status::Idle, egui::Button::new("Verify PDF signatures")).clicked() {
                let result = client::verify_pdf(doc.snapshot.clone(), self.ctx.clone());
                self.start_signature_job(result, Operation::Verify, binding, None);
            }
            if self.has_unsaved_work() { ui.small("Verification covers saved bytes only."); }
        }
        if let Some((_, report)) = &self.signatures.verification {
            ui.label(RichText::new("Current PDF result").strong());
            if verification_summary_ui(ui, report) { self.signatures.report_details = Some(("Current PDF verification".into(), report.clone())); }
        }
        if let Some(path) = &self.signatures.output {
            ui.label(RichText::new("Saved copy — verification at creation").strong());
            ui.label(path.display().to_string());
            if let Some(report) = &self.signatures.output_report {
                if verification_summary_ui(ui, report) { self.signatures.report_details = Some(("Saved copy verification at creation".into(), report.clone())); }
            }
            if ui.button("Open saved copy").clicked() {
                let path = path.clone(); self.unless_unsaved(Discarding::Open(path));
            }
        }
        if busy {
            ui.horizontal(|ui| { ui.spinner(); ui.label("Working…"); });
            if self.signatures.active.is_some() && ui.button("Cancel operation").clicked() { self.cancel_signature_job(); }
        }
        if let Some(message) = &self.signatures.message { ui.label(message); }
        if let Some(error) = &self.signatures.error { ui.colored_label(Color32::from_rgb(180, 45, 45), error); }
        if let Some(warning) = &self.signatures.credential_warning { ui.colored_label(Color32::from_rgb(180, 90, 30), warning); }
    }
    fn sign_copy_dialog(&mut self) {
        if self.signatures.password.expose().contains('\0') {
            self.cancel_signature_placement();
            self.signatures.password.clear();
            reset_password_widgets(&self.ctx);
            self.signatures.error = Some("Passwords cannot contain a NUL character.".into());
            return;
        }
        if self.signatures.active.is_some() || self.sign_block_reason().is_some() { self.cancel_signature_placement(); self.signatures.password.clear(); return; }
        let options = match self.signatures.selected_sign_options() {
            Ok(options) => options,
            Err(error) => {
                self.signatures.password.clear(); reset_password_widgets(&self.ctx);
                self.signatures.fail(Operation::Sign, error.into());
                return;
            }
        };
        let Some(pfx) = self.signatures.identity_path.clone() else { return; };
        let Some(doc) = &self.doc else { return; };
        let name = format!("{}-signed.pdf", doc.path.file_stem().unwrap_or_default().to_string_lossy());
        let destination = rfd::FileDialog::new().add_filter("Signed PDF copy", &["pdf"]).set_file_name(&name).save_file();
        let Some(destination) = destination else { self.cancel_signature_placement(); self.signatures.password.clear(); self.signatures.remember_password = false; reset_password_widgets(&self.ctx); return; };
        if destination == doc.path || destination.exists() {
            self.cancel_signature_placement();
            self.signatures.password.clear();
            reset_password_widgets(&self.ctx);
            self.signatures.error = Some("Choose a NEW output path. The original and existing files cannot be overwritten.".into());
            return;
        }
        let binding = self.signature_binding();
        let pending = remember::Pending::capture(self.signatures.remember_password, pfx.clone(),
            options.expected_identity_fingerprint.clone(), &self.signatures.password);
        let result = client::sign_copy(doc.snapshot.clone(), doc.path.clone(), destination, pfx,
            std::mem::take(&mut self.signatures.password),
            options, self.ctx.clone());
        self.start_signature_job(result, Operation::Sign, binding, pending);
    }
    pub(super) fn signature_dialogs(&mut self, ctx: &egui::Context) {
        if self.signatures.discard_drafts.is_some() {
            let (mut discard, mut cancel) = (false, false);
            let modal = egui::Modal::new(Id::unique("discard-unsealed-signatures")).show(ctx, |ui| {
                ui.set_width(380.0); ui.heading("Discard unsealed drafts?");
                ui.label("These signature drafts exist only in this window. Ordinary Save does not save them; only certification seals them into a new PDF copy.");
                ui.horizontal(|ui| { discard = ui.button("Discard drafts").clicked(); cancel = ui.button("Cancel").clicked(); });
            });
            if discard {
                self.resolve_draft_discard(true);
                if self.allow_close { ctx.send_viewport_cmd(ViewportCommand::Close); }
            } else if cancel || modal.should_close() { self.resolve_draft_discard(false); }
            return;
        }
        if self.signatures.identity_details {
            let mut close = false;
            let modal = egui::Modal::new(Id::unique("digital-id-details")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Digital ID details");
                if let Some(info) = &self.signatures.identity {
                    ui.label(&info.subject);
                    ui.label(format!("Issuer: {}", info.issuer));
                    ui.label(format!("{} · {} bits", info.algorithm, info.key_bits));
                    ui.label(format!("Valid: {} – {}", info.not_before, info.not_after));
                    ui.label(format!("SHA-256: {}", info.sha256_fingerprint));
                    if let Some(path) = &self.signatures.identity_path { ui.label(path.display().to_string()); }
                    if info.self_signed { ui.label("Self-created ID: not independently trusted by other viewers."); }
                    if self.signatures.identity_restored { ui.label("Remembered public metadata only; the helper rechecks the PFX, certificate and validity before signing."); }
                }
                ui.label("The .pfx contains a private key. Keep it and its password private, with a secure backup; share the signed PDF instead.");
                ui.label("No timestamp, PAdES, QES or long-term-validation claim is made.");
                close = ui.button("Close").clicked();
            });
            if close || modal.should_close() { self.signatures.identity_details = false; }
        }
        if let Some((title, report)) = self.signatures.report_details.take() {
            let mut close = false;
            let modal = egui::Modal::new(Id::unique("signature-result-details")).show(ctx, |ui| {
                ui.set_width(480.0);
                ui.heading(&title);
                verification_ui(ui, &report);
                close = ui.button("Close").clicked();
            });
            if !close && !modal.should_close() { self.signatures.report_details = Some((title, report)); }
        }
        if let Some(binding) = self.signatures.confirm_sign.take() {
            if Some(binding) != self.signature_binding() || self.sign_block_reason().is_some()
                || !self.signatures.has_inspected_identity() {
                self.cancel_signature_placement(); self.invalidate_credential_load();
                self.signatures.password.clear();
                reset_password_widgets(ctx);
                self.signatures.error = Some("Document or digital ID selection changed. Review/save the PDF and reopen/import the ID before signing.".into());
            } else {
                let mut sign = false;
                let mut cancel = false;
                let modal = egui::Modal::new(Id::unique("sign-saved-copy")).show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading("Certify PDF");
                    if self.signatures.placement.count() == 0 { ui.label("No visible signature block."); }
                    ui.label(format!("Seal {} visible block(s) and the whole PDF in one P1-certified copy. No subsequent changes permitted.", self.signatures.placement.count()));
                    ui.label("Unsigned history may be normalized in the copy; the original is unchanged.");
                    if self.signatures.identity.as_ref().is_some_and(|i| i.self_signed) {
                        ui.label("Self-created ID: independent identity trust is not established.");
                    }
                    password_field(ui, "PFX password", &mut self.signatures.password, "signature-password");
                    ui.checkbox(&mut self.signatures.remember_password, "Remember password on this device (Windows Credential Manager)")
                        .on_hover_text("Anyone using this Windows account can use the remembered password.");
                    if let Some(notice) = &self.signatures.credential_notice { ui.small(notice); }
                    let valid_password = !self.signatures.password.expose().is_empty() && !self.signatures.password.expose().contains('\0')
                        && !self.signatures.credential.as_ref().is_some_and(|task| task.loading);
                    if self.signatures.password.expose().contains('\0') { ui.label("Passwords cannot contain a NUL character."); }
                    ui.horizontal(|ui| {
                        sign = ui.add_enabled(valid_password, egui::Button::new("Choose new output and certify…")).clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
                if sign { self.sign_copy_dialog(); }
                else if cancel || modal.should_close() {
                    self.cancel_signature_placement();
                    self.invalidate_credential_load(); self.signatures.password.clear(); self.signatures.remember_password = false; reset_password_widgets(ctx);
                }
                else { self.signatures.confirm_sign = Some(binding); }
            }
        }
        if let Some(mut form) = self.signatures.create.take() {
            let mut create = false;
            let mut cancel = false;
            let modal = egui::Modal::new(Id::unique("create-digital-id")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Create Digital ID (.pfx)");
                ui.label("Creates a password-encrypted portable self-signed NON-CA identity. It is not independently trusted by other PDF viewers.");
                ui.label(RichText::new("The .pfx contains your PRIVATE KEY. Do not share it. Share the signed PDF instead; keep your password and a secure backup.").strong());
                ui.label("Name"); ui.add(TextEdit::singleline(&mut form.name).char_limit(128));
                ui.label("Organization (optional)"); ui.add(TextEdit::singleline(&mut form.organization).char_limit(128));
                ui.label("Email (optional)"); ui.add(TextEdit::singleline(&mut form.email).char_limit(254));
                ui.horizontal(|ui| { ui.label("Valid years"); ui.add(egui::DragValue::new(&mut form.years).range(1..=10)); });
                password_field(ui, "Password — minimum 8 characters; recommend 12+", &mut form.password, "identity-create-password");
                password_field(ui, "Confirm password", &mut form.confirmation, "identity-confirm-password");
                ui.checkbox(&mut form.remember, "Remember password on this device (Windows Credential Manager)")
                    .on_hover_text("Anyone using this Windows account can use the remembered password.");
                if let Err(reason) = form.validate() { ui.label(reason); }
                ui.horizontal(|ui| {
                    create = ui.add_enabled(form.validate().is_ok(), egui::Button::new("Create and save .pfx…")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if create {
                let destination = rfd::FileDialog::new().add_filter("Encrypted digital ID", &["pfx"]).set_file_name("Digital-ID.pfx").save_file();
                if let Some(destination) = destination {
                    let spec = CreateIdentitySpec { name: form.name.trim().into(), organization: form.organization.trim().into(),
                        email: form.email.trim().into(), valid_years: form.years };
                    let pending = remember::Pending::capture(form.remember, destination.clone(), None, &form.password);
                    let result = client::create_identity(spec, std::mem::take(&mut form.password), destination, ctx.clone());
                    self.start_signature_job(result, Operation::Create, None, pending);
                }
                form.password.clear(); form.confirmation.clear(); reset_password_widgets(ctx);
            } else if cancel || modal.should_close() { form.password.clear(); form.confirmation.clear(); reset_password_widgets(ctx); }
            else { self.signatures.create = Some(form); }
        }
        if let Some(mut form) = self.signatures.import.take() {
            let mut import = false;
            let mut cancel = false;
            let modal = egui::Modal::new(Id::unique("import-digital-id")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Import encrypted .pfx");
                ui.label(form.path.display().to_string());
                password_field(ui, "PFX password", &mut form.password, "identity-import-password");
                ui.checkbox(&mut form.remember, "Remember password on this device (Windows Credential Manager)")
                    .on_hover_text("Anyone using this Windows account can use the remembered password.");
                ui.horizontal(|ui| { import = ui.button("Inspect and select ID").clicked(); cancel = ui.button("Cancel").clicked(); });
            });
            if import && form.password.expose().contains('\0') {
                form.password.clear(); reset_password_widgets(ctx);
                self.signatures.error = Some("Passwords cannot contain a NUL character.".into());
            } else if import {
                let pending = remember::Pending::capture(form.remember, form.path.clone(), None, &form.password);
                let result = client::inspect_identity(form.path, std::mem::take(&mut form.password), ctx.clone());
                self.start_signature_job(result, Operation::Inspect, None, pending);
                reset_password_widgets(ctx);
            } else if cancel || modal.should_close() { form.password.clear(); reset_password_widgets(ctx); }
            else { self.signatures.import = Some(form); }
        }
    }
}

fn verification_summary_ui(ui: &mut Ui, report: &VerificationReport) -> bool {
    if report.signatures.is_empty() { ui.label("No supported signatures found; not an absence proof."); }
    for (index, check) in report.signatures.iter().enumerate() {
        if report.signatures.len() > 1 { ui.label(format!("Signature {}", index + 1)); }
        ui.label(if check.integrity_valid { "CMS integrity: passed" } else { "CMS integrity: FAILED or unsupported" });
        ui.label(match check.identity_trust {
            IdentityTrust::NotChecked => "Identity trust: NOT CHECKED",
            IdentityTrust::Untrusted => "Identity trust: untrusted",
            IdentityTrust::Trusted => "Identity trust: verifier reports trusted",
        });
        ui.label(if check.later_changes { "Later changes: present" } else { "Later changes: none reported" });
        if let Some(p) = check.certification { ui.label(format!("Declared policy: P{p}")); }
    }
    ui.button("Details…").clicked()
}

fn verification_ui(ui: &mut Ui, report: &VerificationReport) {
    ui.separator();
    ui.label(RichText::new("Verification results — not a trust endorsement").strong());
    if report.signatures.is_empty() { ui.label("No supported signatures found by the verifier; not an exhaustive absence proof."); }
    for (index, check) in report.signatures.iter().enumerate() {
        ui.group(|ui| {
            ui.label(format!("Signature {}", index + 1));
            ui.label(if check.integrity_valid { "CMS integrity: passed for the signed byte ranges" } else { "CMS integrity: FAILED or unsupported" });
            ui.label(match check.identity_trust {
                IdentityTrust::NotChecked => "Identity trust: NOT CHECKED",
                IdentityTrust::Untrusted => "Identity trust: untrusted",
                IdentityTrust::Trusted => "Identity trust: verifier reports trusted (not inferred from CMS)",
            });
            if check.signer.as_ref().is_some_and(|s| s.self_signed) { ui.label("Signer certificate is self-signed; independent identity trust is not established."); }
            ui.label(if check.later_changes { "Later changes: present — current PDF is not wholly covered by this signature" } else { "Later changes: none reported by this check" });
            if let Some(p) = check.certification { ui.label(format!("Declared certification policy: P{p} (not a complete allowed-change validation)")); }
            ui.label(format!("Signed revision end: {} / {} bytes", check.signed_revision_end, check.file_bytes));
            ui.label(&check.message);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::HeadlessOutput;
    pub(super) fn info() -> IdentityInfo {
        IdentityInfo { subject: "Synthetic UI test".into(), issuer: "Synthetic UI test".into(), sha256_fingerprint: "ab".repeat(32),
            not_before: "test start".into(), not_after: "test end".into(), algorithm: "RSA".into(), key_bits: 2048, self_signed: true }
    }
    #[test]
    fn automatic_open_uses_normal_worker_and_reverification_uses_only_new_snapshot() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        let source = app.signature_binding().unwrap(); let original = app.doc.as_ref().unwrap().path.clone();
        let path = std::env::temp_dir().join("synthetic-certified-output-not-read.pdf");
        let (tx, requests) = std::sync::mpsc::channel(); app.tx = tx;
        let (replies, rx) = std::sync::mpsc::channel(); app.rx = rx;
        app.signatures.serial = 1;
        let outcome = JobOutcome::PdfSigned { path: path.clone(), report: VerificationReport::default() };
        assert!(!app.signatures.can_open_signed_outcome(0, Operation::Sign, Some(source), Some(source), &outcome));
        assert!(!app.signatures.can_open_signed_outcome(1, Operation::Sign, Some(source), None, &outcome));
        assert!(!app.signatures.can_open_signed_outcome(1, Operation::Verify, Some(source), Some(source), &outcome));
        assert!(app.signatures.can_open_signed_outcome(1, Operation::Sign, Some(source), Some(source), &outcome));
        app.signatures.accept(1, Operation::Sign, Some(source), Some(source), outcome);
        app.open_certified_copy(source);
        let generation = app.generation;
        assert!(matches!(requests.try_recv().unwrap(), Request::Open { generation: g, path: p } if g == generation && p == path));
        assert_eq!(app.doc.as_ref().unwrap().path, original, "source is retained until worker opens the copy");
        assert!(app.signatures.output_report.is_some() && app.signatures.verification.is_none());
        assert!(app.signatures.output_binding.is_none() && !app.has_unsealed_drafts());
        let snapshot = crate::document::Snapshot::new(b"synthetic reopened bytes; never parsed or trusted by this UI test".to_vec());
        replies.send(Reply::Opened { generation, path: path.clone(), snapshot: snapshot.clone(), page_sizes: vec![[600.0, 800.0]], page_labels: vec![None] }).unwrap();
        replies.send(Reply::SignatureDiscovery { generation, file: snapshot.file(), discovery: crate::domain::SignatureDiscovery::Present { count: 1 } }).unwrap();
        app.drain_replies(&ctx);
        assert!(app.doc.as_ref().unwrap().session.is_signature_read_only());
        assert!(app.signatures.verification.is_none(), "creation receipt must never become current verification by path/length");
        let (binding, verify) = app.opened_signature_copy_for_verification().unwrap();
        assert_eq!(binding, app.signature_binding().unwrap()); assert_ne!(binding, source);
        assert_eq!(verify.bytes(), snapshot.bytes()); assert_eq!(app.doc.as_ref().unwrap().path, path);
        assert!(app.signatures.output_report.is_some() && app.signatures.verification.is_none());
    }
    #[test]
    fn automatic_open_never_discards_unrelated_edits_and_open_failure_keeps_receipt() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        let binding = app.signature_binding().unwrap(); app.signatures.serial = 1;
        app.signatures.accept(1, Operation::Sign, Some(binding), Some(binding), JobOutcome::PdfSigned { path: "synthetic-copy.pdf".into(), report: VerificationReport::default() });
        app.doc.as_mut().unwrap().arrange.select_all(); app.doc.as_mut().unwrap().arrange.rotate(1);
        app.open_certified_copy(binding); assert_eq!(app.generation, binding.generation);
        assert!(app.signatures.opening_copy.is_none() && app.has_unsaved_work());
        let (mut app, _) = super::super::tests::app_with_a_document();
        let binding = app.signature_binding().unwrap(); app.signatures.serial = 1;
        app.signatures.accept(1, Operation::Sign, Some(binding), Some(binding), JobOutcome::PdfSigned { path: "synthetic-copy.pdf".into(), report: VerificationReport::default() });
        app.open_certified_copy(binding); // Fixture has no worker receiver: deliberate open failure.
        assert!(app.opened_signature_copy_for_verification().is_none());
        assert!(app.signatures.output.is_some() && app.signatures.output_report.is_some() && app.signatures.error.is_some());
        assert!(app.signatures.verification.is_none());
    }
    #[test]
    fn cancelling_certify_existing_password_modal_keeps_all_drafts_and_clears_password() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        let binding = app.signature_binding().unwrap(); app.signatures.document_binding = Some(binding);
        app.signatures.identity = Some(info()); app.signatures.identity_path = Some("synthetic-not-opened.pfx".into());
        for final_box in [false, false] {
            app.signatures.placement.arm(binding, final_box);
            app.signatures.placement.finish_box(binding, crate::signing::SignatureAppearance { page: 0, rect: [0.0, 0.0, 200.0, 80.0], rotation: 0 });
        }
        assert!(app.signatures.placement.certify_existing(binding));
        app.signatures.confirm_sign = Some(binding); app.signatures.password = Secret::new("Synthetic cancel-only password".into());
        app.signatures.remember_password = true;
        let _output = HeadlessOutput(ctx.run_ui(egui::RawInput::default(), |ui| app.signature_dialogs(ui.ctx())));
        let raw = egui::RawInput { events: vec![egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], ..Default::default() };
        let _output = HeadlessOutput(ctx.run_ui(raw, |ui| app.signature_dialogs(ui.ctx())));
        assert_eq!(app.signatures.placement.count(), 2); assert!(!app.signatures.placement.final_pending());
        assert!(app.signatures.confirm_sign.is_none() && app.signatures.password.expose().is_empty() && !app.signatures.remember_password);
        assert!(app.signatures.active.is_none() && app.signatures.credential.is_none());
        assert!(!app.has_unsaved_work());
    }
    #[test]
    fn unsealed_drafts_guard_open_new_quit_without_becoming_pdf_dirty_and_chain_the_normal_guard() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        let binding = app.signature_binding().unwrap(); app.signatures.document_binding = Some(binding);
        for page in [0, 2] {
            app.signatures.placement.arm(binding, false);
            app.signatures.placement.finish_box(binding, crate::signing::SignatureAppearance { page, rect: [0.0, 0.0, 200.0, 80.0], rotation: 0 });
        }
        assert!(app.has_unsealed_drafts() && !app.has_unsaved_work());
        assert!(app.sign_block_reason().is_none(), "drafts must never deadlock certification's clean-snapshot guard");
        app.new_pdf(); assert!(app.new_document.is_some() && app.signatures.placement.count() == 2);
        app.new_document = None;
        app.open("synthetic-not-opened.pdf".into());
        assert_eq!(app.signature_binding(), Some(binding));
        assert!(app.signatures.discard_drafts.is_some());
        app.resolve_draft_discard(false); assert_eq!(app.signatures.placement.count(), 2);
        app.unless_unsaved(Discarding::Close); assert!(!app.allow_close && app.signatures.discard_drafts.is_some());
        app.resolve_draft_discard(false); assert_eq!(app.signatures.placement.count(), 2);
        app.doc.as_mut().unwrap().arrange.select_all(); app.doc.as_mut().unwrap().arrange.rotate(1);
        app.unless_unsaved(Discarding::Close); app.resolve_draft_discard(true);
        assert!(!app.has_unsealed_drafts() && app.has_unsaved_work());
        assert!(app.discarding.is_some() && !app.allow_close, "ordinary unsaved edits still require their own consent");
    }
    #[test]
    fn protected_password_write_failure_keeps_actual_authenticated_identity_and_signed_output() {
        let mut state = State::default();
        state.identity = Some(info()); state.identity_path = Some("created.pfx".into());
        state.output = Some("signed-copy.pdf".into()); state.output_report = Some(VerificationReport::default());
        state.message = Some("Signed copy saved; original unchanged.".into());
        state.credential_failure(false, "Synthetic protected-storage failure".into());
        assert_eq!(state.output.as_deref(), Some(std::path::Path::new("signed-copy.pdf")));
        assert!(state.output_report.is_some() && state.identity.is_some());
        assert_eq!(state.identity_path.as_deref(), Some(std::path::Path::new("created.pfx")));
        assert_eq!(state.message.as_deref(), Some("Signed copy saved; original unchanged."));
        assert!(state.credential_warning.as_ref().unwrap().contains("saved password"));
    }
    #[test]
    fn restored_public_selection_never_loads_password_or_starts_crypto_and_loads_reject_stale_context() {
        let path = std::env::temp_dir().join("nonexistent-synthetic-ui-identity.pfx");
        let metadata = prefs::RememberedIdentity { path, info: info() };
        let mut state = State::restore_public(Some(metadata));
        assert!(state.identity_restored && state.active.is_none() && state.credential.is_none());
        assert!(state.password.expose().is_empty() && !state.remember_password);
        let binding = Binding { generation: 5, file: 9 };
        state.confirm_sign = Some(binding); state.credential_epoch = 7;
        let key = state.identity_key().unwrap();
        assert!(state.accepts_credential_load(7, &key, Some(binding)));
        assert!(!state.accepts_credential_load(6, &key, Some(binding)));
        assert!(!state.accepts_credential_load(7, &key, Some(Binding { generation: 5, file: 10 })));
        let wrong = remember::Key { path: key.path.clone(), fingerprint: "cd".repeat(32) };
        assert!(!state.accepts_credential_load(7, &wrong, Some(binding)));
        state.confirm_sign = None;
        assert!(!state.accepts_credential_load(7, &key, Some(binding)));
    }
    #[test]
    fn actual_page_drag_places_only_a_preview_and_does_not_reach_the_drawing_tool() {
        let (mut app, ctx) = super::super::tests::app_with_a_document();
        app.doc.as_mut().unwrap().geometry = vec![Some(PageGeometry { rotation: 0, bounds: crate::domain::PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } }); 3];
        app.tool = Some(MarkupKind::Rectangle);
        app.signatures.identity = Some(info()); app.signatures.identity_path = Some(std::env::temp_dir().join("synthetic-draft-only-id.pfx"));
        app.show_tool_panel(tool_panel::Tab::Signatures);
        app.start_signature_placement(false);
        assert!(app.signature_drawing() && !app.doc.as_ref().unwrap().session.can_edit());
        let binding = app.signature_binding();
        let mut frame = eframe::Frame::_new_kittest();
        let mut draw = |app: &mut App, events| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 900.0))), events, ..Default::default() };
            let mut output = HeadlessOutput(ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut frame)));
            output.textures_delta.clear(); // Headless tests have no texture consumer.
        };
        draw(&mut app, vec![]);
        let rect = app.page_rects[&0];
        let start = pos2(rect.left() + rect.width() * 0.2, rect.top() + rect.height() * 0.15);
        let end = pos2(rect.left() + rect.width() * 0.7, rect.top() + rect.height() * 0.35);
        draw(&mut app, vec![egui::Event::PointerMoved(start), egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        draw(&mut app, vec![egui::Event::PointerMoved(end)]);
        draw(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        let appearance = app.signatures.placement.appearance(binding).expect("real drag must produce a box");
        assert_eq!((appearance.page, appearance.rotation), (0, 0));
        assert!(!app.signature_drawing() && app.drag.is_none() && app.placing.is_none());
        assert!(app.signatures.confirm_sign.is_none() && app.signatures.active.is_none() && app.signatures.credential.is_none(),
            "Sign drafts never open a password dialog, access the vault or start cryptography, even with an ID selected");
        let doc = app.doc.as_ref().unwrap();
        assert!(doc.session.can_edit() && !doc.session.is_dirty() && !doc.session.can_undo() && !doc.arrange.edited());
        assert!(doc.session.markups().is_empty());
        assert_eq!(binding, app.signature_binding());
        app.signatures.identity = Some(info()); app.signatures.identity_path = Some("synthetic.pfx".into());
        assert!(app.signatures.selected_sign_options().is_err(), "draft Sign must not be a cryptographic approval");
        app.signatures.placement.arm(binding.unwrap(), true);
        app.signatures.placement.finish_box(binding.unwrap(), crate::signing::SignatureAppearance { page: 1, ..appearance.clone() });
        let selected = app.signatures.selected_sign_options().unwrap();
        assert!(selected.appearance.is_none()); assert_eq!(selected.appearances.len(), 2);
        assert_eq!(selected.appearances[0].rect, appearance.rect); assert_eq!(selected.appearances[1].page, 1);
        assert!(selected.mode == SignatureMode::CertifyNoChanges);
        assert_eq!(selected.expected_identity_fingerprint, Some(info().sha256_fingerprint));
        app.cancel_signature_placement(); assert_eq!(app.signatures.placement.count(), 1);
        app.clear_signature_placements();
        assert!(app.signatures.placement.appearance(binding).is_none() && app.doc.as_ref().unwrap().session.can_edit());
    }
    #[test]
    fn create_requires_name_password_confirmation_and_bounded_validity() {
        let mut f = CreateForm::default();
        assert!(f.validate().is_err());
        f.name = "Synthetic test".into(); f.password = Secret::new("long-password".into());
        assert!(f.validate().is_err());
        f.confirmation = Secret::new("different".into()); assert!(f.validate().is_err());
        f.confirmation = Secret::new("long-password".into()); assert!(f.validate().is_ok());
        f.years = 0; assert!(f.validate().is_err()); f.years = 11; assert!(f.validate().is_err());
        f.years = 10; assert!(f.validate().is_ok());
        f.password = Secret::new("12345678".into()); f.confirmation = Secret::new("12345678".into()); assert!(f.validate().is_ok());
        f.password = Secret::new("short".into()); f.confirmation = Secret::new("short".into()); assert!(f.validate().is_err());
        f.password = Secret::new("long\0password".into()); f.confirmation = Secret::new("long\0password".into()); assert!(f.validate().is_err());
    }
    #[test]
    fn sign_requires_inspected_metadata_and_binds_its_exact_certificate_fingerprint() {
        let mut state = State::default();
        assert!(state.selected_sign_options().is_err());
        state.identity_path = Some("selected.pfx".into());
        assert!(!state.has_inspected_identity(), "a path alone is not an inspected ID");
        assert!(state.selected_sign_options().is_err());
        let mut identity = info();
        identity.sha256_fingerprint = "ab".repeat(32);
        let expected = identity.sha256_fingerprint.clone();
        state.identity = Some(identity);
        let binding = Binding { generation: 1, file: 2 }; state.document_binding = Some(binding);
        state.placement.arm(binding, true);
        state.placement.finish_box(binding, crate::signing::SignatureAppearance { page: 0, rect: [0.0, 0.0, 200.0, 80.0], rotation: 0 });
        state.reason = "Synthetic approval policy".into();
        let options = state.selected_sign_options().unwrap();
        assert_eq!(options.expected_identity_fingerprint, Some(expected));
        assert_eq!(options.mode, SignatureMode::CertifyNoChanges);
        state.identity.as_mut().unwrap().sha256_fingerprint.clear();
        assert!(!state.has_inspected_identity());
        assert!(state.selected_sign_options().is_err());
    }
    #[test]
    fn changed_identity_failure_keeps_historical_receipt_but_never_claims_new_success() {
        let mut state = State::default();
        state.output = Some("old-copy.pdf".into());
        state.output_report = Some(VerificationReport::default());
        state.message = Some("Previous copy completed".into());
        state.fail(Operation::Sign, "Selected certificate fingerprint changed.".into());
        assert!(state.output.is_some() && state.output_report.is_some() && state.message.is_none());
        assert!(state.error.as_ref().unwrap().contains("reopen/import"));
    }
    #[test]
    fn operation_results_are_correlated_and_secrets_can_be_cleared() {
        let mut state = State::default(); state.serial = 3;
        state.password = Secret::new("never persist".into()); state.clear_passwords(); assert!(state.password.expose().is_empty());
        let binding = Binding { generation: 2, file: 7 };
        state.accept(2, Operation::Create, None, None, JobOutcome::IdentityCreated { path: "stale.pfx".into(), info: info() });
        assert!(state.identity.is_none());
        state.accept(3, Operation::Create, None, None, JobOutcome::IdentityCreated { path: "synthetic.pfx".into(), info: info() });
        assert!(state.identity.is_some());
        state.accept(3, Operation::Verify, Some(binding), Some(Binding { generation: 3, file: 7 }),
            JobOutcome::PdfVerified { report: VerificationReport::default() });
        assert!(state.verification.is_none());
        state.accept(3, Operation::Verify, Some(binding), Some(binding), JobOutcome::PdfVerified { report: VerificationReport::default() });
        assert!(state.verification.is_some());
    }
    #[test]
    fn all_password_forms_clear_and_signed_copy_result_is_not_original_verification() {
        let mut state = State::default();
        state.serial = 1;
        state.password = Secret::new("sign password".into());
        let mut form = CreateForm::default();
        form.password = Secret::new("create password".into());
        form.confirmation = Secret::new("create password".into());
        state.create = Some(form);
        state.import = Some(ImportForm { path: "synthetic.pfx".into(), password: Secret::new("import password".into()), remember: false });
        state.clear_passwords();
        assert!(state.password.expose().is_empty());
        assert!(state.create.as_ref().unwrap().password.expose().is_empty());
        assert!(state.create.as_ref().unwrap().confirmation.expose().is_empty());
        assert!(state.import.as_ref().unwrap().password.expose().is_empty());
        let binding = Binding { generation: 2, file: 7 };
        state.accept(1, Operation::Sign, Some(binding), Some(Binding { generation: 2, file: 8 }),
            JobOutcome::PdfSigned { path: "stale-copy.pdf".into(), report: VerificationReport::default() });
        assert!(state.output.is_none());
        state.accept(1, Operation::Sign, Some(binding), Some(binding),
            JobOutcome::PdfSigned { path: "signed-copy.pdf".into(), report: VerificationReport::default() });
        assert!(state.output.is_some() && state.output_report.is_some());
        assert!(state.verification.is_none(), "output verification must not be shown as the original PDF's result");
    }
    #[test]
    fn document_transition_clears_reports_and_sign_password_but_keeps_selected_id() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        app.signatures.identity_path = Some("synthetic.pfx".into());
        app.signatures.identity = Some(info());
        app.signatures.password = Secret::new("do not retain".into());
        let binding = app.signature_binding().unwrap();
        app.signatures.verification = Some((binding, VerificationReport::default()));
        app.cancel_document_signatures();
        assert!(app.signatures.password.expose().is_empty());
        assert!(app.signatures.verification.is_none());
        assert!(app.signatures.discovery.is_some(), "cancelling a New/Close prompt must not discard the current discovery status");
        assert!(app.signatures.identity.is_some());
        assert!(!app.doc.as_ref().unwrap().session.is_signature_read_only());
    }
    #[test]
    fn a_new_revision_discards_output_reports_confirmation_and_passwords() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        let binding = app.signature_binding().unwrap();
        app.signatures.serial = 1;
        app.signatures.document_binding = Some(binding);
        app.signatures.accept(1, Operation::Sign, Some(binding), Some(binding),
            JobOutcome::PdfSigned { path: "previous-copy.pdf".into(), report: VerificationReport::default() });
        app.signatures.password = Secret::new("must clear on revision".into());
        app.signatures.confirm_sign = Some(binding);
        app.doc.as_mut().unwrap().snapshot = crate::document::Snapshot::new(b"new synthetic revision".to_vec());
        app.poll_signatures();
        assert!(app.signatures.output.is_none() && app.signatures.output_report.is_none());
        assert!(app.signatures.confirm_sign.is_none() && app.signatures.password.expose().is_empty());
        assert!(app.signatures.discovery.is_none());
    }
    #[test]
    fn signatures_action_and_panel_work_without_a_document() {
        let (mut app, ctx) = super::super::tests::app_with_a_document(); app.doc = None;
        assert!(app.action_enabled(palette::Action::Signatures));
        app.run_action(palette::Action::Signatures);
        assert!(app.tool_tab == tool_panel::Tab::Signatures && app.tool_panel_open);
        let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), ..Default::default() };
        let mut output = HeadlessOutput(ctx.run_ui(input(), |ui| { app.tool_panel(ui); }));
        output.textures_delta.clear(); // Also prevent a double panic from hiding an assertion failure.
        let labels: Vec<_> = output.shapes.iter().filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.text().to_owned()), _ => None,
        }).collect();
        assert!(!labels.iter().any(|s| s.contains("PRIVATE KEY") || s.contains("PFX password") || s.contains("PAdES")),
            "technical warnings and passwords belong in dialogs, not the bare main pane");
        for label in ["Choose digital ID…", "Create new…"] {
            for _ in 0..4 {
                if output.shapes.iter().any(|s| matches!(&s.shape, Shape::Text(t) if t.galley.text() == label)) { break; }
                output = HeadlessOutput(ctx.run_ui(input(), |ui| { app.tool_panel(ui); })); output.textures_delta.clear();
            }
            let target = output.shapes.iter().find_map(|s| match &s.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() * 0.5),
                _ => None,
            }).unwrap_or_else(|| panic!("Digital ID dropdown action not visible: {label}"));
            output.textures_delta.clear();
            for pressed in [true, false] {
                let mut raw = input();
                raw.events = vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                    pos: target, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
                }];
                output = HeadlessOutput(ctx.run_ui(raw, |ui| { app.tool_panel(ui); }));
                output.textures_delta.clear();
            }
            // Popup contents get a sizing pass before they become paintable.
            output = HeadlessOutput(ctx.run_ui(input(), |ui| { app.tool_panel(ui); }));
            output.textures_delta.clear();
        }
        assert!(app.signatures.create.is_some(), "the no-PDF dropdown's Create action must actually open its form");
        assert!(app.signatures.active.is_none(), "opening a form must not initialize crypto");
    }
    #[test]
    fn leaving_signature_panel_clears_secret_forms_without_forgetting_public_identity() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        app.show_tool_panel(tool_panel::Tab::Signatures);
        app.signatures.password = Secret::new("synthetic password".into());
        app.signatures.create = Some(CreateForm::default());
        app.signatures.identity = Some(info());
        app.show_tool_panel(tool_panel::Tab::Details);
        assert!(app.signatures.password.expose().is_empty());
        assert!(app.signatures.create.is_none());
        assert!(app.signatures.identity.is_some());
        app.show_tool_panel(tool_panel::Tab::Signatures);
        app.signatures.password = Secret::new("synthetic password".into());
        app.toggle_tool_panel(tool_panel::Tab::Signatures);
        assert!(!app.tool_panel_open && app.signatures.password.expose().is_empty());
    }
    #[test]
    fn verification_text_separates_integrity_unchecked_trust_and_later_changes() {
        let ctx = egui::Context::default();
        let report = VerificationReport { signatures: vec![crate::signing::SignatureCheck {
            signer: Some(info()), integrity_valid: true, identity_trust: IdentityTrust::NotChecked,
            signed_revision_end: 100, file_bytes: 120, later_changes: true, certification: Some(1), message: "Synthetic check".into(),
        }] };
        let mut output = HeadlessOutput(ctx.run_ui(egui::RawInput::default(), |ui| { verification_summary_ui(ui, &report); }));
        let labels: Vec<_> = output.shapes.iter().filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.text().to_owned()), _ => None,
        }).collect();
        assert!(labels.iter().any(|s| s.contains("CMS integrity: passed")));
        assert!(labels.iter().any(|s| s.contains("Identity trust: NOT CHECKED")));
        assert!(labels.iter().any(|s| s.contains("Later changes: present")));
        assert!(labels.iter().any(|s| s.contains("Declared policy: P1")));
        assert!(!labels.iter().any(|s| s.contains("whole-document valid")));
        output.textures_delta.clear();
    }
    #[test]
    fn sign_requires_saved_clean_unsigned_input_and_never_clears_read_only_policy() {
        let (mut app, _) = super::super::tests::app_with_a_document();
        assert!(app.sign_block_reason().is_none());
        app.layer_rename = Some((app.doc.as_ref().unwrap().session.active_layer(), "Synthetic draft".into()));
        assert!(app.sign_block_reason().unwrap().starts_with("Finish or cancel"));
        app.layer_rename = None;
        app.doc.as_mut().unwrap().session.set_signature_discovery(crate::domain::SignatureDiscovery::Unknown);
        assert!(app.sign_block_reason().is_some());
        app.unlock_signature_job(app.signature_binding(), Operation::Sign);
        assert!(app.doc.as_ref().unwrap().session.is_signature_read_only());
        app.doc.as_mut().unwrap().session.set_signature_discovery(crate::domain::SignatureDiscovery::Present { count: 1 });
        assert!(app.sign_block_reason().is_some());
        app.doc.as_mut().unwrap().session.set_signature_discovery(crate::domain::SignatureDiscovery::NoSignaturesDetected);
        app.doc.as_mut().unwrap().arrange.select_all(); app.doc.as_mut().unwrap().arrange.rotate(1);
        assert!(app.has_unsaved_work()); assert!(app.sign_block_reason().unwrap().starts_with("Save first"));
    }
}
