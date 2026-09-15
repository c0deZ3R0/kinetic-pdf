//! The app's side of the render helpers (helper.rs): starting them, handing
//! them page renders most wanted first, stopping renders for pages that have
//! left the view, and turning the pixels they send back into textures. With a
//! page cache (cache.rs) it also serves slow pages from disk, and while
//! nothing else is wanted it draws the rest of the document into the cache,
//! nearest pages first, so they're ready before anyone scrolls to them.
//!
//! A scheduler thread owns the helpers and decides who draws what. Each helper
//! also has a thread reading what it sends, which makes the textures, replies
//! to the UI and fills the cache directly, so a finished page never waits on
//! the scheduler; a cache reader thread does the same for pages coming off
//! disk. The worker thread (worker.rs) still does everything else -- text,
//! highlights, search, saving -- and draws pages itself if no helper can.

use std::collections::HashSet;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::cache::{self, Cache, Key};
use crate::helper::{self, Command, Event, Target};
use crate::merge;
use crate::model::{self, Reply, Request};
use crate::worker::{load_tiles, make_texture, make_tiles, trace, Wanted};

/// Helpers started when the machine has room.
const MOST_HELPERS: usize = 3;

/// How often the scheduler looks at what the view wants while renders are
/// under way, to stop those for pages that have left it.
const CHECK_EVERY: Duration = Duration::from_millis(30);

/// How often an idle scheduler looks for pages to draw ahead into the cache.
/// The view settling doesn't send it anything, so it has to look.
const IDLE_CHECK_EVERY: Duration = Duration::from_millis(150);

/// How long helpers wait for the copy to draw from (merge.rs) when a file
/// might have annotations on layers that are off. Making one took 0.2 s for a
/// Bluebeam overlay; a long drawing set takes longer, and past this pages are
/// drawn from the file itself, and not kept, until the copy is ready.
const COPY_WAIT: Duration = Duration::from_secs(2);

/// Drawings asked for ahead of a zoom that are kept waiting; older ones go,
/// since the pointer has moved on from them.
const MOST_PREDICTED: usize = 6;

/// A command to start this exe for work in the background: the render helpers
/// and the process making a copy to draw from. On Windows it opens no console
/// window and runs below normal priority, so drawing ahead never takes time
/// from anything else running; the app's own process, and so its window, stays
/// at normal priority. After an update is swapped in, it still starts this
/// version, from where the update moved it (update.rs).
fn background(exe: &std::path::Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(crate::update::running_exe(exe));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    command
}

const GB: u64 = 1024 * 1024 * 1024;

/// Which helpers to start.
pub struct Helpers {
    exe: Option<PathBuf>,
    count: usize,
}

impl Helpers {
    /// None: the worker thread draws every page itself.
    pub fn none() -> Self {
        Helpers { exe: None, count: 0 }
    }

    /// `exe` started `count` times. It must run `helper::run` when given
    /// `helper::FLAG`.
    pub fn exe(exe: impl Into<PathBuf>, count: usize) -> Self {
        Helpers { exe: Some(exe.into()), count }
    }

    /// This exe, as many times as suits the machine: three, or fewer with few
    /// cores or little free memory, since a helper can hold a few hundred MB
    /// while it draws a large drawing. `KINETIC_PDF_HELPERS` sets the count,
    /// for comparing.
    pub fn from_current_exe() -> Self {
        let count = std::env::var("KINETIC_PDF_HELPERS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(suggested_count);
        Helpers { exe: std::env::current_exe().ok(), count }
    }
}

fn suggested_count() -> usize {
    // The UI thread and the worker thread keep a core each.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()).saturating_sub(2);
    let free = free_memory();
    let by_memory = if free < 2 * GB {
        0
    } else if free < 4 * GB {
        1
    } else {
        MOST_HELPERS
    };
    cores.min(by_memory)
}

/// Physical memory free now, or as good as unlimited if Windows won't say.
pub(crate) fn free_memory() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return u64::MAX;
    }
    status.ullAvailPhys
}

/// Messages to the scheduler thread.
pub(crate) enum Input {
    Open { generation: u64, path: PathBuf },
    /// The fingerprint of the file just opened, from the worker, for the cache,
    /// and whether it might have annotations on layers that are off.
    File { generation: u64, fingerprint: u64, layers: bool },
    /// A render the UI asked for.
    Render { generation: u64, page: usize, target: Target },
    /// A render ahead of a zoom the user may be about to make.
    Predict { generation: u64, page: usize, target: Target },
    /// A page that was in the cache but didn't read back: draw it after all.
    Draw { generation: u64, page: usize, target: Target },
    /// The worker has saved over the file, so the helpers open it again.
    /// `redrawn` if the save changed how pages are drawn.
    Saved { generation: u64, fingerprint: u64, redrawn: bool },
    /// The copy of the file to draw from, with its stamps' lines merged, has
    /// been made; `None` if there's nothing to merge or it couldn't be made.
    Copy { generation: u64, fingerprint: u64, path: Option<PathBuf> },
    Opened { helper: usize, version: u64, ok: bool },
    /// The helper is free again. `stopped` if its render was cancelled before
    /// it finished.
    Finished { helper: usize, id: u64, stopped: bool },
    Exited { helper: usize },
    /// The app is closing.
    Shutdown,
}

/// A running pool, as the worker's request thread sees it.
pub(crate) struct Pool {
    pub inputs: Sender<Input>,
    /// False once every helper has gone; renders then go to the worker.
    pub alive: Arc<AtomicBool>,
}

/// The reply for a render dropped because its page is no longer wanted, so the
/// UI asks again if the page comes back.
fn skipped(generation: u64, page: usize, target: Target) -> Reply {
    match target {
        Target::Page { .. } => Reply::RenderSkipped { generation, page },
        Target::Region { full, region, annotations } => Reply::RenderedRegion { generation, page, full, region, annotations, tiles: Vec::new() },
    }
}

/// `target` of `page`, to read back from the cache, if the cache has all of it.
fn cached_load(cache: &Cache, file: u64, generation: u64, page: usize, target: Target) -> Option<Load> {
    match target {
        Target::Page { scale, annotations } => {
            let key = Key::new(file, page, scale).annotations(annotations);
            cache.has_image(key).then_some(Load::Page { generation, page, scale, key })
        }
        Target::Region { full, region, annotations } => model::tile_cells(full, region)
            .iter()
            .all(|&(column, row)| cache.has_image(Key::tile(file, page, full, column, row).annotations(annotations)))
            .then_some(Load::Tiles { generation, page, full, region, annotations, file }),
    }
}

/// The sharp view of the part in view before the whole page: when both are
/// wanted, the view is what the user is looking at.
fn order(target: Target) -> u8 {
    match target {
        Target::Region { .. } => 0,
        Target::Page { .. } => 1,
    }
}

fn send(replies: &Sender<Reply>, ctx: &egui::Context, reply: Reply) {
    let _ = replies.send(reply);
    ctx.request_repaint();
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// For the view, or the pages just ahead of it.
    Wanted,
    /// Ahead of a zoom the user may be about to make: replied to like
    /// `Wanted`, but done only when nothing the view wants is waiting.
    Ahead,
    /// Drawn ahead of time, into the cache only.
    Background,
}

/// The render a helper is drawing.
#[derive(Clone, Copy)]
struct Assignment {
    id: u64,
    generation: u64,
    page: usize,
    target: Target,
    kind: Kind,
    /// The file's fingerprint, if known, for keeping the result.
    file: Option<u64>,
}

struct Slot {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    /// Shared with the thread reading this helper's events.
    assignment: Arc<Mutex<Option<Assignment>>>,
    /// The file version this helper has open, once it has said so.
    ready: Option<u64>,
    /// A file version this helper couldn't open.
    failed: Option<u64>,
    busy: Option<u64>,
    cancelled: bool,
    alive: bool,
    /// The page it drew last, which it keeps open while memory allows: the
    /// next render of that page goes to it, so the page isn't loaded again.
    last_page: Option<usize>,
}

impl Slot {
    fn command(&mut self, helper: usize, command: Command) {
        if command.write(&mut self.stdin).and_then(|()| self.stdin.flush()).is_err() {
            trace(format_args!("pool: helper {helper} stopped taking commands"));
            self.alive = false;
            let _ = self.child.kill();
        }
    }

    fn assignment(&self) -> Option<Assignment> {
        *self.assignment.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_assignment(&self, assignment: Option<Assignment>) {
        *self.assignment.lock().unwrap_or_else(|e| e.into_inner()) = assignment;
    }
}

struct Queued {
    generation: u64,
    page: usize,
    target: Target,
    /// Whether the cache has already been looked in for it.
    cache_checked: bool,
}

impl Queued {
    fn into_request(self) -> Request {
        let Queued { generation, page, target, .. } = self;
        match target {
            Target::Page { scale, .. } => Request::Render { generation, page, scale },
            Target::Region { full, region, .. } => Request::RenderRegion { generation, page, full, region },
        }
    }
}

/// Where drawing ahead has got to.
#[derive(Default)]
struct Ahead {
    /// The page it works outwards from, the scales it draws at and the file,
    /// as last seen. Any of them changing starts it again from the nearest page.
    anchor: usize,
    scales: Option<Arc<Vec<f32>>>,
    file: Option<u64>,
    /// The pages pdfium draws without their annotations, and those it needn't
    /// draw at all, as last seen.
    without_annotations: Option<Arc<HashSet<usize>>>,
    drawn_whole: Option<Arc<HashSet<usize>>>,
    /// How far out it has looked: step k is the page (k + 1) / 2 away from
    /// the anchor, after it for odd k and before it for even k.
    step: usize,
    /// Pages, at a scale and with their annotations or without, already
    /// tried since the file was opened.
    tried: HashSet<(usize, u32, bool)>,
}

/// Something to read back from the cache.
enum Load {
    /// A whole page.
    Page { generation: u64, page: usize, scale: f32, key: Key },
    /// Every square of a region of a page drawn zoomed in.
    Tiles { generation: u64, page: usize, full: [u32; 2], region: [u32; 4], annotations: bool, file: u64 },
}

/// Starts the helpers and the thread that schedules them. `None` if none
/// could be started, in which case the worker draws pages itself.
pub(crate) fn start(
    helpers: Helpers,
    ctx: egui::Context,
    wanted: Arc<Mutex<Wanted>>,
    replies: Sender<Reply>,
    fallback: Sender<Request>,
    cache: Option<Arc<Cache>>,
) -> Option<Pool> {
    let exe = helpers.exe?;
    let (inputs_tx, inputs) = mpsc::channel();
    let mut slots = Vec::new();
    for _ in 0..helpers.count {
        let spawned = background(&exe).arg(helper::FLAG).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                trace(format_args!("pool: could not start a helper: {e}"));
                continue;
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            continue;
        };
        let helper = slots.len();
        let assignment = Arc::new(Mutex::new(None));
        read_events(helper, stdout, Arc::clone(&assignment), inputs_tx.clone(), ctx.clone(), replies.clone(), cache.clone());
        slots.push(Slot {
            child,
            stdin: BufWriter::new(stdin),
            assignment,
            ready: None,
            failed: None,
            busy: None,
            cancelled: false,
            alive: true,
            last_page: None,
        });
    }
    if slots.is_empty() {
        return None;
    }
    trace(format_args!("pool: started {} helpers", slots.len()));

    let loads = cache
        .as_ref()
        .and_then(|cache| read_cached(Arc::clone(cache), ctx.clone(), replies.clone(), Arc::clone(&wanted), inputs_tx.clone()));
    let alive = Arc::new(AtomicBool::new(true));
    let scheduler = Scheduler {
        slots,
        inputs,
        wanted,
        replies,
        ctx,
        fallback,
        cache,
        loads,
        alive: Arc::clone(&alive),
        generation: 0,
        path: None,
        drawn_from: None,
        exe,
        to_self: inputs_tx.clone(),
        layers: false,
        unconfirmed: false,
        copy_waiting: None,
        file: None,
        version: 0,
        queue: Vec::new(),
        predicted: Vec::new(),
        ahead: Ahead::default(),
        next_id: 1,
    };
    let started = std::thread::Builder::new().name("render helpers".into()).spawn(move || scheduler.run());
    started.ok().map(|_| Pool { inputs: inputs_tx, alive })
}

/// Keeps a slow page's image in the cache, or notes that the page is quick.
fn remember(cache: Option<&Cache>, file: Option<u64>, page: usize, scale: f32, annotations: bool, size: [usize; 2], rgba: Vec<u8>, took_ms: u32) {
    let (Some(cache), Some(file)) = (cache, file) else { return };
    let key = Key::new(file, page, scale).annotations(annotations);
    if took_ms >= cache::SLOW_MS {
        cache.store(key, size, rgba);
    } else {
        cache.mark_fast(key);
    }
}

/// Reads what helper `helper` sends: textures and replies go to the UI, slow
/// pages to the cache, and the scheduler hears when the helper is free again.
fn read_events(
    helper: usize,
    stdout: ChildStdout,
    assignment: Arc<Mutex<Option<Assignment>>>,
    inputs: Sender<Input>,
    ctx: egui::Context,
    replies: Sender<Reply>,
    cache: Option<Arc<Cache>>,
) {
    let current = move |id: u64| assignment.lock().unwrap_or_else(|e| e.into_inner()).filter(|a| a.id == id);
    let run = move || {
        let mut from_helper = BufReader::new(stdout);
        loop {
            let event = match Event::read(&mut from_helper) {
                Ok(event) => event,
                Err(_) => {
                    trace(format_args!("pool: helper {helper} has gone"));
                    let _ = inputs.send(Input::Exited { helper });
                    return;
                }
            };
            match event {
                Event::Opened { version, error } => {
                    if let Some(error) = &error {
                        trace(format_args!("pool: helper {helper} could not open the file: {error}"));
                    }
                    let _ = inputs.send(Input::Opened { helper, version, ok: error.is_none() });
                }

                Event::Image { id, complete, size, took_ms, rgba } => {
                    let Some(a) = current(id) else { continue };
                    let size = [size[0] as usize, size[1] as usize];
                    let page = a.page;
                    let slow = took_ms >= cache::SLOW_MS;
                    match (a.kind, a.target) {
                        (Kind::Wanted | Kind::Ahead, Target::Page { scale, annotations }) => {
                            let name = if complete { format!("page-{page}") } else { format!("page-{page}-drawing") };
                            let texture = make_texture(&ctx, name, size, &rgba);
                            let reply =
                                Reply::Rendered { generation: a.generation, page, scale, texture, complete, slow: complete && slow, annotations };
                            send(&replies, &ctx, reply);
                            if complete {
                                remember(cache.as_deref(), a.file, page, scale, annotations, size, rgba, took_ms);
                            }
                        }
                        (Kind::Wanted | Kind::Ahead, Target::Region { full, region, annotations }) => {
                            if complete {
                                // Every square is kept for next time: stored well,
                                // they cost little, and plain paper almost nothing.
                                let keep = cache.as_deref().zip(a.file);
                                let tiles = make_tiles(&ctx, page, full, region, annotations, size, &rgba, keep);
                                send(&replies, &ctx, Reply::RenderedRegion { generation: a.generation, page, full, region, annotations, tiles });
                            }
                        }
                        (Kind::Background, Target::Page { scale, annotations }) => {
                            if complete {
                                remember(cache.as_deref(), a.file, page, scale, annotations, size, rgba, took_ms);
                            }
                        }
                        (Kind::Background, Target::Region { .. }) => {}
                    }
                    if complete {
                        let what = match a.target {
                            Target::Page { .. } => "the whole of",
                            Target::Region { .. } => "the area in view of",
                        };
                        trace(format_args!("pool: helper {helper} drew {what} page {page} ({:?}) in {took_ms} ms", a.kind));
                        let _ = inputs.send(Input::Finished { helper, id, stopped: false });
                    }
                }

                Event::Stopped { id, error } => {
                    if let Some(a) = current(id).filter(|a| a.kind != Kind::Background) {
                        let reply = match (a.target, &error) {
                            (Target::Page { .. }, Some(error)) => {
                                trace(format_args!("pool: helper {helper} failed on page {}: {error}", a.page));
                                Reply::RenderFailed { generation: a.generation, page: a.page, error: error.clone() }
                            }
                            _ => skipped(a.generation, a.page, a.target),
                        };
                        send(&replies, &ctx, reply);
                    }
                    // A page that failed isn't retried; one that was stopped is.
                    let _ = inputs.send(Input::Finished { helper, id, stopped: error.is_none() });
                }
            }
        }
    };
    if let Err(e) = std::thread::Builder::new().name(format!("render helper {helper}")).spawn(run) {
        trace(format_args!("pool: could not start the thread for helper {helper}: {e}"));
    }
}

/// Starts the thread that reads cached pages back, so the scheduler never
/// waits on the disk. A page that doesn't read back goes back to be drawn.
fn read_cached(
    cache: Arc<Cache>,
    ctx: egui::Context,
    replies: Sender<Reply>,
    wanted: Arc<Mutex<Wanted>>,
    inputs: Sender<Input>,
) -> Option<Sender<Load>> {
    let (loads, queue) = mpsc::channel::<Load>();
    let run = move || {
        for load in queue {
            let (generation, page, target) = match load {
                Load::Page { generation, page, scale, key } => (generation, page, Target::Page { scale, annotations: key.has_annotations() }),
                Load::Tiles { generation, page, full, region, annotations, .. } => (generation, page, Target::Region { full, region, annotations }),
            };
            let still_wanted = wanted.lock().map(|w| w.rank(generation, page).is_some()).unwrap_or(true);
            if !still_wanted {
                send(&replies, &ctx, skipped(generation, page, target));
                continue;
            }
            let found = match load {
                Load::Page { scale, key, .. } => cache.load(key).map(|(size, rgba)| {
                    let texture = make_texture(&ctx, format!("page-{page}"), size, &rgba);
                    Reply::Rendered { generation, page, scale, texture, complete: true, slow: true, annotations: key.has_annotations() }
                }),
                Load::Tiles { full, region, annotations, file, .. } => load_tiles(&ctx, &cache, file, page, full, region, annotations)
                    .map(|tiles| Reply::RenderedRegion { generation, page, full, region, annotations, tiles }),
            };
            match found {
                Some(reply) => {
                    send(&replies, &ctx, reply);
                    trace(format_args!("pool: page {page} came from the cache"));
                }
                None => {
                    let _ = inputs.send(Input::Draw { generation, page, target });
                }
            }
        }
    };
    std::thread::Builder::new().name("page cache reader".into()).spawn(run).ok().map(|_| loads)
}

struct Scheduler {
    slots: Vec<Slot>,
    inputs: Receiver<Input>,
    wanted: Arc<Mutex<Wanted>>,
    replies: Sender<Reply>,
    ctx: egui::Context,
    fallback: Sender<Request>,
    cache: Option<Arc<Cache>>,
    loads: Option<Sender<Load>>,
    alive: Arc<AtomicBool>,
    generation: u64,
    path: Option<PathBuf>,
    /// A copy of the file the helpers draw from instead, with its stamps'
    /// lines merged (merge.rs), once there is one.
    drawn_from: Option<PathBuf>,
    /// This exe, to start the process that makes that copy, and the way it
    /// reports back.
    exe: PathBuf,
    to_self: Sender<Input>,
    /// Whether the open file might have annotations on layers that are off,
    /// which drawing from the file itself would show (merge.rs).
    layers: bool,
    /// While that copy is being made for such a file, nothing drawn from the
    /// file itself is kept in the cache; and until `COPY_WAIT` after this, the
    /// helpers wait for the copy rather than draw.
    unconfirmed: bool,
    copy_waiting: Option<Instant>,
    /// The open file's fingerprint, once the worker has sent it.
    file: Option<u64>,
    /// Goes up each time the helpers are told to open the file.
    version: u64,
    queue: Vec<Queued>,
    /// Renders asked for ahead of a zoom, oldest first.
    predicted: Vec<Queued>,
    ahead: Ahead,
    next_id: u64,
}

impl Scheduler {
    fn run(mut self) {
        loop {
            // While anything is drawing or waiting, look at the view often, to
            // stop what it no longer wants; while idle with a cache, now and
            // then, to draw ahead once the view settles.
            let busy = !self.queue.is_empty() || self.slots.iter().any(|s| s.busy.is_some());
            let wait = if busy {
                Some(CHECK_EVERY)
            } else if self.cache.is_some() && self.file.is_some() {
                Some(IDLE_CHECK_EVERY)
            } else {
                None
            };
            let first = match wait {
                Some(timeout) => match self.inputs.recv_timeout(timeout) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match self.inputs.recv() {
                    Ok(input) => Some(input),
                    Err(_) => break,
                },
            };
            let inputs: Vec<Input> = first.into_iter().chain(self.inputs.try_iter()).collect();
            if !inputs.into_iter().all(|input| self.handle(input)) {
                break;
            }
            self.schedule();
        }
        for slot in &mut self.slots {
            let _ = slot.child.kill();
        }
    }

    fn queue(&mut self, generation: u64, page: usize, target: Target, cache_checked: bool) {
        if generation == self.generation {
            self.queue.retain(|q| !(q.page == page && order(q.target) == order(target)));
            self.queue.push(Queued { generation, page, target, cache_checked });
        }
    }

    /// Returns false when it's time to stop.
    fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Open { generation, path } => {
                self.generation = generation;
                self.path = Some(path);
                self.drawn_from = None;
                self.layers = false;
                self.unconfirmed = false;
                self.copy_waiting = None;
                self.file = None;
                self.ahead = Ahead::default();
                // Renders of the last file are no use now.
                self.queue.clear();
                self.predicted.clear();
                self.cancel_renders();
                self.open_everywhere();
            }

            Input::File { generation, fingerprint, layers } => {
                if generation == self.generation {
                    self.file = Some(fingerprint);
                    self.layers = layers;
                    self.use_copy(fingerprint);
                }
            }

            // A render under way finishes from the file as it was, which looks
            // the same when only highlights changed, since they aren't drawn;
            // the worker has already moved the copy to draw from to the new
            // fingerprint along with the cached pages. When markups changed,
            // renders under way are stopped, and the copy is made again.
            Input::Saved { generation, fingerprint, redrawn } if generation == self.generation => {
                self.file = Some(fingerprint);
                if redrawn {
                    self.cancel_renders();
                    self.ahead = Ahead::default();
                    self.drawn_from = None;
                } else if self.drawn_from.is_some() {
                    self.drawn_from = self.cache.as_ref().and_then(|c| c.copy(fingerprint).flatten());
                }
                self.open_everywhere();
                if redrawn {
                    self.use_copy(fingerprint);
                }
            }
            Input::Saved { .. } => {}

            Input::Copy { generation, fingerprint, path } => {
                if generation == self.generation && self.file == Some(fingerprint) {
                    self.unconfirmed = false;
                    self.copy_waiting = None;
                    if let Some(path) = path {
                        self.drawn_from = Some(path);
                        self.open_everywhere();
                    }
                }
            }

            Input::Render { generation, page, target } => self.queue(generation, page, target, false),
            Input::Draw { generation, page, target } => self.queue(generation, page, target, true),

            Input::Predict { generation, page, target } => {
                if generation == self.generation && !self.predicted.iter().any(|q| q.page == page && q.target == target) {
                    self.predicted.push(Queued { generation, page, target, cache_checked: false });
                    if self.predicted.len() > MOST_PREDICTED {
                        let old = self.predicted.remove(0);
                        send(&self.replies, &self.ctx, skipped(old.generation, old.page, old.target));
                    }
                }
            }

            Input::Opened { helper, version, ok } => {
                if version == self.version {
                    // A copy pdfium can't open is given up on for the original.
                    if !ok && self.drawn_from.is_some() {
                        trace(format_args!("pool: helper {helper} couldn't open the copy to draw from; using the file itself"));
                        self.drawn_from = None;
                        self.open_everywhere();
                        return true;
                    }
                    let slot = &mut self.slots[helper];
                    if ok {
                        slot.ready = Some(version);
                    } else {
                        slot.failed = Some(version);
                    }
                }
            }

            Input::Finished { helper, id, stopped } => {
                if self.slots[helper].busy == Some(id) {
                    // A page drawn ahead that was stopped to make way is tried again.
                    if let Some(a) = self.slots[helper].assignment().filter(|a| stopped && a.kind == Kind::Background) {
                        if let Target::Page { scale, annotations } = a.target {
                            self.ahead.tried.remove(&(a.page, scale.to_bits(), annotations));
                            self.ahead.step = 0;
                        }
                    }
                    let slot = &mut self.slots[helper];
                    slot.busy = None;
                    slot.cancelled = false;
                    slot.set_assignment(None);
                }
            }

            Input::Exited { helper } => {
                let slot = &mut self.slots[helper];
                slot.alive = false;
                // Whatever it was drawing for the view, the UI can ask for again.
                if let Some(a) = slot.assignment().filter(|a| a.kind != Kind::Background) {
                    send(&self.replies, &self.ctx, skipped(a.generation, a.page, a.target));
                }
                slot.busy = None;
                slot.set_assignment(None);
            }

            Input::Shutdown => return false,
        }
        true
    }

    /// The fingerprint to keep what's drawn under in the cache: none while a
    /// file that might have annotations on layers that are off is still drawn
    /// from itself, since those images could show them.
    fn kept_file(&self) -> Option<u64> {
        self.file.filter(|_| !self.unconfirmed)
    }

    /// Has the helpers draw from a copy of the file with its stamps' lines
    /// merged: the cache's, if it has one, or one made now by a process of its
    /// own, which says when it's done.
    fn use_copy(&mut self, fingerprint: u64) {
        let (Some(cache), Some(source)) = (self.cache.clone(), self.path.clone()) else { return };
        if std::env::var_os("KINETIC_PDF_MERGE").is_some_and(|v| v == "0") {
            return;
        }
        match cache.copy(fingerprint) {
            Some(Some(path)) => {
                trace(format_args!("pool: drawing from the merged copy in the cache"));
                self.drawn_from = Some(path);
                self.open_everywhere();
            }
            Some(None) => {}
            None => {
                if self.layers {
                    self.unconfirmed = true;
                    self.copy_waiting = Some(Instant::now());
                }
                let (exe, to_self, generation) = (self.exe.clone(), self.to_self.clone(), self.generation);
                let make = move || {
                    let started = Instant::now();
                    let status = background(&exe)
                        .arg(merge::FLAG)
                        .arg(&source)
                        .arg(format!("{fingerprint:x}"))
                        .arg(cache.copy_path(fingerprint))
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .status();
                    let made = status.as_ref().is_ok_and(|s| s.success());
                    if made {
                        cache.adopt_copy(fingerprint);
                    }
                    let path = if made { cache.copy(fingerprint).flatten() } else { None };
                    trace(format_args!(
                        "pool: {} in {:.0} ms",
                        match (&path, made) {
                            (Some(_), _) => "made a copy with the stamps' lines merged",
                            (None, true) => "found nothing to merge",
                            (None, false) => "couldn't make a merged copy",
                        },
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                    let _ = to_self.send(Input::Copy { generation, fingerprint, path });
                };
                if let Err(e) = std::thread::Builder::new().name("merged copy".into()).spawn(make) {
                    trace(format_args!("pool: could not start making a merged copy: {e}"));
                }
            }
        }
    }

    /// Stops every render under way.
    fn cancel_renders(&mut self) {
        for (helper, slot) in self.slots.iter_mut().enumerate() {
            if let (Some(id), false) = (slot.busy, slot.cancelled) {
                slot.command(helper, Command::Cancel { id });
                slot.cancelled = true;
            }
        }
    }

    fn open_everywhere(&mut self) {
        let Some(path) = self.drawn_from.clone().or_else(|| self.path.clone()) else { return };
        self.version += 1;
        let version = self.version;
        for (helper, slot) in self.slots.iter_mut().enumerate().filter(|(_, s)| s.alive) {
            slot.ready = None;
            slot.failed = None;
            slot.last_page = None;
            slot.command(helper, Command::Open { version, path: path.clone() });
        }
    }

    fn schedule(&mut self) {
        let alive = self.slots.iter().any(|s| s.alive);
        self.alive.store(alive, Ordering::Relaxed);
        // With no helper able to draw this file, the worker draws.
        if !self.slots.iter().any(|s| s.alive && s.failed != Some(self.version)) {
            for queued in self.queue.drain(..) {
                let _ = self.fallback.send(queued.into_request());
            }
            return;
        }

        // A file that might have annotations on layers that are off waits a
        // moment for its copy to draw from, rather than show them.
        if let Some(since) = self.copy_waiting {
            if since.elapsed() < COPY_WAIT {
                return;
            }
            trace(format_args!("pool: no copy to draw from after {COPY_WAIT:?}; drawing from the file itself"));
            self.copy_waiting = None;
        }

        let (wanted_generation, wanted_pages, moving, scales, without_annotations, drawn_whole, skip_drawing_ahead) = {
            let w = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            let sets = (Arc::clone(&w.without_annotations), Arc::clone(&w.drawn_whole));
            (w.generation, w.pages.clone(), w.moving, Arc::clone(&w.render_scales), sets.0, sets.1, w.skip_drawing_ahead)
        };
        let rank = |generation: u64, page: usize| {
            if generation == wanted_generation {
                wanted_pages.iter().position(|&p| p == page)
            } else {
                None
            }
        };

        // Stop renders for pages that have left the view.
        for (helper, slot) in self.slots.iter_mut().enumerate() {
            let (Some(id), false, true) = (slot.busy, slot.cancelled, slot.alive) else { continue };
            if slot.assignment().is_some_and(|a| a.kind != Kind::Background && rank(a.generation, a.page).is_none()) {
                trace(format_args!("pool: helper {helper}'s page is no longer wanted, stopping it"));
                slot.command(helper, Command::Cancel { id });
                slot.cancelled = true;
            }
        }

        // While the view moves -- scrolling fast, or a zoom settling -- drawing
        // ahead stops, so the helpers are free by the time it lands. Loading a
        // dense page can't be stopped part-way, so waiting until then to stop
        // them held up the page it landed on by most of a second.
        if moving {
            for (helper, slot) in self.slots.iter_mut().enumerate() {
                let (Some(id), false, true) = (slot.busy, slot.cancelled, slot.alive) else { continue };
                if slot.assignment().is_some_and(|a| a.kind != Kind::Wanted) {
                    trace(format_args!("pool: the view is moving, stopping helper {helper}'s drawing ahead"));
                    slot.command(helper, Command::Cancel { id });
                    slot.cancelled = true;
                }
            }
        }

        // Drop what's waiting for pages no longer wanted.
        let (keep, unwanted): (Vec<Queued>, Vec<Queued>) = self.queue.drain(..).partition(|q| rank(q.generation, q.page).is_some());
        self.queue = keep;
        for q in unwanted {
            send(&self.replies, &self.ctx, skipped(q.generation, q.page, q.target));
        }

        // With a cache, nothing is drawn until the file's fingerprint is
        // known, so a cached page is never drawn again; then pages already
        // cached go to be read back instead.
        if let Some(cache) = &self.cache {
            let Some(file) = self.file else { return };
            for mut q in std::mem::take(&mut self.queue) {
                if !q.cache_checked {
                    if let Some(load) = cached_load(cache, file, q.generation, q.page, q.target) {
                        if self.loads.as_ref().is_some_and(|loads| loads.send(load).is_ok()) {
                            continue;
                        }
                    }
                    q.cache_checked = true;
                }
                self.queue.push(q);
            }
        }

        // What the view wants comes before drawing ahead: stop enough
        // background renders to free a helper for each waiting render.
        let version = self.version;
        let free = self.slots.iter().filter(|s| s.alive && s.busy.is_none() && s.ready == Some(version)).count();
        let freeing = self.slots.iter().filter(|s| s.alive && s.busy.is_some() && s.cancelled).count();
        let mut to_free = self.queue.len().saturating_sub(free + freeing);
        for (helper, slot) in self.slots.iter_mut().enumerate() {
            if to_free == 0 {
                break;
            }
            let (Some(id), false, true) = (slot.busy, slot.cancelled, slot.alive) else { continue };
            if slot.assignment().is_some_and(|a| a.kind != Kind::Wanted) {
                trace(format_args!("pool: stopping helper {helper}'s drawing ahead for the view"));
                slot.command(helper, Command::Cancel { id });
                slot.cancelled = true;
                to_free -= 1;
            }
        }

        // Hand the most wanted to free helpers -- to the one that drew the page
        // last if it's free, since it still has the page open.
        let free = |s: &Slot| s.alive && s.busy.is_none() && s.ready == Some(version);
        while !self.queue.is_empty() && self.slots.iter().any(free) {
            let best = (0..self.queue.len())
                .min_by_key(|&i| (rank(self.queue[i].generation, self.queue[i].page), order(self.queue[i].target)))
                .unwrap_or(0);
            let q = self.queue.remove(best);
            let helper = self
                .slots
                .iter()
                .position(|s| free(s) && s.last_page == Some(q.page))
                .or_else(|| self.slots.iter().position(free))
                .unwrap_or(0);
            let assignment = Assignment { id: self.next_id, generation: q.generation, page: q.page, target: q.target, kind: Kind::Wanted, file: self.kept_file() };
            self.next_id += 1;
            trace(format_args!("pool: helper {helper} draws page {} ({} waiting)", q.page, self.queue.len()));
            self.assign(helper, assignment);
        }

        // Then what's asked for ahead of a zoom.
        if self.queue.is_empty() && !moving {
            self.predict(&wanted_pages, wanted_generation);
        }

        // With nothing else to do, draw the document ahead into the cache.
        if self.queue.is_empty() && self.predicted.is_empty() && !moving && !skip_drawing_ahead && wanted_generation == self.generation {
            if let (Some(cache), Some(file)) = (self.cache.clone(), self.kept_file()) {
                self.draw_ahead(&cache, file, &wanted_pages, scales, without_annotations, drawn_whole);
            }
        }
    }

    /// Gives free helpers what's asked for ahead of a zoom, oldest first --
    /// from the cache when it's all there. Anything for a page that has left
    /// the view is dropped, with a reply so the UI can ask again.
    fn predict(&mut self, wanted: &[usize], wanted_generation: u64) {
        let version = self.version;
        let free = |s: &Slot| s.alive && s.busy.is_none() && s.ready == Some(version);
        while !self.predicted.is_empty() {
            let q = self.predicted.remove(0);
            if q.generation != self.generation || wanted_generation != self.generation || !wanted.contains(&q.page) {
                send(&self.replies, &self.ctx, skipped(q.generation, q.page, q.target));
                continue;
            }
            if let (Some(cache), Some(file), Some(loads)) = (&self.cache, self.file, &self.loads) {
                if let Some(load) = cached_load(cache, file, q.generation, q.page, q.target) {
                    if loads.send(load).is_ok() {
                        continue;
                    }
                }
            }
            let Some(helper) = self.slots.iter().position(|s| free(s) && s.last_page == Some(q.page)).or_else(|| self.slots.iter().position(free))
            else {
                self.predicted.insert(0, q);
                break;
            };
            let assignment = Assignment { id: self.next_id, generation: q.generation, page: q.page, target: q.target, kind: Kind::Ahead, file: self.kept_file() };
            self.next_id += 1;
            trace(format_args!("pool: helper {helper} draws page {} ahead of a zoom", q.page));
            self.assign(helper, assignment);
        }
    }

    fn assign(&mut self, helper: usize, assignment: Assignment) {
        let slot = &mut self.slots[helper];
        slot.set_assignment(Some(assignment));
        slot.busy = Some(assignment.id);
        slot.last_page = Some(assignment.page);
        slot.command(helper, Command::Render { id: assignment.id, page: assignment.page as u32, target: assignment.target });
    }

    /// Gives free helpers the nearest pages to the view that aren't cached yet.
    fn draw_ahead(
        &mut self,
        cache: &Cache,
        file: u64,
        wanted: &[usize],
        scales: Arc<Vec<f32>>,
        without_annotations: Arc<HashSet<usize>>,
        drawn_whole: Arc<HashSet<usize>>,
    ) {
        if scales.is_empty() {
            return;
        }
        let anchor = wanted.first().copied().unwrap_or(0).min(scales.len() - 1);
        let same = |seen: &Option<Arc<HashSet<usize>>>, now: &Arc<HashSet<usize>>| seen.as_ref().is_some_and(|s| Arc::ptr_eq(s, now));
        let same_scales = self.ahead.scales.as_ref().is_some_and(|s| Arc::ptr_eq(s, &scales));
        let same_sets = same(&self.ahead.without_annotations, &without_annotations) && same(&self.ahead.drawn_whole, &drawn_whole);
        if self.ahead.anchor != anchor || !same_scales || !same_sets || self.ahead.file != Some(file) {
            self.ahead.anchor = anchor;
            self.ahead.scales = Some(Arc::clone(&scales));
            self.ahead.without_annotations = Some(Arc::clone(&without_annotations));
            self.ahead.drawn_whole = Some(Arc::clone(&drawn_whole));
            self.ahead.file = Some(file);
            self.ahead.step = 0;
        }
        let version = self.version;
        let free = |s: &Slot| s.alive && s.busy.is_none() && s.ready == Some(version);
        // Helpers holding a page in view open are left for the view if possible.
        let pick = |slots: &[Slot]| {
            slots
                .iter()
                .position(|s| free(s) && !s.last_page.is_some_and(|p| wanted.contains(&p)))
                .or_else(|| slots.iter().position(free))
        };
        while let Some(helper) = pick(&self.slots) {
            let Some((page, scale, annotations)) = self.next_ahead(cache, file, wanted, &scales, &without_annotations, &drawn_whole) else { break };
            let target = Target::Page { scale, annotations };
            let assignment = Assignment { id: self.next_id, generation: self.generation, page, target, kind: Kind::Background, file: Some(file) };
            self.next_id += 1;
            trace(format_args!("pool: helper {helper} draws page {page} ahead, into the cache"));
            self.assign(helper, assignment);
        }
    }

    /// The nearest page to the anchor that isn't wanted, drawn whole by the
    /// app, cached, known to be quick, or already tried: with its scale, and
    /// whether pdfium draws its annotations.
    fn next_ahead(
        &mut self,
        cache: &Cache,
        file: u64,
        wanted: &[usize],
        scales: &[f32],
        without_annotations: &HashSet<usize>,
        drawn_whole: &HashSet<usize>,
    ) -> Option<(usize, f32, bool)> {
        let n = scales.len();
        let anchor = self.ahead.anchor;
        while self.ahead.step < 2 * n {
            let step = self.ahead.step;
            self.ahead.step += 1;
            let distance = (step + 1) / 2;
            let page = if step % 2 == 1 { Some(anchor + distance) } else { anchor.checked_sub(distance) };
            let Some(page) = page.filter(|&p| p < n) else { continue };
            let (scale, annotations) = (scales[page], !without_annotations.contains(&page));
            if wanted.contains(&page) || drawn_whole.contains(&page) || !self.ahead.tried.insert((page, scale.to_bits(), annotations)) {
                continue;
            }
            let key = Key::new(file, page, scale).annotations(annotations);
            if cache.has_image(key) || cache.is_fast(key) {
                continue;
            }
            return Some((page, scale, annotations));
        }
        None
    }
}
