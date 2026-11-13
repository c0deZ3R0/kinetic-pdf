//! Deferred, PDFium-free signature inventory and bounded latest-job service.
//!
//! PATH: src/signature_discovery.rs
//! CONSUMED BY: worker (private authority, scheduling and revision-bound replies).
//! Pending/returned errors are Unknown. Panic recovery applies only to unwind builds:
//! release/quick use panic=abort, so parser panics still abort the app; this is not
//! process isolation. One running job plus one latest pending job; cancellation
//! is cooperative (lopdf load itself is not interruptible). Drop
//! cancels/detaches, never joins a busy parser on the worker/UI path.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use crate::document::Snapshot;
use crate::domain::SignatureDiscovery as Discovery;

// Helpers can deliver rendering without passing through this worker; headless
// clients may never render. Both get a bounded scheduling fallback, not a poll.
const RENDER_GRACE: Duration = Duration::from_millis(100);

type Analyze = dyn Fn(&Snapshot, &AtomicBool, bool) -> Discovery + Send + Sync;
type Publish = dyn Fn(Completion) + Send + Sync;

pub(crate) struct Completion {
    pub generation: u64,
    pub file: u64,
    pub discovery: Discovery,
}

struct Flags {
    cancelled: AtomicBool,
    released: AtomicBool,
}

/// No public protocol value can set this once-only worker authority.
#[derive(Clone)]
pub(crate) struct Status {
    result: Arc<OnceLock<Discovery>>,
    flags: Option<Arc<Flags>>,
    wake: Weak<Shared>,
}

impl Status {
    pub fn known(discovery: Discovery) -> Self {
        let result = OnceLock::new();
        let _ = result.set(discovery);
        Self { result: Arc::new(result), flags: None, wake: Weak::new() }
    }

    pub fn current(&self) -> Discovery {
        self.result.get().copied().unwrap_or(Discovery::Unknown)
    }

    /// Scheduling only: result remains Unknown until actual inventory completes.
    pub fn release(&self) {
        if let Some(shared) = self.wake.upgrade() {
            let _queue = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(flags) = &self.flags { flags.released.store(true, Ordering::Release); }
            shared.wake.notify_one();
        }
    }
}

struct Job {
    generation: u64,
    snapshot: Snapshot,
    status: Status,
    expect_signature_fields: bool,
    deadline: Instant,
}

#[derive(Default)]
struct Queue {
    pending: Option<Job>,
    running: Option<Arc<Flags>>,
    stopped: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

pub(crate) struct Service {
    shared: Arc<Shared>,
}

impl Service {
    /// Created lazily only when a supported form/signature hint needs inventory.
    pub fn new(publish: impl Fn(Completion) + Send + Sync + 'static) -> std::io::Result<Self> {
        Self::start(Arc::new(|snapshot, cancelled, expected| inventory(snapshot.bytes(), cancelled, expected).unwrap_or(Discovery::Unknown)), Arc::new(publish))
    }

    fn start(analyze: Arc<Analyze>, publish: Arc<Publish>) -> std::io::Result<Self> {
        let shared = Arc::new(Shared { queue: Mutex::new(Queue::default()), wake: Condvar::new() });
        let thread_shared = shared.clone();
        std::thread::Builder::new().name("signature-inventory".into()).spawn(move || {
            loop {
                let mut queue = thread_shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                while queue.pending.is_none() && !queue.stopped {
                    queue = thread_shared.wake.wait(queue).unwrap_or_else(|e| e.into_inner());
                }
                if queue.stopped { return; }
                let job = queue.pending.take().unwrap();
                let flags = job.status.flags.as_ref().unwrap().clone();
                queue.running = Some(flags.clone());
                while !queue.stopped && !flags.cancelled.load(Ordering::Acquire)
                    && !flags.released.load(Ordering::Acquire) && Instant::now() < job.deadline
                {
                    let remaining = job.deadline.saturating_duration_since(Instant::now());
                    queue = thread_shared.wake.wait_timeout(queue, remaining).unwrap_or_else(|e| e.into_inner()).0;
                }
                if queue.stopped { return; }
                if flags.cancelled.load(Ordering::Acquire) { queue.running = None; continue; }
                drop(queue);
                // Only unwind builds recover here; release panic=abort still needs
                // process isolation before crash-recovery guarantees are qualified.
                let discovery = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze(&job.snapshot, &flags.cancelled, job.expect_signature_fields)))
                    .unwrap_or(Discovery::Unknown);
                let mut queue = thread_shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                queue.running = None;
                if queue.stopped { return; }
                if flags.cancelled.load(Ordering::Acquire) { continue; }
                // Authority first, publication second. A UI unlock can never
                // race ahead of worker Save/Extract seeing the same result.
                let _ = job.status.result.set(discovery);
                let completion = Completion { generation: job.generation, file: job.snapshot.file(), discovery };
                drop(queue);
                publish(completion);
            }
        })?; // The JoinHandle is deliberately detached, never joined on drop/open.
        Ok(Self { shared })
    }

    pub fn cancel(&self) {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flags) = &queue.running { flags.cancelled.store(true, Ordering::Release); }
        if let Some(job) = queue.pending.take() {
            job.status.flags.as_ref().unwrap().cancelled.store(true, Ordering::Release);
        }
        self.shared.wake.notify_one();
    }

    pub fn submit(&self, generation: u64, snapshot: Snapshot, expect_signature_fields: bool) -> Status {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flags) = &queue.running { flags.cancelled.store(true, Ordering::Release); }
        if let Some(job) = queue.pending.take() {
            job.status.flags.as_ref().unwrap().cancelled.store(true, Ordering::Release);
        }
        let status = Status {
            result: Arc::new(OnceLock::new()),
            flags: Some(Arc::new(Flags { cancelled: AtomicBool::new(false), released: AtomicBool::new(false) })),
            wake: Arc::downgrade(&self.shared),
        };
        queue.pending = Some(Job { generation, snapshot, status: status.clone(), expect_signature_fields, deadline: Instant::now() + RENDER_GRACE });
        self.shared.wake.notify_one();
        status
    }

    #[cfg(test)]
    pub(crate) fn with_analyzer(analyze: impl Fn(&Snapshot, &AtomicBool, bool) -> Discovery + Send + Sync + 'static,
        publish: impl Fn(Completion) + Send + Sync + 'static,
    ) -> Self {
        Self::start(Arc::new(analyze), Arc::new(publish)).unwrap()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.cancel();
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.shared.wake.notify_one();
    }
}

/// Bounded structural discovery, not cryptographic validation/absence proof.
/// Unlike the native top-level count, this visits nested/inherited form fields.
pub(crate) fn inventory(bytes: &[u8], cancelled: &AtomicBool, expect_signature_fields: bool) -> Result<Discovery, ()> {
    use pdf_content::lopdf::{Document, LoadOptions, Object, ObjectId};
    const MAX_NODES: usize = 100_000;
    const MAX_DEPTH: usize = 64;
    if cancelled.load(Ordering::Acquire) || bytes.len() > 256 * 1024 * 1024 { return Err(()); }
    let doc = Document::load_mem_with_options(bytes, LoadOptions {
        strict: true, max_decompressed_size: Some(16 * 1024 * 1024), ..Default::default()
    }).map_err(|_| ())?;
    if cancelled.load(Ordering::Acquire) || doc.is_encrypted() || doc.trailer.has(b"Encrypt") || doc.objects.len() > MAX_NODES { return Err(()); }
    let catalog = doc.catalog().map_err(|_| ())?;
    if catalog.has(b"Perms") { return Err(()); }
    let form = doc.dereference(catalog.get(b"AcroForm").map_err(|_| ())?).map_err(|_| ())?.1;
    let form = form.as_dict().map_err(|_| ())?;
    if form.has(b"XFA") { return Err(()); }
    let fields = doc.dereference(form.get(b"Fields").map_err(|_| ())?).map_err(|_| ())?.1;
    let fields = fields.as_array().map_err(|_| ())?;

    struct Fields<'a> {
        doc: &'a Document,
        cancelled: &'a AtomicBool,
        seen: HashSet<ObjectId>,
        nodes: usize,
        saw_signature_field: bool,
        populated: HashSet<usize>,
    }
    impl<'a> Fields<'a> {
        fn visit(&mut self, field: &'a Object, inherited_kind: Option<&'a [u8]>,
            inherited_value: Option<(&'a Object, usize)>, parent_id: Option<ObjectId>, depth: usize,
        ) -> Result<(), ()> {
            if self.cancelled.load(Ordering::Acquire) || depth > MAX_DEPTH || self.nodes >= MAX_NODES { return Err(()); }
            let node = self.nodes;
            self.nodes += 1;
            let (id, field) = self.doc.dereference(field).map_err(|_| ())?;
            if id.is_some_and(|id| !self.seen.insert(id)) { return Err(()); }
            let field = field.as_dict().map_err(|_| ())?;
            if let Ok(parent) = field.get(b"Parent") {
                if parent_id.is_none() || parent.as_reference().ok() != parent_id { return Err(()); }
            }
            let kind = match field.get(b"FT").ok() {
                None => inherited_kind,
                Some(kind) => Some(self.doc.dereference(kind).map_err(|_| ())?.1.as_name().map_err(|_| ())?),
            };
            let value = match field.get(b"V").ok() {
                None => inherited_value,
                Some(value) => {
                    let value = self.doc.dereference(value).map_err(|_| ())?.1;
                    if matches!(value, Object::Null) { None } else { Some((value, node)) }
                }
            };
            if kind.is_some() && !matches!(kind, Some(b"Sig" | b"Tx" | b"Btn" | b"Ch")) { return Err(()); }
            if value.is_some() && kind.is_none() { return Err(()); }
            if kind == Some(b"Sig".as_slice()) {
                self.saw_signature_field = true;
                if let Some((_, owner)) = value {
                    // Inherited parent value counted once across widgets. Invalid
                    // populated values and zero-length Contents remain protected.
                    self.populated.insert(owner);
                }
            }
            if let Ok(kids) = field.get(b"Kids") {
                let kids = self.doc.dereference(kids).map_err(|_| ())?.1.as_array().map_err(|_| ())?;
                for kid in kids { self.visit(kid, kind, value, id, depth + 1)?; }
            }
            Ok(())
        }
    }
    let mut inventory = Fields { doc: &doc, cancelled, seen: HashSet::new(), nodes: 0, saw_signature_field: false, populated: HashSet::new() };
    for field in fields { inventory.visit(field, None, None, None, 0)?; }
    if expect_signature_fields && !inventory.saw_signature_field { return Err(()); }
    Ok(if inventory.populated.is_empty() { Discovery::NoSignaturesDetected }
        else { Discovery::Present { count: inventory.populated.len() } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn pending_authority_stays_locked_until_completion_before_publication() {
        let (started, start_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let (published, published_rx) = mpsc::channel();
        let service = Service::with_analyzer(move |_, _, _| {
            started.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Discovery::NoSignaturesDetected
        }, move |completion| { published.send(completion).unwrap(); });
        let status = service.submit(7, Snapshot::new(b"revision".to_vec()), false);
        status.release();
        start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(status.current(), Discovery::Unknown);
        assert!(!status.current().allows_document_mutation());
        release.send(()).unwrap();
        let completion = published_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(completion.generation, 7);
        assert_eq!(completion.file, Snapshot::new(b"revision".to_vec()).file());
        assert_eq!(status.current(), completion.discovery);
    }

    #[test]
    fn superseded_jobs_keep_only_one_latest_pending_and_cannot_unlock() {
        let (started, start_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let (published, published_rx) = mpsc::channel();
        let service = Service::with_analyzer(move |snapshot, _, _| {
            started.send(snapshot.bytes()[0]).unwrap();
            if snapshot.bytes()[0] == 1 { release_rx.lock().unwrap().recv().unwrap(); }
            Discovery::NoSignaturesDetected
        }, move |completion| { published.send(completion).unwrap(); });
        let first = service.submit(1, Snapshot::new(vec![1]), false);
        first.release();
        assert_eq!(start_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        let stale = service.submit(2, Snapshot::new(vec![2]), false);
        let latest = service.submit(3, Snapshot::new(vec![3]), false);
        latest.release();
        {
            let queue = service.shared.queue.lock().unwrap();
            assert!(queue.running.is_some());
            assert_eq!(queue.pending.as_ref().unwrap().generation, 3);
        }
        release.send(()).unwrap();
        assert_eq!(start_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
        assert_eq!(published_rx.recv_timeout(Duration::from_secs(2)).unwrap().generation, 3);
        assert!(published_rx.try_recv().is_err());
        assert_eq!(first.current(), Discovery::Unknown);
        assert_eq!(stale.current(), Discovery::Unknown);
        assert_eq!(latest.current(), Discovery::NoSignaturesDetected);
    }

    #[test]
    fn headless_fallback_finishes_and_parse_panic_or_error_never_unlocks() {
        for panic in [false, true] {
            let (published, published_rx) = mpsc::channel();
            let service = Service::with_analyzer(move |snapshot, cancelled, expected| {
                if panic { panic!("injected inventory panic"); }
                inventory(snapshot.bytes(), cancelled, expected).unwrap_or(Discovery::Unknown)
            }, move |completion| { published.send(completion).unwrap(); });
            // Deliberately no render/release: bounded fallback must schedule it.
            let status = service.submit(1, Snapshot::new(b"broken PDF".to_vec()), false);
            assert_eq!(published_rx.recv_timeout(Duration::from_secs(3)).unwrap().discovery, Discovery::Unknown);
            assert_eq!(status.result.get(), Some(&Discovery::Unknown));
        }
    }

    #[test]
    fn dropping_busy_service_does_not_join_parser_or_publish_old_result() {
        let (started, start_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let (finished, finish_rx) = mpsc::channel();
        let (published, published_rx) = mpsc::channel();
        let service = Service::with_analyzer(move |_, _, _| {
            started.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            finished.send(()).unwrap();
            Discovery::NoSignaturesDetected
        }, move |completion| { let _ = published.send(completion); });
        let status = service.submit(1, Snapshot::new(vec![]), false);
        status.release();
        start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(service); // Would deadlock here if Drop joined the blocked analyzer.
        release.send(()).unwrap();
        finish_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(status.current(), Discovery::Unknown);
        assert!(published_rx.recv_timeout(Duration::from_secs(2)).is_err());
    }
}
