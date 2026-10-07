//! Replay format and clock-driven player. No egui, transport, or video encoder here.
use crate::control::{
    Command, DocumentTarget, Error, ErrorCode, Operation, OperationStatus, Response, State,
};
#[cfg(feature = "mcp")]
use rmcp::schemars;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub version: u32,
    /// Logical window size. Resizing belongs to the app adapter.
    pub window: Option<[f32; 2]>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Command {
        command: Command,
    },
    Pause {
        ms: u32,
    },
    WaitView {
        timeout_ms: u32,
    },
    /// Smoothstep easing; zoom is absolute, pan is a relative logical-pixel delta.
    Animate {
        zoom: Option<f32>,
        pan: [f32; 2],
        ms: u32,
    },
    /// A visual cue, never an OS mouse event. Position is relative to the viewer.
    Cue {
        caption: String,
        shortcut: Option<String>,
        pointer: Option<[f32; 2]>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Cue {
    pub caption: String,
    pub shortcut: Option<String>,
    pub pointer: Option<[f32; 2]>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Complete,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Progress {
    pub status: Status,
    pub step: usize,
    pub total: usize,
    pub error: Option<Error>,
}

pub struct Player {
    script: Script,
    pub cue: Cue,
    pub progress: Progress,
    expected: Option<DocumentTarget>,
    start: Option<f64>,
    animation: Option<(f32, [f32; 2])>,
    waiting: Option<Operation>,
    operation_started: f64,
    ready_frames: u8,
}

pub enum Effect {
    Command(Command),
    View { zoom: f32, scroll: [f32; 2] },
}

impl Script {
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |s| Error::new(ErrorCode::InvalidParameters, s);
        if self.version != 1 || self.steps.is_empty() || self.steps.len() > 1000 {
            return Err(invalid("Demo requires version 1 and 1–1000 steps"));
        }
        if self.window.is_some_and(|v| {
            v.iter()
                .any(|n| !n.is_finite() || !(320.0..=7680.0).contains(n))
        }) {
            return Err(invalid("Invalid demo window dimensions"));
        }
        for step in &self.steps {
            match step {
                Step::Command {
                    command:
                        Command::RunDemo { .. }
                        | Command::CancelDemo
                        | Command::DemoStatus
                        | Command::Invoke { action: crate::control::Action::OpenDialog | crate::control::Action::SaveAsDialog }
                        | Command::Experiment { .. }
                        | Command::Once { .. },
                } => {
                    return Err(invalid(
                        "Nested demos, native file dialogs, experiments and replay wrappers are not demo steps",
                    ))
                }
                Step::Pause { ms } | Step::WaitView { timeout_ms: ms }
                    if *ms == 0 || *ms > 60000 =>
                {
                    return Err(invalid("Demo durations must be 1–60000 ms"))
                }
                Step::Animate { zoom, pan, ms }
                    if *ms == 0
                        || *ms > 60000
                        || zoom.is_some_and(|v| !v.is_finite() || !(0.05..=8.0).contains(&v))
                        || pan.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) =>
                {
                    return Err(invalid("Invalid demo animation"))
                }
                Step::Cue {
                    caption,
                    shortcut,
                    pointer,
                } if caption.len() > 1024
                    || shortcut.as_ref().is_some_and(|s| s.len() > 128)
                    || pointer.is_some_and(|p| {
                        p.iter().any(|n| !n.is_finite() || !(0.0..=1.0).contains(n))
                    }) =>
                {
                    return Err(invalid("Invalid demo cue"))
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl Player {
    pub fn new(script: Script, state: &State) -> Result<Self, Error> {
        script.validate()?;
        Ok(Self {
            progress: Progress {
                status: Status::Running,
                step: 0,
                total: script.steps.len(),
                error: None,
            },
            script,
            cue: Cue::default(),
            expected: state.document.clone(),
            start: None,
            animation: None,
            waiting: None,
            operation_started: 0.0,
            ready_frames: 0,
        })
    }
    pub fn running(&self) -> bool {
        self.progress.status == Status::Running
    }
    pub fn target(&self) -> Option<DocumentTarget> {
        self.expected.clone()
    }
    pub fn cancel(&mut self) {
        if self.running() {
            self.progress.status = Status::Cancelled;
            self.cue = Cue::default();
        }
    }
    pub fn fail(&mut self, error: Error) {
        self.progress.status = Status::Failed;
        self.progress.error = Some(error);
        self.cue = Cue::default();
    }
    fn advance(&mut self) {
        self.progress.step += 1;
        self.start = None;
        self.animation = None;
        self.ready_frames = 0;
    }
    pub fn accept(&mut self, result: crate::control::Result, now: f64) {
        match result {
            Err(error) => self.fail(error),
            Ok(response) => {
                self.expected = response.state.document;
                self.waiting = response
                    .operation
                    .filter(|o| o.status == OperationStatus::Pending);
                if self.waiting.is_some() {
                    self.operation_started = now;
                } else {
                    self.advance();
                }
            }
        }
    }
    pub fn next(&mut self, now: f64, state: &State) -> Option<Effect> {
        if !self.running() {
            return None;
        }
        if let Some(operation) = &self.waiting {
            if now - self.operation_started > 65.0 {
                self.fail(Error::new(ErrorCode::Timeout, "Demo operation timed out"));
                return None;
            }
            return Some(Effect::Command(Command::PollOperation { id: operation.id }));
        }
        if state.document != self.expected {
            self.fail(Error::new(
                ErrorCode::StaleDocument,
                "Document changed outside demo playback",
            ));
            return None;
        }
        let Some(step) = self.script.steps.get(self.progress.step).cloned() else {
            self.progress.status = Status::Complete;
            self.cue = Cue::default();
            return None;
        };
        let started = *self.start.get_or_insert(now);
        match step {
            Step::Command { command } => Some(Effect::Command(command)),
            Step::Cue {
                caption,
                shortcut,
                pointer,
            } => {
                self.cue = Cue {
                    caption,
                    shortcut,
                    pointer,
                };
                self.advance();
                None
            }
            Step::Pause { ms } => {
                if (now - started) * 1000.0 >= f64::from(ms) {
                    self.advance();
                }
                None
            }
            Step::WaitView { timeout_ms } => {
                if state.view_ready && !state.busy {
                    self.ready_frames += 1;
                } else {
                    self.ready_frames = 0;
                }
                if self.ready_frames >= 2 {
                    self.advance();
                } else if (now - started) * 1000.0 > f64::from(timeout_ms) {
                    self.fail(Error::new(
                        ErrorCode::Timeout,
                        "Demo view did not become ready",
                    ));
                }
                None
            }
            Step::Animate { zoom, pan, ms } => {
                if state.document.is_none() {
                    self.fail(Error::new(
                        ErrorCode::Unavailable,
                        "Demo animation needs an open document",
                    ));
                    return None;
                }
                if state.busy {
                    self.fail(Error::new(
                        ErrorCode::Busy,
                        "Cannot animate during document work",
                    ));
                    return None;
                }
                let (initial_zoom, initial_scroll) =
                    *self.animation.get_or_insert((state.zoom, state.scroll));
                let t = (((now - started) * 1000.0) / f64::from(ms)).clamp(0.0, 1.0) as f32;
                let t_eased = t * t * (3.0 - 2.0 * t);
                let result = Effect::View {
                    zoom: initial_zoom + (zoom.unwrap_or(initial_zoom) - initial_zoom) * t_eased,
                    scroll: [
                        initial_scroll[0] + pan[0] * t_eased,
                        initial_scroll[1] + pan[1] * t_eased,
                    ],
                };
                if t >= 1.0 {
                    self.advance();
                }
                Some(result)
            }
        }
    }
    pub fn observe_operation(&mut self, response: Response) {
        let Some(operation) = response.operation else {
            self.fail(Error::new(
                ErrorCode::Failed,
                "Missing demo operation result",
            ));
            return;
        };
        match operation.status {
            OperationStatus::Pending => {}
            OperationStatus::Complete => {
                self.expected = response.state.document;
                self.waiting = None;
                self.advance();
            }
            OperationStatus::Failed => self.fail(
                operation
                    .error
                    .unwrap_or_else(|| Error::new(ErrorCode::Failed, "Demo operation failed")),
            ),
        }
    }
}
