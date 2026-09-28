//! Copying and pasting markups: measurements, clips and drawn markups, on the
//! clipboard between any windows of the app.
//!
//! What's copied is placed as it was on the sheet it came from, as the sheet
//! was seen: points across and down from its top left, whichever way it and
//! its page are turned. That, and the sheet's size, are all a paste needs to
//! put it anywhere: under the pointer with Ctrl+V, or with Ctrl+Shift+V where
//! it was -- the same place on a sheet the same size, and the same place
//! relative to the sheet on one that isn't. Either way it keeps the size it
//! had on paper, and what it measures comes from the sheet it lands on.
//!
//! The Clip tool makes one of these too: a clip, placed where it was lifted.

use serde::{Deserialize, Serialize};

use markup_model::markup::{ClipArt, Geometry, MarkupKind as MeasureKind};
use markup_model::{MarkupId, Pt, ScaleRef};

use super::*;
use crate::model::{DrawStyle, MeasureMarkup};

/// What's on the clipboard: markups, placed on a sheet `sheet` points across
/// and down as they were on the one they came from.
#[derive(Clone, Debug)]
pub(super) struct Copied {
    pub(super) sheet: [f32; 2],
    pub(super) items: Vec<Item>,
}

/// One markup copied, its points in the sheet's space: from its top left, y
/// down.
#[derive(Clone, Debug)]
pub(super) enum Item {
    /// A measurement or a clip, with what a clip draws.
    Measure(Box<MeasureMarkup>),
    Drawing(Drawing),
}

/// A drawn markup, as much of it as another sheet needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Drawing {
    kind: MarkupKind,
    points: Vec<[f32; 2]>,
    color: Rgb,
    width: f32,
    style: DrawStyle,
    name: String,
    comment: String,
}

/// A sheet's user space and the space things are copied in: points across
/// and down the sheet as it's seen.
struct SheetSpace {
    g: PageGeometry,
    across: f32,
    down: f32,
}

impl SheetSpace {
    fn of(doc: &Doc, sheet: usize) -> Option<SheetSpace> {
        let g = doc.sheet_geometry(sheet)?;
        let (across, down) = if g.rotation % 2 == 1 { (g.bounds.height(), g.bounds.width()) } else { (g.bounds.width(), g.bounds.height()) };
        Some(SheetSpace { g, across, down })
    }

    fn seen(&self, (x, y): (f32, f32)) -> [f32; 2] {
        let (fx, fy) = self.g.to_view(x, y);
        [fx * self.across, fy * self.down]
    }

    fn user(&self, [x, y]: [f32; 2]) -> (f32, f32) {
        self.g.from_view(x / self.across.max(f32::EPSILON), y / self.down.max(f32::EPSILON))
    }
}

impl Copied {
    /// The box round everything copied, in the sheet's space.
    fn bounds(&self) -> Option<[f32; 4]> {
        let mut points: Vec<[f32; 2]> = Vec::new();
        for item in &self.items {
            match item {
                Item::Measure(m) => points.extend(m.geometry.rings().into_iter().flatten().map(|p| [p.x as f32, p.y as f32])),
                Item::Drawing(d) => points.extend(&d.points),
            }
        }
        let first = *points.first()?;
        Some(points.iter().fold([first[0], first[1], first[0], first[1]], |[l, t, r, b], &[x, y]| [l.min(x), t.min(y), r.max(x), b.max(y)]))
    }

    /// A clip lifted from a sheet `sheet` points across and down, with its
    /// corners there, the drawing's bottom left first.
    pub(super) fn clip(art: ClipArt, sheet: [f32; 2], corners: [[f32; 2]; 4]) -> Copied {
        let pts = corners.iter().map(|&[x, y]| Pt::new(f64::from(x), f64::from(y))).collect();
        let mut markup = MeasureMarkup::new(0, MeasureKind::Clip, Geometry::Polygon { pts, holes: Vec::new() });
        markup.extras.clip = Some(art);
        Copied { sheet, items: vec![Item::Measure(Box::new(markup))] }
    }
}

/* ------------------------------------------------------------------ *
 * On the clipboard
 *
 * What's copied goes in a format of its own, which only this app asks for:
 * the markups as JSON, then the PDF each clip carries, after it as it is.
 * ------------------------------------------------------------------ */

/// What the clipboard's bytes start with, and the version of what follows.
const MAGIC: &[u8; 8] = b"KPDFCPY1";

#[derive(Serialize, Deserialize)]
struct Wire {
    sheet: [f32; 2],
    items: Vec<WireItem>,
}

#[derive(Serialize, Deserialize)]
enum WireItem {
    /// `art` is which of the PDFs after the JSON a clip carries, and its size.
    Measure { markup: Box<MeasureMarkup>, art: Option<(usize, [f64; 2])> },
    Drawing(Drawing),
}

impl Copied {
    fn to_bytes(&self) -> Vec<u8> {
        let mut blobs: Vec<std::sync::Arc<Vec<u8>>> = Vec::new();
        let items = self
            .items
            .iter()
            .map(|item| match item {
                Item::Measure(markup) => {
                    let art = markup.extras.clip.as_ref().map(|art| {
                        blobs.push(std::sync::Arc::clone(&art.pdf));
                        (blobs.len() - 1, art.size)
                    });
                    WireItem::Measure { markup: markup.clone(), art }
                }
                Item::Drawing(drawing) => WireItem::Drawing(drawing.clone()),
            })
            .collect();
        let json = serde_json::to_vec(&Wire { sheet: self.sheet, items }).unwrap_or_default();
        let mut out = Vec::with_capacity(json.len() + blobs.iter().map(|b| b.len() + 8).sum::<usize>() + 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(json.len() as u64).to_le_bytes());
        out.extend_from_slice(&json);
        for blob in blobs {
            out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
            out.extend_from_slice(&blob);
        }
        out
    }

    fn from_bytes(bytes: &[u8]) -> Option<Copied> {
        let mut rest = bytes.strip_prefix(MAGIC)?;
        let take = |rest: &mut &[u8]| -> Option<Vec<u8>> {
            let (length, after) = rest.split_first_chunk::<8>()?;
            let length = usize::try_from(u64::from_le_bytes(*length)).ok()?;
            let (taken, after) = (after.get(..length)?, after.get(length..)?);
            *rest = after;
            Some(taken.to_vec())
        };
        let wire: Wire = serde_json::from_slice(&take(&mut rest)?).ok()?;
        let mut blobs = Vec::new();
        while !rest.is_empty() {
            blobs.push(take(&mut rest)?);
        }
        let items = wire
            .items
            .into_iter()
            .map(|item| match item {
                WireItem::Measure { mut markup, art } => {
                    markup.extras.clip = art.and_then(|(blob, size)| blobs.get(blob).map(|pdf| ClipArt::new(pdf.clone(), size)));
                    Item::Measure(markup)
                }
                WireItem::Drawing(drawing) => Item::Drawing(drawing),
            })
            .collect();
        Some(Copied { sheet: wire.sheet, items })
    }
}

/// What's copied here, for when the clipboard can't be read, and the V key
/// as it last stood.
#[derive(Default)]
pub(super) struct Copying {
    held: Option<Copied>,
    v_down: bool,
}

/// The virtual key code of V.
const VK_V: i32 = 0x56;

impl App {
    /// Puts `copied` on the clipboard, and keeps it here too.
    pub(super) fn hold(&mut self, copied: Copied) {
        if !clipboard::write(&copied.to_bytes()) {
            crate::worker::trace(format_args!("copy: couldn't put it on the clipboard"));
        }
        self.copying.held = Some(copied);
    }

    /// Ctrl+C, Ctrl+V and Ctrl+Shift+V on markups, once a frame, unless
    /// something is being typed or a dialog is up.
    pub(super) fn copy_paste_keys(&mut self, ctx: &egui::Context) {
        // Ctrl+V never reaches egui as a key: egui-winit turns it into a
        // paste of text, and only when there is text to paste. So the key is
        // read as the window's thread last saw it, and taken as pressed the
        // frame it goes down with Ctrl held.
        let v_down = clipboard::key_is_down(VK_V);
        let pressed = v_down && !self.copying.v_down;
        self.copying.v_down = v_down;
        let typing = ctx.memory(|m| m.focused().is_some());
        let dialog = self.insert_sheet.is_some() || self.tool_creator.is_some() || self.palette.open || self.popup.is_some() || self.discarding.is_some();
        if typing || dialog || self.doc.is_none() || self.sheet_mode() {
            return;
        }
        let (command, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        if pressed && command {
            self.paste(shift);
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            self.copy_picked();
        }
    }

    /// Copies what's picked out on the page the first of it is on:
    /// measurements and clips, and markups drawn since the last save --
    /// those saved are drawn by the page, and have no points of their own to
    /// copy. Says whether anything was.
    pub(super) fn copy_picked(&mut self) -> bool {
        let picked = self.picked_rows();
        let Some(doc) = self.doc.as_ref() else { return false };
        let page_of = |row: &RowId| match *row {
            RowId::Measure(id) => doc.session.measures().get(id).map(|m| m.page as usize),
            RowId::Drawing(uid) => doc.session.markup(uid).map(|e| e.markup.page),
            RowId::Note(_) => None,
        };
        let Some(page) = picked.iter().find_map(page_of) else { return false };
        let Some(space) = doc.first_sheet_showing(page).and_then(|sheet| SheetSpace::of(doc, sheet)) else { return false };
        let mut items = Vec::new();
        for row in &picked {
            match *row {
                RowId::Measure(id) => {
                    let Some(markup) = doc.session.measures().get(id).filter(|m| m.page as usize == page) else { continue };
                    let mut copied = markup.clone();
                    copied.for_each_place_mut(|p| {
                        let [x, y] = space.seen((p.x as f32, p.y as f32));
                        *p = Pt::new(f64::from(x), f64::from(y));
                    });
                    items.push(Item::Measure(Box::new(copied)));
                }
                RowId::Drawing(uid) => {
                    let Some(m) = doc.session.markup(uid).map(|e| &e.markup).filter(|m| m.page == page && !m.points.is_empty()) else { continue };
                    let points = m.points.iter().map(|&[x, y]| space.seen((x, y))).collect();
                    items.push(Item::Drawing(Drawing { kind: m.kind, points, color: m.color, width: m.width, style: m.style.clone(), name: m.name.clone(), comment: m.comment.clone() }));
                }
                RowId::Note(_) => {}
            }
        }
        if items.is_empty() {
            return false;
        }
        let count = items.len();
        self.hold(Copied { sheet: [space.across, space.down], items });
        self.toast(if count == 1 { "Copied".to_owned() } else { format!("Copied {count}") });
        true
    }

    /// Puts down what's on the clipboard, as new markups picked out
    /// together: under the pointer if it's over a sheet, else in the middle
    /// of the sheet in view -- or `in_place`, where it was on the sheet it
    /// came from, as far across and down that sheet as it was.
    pub(super) fn paste(&mut self, in_place: bool) {
        let Some(copied) = clipboard::read().and_then(|bytes| Copied::from_bytes(&bytes)).or_else(|| self.copying.held.clone()) else {
            self.toast("Nothing to paste: copy or clip something first.".to_owned());
            return;
        };
        let Some([left, top, right, bottom]) = copied.bounds() else { return };
        let pointer = self.ctx.pointer_latest_pos().and_then(|pos| self.page_rects.iter().find(|(_, rect)| rect.contains(pos)).map(|(&sheet, _)| (sheet, pos)));
        let sheet = pointer.map_or(self.current_page, |(sheet, _)| sheet);
        let Some(doc) = self.doc.as_ref() else { return };
        let (Some(page), Some(space), Some(&rect)) = (doc.sheet_page(sheet), SheetSpace::of(doc, sheet), self.page_rects.get(&sheet)) else { return };
        let middle = [(left + right) / 2.0, (top + bottom) / 2.0];
        let to = match (in_place, pointer) {
            (true, _) => [middle[0] / copied.sheet[0].max(1.0) * space.across, middle[1] / copied.sheet[1].max(1.0) * space.down],
            (false, Some((_, pos))) => [(pos.x - rect.min.x) / rect.width() * space.across, (pos.y - rect.min.y) / rect.height() * space.down],
            (false, None) => {
                let seen = rect.intersect(self.viewer_rect).center();
                [(seen.x - rect.min.x) / rect.width() * space.across, (seen.y - rect.min.y) / rect.height() * space.down]
            }
        };
        let place = |[x, y]: [f32; 2]| space.user([x - middle[0] + to[0], y - middle[1] + to[1]]);

        let author = self.author_name();
        let now = chrono::Utc::now().timestamp_millis();
        let mut commands = Vec::new();
        let mut measures = Vec::new();
        for item in copied.items {
            match item {
                Item::Measure(mut markup) => {
                    markup.for_each_place_mut(|p| {
                        let (x, y) = place([p.x as f32, p.y as f32]);
                        *p = Pt::new(f64::from(x), f64::from(y));
                    });
                    // A new markup of the sheet it lands on, measured by its
                    // scale.
                    markup.id = MarkupId::new();
                    markup.page = page as u32;
                    markup.scale_ref = ScaleRef::Page;
                    markup.extras.foreign_nm = None;
                    markup.extras.changed_externally = false;
                    markup.meta.author.clone_from(&author);
                    markup.meta.created_ms = Some(now);
                    markup.meta.modified_ms = Some(now);
                    measures.push(markup.id);
                    commands.push(Command::AddMeasure(markup));
                }
                Item::Drawing(d) => {
                    let points: Vec<[f32; 2]> = d.points.iter().map(|&p| place(p).into()).collect();
                    let bounds = crate::markup::bounds(d.kind, &points, d.width);
                    commands.push(Command::AddMarkup(Markup {
                        key: None,
                        page,
                        kind: d.kind,
                        points,
                        bounds,
                        color: d.color,
                        width: d.width,
                        style: d.style,
                        name: d.name,
                        comment: d.comment,
                        author: author.clone(),
                    }));
                }
            }
        }
        let Some(doc) = self.doc.as_mut() else { return };
        let drawn = doc.session.apply(Command::Batch(commands));
        let rows: Vec<RowId> = measures.into_iter().map(RowId::Measure).chain(drawn.into_iter().map(RowId::Drawing)).collect();
        self.take_up_select();
        self.pick(&rows);
        self.want_measurements();
    }
}

#[cfg(windows)]
mod clipboard {
    use windows_sys::Win32::Foundation::{GlobalFree, HWND};
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, GetKeyState};

    /// The clipboard format clips go in, registered once.
    fn format() -> u32 {
        static FORMAT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *FORMAT.get_or_init(|| {
            let name: Vec<u16> = "Kinetic PDF markups".encode_utf16().chain([0]).collect();
            // SAFETY: a null-terminated UTF-16 string that outlives the call.
            unsafe { RegisterClipboardFormatW(name.as_ptr()) }
        })
    }

    /// Opens the clipboard for the window in front, which is this app's while
    /// its keys are being read. A clipboard opened with no window can't be
    /// written to once it's emptied.
    fn open() -> bool {
        // SAFETY: plain Win32 calls with no pointers of ours.
        unsafe {
            let window: HWND = GetActiveWindow();
            (0..5).any(|attempt| {
                // Another program may have it open for a moment.
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                OpenClipboard(window) != 0
            })
        }
    }

    pub(super) fn write(bytes: &[u8]) -> bool {
        let format = format();
        if format == 0 || !open() {
            return false;
        }
        // SAFETY: the memory is ours until SetClipboardData takes it, is
        // locked while it's written, and is written within its size.
        unsafe {
            let written = (|| {
                if EmptyClipboard() == 0 {
                    return false;
                }
                let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                if memory.is_null() {
                    return false;
                }
                let into = GlobalLock(memory) as *mut u8;
                if into.is_null() {
                    GlobalFree(memory);
                    return false;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), into, bytes.len());
                GlobalUnlock(memory);
                if SetClipboardData(format, memory).is_null() {
                    GlobalFree(memory);
                    return false;
                }
                true
            })();
            CloseClipboard();
            written
        }
    }

    pub(super) fn read() -> Option<Vec<u8>> {
        let format = format();
        // SAFETY: the clipboard's memory is only read while it's locked, and
        // within the size it says it has.
        unsafe {
            if format == 0 || IsClipboardFormatAvailable(format) == 0 || !open() {
                return None;
            }
            let read = (|| {
                let memory = GetClipboardData(format);
                if memory.is_null() {
                    return None;
                }
                let size = GlobalSize(memory);
                let from = GlobalLock(memory) as *const u8;
                if from.is_null() {
                    return None;
                }
                let bytes = std::slice::from_raw_parts(from, size).to_vec();
                GlobalUnlock(memory);
                Some(bytes)
            })();
            CloseClipboard();
            read
        }
    }

    pub(super) fn key_is_down(key: i32) -> bool {
        // SAFETY: no pointers.
        unsafe { GetKeyState(key) < 0 }
    }
}

#[cfg(not(windows))]
mod clipboard {
    pub(super) fn write(_: &[u8]) -> bool {
        false
    }

    pub(super) fn read() -> Option<Vec<u8>> {
        None
    }

    pub(super) fn key_is_down(_: i32) -> bool {
        false
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_copied_goes_on_the_clipboard_and_comes_back_the_same() {
        let art = ClipArt::new(b"%PDF-1.7 the drawing".to_vec(), [120.5, 80.25]);
        let mut copied = Copied::clip(art, [841.0, 594.0], [[10.0, 90.0], [130.5, 90.0], [130.5, 9.75], [10.0, 9.75]]);
        let length = MeasureMarkup::new(3, MeasureKind::Length, Geometry::Line { a: Pt::new(1.0, 2.0), b: Pt::new(3.0, 4.0) });
        copied.items.push(Item::Measure(Box::new(length.clone())));
        copied.items.push(Item::Drawing(Drawing {
            kind: MarkupKind::Arrow,
            points: vec![[5.0, 5.0], [50.0, 60.0]],
            color: [1.0, 0.0, 0.0],
            width: 2.0,
            style: DrawStyle::default(),
            name: "Up".into(),
            comment: "this way".into(),
        }));
        let back = Copied::from_bytes(&copied.to_bytes()).expect("it reads back");
        assert_eq!(back.sheet, [841.0, 594.0]);
        let Item::Measure(clip) = &back.items[0] else { panic!("the clip first") };
        let art = clip.extras.clip.as_ref().expect("with its drawing");
        assert_eq!((art.size, art.pdf.as_slice()), ([120.5, 80.25], b"%PDF-1.7 the drawing".as_slice()));
        let Item::Measure(back_length) = &back.items[1] else { panic!("then the length") };
        assert_eq!(back_length.geometry, length.geometry);
        let Item::Drawing(arrow) = &back.items[2] else { panic!("then the arrow") };
        assert_eq!((arrow.kind, arrow.points.clone(), arrow.comment.as_str()), (MarkupKind::Arrow, vec![[5.0, 5.0], [50.0, 60.0]], "this way"));
        assert!(Copied::from_bytes(b"KPDFCPY1 but nothing after").is_none());
        assert!(Copied::from_bytes(b"something else entirely").is_none());
    }

    #[test]
    fn a_sheet_s_space_runs_across_and_down_it_as_it_is_seen() {
        // A portrait page turned a quarter: its left edge along the top.
        let g = PageGeometry { rotation: 1, bounds: crate::model::PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } };
        let space = SheetSpace { g, across: 800.0, down: 600.0 };
        assert_eq!(space.seen((0.0, 0.0)), [0.0, 0.0], "the page's bottom left is the sheet's top left");
        assert_eq!(space.seen((0.0, 100.0)), [100.0, 0.0], "up the page is across the sheet");
        let (x, y) = space.user([100.0, 50.0]);
        assert!((x - 50.0).abs() < 1e-3 && (y - 100.0).abs() < 1e-3, "{x}, {y}");
    }
}
