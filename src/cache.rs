//! A disk cache of drawn pages, and of the squares of pages drawn zoomed in.
//!
//! A page carrying hundreds of thousands of drawing objects -- a dense drawing,
//! or a Bluebeam overlay of stamps -- takes pdfium one to two seconds to draw
//! at almost any size, and nothing in pdfium makes that faster. So once such a
//! page has been drawn its image is kept on disk, and the next time it's wanted
//! at that size, in this session or a later one, it comes back in tens of
//! milliseconds. Squares of the zoomed-in view (see `model::TILE`) are all
//! kept, however quickly they drew: stored well they cost very little.
//!
//! Entries are keyed by a fingerprint of the file's contents, the page and the
//! drawing size. The same file under another name or in another folder finds
//! its pages; a file changed by anything else simply stops matching, so a stale
//! page is never shown. Saving notes is the one change the app makes itself,
//! and highlights are never part of a drawn page, so after a save the file's
//! entries move to its new fingerprint (`rekey`).
//!
//! Images are stored as PNG -- RGB, or a palette where an image has 256
//! colours or fewer, with PNG's per-row prediction -- which on drawings is
//! 3-8 times smaller than the compressed pixels stored before, and far smaller
//! again for a square of plain paper, which is stored as its one colour. Whole
//! pages and squares each keep to their own share of the size limit, deleting
//! those used longest ago, so neither crowds out the other.
//!
//! A page that turns out to be quick to draw isn't worth the disk space. A
//! small marker records that instead, so it isn't drawn again just to find out.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

/// A whole page that took at least this long to draw is cached; a quicker one
/// gets a marker instead.
pub const SLOW_MS: u32 = 150;

/// The disk space the cache keeps within.
pub const DEFAULT_LIMIT: u64 = 1024 * 1024 * 1024;

/// The share of the limit kept for squares of pages drawn zoomed in; whole
/// pages get the rest.
const TILE_SHARE: f64 = 0.6;

/// Images waiting to be written, past which new ones are skipped rather than
/// queued: keeping every square isn't worth unbounded memory while scrolling
/// deep in.
const MOST_QUEUED: usize = 256 * 1024 * 1024;

/// Threads compressing and writing images.
const WRITERS: usize = 2;

/// How whole pages and squares are stored, chosen by measuring each format on
/// real drawings (floors `cache-formats`).
///
/// Squares are small and many, and written in the background, so they're
/// compressed hard: at 800% a block of 64 took 0.17-3.95 MB as balanced PNG
/// against 0.46-7.93 MB as zlib, read back just as fast (0.4-1.5 ms each).
/// Whole pages are written once but large, so they're compressed fast: read
/// back in 15-17 ms against zlib's 11-29 ms, and 10-15% smaller.
const PAGE_FORMAT: Format = Format::PngFast;
const TILE_FORMAT: Format = Format::PngBalanced;

const ZLIB_MAGIC: &[u8; 8] = b"PDFAPG01";
const SOLID_MAGIC: &[u8; 8] = b"PDFAPG02";
const PNG_SIGNATURE: &[u8; 8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
// The 2 marks what's drawn since annotations on layers that are off stopped
// being drawn (see merge.rs): anything cached before could show them, so the
// old names are deleted when the cache opens.
const PAGE_EXTENSION: &str = "page2";
const TILE_EXTENSION: &str = "tile2";
const FAST_EXTENSION: &str = "fast2";
/// The shapes a page was read into for the GPU (see crates/gpu-lines), zlib
/// squeezed. They count against the whole pages' share of the limit, which
/// suits them: a page the GPU draws needs no image of itself.
const SHAPE_EXTENSION: &str = "shape1";
/// A copy of a file made for drawing, with annotations on layers that are off
/// taken out and its stamps' lines merged (see merge.rs); empty when there was
/// nothing to change.
const COPY_EXTENSION: &str = "copy2";
const OLD_EXTENSIONS: [&str; 4] = ["page", "tile", "fast", "copy"];

fn copy_name(file: u64) -> String {
    format!("{file:016x}-drawn.{COPY_EXTENSION}")
}

/// Where a page's shapes for the GPU are kept, at the density its images were
/// kept at. Named as images are, so a save moves them with the rest
/// (`rekey`) and drops those of pages it changed (`forget_drawn`).
fn shapes_name(file: u64, page: usize, density: f32) -> String {
    format!("{file:016x}-{page}-{:08x}-shapes.{SHAPE_EXTENSION}", density.to_bits())
}

/// No stored image is ever wider or taller than this; a damaged file claiming
/// otherwise mustn't ask for gigabytes.
const LARGEST_SIDE: usize = 1 << 15;

/// How an image is stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    /// RGBA squeezed with fast zlib: quick, but large. What the cache stored
    /// before PNG, and still reads.
    Zlib,
    /// PNG: RGB, or a palette where the image has 256 colours or fewer, with
    /// per-row prediction, compressed fast.
    PngFast,
    /// The same, compressed harder.
    PngBalanced,
}

/// The cache's usual home: the per-user cache folder the OS provides.
pub fn default_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("kinetic-pdf").join("pages")
}

/// A fingerprint of a file's contents. Rust's built-in hasher is stable for a
/// given build of the app; a new build that hashes differently only costs
/// cache misses.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// One drawn page, or one square of a page drawn zoomed in: which file, which
/// page, at what size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub file: u64,
    pub page: usize,
    scale_bits: u32,
    /// For a square: the page's full size in pixels, and the square's column
    /// and row (see `model::TILE`).
    tile: Option<[u32; 4]>,
    /// A small image of the whole page, kept whatever the zoom.
    thumbnail: bool,
    /// Whether pdfium drew the page's annotations, as it does unless the app
    /// draws them itself.
    annotations: bool,
}

impl Key {
    /// A whole page drawn at `scale`, with its annotations.
    pub fn new(file: u64, page: usize, scale: f32) -> Self {
        Key { file, page, scale_bits: scale.to_bits(), tile: None, thumbnail: false, annotations: true }
    }

    /// One square of a page drawn `full` pixels in size, with its annotations.
    pub fn tile(file: u64, page: usize, full: [u32; 2], column: u32, row: u32) -> Self {
        Key { file, page, scale_bits: 0, tile: Some([full[0], full[1], column, row]), thumbnail: false, annotations: true }
    }

    /// A small image of a page, kept for showing it while whatever draws it
    /// properly is on its way.
    pub fn thumbnail(file: u64, page: usize) -> Self {
        Key { file, page, scale_bits: 0, tile: None, thumbnail: true, annotations: true }
    }

    /// The same drawing, with the page's annotations drawn or not.
    pub fn annotations(self, drawn: bool) -> Self {
        Key { annotations: drawn, ..self }
    }

    pub fn has_annotations(&self) -> bool {
        self.annotations
    }

    /// What tells a drawing without its annotations apart in its name.
    fn bare(&self) -> &'static str {
        if self.annotations {
            ""
        } else {
            "-bare"
        }
    }

    fn image_name(&self) -> String {
        let bare = self.bare();
        match self.tile {
            None if self.thumbnail => format!("{:016x}-{}-thumb.{PAGE_EXTENSION}", self.file, self.page),
            None => format!("{:016x}-{}-{:08x}{bare}.{PAGE_EXTENSION}", self.file, self.page, self.scale_bits),
            Some([w, h, column, row]) => format!("{:016x}-{}-{w}x{h}-{column}_{row}{bare}.{TILE_EXTENSION}", self.file, self.page),
        }
    }

    fn fast_name(&self) -> String {
        format!("{:016x}-{}{}.{FAST_EXTENSION}", self.file, self.page, self.bare())
    }
}

fn is_tile(name: &str) -> bool {
    name.ends_with(TILE_EXTENSION)
}

struct Entry {
    bytes: u64,
    used: SystemTime,
}

#[derive(Default)]
struct Index {
    entries: HashMap<String, Entry>,
    /// Disk space used by whole pages (and markers), and by squares.
    pages: u64,
    tiles: u64,
}

impl Index {
    fn total(&mut self, tile: bool) -> &mut u64 {
        if tile {
            &mut self.tiles
        } else {
            &mut self.pages
        }
    }

    fn insert(&mut self, name: String, bytes: u64, used: SystemTime) {
        let tile = is_tile(&name);
        let old = self.entries.insert(name, Entry { bytes, used }).map_or(0, |e| e.bytes);
        let total = self.total(tile);
        *total = *total - old + bytes;
    }

    fn remove(&mut self, name: &str) -> Option<Entry> {
        let entry = self.entries.remove(name)?;
        *self.total(is_tile(name)) -= entry.bytes;
        Some(entry)
    }
}

enum Job {
    Store { key: Key, size: [usize; 2], rgba: Vec<u8> },
    StoreShapes { name: String, shapes: Vec<u8> },
    MarkFast(Key),
    Clear,
}

struct Shared {
    dir: PathBuf,
    page_limit: u64,
    tile_limit: u64,
    index: Mutex<Index>,
    /// Jobs sent and not yet done, for `flush`.
    pending: AtomicUsize,
    /// Bytes of images waiting to be written; see `MOST_QUEUED`.
    queued: AtomicUsize,
}

/// The page cache. Reads happen on the caller's thread; writes queue for
/// threads of their own, so drawing never waits on the disk.
pub struct Cache {
    shared: Arc<Shared>,
    jobs: Sender<Job>,
}

impl Cache {
    /// Opens the cache in `dir`, creating it if needed, keeping it within
    /// `limit` bytes shared between whole pages and squares.
    pub fn open(dir: impl Into<PathBuf>, limit: u64) -> io::Result<Cache> {
        let tiles = (limit as f64 * TILE_SHARE) as u64;
        Cache::open_with_limits(dir, limit - tiles, tiles)
    }

    /// `open`, with separate limits for whole pages and for squares.
    pub fn open_with_limits(dir: impl Into<PathBuf>, page_limit: u64, tile_limit: u64) -> io::Result<Cache> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let started = std::time::Instant::now();
        let mut index = Index::default();
        for entry in fs::read_dir(&dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if name.ends_with(".tmp") {
                // Left by a write that never finished.
                let _ = fs::remove_file(entry.path());
            } else if name.rsplit('.').next().is_some_and(|ext| OLD_EXTENSIONS.contains(&ext)) {
                let _ = fs::remove_file(entry.path());
            } else if [PAGE_EXTENSION, TILE_EXTENSION, FAST_EXTENSION, COPY_EXTENSION, SHAPE_EXTENSION].iter().any(|ext| name.ends_with(ext)) {
                index.insert(name, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
            }
        }
        crate::worker::trace(format_args!(
            "cache: listed {} entries in {:.1} ms",
            index.entries.len(),
            started.elapsed().as_secs_f64() * 1000.0
        ));
        let shared = Arc::new(Shared {
            dir,
            page_limit,
            tile_limit,
            index: Mutex::new(index),
            pending: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
        });
        let (jobs, queue) = mpsc::channel();
        let queue: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(queue));
        for n in 0..WRITERS {
            let (queue, writer) = (Arc::clone(&queue), Arc::clone(&shared));
            std::thread::Builder::new().name(format!("page cache writer {n}")).spawn(move || loop {
                let job = queue.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok(job) = job else { return };
                writer.run(job);
                writer.pending.fetch_sub(1, Ordering::Relaxed);
            })?;
        }
        Ok(Cache { shared, jobs })
    }

    pub fn has_image(&self, key: Key) -> bool {
        self.shared.index().entries.contains_key(&key.image_name())
    }

    /// Whether the page is known to draw quickly, so isn't worth caching.
    pub fn is_fast(&self, key: Key) -> bool {
        self.shared.index().entries.contains_key(&key.fast_name())
    }

    /// The image, if it's cached and reads back intact.
    pub fn load(&self, key: Key) -> Option<([usize; 2], Vec<u8>)> {
        let name = key.image_name();
        if !self.shared.index().entries.contains_key(&name) {
            return None;
        }
        let path = self.shared.dir.join(&name);
        match fs::read(&path).and_then(|data| decode(&data)) {
            Ok(image) => {
                self.shared.touch(&name, &path);
                Some(image)
            }
            Err(_) => {
                self.shared.forget(&name);
                None
            }
        }
    }

    /// Keeps an image. It's written in the background -- or not at all, if too
    /// much is already waiting to be written.
    pub fn store(&self, key: Key, size: [usize; 2], rgba: Vec<u8>) {
        let bytes = rgba.len();
        if self.shared.queued.load(Ordering::Relaxed) + bytes > MOST_QUEUED {
            return;
        }
        let queued = self.shared.queued.fetch_add(bytes, Ordering::Relaxed) + bytes;
        {
            // Traced whenever the backlog moves by 32 MB.
            static LAST_STEP: AtomicUsize = AtomicUsize::new(0);
            let step = queued >> 25;
            if LAST_STEP.swap(step, Ordering::Relaxed) != step {
                crate::worker::trace(format_args!("cache: {} MB of images waiting to be written", queued >> 20));
            }
        }
        if !self.send(Job::Store { key, size, rgba }) {
            self.shared.queued.fetch_sub(bytes, Ordering::Relaxed);
        }
    }

    /// The shapes kept for a page, as `gpu_lines::Shapes::from_bytes` reads
    /// them; `None` if there are none, or they don't read back.
    pub fn load_shapes(&self, file: u64, page: usize, density: f32) -> Option<Vec<u8>> {
        let name = shapes_name(file, page, density);
        if !self.shared.index().entries.contains_key(&name) {
            return None;
        }
        let path = self.shared.dir.join(&name);
        let read = fs::read(&path).and_then(|data| {
            let mut shapes = Vec::new();
            ZlibDecoder::new(data.as_slice()).read_to_end(&mut shapes)?;
            Ok(shapes)
        });
        match read {
            Ok(shapes) => {
                self.shared.touch(&name, &path);
                Some(shapes)
            }
            Err(_) => {
                self.shared.forget(&name);
                None
            }
        }
    }

    /// Keeps a page's shapes, squeezed and written in the background like an
    /// image -- or not at all, if too much is already waiting to be written.
    /// One page's shapes can be more than that on their own -- a sheet of a
    /// million outlined triangles comes to 279 MB -- and are kept all the same
    /// if nothing else is waiting.
    pub fn store_shapes(&self, file: u64, page: usize, density: f32, shapes: Vec<u8>) {
        let bytes = shapes.len();
        let queued = self.shared.queued.load(Ordering::Relaxed);
        if queued > 0 && queued + bytes > MOST_QUEUED {
            return;
        }
        self.shared.queued.fetch_add(bytes, Ordering::Relaxed);
        if !self.send(Job::StoreShapes { name: shapes_name(file, page, density), shapes }) {
            self.shared.queued.fetch_sub(bytes, Ordering::Relaxed);
        }
    }

    /// Whether a page's shapes are kept, without reading them.
    pub fn has_shapes(&self, file: u64, page: usize, density: f32) -> bool {
        self.shared.index().entries.contains_key(&shapes_name(file, page, density))
    }

    /// Notes that the page draws quickly.
    pub fn mark_fast(&self, key: Key) {
        self.send(Job::MarkFast(key));
    }

    /// Moves a file's entries to a new fingerprint, after a save that changed
    /// nothing drawn. Done straight away, not queued, so the saved file's pages
    /// are found as soon as this returns.
    pub fn rekey(&self, old: u64, new: u64) {
        if old == new {
            return;
        }
        let (from, to) = (format!("{old:016x}-"), format!("{new:016x}-"));
        let names: Vec<String> = self.shared.index().entries.keys().filter(|n| n.starts_with(&from)).cloned().collect();
        for name in names {
            let renamed = name.replacen(&from, &to, 1);
            if fs::rename(self.shared.dir.join(&name), self.shared.dir.join(&renamed)).is_ok() {
                let mut index = self.shared.index();
                if let Some(entry) = index.remove(&name) {
                    index.insert(renamed, entry.bytes, entry.used);
                }
            }
        }
    }

    /// Deletes what's kept of pages `pages` of file `file`, and the copy of it
    /// to draw from, after a save that changed how those pages are drawn.
    pub fn forget_drawn(&self, file: u64, pages: &BTreeSet<usize>) {
        let (prefix, copy) = (format!("{file:016x}-"), copy_name(file));
        let page_of = |name: &str| {
            let rest = name.strip_prefix(&prefix)?;
            let digits = rest.find(|c: char| !c.is_ascii_digit())?;
            rest[..digits].parse::<usize>().ok()
        };
        let names: Vec<String> =
            self.shared.index().entries.keys().filter(|name| **name == copy || page_of(name).is_some_and(|p| pages.contains(&p))).cloned().collect();
        for name in names {
            self.shared.forget(&name);
        }
    }

    /// Deletes everything.
    pub fn clear(&self) {
        self.send(Job::Clear);
    }

    /// Where the copy of file `file` made for drawing is kept (see merge.rs).
    pub fn copy_path(&self, file: u64) -> PathBuf {
        self.shared.dir.join(copy_name(file))
    }

    /// The copy of `file` made for drawing: `Some(Some(path))` if there is
    /// one, `Some(None)` if there was nothing to merge, `None` if it hasn't
    /// been made.
    pub fn copy(&self, file: u64) -> Option<Option<PathBuf>> {
        let name = copy_name(file);
        let bytes = self.shared.index().entries.get(&name).map(|e| e.bytes)?;
        if bytes == 0 {
            return Some(None);
        }
        let path = self.shared.dir.join(&name);
        self.shared.touch(&name, &path);
        Some(Some(path))
    }

    /// Takes note of a copy just written to `copy_path(file)`, empty or not,
    /// and keeps to the limit.
    pub fn adopt_copy(&self, file: u64) {
        let name = copy_name(file);
        let Ok(meta) = fs::metadata(self.shared.dir.join(&name)) else { return };
        self.shared.index().insert(name, meta.len(), SystemTime::now());
        self.shared.keep_to_limit(false);
    }

    /// Waits until everything sent so far has been written.
    pub fn flush(&self) {
        while self.shared.pending.load(Ordering::Relaxed) > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Disk space in use, whole pages and squares together.
    pub fn bytes(&self) -> u64 {
        let index = self.shared.index();
        index.pages + index.tiles
    }

    /// Disk space used by squares.
    pub fn tile_bytes(&self) -> u64 {
        self.shared.index().tiles
    }

    fn send(&self, job: Job) -> bool {
        self.shared.pending.fetch_add(1, Ordering::Relaxed);
        let sent = self.jobs.send(job).is_ok();
        if !sent {
            self.shared.pending.fetch_sub(1, Ordering::Relaxed);
        }
        sent
    }
}

impl Shared {
    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self, name: &str, path: &Path) {
        let now = SystemTime::now();
        if let Some(entry) = self.index().entries.get_mut(name) {
            entry.used = now;
        }
        // On disk too, so the order survives the app closing.
        if let Ok(file) = fs::File::options().write(true).open(path) {
            let _ = file.set_modified(now);
        }
    }

    fn forget(&self, name: &str) {
        self.index().remove(name);
        let _ = fs::remove_file(self.dir.join(name));
    }

    fn run(&self, job: Job) {
        match job {
            Job::Store { key, size, rgba } => {
                let name = key.image_name();
                let tile = key.tile.is_some();
                if !self.index().entries.contains_key(&name) {
                    let format = if tile { TILE_FORMAT } else { PAGE_FORMAT };
                    if let Ok(bytes) = write_image(&self.dir.join(&name), size, &rgba, format) {
                        self.index().insert(name, bytes, SystemTime::now());
                        self.keep_to_limit(tile);
                    }
                }
                self.queued.fetch_sub(rgba.len(), Ordering::Relaxed);
            }
            Job::StoreShapes { name, shapes } => {
                if !self.index().entries.contains_key(&name) {
                    let squeezed = || {
                        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                        encoder.write_all(&shapes)?;
                        encoder.finish()
                    };
                    if let Ok(bytes) = squeezed().and_then(|data| write_atomically(&self.dir.join(&name), &data)) {
                        self.index().insert(name, bytes, SystemTime::now());
                        self.keep_to_limit(false);
                    }
                }
                self.queued.fetch_sub(shapes.len(), Ordering::Relaxed);
            }
            Job::MarkFast(key) => {
                let name = key.fast_name();
                if !self.index().entries.contains_key(&name) && fs::write(self.dir.join(&name), []).is_ok() {
                    self.index().insert(name, 0, SystemTime::now());
                }
            }
            Job::Clear => {
                let names: Vec<String> = self.index().entries.keys().cloned().collect();
                for name in names {
                    self.forget(&name);
                }
            }
        }
    }

    /// Deletes the whole pages, or the squares, used longest ago until they
    /// are within their limit.
    fn keep_to_limit(&self, tiles: bool) {
        let limit = if tiles { self.tile_limit } else { self.page_limit };
        let mut index = self.index();
        if *index.total(tiles) <= limit {
            return;
        }
        let mut oldest_first: Vec<(SystemTime, String)> = index
            .entries
            .iter()
            .filter(|(name, entry)| is_tile(name) == tiles && entry.bytes > 0)
            .map(|(name, entry)| (entry.used, name.clone()))
            .collect();
        oldest_first.sort();
        for (_, name) in oldest_first {
            if *index.total(tiles) <= limit {
                break;
            }
            index.remove(&name);
            let _ = fs::remove_file(self.dir.join(&name));
        }
    }
}

/// Writes under a temporary name and renames into place, so a reader -- or
/// another copy of the app sharing the cache -- never sees half a file.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<u64> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!("{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let written = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, path)).map(|()| data.len() as u64);
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// An image encoded in `format` and written into place.
fn write_image(path: &Path, size: [usize; 2], rgba: &[u8], format: Format) -> io::Result<u64> {
    encode(size, rgba, format).and_then(|data| write_atomically(path, &data))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "not a cached image")
}

fn sides_ok(width: usize, height: usize) -> bool {
    (1..=LARGEST_SIDE).contains(&width) && (1..=LARGEST_SIDE).contains(&height)
}

/// An opaque RGBA image as the cache stores it in `format`. An image of one
/// colour is stored as that colour, in 20 bytes, whatever the format.
pub fn encode(size: [usize; 2], rgba: &[u8], format: Format) -> io::Result<Vec<u8>> {
    if !sides_ok(size[0], size[1]) || rgba.len() != size[0] * size[1] * 4 {
        return Err(invalid());
    }
    let header = |magic: &[u8; 8]| {
        let mut out = Vec::with_capacity(20);
        out.extend_from_slice(magic);
        out.extend_from_slice(&(size[0] as u32).to_le_bytes());
        out.extend_from_slice(&(size[1] as u32).to_le_bytes());
        out
    };
    if rgba.chunks_exact(4).all(|p| p == &rgba[..4]) {
        let mut out = header(SOLID_MAGIC);
        out.extend_from_slice(&rgba[..4]);
        return Ok(out);
    }
    match format {
        Format::Zlib => {
            let mut encoder = ZlibEncoder::new(header(ZLIB_MAGIC), flate2::Compression::fast());
            encoder.write_all(rgba)?;
            encoder.finish()
        }
        Format::PngFast => encode_png(size, rgba, png::Compression::Fast),
        Format::PngBalanced => encode_png(size, rgba, png::Compression::Balanced),
    }
}

fn encode_png(size: [usize; 2], rgba: &[u8], compression: png::Compression) -> io::Result<Vec<u8>> {
    // A palette when there are 256 colours or fewer, as on most drawings. Runs
    // of one colour are the common case, so the last colour is checked first.
    let pixels = size[0] * size[1];
    let mut lookup: HashMap<[u8; 3], u8> = HashMap::new();
    let mut palette: Vec<u8> = Vec::new();
    let mut indices: Vec<u8> = Vec::with_capacity(pixels);
    let mut last: Option<([u8; 3], u8)> = None;
    let mut fits = true;
    for p in rgba.chunks_exact(4) {
        let colour = [p[0], p[1], p[2]];
        if let Some((seen, index)) = last.filter(|(seen, _)| *seen == colour) {
            let _ = seen;
            indices.push(index);
            continue;
        }
        let index = match lookup.get(&colour) {
            Some(&index) => index,
            None if lookup.len() == 256 => {
                fits = false;
                break;
            }
            None => {
                let index = lookup.len() as u8;
                lookup.insert(colour, index);
                palette.extend_from_slice(&colour);
                index
            }
        };
        last = Some((colour, index));
        indices.push(index);
    }

    let mut out = Vec::with_capacity(pixels / 4);
    let mut encoder = png::Encoder::new(&mut out, size[0] as u32, size[1] as u32);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    encoder.set_filter(png::Filter::Adaptive);
    if fits {
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_palette(palette);
    } else {
        encoder.set_color(png::ColorType::Rgb);
    }
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    if fits {
        writer.write_image_data(&indices).map_err(io::Error::other)?;
    } else {
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        writer.write_image_data(&rgb).map_err(io::Error::other)?;
    }
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}

/// A stored image read back as RGBA, whichever format it was stored in.
pub fn decode(data: &[u8]) -> io::Result<([usize; 2], Vec<u8>)> {
    if data.starts_with(PNG_SIGNATURE) {
        let mut decoder = png::Decoder::new(io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(|_| invalid())?;
        let (width, height) = {
            let info = reader.info();
            (info.width as usize, info.height as usize)
        };
        if !sides_ok(width, height) {
            return Err(invalid());
        }
        let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(invalid)?];
        let frame = reader.next_frame(&mut buffer).map_err(|_| invalid())?;
        let pixels = width * height;
        let rgba = match frame.color_type {
            png::ColorType::Rgb if buffer.len() >= pixels * 3 => {
                let mut rgba = vec![255u8; pixels * 4];
                for (out, rgb) in rgba.chunks_exact_mut(4).zip(buffer.chunks_exact(3)) {
                    out[..3].copy_from_slice(rgb);
                }
                rgba
            }
            png::ColorType::Rgba if buffer.len() >= pixels * 4 => {
                buffer.truncate(pixels * 4);
                buffer
            }
            _ => return Err(invalid()),
        };
        return Ok(([width, height], rgba));
    }

    if data.len() < 16 {
        return Err(invalid());
    }
    let dimension = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
    let size = [dimension(8), dimension(12)];
    if !sides_ok(size[0], size[1]) {
        return Err(invalid());
    }
    let expected = size[0] * size[1] * 4;
    if &data[..8] == SOLID_MAGIC {
        let colour = data.get(16..20).ok_or_else(invalid)?;
        return Ok((size, colour.repeat(size[0] * size[1])));
    }
    if &data[..8] != ZLIB_MAGIC {
        return Err(invalid());
    }
    let mut rgba = Vec::new();
    ZlibDecoder::new(&data[16..]).take(expected as u64 + 1).read_to_end(&mut rgba)?;
    if rgba.len() != expected {
        return Err(invalid());
    }
    Ok((size, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kinetic-pdf-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Opaque pixels of random colours, which don't compress, so entries have
    /// a predictable size.
    fn noise(size: [usize; 2], seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        };
        (0..size[0] * size[1]).flat_map(|_| [next(), next(), next(), 255]).collect()
    }

    /// Opaque pixels in a handful of colours, like a drawing: white paper with
    /// a black grid, a red diagonal and a blue box.
    fn drawing(size: [usize; 2]) -> Vec<u8> {
        let (paper, ink, red, blue) = ([255, 255, 255, 255], [0, 0, 0, 255], [200, 30, 30, 255], [30, 80, 200, 255]);
        (0..size[0] * size[1])
            .flat_map(|i| {
                let (x, y) = (i % size[0], i / size[0]);
                if x == y {
                    red
                } else if x % 40 == 0 || y % 40 == 0 {
                    ink
                } else if (10..20).contains(&x) && (10..30).contains(&y) {
                    blue
                } else {
                    paper
                }
            })
            .collect()
    }

    #[test]
    fn every_format_reads_back_exactly() {
        let size = [37, 23];
        for image in [noise(size, 9), drawing(size)] {
            for format in [Format::Zlib, Format::PngFast, Format::PngBalanced] {
                let stored = encode(size, &image, format).unwrap();
                assert_eq!(decode(&stored).unwrap(), (size, image.clone()), "{format:?}");
            }
        }
        // Plain paper is stored as its colour.
        let paper = vec![255u8; size[0] * size[1] * 4];
        let stored = encode(size, &paper, Format::PngFast).unwrap();
        assert_eq!(stored.len(), 20);
        assert_eq!(decode(&stored).unwrap(), (size, paper));
        // A drawing's few colours make a palette, far smaller than the pixels.
        let stored = encode([512, 512], &drawing([512, 512]), Format::PngFast).unwrap();
        assert!(stored.len() < 512 * 512 / 4, "a 4-colour square took {} bytes", stored.len());
    }

    #[test]
    fn a_stored_page_reads_back_and_survives_reopening() {
        let dir = scratch("roundtrip");
        let key = Key::new(7, 3, 0.8);
        let rgba = noise([5, 4], 1);
        {
            let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
            assert!(!cache.has_image(key));
            cache.store(key, [5, 4], rgba.clone());
            cache.flush();
            assert!(cache.has_image(key));
            assert_eq!(cache.load(key), Some(([5, 4], rgba.clone())));
            assert_eq!(cache.load(Key::new(7, 3, 0.9)), None, "another scale is another entry");
        }
        let reopened = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        assert_eq!(reopened.load(key), Some(([5, 4], rgba)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn over_the_limit_the_page_used_longest_ago_goes() {
        let dir = scratch("limit");
        let (a, b, c) = (Key::new(1, 0, 1.0), Key::new(1, 1, 1.0), Key::new(1, 2, 1.0));
        let size = [10, 10];

        // Room for two pages of noise, not three.
        let probe = Cache::open(dir.join("probe"), DEFAULT_LIMIT).unwrap();
        probe.store(a, size, noise(size, 1));
        probe.flush();
        let one = probe.bytes();
        let room = one * 2 + one / 2;
        let cache = Cache::open_with_limits(dir.join("cache"), room, DEFAULT_LIMIT).unwrap();

        cache.store(a, size, noise(size, 1));
        cache.flush();
        std::thread::sleep(Duration::from_millis(20));
        cache.store(b, size, noise(size, 2));
        cache.flush();
        std::thread::sleep(Duration::from_millis(20));
        // Reading `a` makes `b` the one used longest ago.
        assert!(cache.load(a).is_some());
        std::thread::sleep(Duration::from_millis(20));
        cache.store(c, size, noise(size, 3));
        cache.flush();

        assert!(cache.has_image(a) && cache.has_image(c), "the recently used pages stay");
        assert!(!cache.has_image(b), "the page used longest ago goes");
        assert!(cache.bytes() <= room);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn squares_keep_to_their_own_limit_without_pushing_out_pages() {
        let dir = scratch("tile-limit");
        let size = [16, 16];
        let probe = Cache::open(dir.join("probe"), DEFAULT_LIMIT).unwrap();
        probe.store(Key::tile(1, 0, [9000, 6000], 0, 0), size, noise(size, 5));
        probe.flush();
        let one = probe.tile_bytes();

        let cache = Cache::open_with_limits(dir.join("cache"), DEFAULT_LIMIT, one * 3 + one / 2).unwrap();
        let page = Key::new(1, 0, 1.0);
        cache.store(page, size, noise(size, 4));
        cache.flush();
        for column in 0..6 {
            cache.store(Key::tile(1, 0, [9000, 6000], column, 0), size, noise(size, 10 + column));
            cache.flush();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(cache.has_image(page), "squares never push out whole pages");
        assert!(cache.tile_bytes() <= one * 3 + one / 2);
        assert!(cache.has_image(Key::tile(1, 0, [9000, 6000], 5, 0)), "the newest squares stay");
        assert!(!cache.has_image(Key::tile(1, 0, [9000, 6000], 0, 0)), "the oldest squares go");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn after_a_save_the_pages_move_to_the_new_fingerprint() {
        let dir = scratch("rekey");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        let rgba = noise([3, 3], 4);
        cache.store(Key::new(0xaaaa, 2, 1.5), [3, 3], rgba.clone());
        cache.mark_fast(Key::new(0xaaaa, 5, 1.5));
        cache.store(Key::new(0xbbbb, 2, 1.5), [3, 3], noise([3, 3], 5));
        cache.flush();

        cache.rekey(0xaaaa, 0xcccc);
        assert!(!cache.has_image(Key::new(0xaaaa, 2, 1.5)));
        assert_eq!(cache.load(Key::new(0xcccc, 2, 1.5)), Some(([3, 3], rgba)));
        assert!(cache.is_fast(Key::new(0xcccc, 5, 1.5)));
        assert!(cache.has_image(Key::new(0xbbbb, 2, 1.5)), "other files are left alone");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_save_that_changes_drawing_forgets_those_pages_and_the_copy() {
        let dir = scratch("forget");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        cache.store(Key::new(0xaaaa, 1, 1.5), [3, 3], noise([3, 3], 4));
        cache.store(Key::tile(0xaaaa, 12, [9000, 6000], 3, 1), [3, 3], noise([3, 3], 5));
        cache.mark_fast(Key::new(0xaaaa, 12, 1.5).annotations(false));
        cache.store(Key::new(0xaaaa, 2, 1.5), [3, 3], noise([3, 3], 6));
        cache.store(Key::new(0xaaaa, 120, 1.5), [3, 3], noise([3, 3], 7));
        cache.flush();
        fs::write(cache.copy_path(0xaaaa), b"%PDF-1.7").unwrap();
        cache.adopt_copy(0xaaaa);

        cache.forget_drawn(0xaaaa, &BTreeSet::from([1, 12]));
        assert!(!cache.has_image(Key::new(0xaaaa, 1, 1.5)));
        assert!(!cache.has_image(Key::tile(0xaaaa, 12, [9000, 6000], 3, 1)), "squares go too");
        assert!(!cache.is_fast(Key::new(0xaaaa, 12, 1.5).annotations(false)), "and markers");
        assert!(cache.has_image(Key::new(0xaaaa, 2, 1.5)) && cache.has_image(Key::new(0xaaaa, 120, 1.5)), "other pages stay");
        assert_eq!(cache.copy(0xaaaa), None, "the copy to draw from is made again");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_entry_is_ignored_and_forgotten() {
        let dir = scratch("damaged");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        let key = Key::new(9, 0, 1.0);
        cache.store(key, [4, 4], noise([4, 4], 6));
        cache.flush();
        fs::write(dir.join(key.image_name()), b"PDFAPG01 not really").unwrap();
        assert_eq!(cache.load(key), None);
        assert!(!cache.has_image(key));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn quick_pages_get_a_marker_and_clearing_empties_everything() {
        let dir = scratch("fast");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        let key = Key::new(3, 1, 0.5);
        cache.mark_fast(key);
        cache.store(Key::new(3, 2, 0.5), [2, 2], noise([2, 2], 7));
        cache.flush();
        assert!(cache.is_fast(key));
        assert!(cache.is_fast(Key::new(3, 1, 2.0)), "a quick page is quick at any size");
        assert!(!cache.has_image(key));

        cache.clear();
        cache.flush();
        assert!(!cache.is_fast(key) && !cache.has_image(Key::new(3, 2, 0.5)));
        assert_eq!(cache.bytes(), 0);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn squares_are_kept_apart_from_pages_and_from_each_other() {
        let dir = scratch("tiles");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        let square = Key::tile(4, 2, [9000, 6000], 3, 1);
        let rgba = noise([8, 8], 8);
        cache.store(square, [8, 8], rgba.clone());
        cache.flush();
        assert_eq!(cache.load(square), Some(([8, 8], rgba)));
        assert!(!cache.has_image(Key::tile(4, 2, [9000, 6000], 1, 3)));
        assert!(!cache.has_image(Key::tile(4, 2, [4500, 3000], 3, 1)), "another zoom is another square");
        assert!(!cache.has_image(Key::new(4, 2, 1.0)));
        cache.rekey(4, 5);
        assert!(cache.has_image(Key::tile(5, 2, [9000, 6000], 3, 1)), "squares move with their file after a save");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pages_drawn_without_their_annotations_are_kept_apart() {
        let dir = scratch("bare");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        let (drawn, bare) = (Key::new(6, 0, 1.0), Key::new(6, 0, 1.0).annotations(false));
        let bare_square = Key::tile(6, 0, [900, 600], 1, 1).annotations(false);
        cache.store(bare, [2, 2], noise([2, 2], 3));
        cache.store(bare_square, [2, 2], noise([2, 2], 4));
        cache.mark_fast(Key::new(6, 1, 1.0).annotations(false));
        cache.flush();
        assert!(cache.has_image(bare) && !cache.has_image(drawn));
        assert!(cache.has_image(bare_square) && !cache.has_image(Key::tile(6, 0, [900, 600], 1, 1)));
        assert!(cache.is_fast(Key::new(6, 1, 1.0).annotations(false)) && !cache.is_fast(Key::new(6, 1, 1.0)));
        assert!(!bare.has_annotations() && drawn.has_annotations());
        cache.rekey(6, 7);
        assert!(cache.has_image(Key::new(7, 0, 1.0).annotations(false)), "they move with their file after a save too");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_same_bytes_give_the_same_fingerprint() {
        let a = b"%PDF-1.7 some content".to_vec();
        let mut b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b[5] = b'6';
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn copies_for_drawing_are_kept_and_follow_their_file() {
        let dir = scratch("copies");
        let cache = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        assert_eq!(cache.copy(7), None);
        fs::write(cache.copy_path(7), b"%PDF-1.7 merged").unwrap();
        cache.adopt_copy(7);
        assert_eq!(cache.copy(7), Some(Some(cache.copy_path(7))));
        fs::write(cache.copy_path(8), b"").unwrap();
        cache.adopt_copy(8);
        assert_eq!(cache.copy(8), Some(None), "an empty copy means nothing to merge");
        cache.rekey(7, 9);
        assert_eq!(cache.copy(7), None);
        assert_eq!(cache.copy(9), Some(Some(cache.copy_path(9))), "a copy moves with its file after a save");
        let reopened = Cache::open(&dir, DEFAULT_LIMIT).unwrap();
        assert_eq!(reopened.copy(8), Some(None), "remembered after reopening");
        let _ = fs::remove_dir_all(dir);
    }
}
