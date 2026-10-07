//! Transport-independent requests to the live application.
//! The app remains the sole owner of document and presentation state.

mod queue;
pub use queue::{channel, Client, Inbox, Ticket};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DocumentTarget {
    pub instance: String,
    pub generation: u64,
    pub revision: DocumentRevision,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DocumentRevision {
    pub edits: u64,
    pub arrangement: u64,
    pub file: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Inspect,
    /// Page positions are one-based displayed sheets, not PDF file indices.
    GoToPage { page: u32 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub command: Command,
    pub target: Option<DocumentTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct State {
    pub instance: String,
    pub document: Option<DocumentTarget>,
    pub path: Option<String>,
    pub pages: usize,
    pub current_page: usize,
    pub zoom: f32,
    /// egui logical pixels, from the top-left of the scrolling canvas.
    pub scroll: [f32; 2],
    pub viewport: [f32; 2],
    pub dirty: bool,
    pub busy: bool,
    pub view_ready: bool,
    pub palette_open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode { Unavailable, Busy, StaleDocument, InvalidParameters, Cancelled, Timeout, QueueFull, Failed }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Error { pub code: ErrorCode, pub message: String }

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}

pub type Result = std::result::Result<State, Error>;
