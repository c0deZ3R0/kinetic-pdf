//! Explicit, asynchronous Windows credential operations for one inspected ID.
//!
//! PATH: src/app/signatures/remember.rs
//! CONSUMED BY: Signatures password dialogs and authenticated job completion.
//! No startup enumeration, preferences or logging. Secrets have no Debug/Clone;
//! a dropped load receiver also drops its returned owned Secret without using it.

use super::*;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use crate::signing::credentials;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Key { pub path: PathBuf, pub fingerprint: String }

pub(super) enum Work { Load, Save(Secret), Forget }
pub(super) enum Reply { Loaded(Option<Secret>), Saved, Forgotten }

pub(super) struct Task {
    pub epoch: u64,
    pub key: Key,
    pub loading: bool,
    result: Receiver<Result<Reply, String>>,
}
impl Task {
    pub fn start(epoch: u64, key: Key, work: Work, ctx: egui::Context) -> Result<Self, String> {
        let loading = matches!(&work, Work::Load);
        let (send, result) = mpsc::channel();
        let selected = key.clone();
        std::thread::Builder::new().name("signature-credential".into()).spawn(move || {
            let result = match work {
                Work::Load => credentials::load(&selected.path, &selected.fingerprint).map(Reply::Loaded),
                Work::Save(password) => credentials::save(&selected.path, &selected.fingerprint, &password).map(|_| Reply::Saved),
                Work::Forget => credentials::forget(&selected.path, &selected.fingerprint).map(|_| Reply::Forgotten),
            };
            let _ = send.send(result);
            ctx.request_repaint();
        }).map_err(|e| format!("Could not start protected credential operation: {e}"))?;
        Ok(Self { epoch, key, loading, result })
    }
    pub fn poll(&self) -> Option<Result<Reply, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Protected credential operation stopped without a result.".into())),
        }
    }
}

/// Exists only when the user explicitly checks Remember for an authentication
/// operation. Drop clears its owned password on failure/cancel/stale completion.
pub(super) struct Pending {
    pub path: PathBuf,
    pub fingerprint: Option<String>,
    pub password: Secret,
}
impl Pending {
    pub fn capture(remember: bool, path: PathBuf, fingerprint: Option<String>, password: &Secret) -> Option<Self> {
        remember.then(|| Self { path, fingerprint, password: Secret::new(password.expose().to_owned()) })
    }
    pub fn authenticated_key(&self, operation: Operation, outcome: &JobOutcome) -> Option<Key> {
        let fingerprint = match (operation, outcome) {
            (Operation::Create, JobOutcome::IdentityCreated { path, info })
            | (Operation::Inspect, JobOutcome::IdentityInspected { path, info }) if *path == self.path => info.sha256_fingerprint.clone(),
            (Operation::Sign, JobOutcome::PdfSigned { report, .. }) => {
                report.signatures.first()?.signer.as_ref()?.sha256_fingerprint.clone()
            }
            _ => return None,
        };
        if fingerprint.is_empty() || self.fingerprint.as_ref().is_some_and(|expected| *expected != fingerprint) { return None; }
        Some(Key { path: self.path.clone(), fingerprint })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remember_is_opt_in_and_requires_actual_matching_authentication() {
        let password = Secret::new("Synthetic credential only".into());
        assert!(Pending::capture(false, "id.pfx".into(), None, &password).is_none());
        let pending = Pending::capture(true, "id.pfx".into(), Some("expected".into()), &password).unwrap();
        let wrong = JobOutcome::IdentityInspected { path: "other.pfx".into(), info: super::super::tests::info() };
        assert!(pending.authenticated_key(Operation::Inspect, &wrong).is_none());
        assert!(pending.authenticated_key(Operation::Sign, &JobOutcome::PdfSigned { path: "copy.pdf".into(), report: VerificationReport::default() }).is_none());
    }
}
