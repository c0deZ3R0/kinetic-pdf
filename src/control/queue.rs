//! Bounded handoff to the app thread. Dropping a ticket cancels queued work.
use super::{Error, ErrorCode, Request, Result};
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Client { tx: mpsc::SyncSender<Envelope>, wake: Arc<dyn Fn() + Send + Sync> }
pub struct Inbox(mpsc::Receiver<Envelope>);
pub struct Ticket { rx: mpsc::Receiver<Result>, cancelled: Arc<AtomicBool> }
pub struct Envelope {
    pub request: Request,
    tx: mpsc::SyncSender<Result>,
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}

pub fn channel(wake: impl Fn() + Send + Sync + 'static) -> (Client, Inbox) {
    let (tx, rx) = mpsc::sync_channel(32);
    (Client { tx, wake: Arc::new(wake) }, Inbox(rx))
}

impl Client {
    pub fn submit(&self, request: Request, timeout: Duration) -> std::result::Result<Ticket, Error> {
        let (tx, rx) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let envelope = Envelope { request, tx, cancelled: cancelled.clone(), deadline: Instant::now() + timeout };
        self.tx.try_send(envelope).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => Error::new(ErrorCode::QueueFull, "The app request queue is full"),
            mpsc::TrySendError::Disconnected(_) => Error::new(ErrorCode::Unavailable, "The app has closed"),
        })?;
        (self.wake)();
        Ok(Ticket { rx, cancelled })
    }
}

impl Inbox { pub fn next(&self) -> Option<Envelope> { self.0.try_recv().ok() } }
impl Envelope {
    pub fn can_start(&self) -> bool { !self.cancelled.load(Ordering::Acquire) && Instant::now() < self.deadline }
    pub fn finish(self, result: Result) { let _ = self.tx.try_send(result); }
}
impl Ticket {
    pub fn try_result(&self) -> Option<Result> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(Error::new(ErrorCode::Cancelled, "Request was cancelled before execution"))),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
impl Drop for Ticket { fn drop(&mut self) { self.cancelled.store(true, Ordering::Release); } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Command;
    fn request() -> Request { Request { command: Command::Inspect, target: None } }
    #[test]
    fn dropped_or_expired_requests_cannot_start() {
        let (client, inbox) = channel(|| {});
        let ticket = client.submit(request(), Duration::from_secs(1)).unwrap();
        drop(ticket);
        assert!(!inbox.next().unwrap().can_start());
        let _ticket = client.submit(request(), Duration::ZERO).unwrap();
        assert!(!inbox.next().unwrap().can_start());
    }
    #[test]
    fn queue_is_bounded_and_closed_apps_report_unavailable() {
        let (client, inbox) = channel(|| {});
        let tickets: Vec<_> = (0..32).map(|_| client.submit(request(), Duration::from_secs(1)).unwrap()).collect();
        assert_eq!(client.submit(request(), Duration::from_secs(1)).err().unwrap().code, ErrorCode::QueueFull);
        drop(tickets);
        drop(inbox);
        assert_eq!(client.submit(request(), Duration::from_secs(1)).err().unwrap().code, ErrorCode::Unavailable);
    }
}
