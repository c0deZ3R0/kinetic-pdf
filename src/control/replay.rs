//! Bounded, per-window memory of successful writes. This is not durable deduplication.
use super::{Command, Error, ErrorCode, Request, Response};
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct ReplayCache(VecDeque<Entry>);
struct Entry {
    key: String,
    request: Vec<u8>,
    response: Response,
}

impl ReplayCache {
    pub(crate) fn lookup(&self, key: &str, request: &[u8]) -> Result<Option<Response>, Error> {
        let Some(entry) = self.0.iter().find(|entry| entry.key == key) else {
            return Ok(None);
        };
        if entry.request != request {
            return Err(Error::new(
                ErrorCode::InvalidParameters,
                "Replay key was used for a different request; use a new key",
            ));
        }
        Ok(Some(entry.response.clone()))
    }
    pub(crate) fn remember(&mut self, key: String, request: Vec<u8>, response: Response) {
        if self.0.len() == 64 {
            self.0.pop_front();
        }
        self.0.push_back(Entry {
            key,
            request,
            response,
        });
    }
}

pub(crate) fn fingerprint(key: &str, request: &Request) -> Result<Vec<u8>, Error> {
    let invalid = |s| Error::new(ErrorCode::InvalidParameters, s);
    if key.is_empty() || key.len() > 128 {
        return Err(invalid("Replay key must contain 1–128 bytes"));
    }
    if !matches!(
        request.command,
        Command::Save
            | Command::SaveAs { .. }
            | Command::ConfigureTool { .. }
            | Command::CreateLayer { .. }
            | Command::RenameLayer { .. }
            | Command::SetPageLabel { .. }
            | Command::InsertBlankPage { .. }
            | Command::AddHighlight { .. }
            | Command::EditNote { .. }
            | Command::Experiment {
                request: crate::experiment::Request::Commit { .. }
            }
    ) {
        return Err(invalid("Replay wrapper supports explicit writes only; nested wrappers and presentation actions are unsupported"));
    }
    let bytes = serde_json::to_vec(request)
        .map_err(|e| Error::new(ErrorCode::InvalidParameters, e.to_string()))?;
    if bytes.len() > 256 * 1024 {
        return Err(invalid("Replay request exceeds 256 KiB"));
    }
    Ok(bytes)
}
