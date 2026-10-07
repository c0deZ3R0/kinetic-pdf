//! Transport-independent requests to the live application.
//! The app remains the sole owner of document and presentation state.

mod queue;
pub use queue::{channel, Client, Inbox, Ticket};
use serde::{Deserialize, Serialize};
#[cfg(feature = "mcp")]
use rmcp::schemars;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
pub struct DocumentTarget {
    pub instance: String,
    pub generation: u64,
    pub revision: DocumentRevision,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
pub struct DocumentRevision {
    pub edits: u64,
    pub arrangement: u64,
    pub file: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Inspect,
    /// Page positions are one-based displayed sheets, not PDF file indices.
    GoToPage { page: u32 },
    ListPages,
    Open { path: String },
    Save,
    SaveAs { path: String, #[serde(default)] overwrite: bool },
    PollOperation { id: u64 },
    ReadText { page: u32 },
    ListAnnotations,
    ListLayers,
    CreateLayer { name: String },
    RenameLayer { id: String, name: String },
    SelectLayer { id: String },
    SetPageLabel { page: u32, label: Option<String> },
    /// Insert at a one-based position; pages+1 appends. Size is PDF points.
    InsertBlankPage { at: u32, size: [f32; 2] },
    ListTools,
    ToolSchema,
    /// Known settings can be patched; omitted settings use this tool's defaults.
    ConfigureTool { name: String, #[serde(default)] group: String, kind: String, settings: Option<serde_json::Value>, #[serde(default)] replace: bool },
    SelectTool { kind: String },
    SelectSavedTool { name: String, #[serde(default)] group: String },
    SetZoom { zoom: f32 },
    /// Pan deltas are egui logical pixels, independent of display DPI.
    Pan { delta: [f32; 2] },
    /// Coordinates are PDF points in unrotated user space, origin bottom-left.
    CentreOn { page: u32, point: [f32; 2] },
    ZoomToRegion { page: u32, region: PdfRect },
    Find { query: String },
    SearchResults,
    GoToResult { result: u32 },
    Invoke { action: Action },
    PaletteQuery { query: String },
    PaletteChoose { label: String },
    AddHighlight { page: u32, quads: Vec<PdfRect>, comment: String, color: [f32; 3] },
    EditNote { id: u64, comment: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Action { OpenDialog, SaveAsDialog, Find, GoTo, QuickAccess, CommandPalette, CloseMenus, ZoomIn, ZoomOut, FitPage, FitWidth, NextPage, PreviousPage, FirstPage, LastPage, FindNext, FindPrevious, Undo, Redo, Select, Details, Tools, Layers, Quantities }

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PdfRect { pub left: f32, pub bottom: f32, pub right: f32, pub top: f32 }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Page { pub position: usize, pub file_page: usize, pub label: Option<String>, pub size: [f32; 2], pub turns: u8 }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Layer { pub id: String, pub name: String, pub path: String, pub visible: bool, pub locked: bool, pub active: bool }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tool { pub name: String, pub group: String, pub kind: String, pub settings: serde_json::Value }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Annotation { pub id: String, pub page: usize, pub kind: String, pub comment: String, pub can_reshape: bool }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchResult { pub index: usize, pub page: Option<usize>, pub matched: String, pub before: String, pub after: String }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Data {
    Pages(Vec<Page>), Layers(Vec<Layer>),
    Tools { saved: Vec<Tool>, available: Vec<Tool> },
    /// Uses Kinetic's existing tool-file configuration schema.
    ToolSchema(serde_json::Value),
    Text { page: usize, text: String, truncated: bool },
    Annotations { items: Vec<Annotation>, complete: bool },
    Search { results: Vec<SearchResult>, complete: bool },
    Identifiers(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus { Pending, Complete, Failed }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Operation { pub id: u64, pub status: OperationStatus, pub error: Option<Error> }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response { pub state: State, pub data: Option<Data>, pub operation: Option<Operation> }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
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
    pub active_tool: Option<String>,
    pub active_layer: Option<String>,
    pub search_query: String,
    pub search_complete: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode { Unavailable, Busy, StaleDocument, InvalidParameters, Cancelled, Timeout, QueueFull, Failed, NotFound, NeedsConfirmation, Unsupported }

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Error { pub code: ErrorCode, pub message: String }

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}

pub type Result = std::result::Result<Response, Error>;
