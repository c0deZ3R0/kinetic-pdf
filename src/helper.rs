//! Render helpers: this same exe, started again by the app with
//! `--render-helper`, each with its own pdfium.
//!
//! pdfium draws one page at a time in a process, so drawing the page in view
//! and the pages ahead of it at the same time takes more processes. A helper
//! draws exactly what it is told, reading the file from disk as it needs it
//! rather than holding a copy, and closes each page as soon as it is drawn:
//! pdfium keeps a page's decoded images, often hundreds of MB for a large
//! drawing, until the page is closed. It talks to the app over its stdin and
//! stdout -- `Command`s in, `Event`s out -- and quits as soon as the app closes
//! its end, even mid-render, so a helper never outlives the app.
//!
//! The app's side of this is pool.rs.

use std::ffi::OsString;
use std::io::{self, BufReader, BufWriter, Read, Write};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use pdfium_render::prelude::*;

use crate::annots;
use crate::worker::{self, trace};

/// The argument that starts the exe as a helper instead of the app.
pub const FLAG: &str = "--render-helper";

/// How often a page that is slow to draw sends what it has drawn so far.
const PARTIAL_EVERY: Duration = Duration::from_millis(250);

/// Past this much private memory after a render, with no page kept open, the
/// helper reopens the file to let go of pdfium's document-wide caches, which
/// grow with every page it draws (about 150 MB once every page of a large
/// drawing set has been drawn).
const REOPEN_ABOVE: usize = 256 * 1024 * 1024;

/// The page last drawn stays open while the helper's private memory is under
/// this. A dense page holds 100-250 MB loaded.
const KEEP_BELOW: usize = 400 * 1024 * 1024;

/// What to draw of a page. `annotations` says whether pdfium draws the page's
/// annotations too, or leaves them for the app to draw itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// The whole page at `scale` pixels per point.
    Page { scale: f32, annotations: bool },
    /// Part of the page as drawn `full` pixels in size; see
    /// `Request::RenderRegion`.
    Region { full: [u32; 2], region: [u32; 4], annotations: bool },
}

/// App -> helper.
#[derive(Debug, PartialEq)]
pub enum Command {
    /// Open the file, closing any other. Sent again after the app saves, since
    /// saving replaces the file on disk.
    Open { version: u64, path: PathBuf },
    Render { id: u64, page: u32, target: Target },
    /// Stop render `id` if it is still going.
    Cancel { id: u64 },
}

/// Helper -> app.
#[derive(Debug, PartialEq)]
pub enum Event {
    Opened { version: u64, error: Option<String> },
    /// Render `id`'s pixels: part-drawn, or complete, which ends the render.
    /// `took_ms` is how long it has taken so far, loading the page included.
    Image { id: u64, complete: bool, size: [u32; 2], took_ms: u32, rgba: Vec<u8> },
    /// Render `id` ended without an image: it was cancelled, or it failed.
    Stopped { id: u64, error: Option<String> },
}

/* ------------------------------------------------------------------ *
 * The wire format: a tag byte, then fixed-size little-endian fields.
 * ------------------------------------------------------------------ */

fn put_u8(w: &mut impl Write, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}

fn put_u32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn put_u64(w: &mut impl Write, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn put_bytes(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    put_u32(w, bytes.len() as u32)?;
    w.write_all(bytes)
}

fn put_text(w: &mut impl Write, text: Option<&str>) -> io::Result<()> {
    match text {
        None => put_u8(w, 0),
        Some(text) => {
            put_u8(w, 1)?;
            put_bytes(w, text.as_bytes())
        }
    }
}

fn get_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn get_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// Windows paths are UTF-16 and needn't be valid Unicode, so they go as they
/// are.
#[cfg(windows)]
fn put_path(w: &mut impl Write, path: &Path) -> io::Result<()> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    put_u32(w, wide.len() as u32)?;
    wide.iter().try_for_each(|unit| w.write_all(&unit.to_le_bytes()))
}

#[cfg(windows)]
fn get_path(r: &mut impl Read) -> io::Result<PathBuf> {
    let len = get_u32(r)? as usize;
    let mut wide = Vec::with_capacity(len);
    for _ in 0..len {
        let mut unit = [0; 2];
        r.read_exact(&mut unit)?;
        wide.push(u16::from_le_bytes(unit));
    }
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

/// Paths elsewhere are bytes and needn't be valid UTF-8, so they too go as they
/// are.
#[cfg(unix)]
fn put_path(w: &mut impl Write, path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    put_u32(w, bytes.len() as u32)?;
    w.write_all(bytes)
}

#[cfg(unix)]
fn get_path(r: &mut impl Read) -> io::Result<PathBuf> {
    let mut bytes = vec![0; get_u32(r)? as usize];
    r.read_exact(&mut bytes)?;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn get_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn get_bytes(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; get_u32(r)? as usize];
    r.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn get_text(r: &mut impl Read) -> io::Result<Option<String>> {
    Ok(match get_u8(r)? {
        0 => None,
        _ => Some(String::from_utf8_lossy(&get_bytes(r)?).into_owned()),
    })
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

impl Target {
    /// Whether pdfium draws the page's annotations.
    pub fn annotations(&self) -> bool {
        match *self {
            Target::Page { annotations, .. } | Target::Region { annotations, .. } => annotations,
        }
    }

    /// The same target, with pdfium drawing the page's annotations or not.
    pub fn with_annotations(self, drawn: bool) -> Self {
        match self {
            Target::Page { scale, .. } => Target::Page { scale, annotations: drawn },
            Target::Region { full, region, .. } => Target::Region { full, region, annotations: drawn },
        }
    }

    fn write(&self, w: &mut impl Write) -> io::Result<()> {
        match *self {
            Target::Page { scale, .. } => {
                put_u8(w, 0)?;
                put_u32(w, scale.to_bits())?;
            }
            Target::Region { full, region, .. } => {
                put_u8(w, 1)?;
                full.iter().chain(&region).try_for_each(|&v| put_u32(w, v))?;
            }
        }
        put_u8(w, u8::from(self.annotations()))
    }

    fn read(r: &mut impl Read) -> io::Result<Self> {
        let target = match get_u8(r)? {
            0 => Target::Page { scale: f32::from_bits(get_u32(r)?), annotations: true },
            1 => {
                let mut v = [0; 6];
                for x in &mut v {
                    *x = get_u32(r)?;
                }
                Target::Region { full: [v[0], v[1]], region: [v[2], v[3], v[4], v[5]], annotations: true }
            }
            _ => return Err(invalid("unknown render target")),
        };
        Ok(target.with_annotations(get_u8(r)? != 0))
    }
}

impl Command {
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            Command::Open { version, path } => {
                put_u8(w, 1)?;
                put_u64(w, *version)?;
                put_path(w, path)
            }
            Command::Render { id, page, target } => {
                put_u8(w, 2)?;
                put_u64(w, *id)?;
                put_u32(w, *page)?;
                target.write(w)
            }
            Command::Cancel { id } => {
                put_u8(w, 3)?;
                put_u64(w, *id)
            }
        }
    }

    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        match get_u8(r)? {
            1 => {
                let version = get_u64(r)?;
                Ok(Command::Open { version, path: get_path(r)? })
            }
            2 => Ok(Command::Render { id: get_u64(r)?, page: get_u32(r)?, target: Target::read(r)? }),
            3 => Ok(Command::Cancel { id: get_u64(r)? }),
            _ => Err(invalid("unknown command")),
        }
    }
}

impl Event {
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            Event::Opened { version, error } => {
                put_u8(w, 1)?;
                put_u64(w, *version)?;
                put_text(w, error.as_deref())
            }
            Event::Image { id, complete, size, took_ms, rgba } => {
                put_u8(w, 2)?;
                put_u64(w, *id)?;
                put_u8(w, u8::from(*complete))?;
                put_u32(w, size[0])?;
                put_u32(w, size[1])?;
                put_u32(w, *took_ms)?;
                put_bytes(w, rgba)
            }
            Event::Stopped { id, error } => {
                put_u8(w, 3)?;
                put_u64(w, *id)?;
                put_text(w, error.as_deref())
            }
        }
    }

    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        match get_u8(r)? {
            1 => Ok(Event::Opened { version: get_u64(r)?, error: get_text(r)? }),
            2 => Ok(Event::Image {
                id: get_u64(r)?,
                complete: get_u8(r)? != 0,
                size: [get_u32(r)?, get_u32(r)?],
                took_ms: get_u32(r)?,
                rgba: get_bytes(r)?,
            }),
            3 => Ok(Event::Stopped { id: get_u64(r)?, error: get_text(r)? }),
            _ => Err(invalid("unknown event")),
        }
    }
}

/* ------------------------------------------------------------------ *
 * The helper process
 * ------------------------------------------------------------------ */

/// The helper's whole life. Returns once the app has gone. Nothing else may
/// write to stdout in a helper: everything there is read as `Event`s.
pub fn run() {
    // Cancelling can't wait its turn behind the render it is meant to stop,
    // so the thread reading commands handles it straight away.
    let cancelled = Arc::new(AtomicU64::new(0));
    let (commands_tx, commands) = mpsc::channel();
    {
        let cancelled = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            let mut input = BufReader::new(io::stdin());
            loop {
                match Command::read(&mut input) {
                    Ok(Command::Cancel { id }) => cancelled.store(id, Ordering::Relaxed),
                    Ok(command) => {
                        if commands_tx.send(command).is_err() {
                            return;
                        }
                    }
                    // The app has gone or closed the pipe: stop at once.
                    Err(_) => std::process::exit(0),
                }
            }
        });
    }

    let pdfium = match worker::bind() {
        Ok(pdfium) => pdfium,
        Err(message) => {
            trace(format_args!("helper: {message}"));
            std::process::exit(1);
        }
    };
    let mut out = BufWriter::new(io::stdout().lock());
    let mut open: Option<(u64, PathBuf, PdfDocument)> = None;
    // The page last drawn, still open (see `draw`). Declared after the
    // document so it's always closed first.
    let mut kept: Option<(u32, PdfPage)> = None;

    for command in commands {
        let written = match command {
            Command::Open { version, path } => {
                kept = None;
                open = None;
                let started = Instant::now();
                let error = match open_shared(&pdfium, &path) {
                    Ok(doc) => {
                        open = Some((version, path, doc));
                        None
                    }
                    Err(error) => Some(error),
                };
                trace(format_args!("helper: opened the file in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0));
                Event::Opened { version, error }.write(&mut out)
            }
            Command::Render { id, page, target } => {
                draw(&mut out, open.as_ref().map(|(_, _, doc)| doc), &mut kept, id, page, target, &cancelled)
            }
            Command::Cancel { .. } => Ok(()),
        };
        if written.and_then(|()| out.flush()).is_err() {
            return;
        }

        if kept.is_some() && worker::private_bytes() > KEEP_BELOW {
            kept = None;
            trace(format_args!("helper: closed the page it kept, now {} MB", worker::private_bytes() >> 20));
        }
        if kept.is_none() && worker::private_bytes() > REOPEN_ABOVE {
            if let Some((version, path)) = open.as_ref().map(|(v, p, _)| (*v, p.clone())) {
                if let Ok(doc) = open_shared(&pdfium, &path) {
                    open = Some((version, path, doc));
                    trace(format_args!("helper: reopened the file to free its caches, now {} MB", worker::private_bytes() >> 20));
                }
            }
        }
    }
}

/// Opens the file for pdfium to read as it needs to. It is shared for reading,
/// writing and deleting, so the app can still save over it: the save replaces
/// the file on disk, this goes on reading the old one, and the app then says
/// to open it again.
#[cfg(windows)]
fn open_shared<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>, String> {
    use std::os::windows::fs::OpenOptionsExt;

    const SHARE_READ_WRITE_DELETE: u32 = 0x1 | 0x2 | 0x4;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE_DELETE)
        .open(path)
        .map_err(|e| e.to_string())?;
    pdfium.load_pdf_from_reader(BufReader::new(file), None).map_err(|e| e.to_string())
}

/// Opens the file for pdfium to read as it needs to. An open file never stops
/// another from replacing it here, so there is nothing to share.
#[cfg(unix)]
fn open_shared<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    pdfium.load_pdf_from_reader(BufReader::new(file), None).map_err(|e| e.to_string())
}

/// Draws one render, sending part-drawn images of a slow page as it goes, then
/// the finished image or why there isn't one.
///
/// The page stays open afterwards, in `kept`, until another page is drawn or
/// memory runs high. Zoomed in, the view is drawn again and again from one
/// page as it scrolls, and loading a dense page takes up to a second each
/// time, many times longer than drawing the view itself.
fn draw<'a>(
    out: &mut impl Write,
    doc: Option<&PdfDocument<'a>>,
    kept: &mut Option<(u32, PdfPage<'a>)>,
    id: u64,
    page: u32,
    target: Target,
    cancelled: &AtomicU64,
) -> io::Result<()> {
    let Some(doc) = doc else {
        return Event::Stopped { id, error: Some("no file is open".to_owned()) }.write(out);
    };
    if cancelled.load(Ordering::Relaxed) == id {
        return Event::Stopped { id, error: None }.write(out);
    }
    let started = Instant::now();
    let took_ms = || started.elapsed().as_millis().min(u32::MAX as u128) as u32;
    if kept.as_ref().is_none_or(|(open, _)| *open != page) {
        *kept = None;
        let mut loaded = match doc.pages().get(page as PdfPageIndex) {
            Ok(loaded) => loaded,
            Err(e) => return Event::Stopped { id, error: Some(e.to_string()) }.write(out),
        };
        // The app draws highlights itself; see `annots::strip_highlights`.
        annots::strip_loaded_page(&mut loaded);
        *kept = Some((page, loaded));
    }
    let Some((_, loaded)) = kept.as_ref() else {
        return Event::Stopped { id, error: Some("the page could not be kept open".to_owned()) }.write(out);
    };

    let mut shown = Instant::now();
    let mut write_failed = None;
    let keep_going = |bitmap: &PdfBitmap| {
        if matches!(target, Target::Page { .. }) && write_failed.is_none() && shown.elapsed() >= PARTIAL_EVERY {
            let size = [bitmap.width() as u32, bitmap.height() as u32];
            let event = Event::Image { id, complete: false, size, took_ms: took_ms(), rgba: bitmap.as_rgba_bytes() };
            if let Err(e) = event.write(out).and_then(|()| out.flush()) {
                write_failed = Some(e);
            }
            shown = Instant::now();
        }
        write_failed.is_none() && cancelled.load(Ordering::Relaxed) != id
    };
    let rendered = match target {
        Target::Page { scale, annotations } => annots::render_page_in_steps(loaded, scale, annotations, keep_going),
        Target::Region { full, region, annotations } => annots::render_region_in_steps(loaded, full, region, annotations, keep_going),
    };

    if let Some(e) = write_failed {
        return Err(e);
    }
    match rendered {
        Ok(Some((size, rgba))) => {
            Event::Image { id, complete: true, size: [size[0] as u32, size[1] as u32], took_ms: took_ms(), rgba }.write(out)
        }
        Ok(None) => Event::Stopped { id, error: None }.write(out),
        Err(error) => Event::Stopped { id, error: Some(error) }.write(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_command(command: Command) {
        let mut bytes = Vec::new();
        command.write(&mut bytes).unwrap();
        assert_eq!(Command::read(&mut bytes.as_slice()).unwrap(), command);
    }

    fn round_trip_event(event: Event) {
        let mut bytes = Vec::new();
        event.write(&mut bytes).unwrap();
        assert_eq!(Event::read(&mut bytes.as_slice()).unwrap(), event);
    }

    #[test]
    fn commands_read_back_as_written() {
        round_trip_command(Command::Open { version: 7, path: PathBuf::from(r"C:\Drawings\Sheet [A] ü.pdf") });
        round_trip_command(Command::Render { id: 3, page: 48, target: Target::Page { scale: 0.795, annotations: true } });
        round_trip_command(Command::Render {
            id: u64::MAX,
            page: 0,
            target: Target::Region { full: [30_000, 21_000], region: [100, 200, 2768, 2045], annotations: false },
        });
        round_trip_command(Command::Cancel { id: 12 });
    }

    #[test]
    fn events_read_back_as_written() {
        round_trip_event(Event::Opened { version: 2, error: None });
        round_trip_event(Event::Opened { version: 3, error: Some("not a PDF".to_owned()) });
        round_trip_event(Event::Image { id: 9, complete: false, size: [2, 1], took_ms: 1250, rgba: vec![1, 2, 3, 255, 4, 5, 6, 255] });
        round_trip_event(Event::Stopped { id: 9, error: None });
        round_trip_event(Event::Stopped { id: 10, error: Some("page failed".to_owned()) });
    }

    #[test]
    fn a_stream_of_messages_reads_back_in_order() {
        let mut bytes = Vec::new();
        Event::Opened { version: 1, error: None }.write(&mut bytes).unwrap();
        Event::Image { id: 1, complete: true, size: [1, 1], took_ms: 3, rgba: vec![9; 4] }.write(&mut bytes).unwrap();
        let mut reader = bytes.as_slice();
        assert!(matches!(Event::read(&mut reader).unwrap(), Event::Opened { version: 1, .. }));
        assert!(matches!(Event::read(&mut reader).unwrap(), Event::Image { id: 1, complete: true, .. }));
        assert!(Event::read(&mut reader).is_err(), "nothing more to read");
    }
}
