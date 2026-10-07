//! Tracks completion from worker replies, rather than treating submission as success.
use super::*;
use api::{Operation, OperationStatus};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum Wait {
    Open,
    Save,
    Text(usize),
    Search(u64),
}
pub(super) struct Pending {
    pub(super) operation: Operation,
    pub(super) wait: Wait,
    generation: u64,
    started: Instant,
    done: bool,
}

impl Control {
    pub(in crate::app) fn observe_reply(&mut self, reply: &Reply) {
        for pending in &mut self.operations {
            if pending.operation.status != OperationStatus::Pending {
                continue;
            }
            match (pending.wait, reply) {
                (Wait::Open, Reply::Opened { generation, .. })
                    if *generation == pending.generation =>
                {
                    pending.done = true
                }
                (Wait::Save, Reply::Saved { generation, .. })
                    if *generation == pending.generation =>
                {
                    pending.done = true
                }
                (
                    Wait::Text(page),
                    Reply::Text {
                        generation,
                        page: read,
                        ..
                    },
                ) if *generation == pending.generation && page == *read => pending.done = true,
                (
                    Wait::Search(id),
                    Reply::Search {
                        generation,
                        id: read,
                        done: true,
                        ..
                    },
                ) if *generation == pending.generation && id == *read => pending.done = true,
                (Wait::Open, Reply::OpenFailed { generation, error })
                | (Wait::Save, Reply::SaveFailed { generation, error })
                    if *generation == pending.generation =>
                {
                    pending.operation.status = OperationStatus::Failed;
                    pending.operation.error = Some(Error::new(ErrorCode::Failed, error.clone()));
                }
                (Wait::Save, Reply::OpenFailed { generation, error })
                    if pending.done && *generation == pending.generation + 1 =>
                {
                    pending.operation.status = OperationStatus::Failed;
                    pending.operation.error = Some(Error::new(
                        ErrorCode::Failed,
                        format!("File saved but reload failed: {error}"),
                    ));
                }
                _ => {}
            }
        }
    }

    pub(super) fn operation_slot(&mut self) -> std::result::Result<(), Error> {
        if self.operations.len() == 64 {
            if let Some(at) = self
                .operations
                .iter()
                .position(|p| p.operation.status != OperationStatus::Pending)
            {
                self.operations.remove(at);
            } else {
                return Err(Error::new(
                    ErrorCode::QueueFull,
                    "Too many unfinished operations",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn begin(&mut self, wait: Wait, generation: u64) -> Operation {
        self.next_operation += 1;
        let operation = Operation {
            id: self.next_operation,
            status: OperationStatus::Pending,
            error: None,
        };
        self.operations.push(Pending {
            operation: operation.clone(),
            wait,
            generation,
            started: Instant::now(),
            done: false,
        });
        operation
    }
}

impl App {
    pub(super) fn refresh_operations(&mut self) {
        let busy = self.lifecycle.status() != Status::Idle;
        let unavailable = self.lifecycle.status() == Status::Unavailable;
        for pending in &mut self.control.operations {
            if pending.operation.status != OperationStatus::Pending {
                continue;
            }
            if unavailable || matches!(pending.wait, Wait::Search(id) if id != self.search.id) {
                pending.operation.status = OperationStatus::Failed;
                pending.operation.error = Some(Error::new(
                    if unavailable {
                        ErrorCode::Unavailable
                    } else {
                        ErrorCode::Cancelled
                    },
                    "The app became unavailable or this search was replaced",
                ));
            } else if pending.done && (!matches!(pending.wait, Wait::Save) || !busy) {
                pending.operation.status = OperationStatus::Complete;
            } else if self.generation != pending.generation
                && !(matches!(pending.wait, Wait::Save) && pending.done)
            {
                pending.operation.status = OperationStatus::Failed;
                pending.operation.error = Some(Error::new(
                    ErrorCode::StaleDocument,
                    "The document was replaced during the operation",
                ));
            } else if pending.started.elapsed() > Duration::from_secs(60) {
                pending.operation.status = OperationStatus::Failed;
                pending.operation.error = Some(Error::new(
                    ErrorCode::Timeout,
                    "Operation did not finish in 60 seconds; inspect the app before retrying",
                ));
            }
        }
    }
}
