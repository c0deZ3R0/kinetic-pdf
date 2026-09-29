//! Text boxes on the page: putting one down, typing into it where it is,
//! resizing it by its corners and pointing its arrow, and drawing it.
//!
//! A box is drawn from the same layout its appearance in the file is written
//! from (`text_layout`), in the same fonts -- each loaded into egui the first
//! time a box is set in it -- so what's on screen is what the file shows.
//! While a box is being typed into, an editor stands over it in its place,
//! at the zoom in view, its text shrunk as the box would shrink it.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::epaint::text::{FontData, FontInsert, FontPriority, InsertFontFamily};
use eframe::egui::epaint::TextShape;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::FontFamily;
use markup_model::markup::{Geometry, MarkupKind as MeasureKind};
use markup_model::{callout_start, Frame, Pt, RunFormat, TextBox};
use text_layout::{catalogue, layout, Face, Laid};

use super::*;
use crate::model::MeasureMarkup;

/// A box put down with a click rather than dragged out: points across and
/// down the sheet.
const CLICKED_BOX: (f32, f32) = (180.0, 48.0);

/// The smallest a box is dragged out or resized to, in points.
const LEAST_BOX: f64 = 12.0;

/// Where a click puts an arrow's box, from its tip: points across and up the
/// sheet.
const CLICKED_ARROW: (f32, f32) = (36.0, 36.0);

/// The box being typed into.
pub(super) struct Editing {
    pub(super) id: MarkupId,
    pub(super) text: String,
    /// Put down to be typed into: gone again if nothing is.
    fresh: bool,
    /// Whether anything has been typed since it opened.
    typed: bool,
    /// The editor is to take the keyboard when it next draws.
    focus: bool,
    /// The characters picked out in it, as the editor last had them.
    pub(super) selection: Range<usize>,
    /// The format what's typed at the caret takes, when one was set with
    /// nothing picked out: bold turned on, and then typed in bold.
    pub(super) pending: Option<RunFormat>,
    /// Where it is on screen: a press on the page anywhere else closes it.
    pub(super) screen: Rect,
}

impl Editing {
    fn new(id: MarkupId, text: String, fresh: bool) -> Editing {
        let end = text.chars().count();
        Editing { id, text, fresh, typed: false, focus: true, selection: end..end, pending: None, screen: Rect::NOTHING }
    }
}

/// What text boxes are drawn with: the fonts loaded into egui, by the file
/// they come from, and each box's words as last laid out, since fitting
/// them to the box sets them a dozen times over.
#[derive(Default)]
pub(super) struct Fonts {
    loaded: HashSet<std::path::PathBuf>,
    laid: HashMap<MarkupId, (TextBox, f64, f64, Arc<Laid>)>,
    /// Each box's words as egui last laid them out to draw, kept while the
    /// box and the zoom stay as they are.
    drawn: HashMap<MarkupId, Drawn>,
    /// A scrap of text egui laid out, to see it's still the same one: egui
    /// starts its glyphs again when the screen's scale changes, a font comes
    /// in or they fill up, and galleys kept from before would draw the old.
    sentinel: Option<Arc<egui::Galley>>,
}

/// A box's words laid out by egui, ready to be put where the box is.
struct Drawn {
    laid: Arc<Laid>,
    per_point: f32,
    /// Every font was egui's to draw with: none stood in for one still
    /// loading.
    complete: bool,
    words: Vec<DrawnWord>,
}

struct DrawnWord {
    galley: Arc<egui::Galley>,
    /// Its baseline's start, in points across and up the box.
    at: (f64, f64),
    /// Screen points from the top of its galley to its baseline.
    ascent: f32,
    /// How far apart a bold the font hasn't got is drawn twice, if it is.
    bold: Option<f32>,
}

impl Fonts {
    /// Lets go of every box's galleys if egui has started its glyphs again
    /// since they were laid out. Once a page drawn, before `drawn`.
    fn check_glyphs(&mut self, ctx: &egui::Context) {
        let sentinel = ctx.fonts_mut(|f| f.layout_no_wrap("·".to_owned(), FontId::proportional(7.0), Color32::WHITE));
        if !self.sentinel.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, &sentinel)) {
            self.drawn.clear();
            self.sentinel = Some(sentinel);
        }
    }

    /// Box `id`'s words, laid out `laid` in its inside, as egui draws them at
    /// `per_point` screen points to a point: made again only when the box,
    /// the zoom or egui's glyphs have changed.
    fn drawn(&mut self, ctx: &egui::Context, id: MarkupId, laid: &Arc<Laid>, per_point: f32, pad: f64, height: f64) -> &Drawn {
        let fresh = self.drawn.get(&id).is_some_and(|d| Arc::ptr_eq(&d.laid, laid) && d.per_point == per_point && d.complete);
        if !fresh {
            if self.drawn.len() > 2000 {
                self.drawn.clear();
            }
            let mut complete = true;
            let mut words = Vec::new();
            for line in &laid.lines {
                for word in &line.words {
                    let size = word.size * per_point;
                    if !(1.5..=600.0).contains(&size) {
                        continue;
                    }
                    let (family, loaded) = self.family(ctx, &word.face);
                    complete &= loaded;
                    let colour = to_color32(word.colour);
                    let underline = if word.underline { Stroke::new((size * 0.06).max(1.0), colour) } else { Stroke::NONE };
                    let mut job = LayoutJob::default();
                    job.append(word.text.trim_end(), 0.0, TextFormat { font_id: FontId::new(size, family), color: colour, italics: word.fake_italic, underline, ..Default::default() });
                    let galley = ctx.fonts_mut(|f| f.layout_job(job));
                    // Placed by its baseline, which is where the layout put it.
                    let ascent = galley.rows.first().and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(size * 0.8);
                    let at = (pad + f64::from(word.x), height - pad - f64::from(line.baseline));
                    words.push(DrawnWord { galley, at, ascent, bold: word.fake_bold.then(|| (size * 0.035).max(0.5)) });
                }
            }
            self.drawn.insert(id, Drawn { laid: Arc::clone(laid), per_point, complete, words });
        }
        &self.drawn[&id]
    }

    /// Box `id`'s words laid out in its inside, `width` by `height` points.
    fn laid(&mut self, id: MarkupId, words: &TextBox, width: f64, height: f64) -> Arc<Laid> {
        if let Some((was, w, h, laid)) = self.laid.get(&id) {
            if was == words && *w == width && *h == height {
                return Arc::clone(laid);
            }
        }
        if self.laid.len() > 2000 {
            self.laid.clear();
        }
        let laid = Arc::new(layout(words, width.max(1.0) as f32, height.max(1.0) as f32, catalogue()));
        self.laid.insert(id, (words.clone(), width, height, Arc::clone(&laid)));
        laid
    }

    /// The egui font for text in `format` at `size` screen points, and
    /// whether it has to be slanted to look italic.
    fn font(&mut self, ctx: &egui::Context, format: &RunFormat, size: f32) -> (FontId, bool) {
        match catalogue().face(&format.font, format.bold, format.italic) {
            Some(face) => (FontId::new(size, self.family(ctx, &face).0), format.italic && !face.entry.italic),
            None => (FontId::proportional(size), format.italic),
        }
    }

    /// The egui family that draws in `face`, loading it the first time it's
    /// wanted. egui takes a font on at the start of its next frame, so until
    /// then -- a frame -- its own font stands in, and it says so.
    fn family(&mut self, ctx: &egui::Context, face: &Face) -> (FontFamily, bool) {
        let name = format!("kpdf:{}", face.entry.path.display());
        let family = FontFamily::Name(name.clone().into());
        if self.loaded.insert(face.entry.path.clone()) {
            ctx.add_font(FontInsert::new(&name, FontData::from_owned(face.data.to_vec()), vec![InsertFontFamily { family: family.clone(), priority: FontPriority::Highest }]));
            ctx.request_repaint();
        }
        if ctx.fonts(|f| f.families().contains(&family)) { (family, true) } else { (FontFamily::Proportional, false) }
    }
}

/// How a box sits on screen: its frame on the page, and what takes a point
/// of it to the screen.
struct Placed {
    frame: Frame,
    /// Screen points to a point along the box.
    per_point: f32,
    /// The box's right on screen, in radians from the screen's.
    angle: f32,
}

/// The sheet drawn at `rect` with geometry `g`: where a point of its page
/// lands on screen.
fn screen(rect: Rect, g: &PageGeometry) -> impl Fn(Pt) -> Pos2 + '_ {
    move |p: Pt| {
        let (fx, fy) = g.to_view(p.x as f32, p.y as f32);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    }
}

fn placed(corners: &[Pt], at: &impl Fn(Pt) -> Pos2) -> Option<Placed> {
    let frame = Frame::of(corners)?;
    let origin = at(frame.to_user(0.0, 0.0));
    let along = at(frame.to_user(1.0, 0.0)) - origin;
    Some(Placed { frame, per_point: along.length(), angle: along.y.atan2(along.x) })
}

/// Draws page `page`'s text boxes, drawn at `rect` with geometry `g`, over
/// everything else on it: the one picked out with its corners and arrow tip
/// to drag, and the one being typed into without its text, which the editor
/// over it shows.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_text_boxes(painter: &egui::Painter, fonts: &mut Fonts, rect: Rect, g: &PageGeometry, active: Option<MarkupId>, picked: &[MarkupId], editing: Option<MarkupId>, items: &[measure::Stacked]) {
    let ctx = painter.ctx().clone();
    let at = screen(rect, g);
    let mut checked = false;
    for &(m, _) in items {
        if !std::mem::replace(&mut checked, true) {
            fonts.check_glyphs(&ctx);
        }
        let (Some(words), Geometry::Polygon { pts, .. }) = (&m.extras.text, &m.geometry) else { continue };
        let Some(box_) = placed(pts, &at) else { continue };
        let corners: Vec<Pos2> = pts.iter().map(|&p| at(p)).collect();
        let style = &m.style;
        let (w, h) = (box_.frame.width, box_.frame.height);

        if let Some(fill) = style.fill {
            painter.add(Shape::convex_polygon(corners.clone(), to_color32(fill).gamma_multiply(style.fill_opacity), Stroke::NONE));
        }
        if editing != Some(m.id) {
            let pad = words.padding.max(0.0);
            let laid = fonts.laid(m.id, words, w - 2.0 * pad, h - 2.0 * pad);
            let clip = Rect::from_points(&corners).intersect(painter.clip_rect());
            let painter = painter.with_clip_rect(clip);
            let (sin, cos) = box_.angle.sin_cos();
            for word in &fonts.drawn(&ctx, m.id, &laid, box_.per_point, pad, h).words {
                let baseline = at(box_.frame.to_user(word.at.0, word.at.1));
                let top_left = baseline - vec2(-word.ascent * sin, word.ascent * cos);
                painter.add(TextShape::new(top_left, Arc::clone(&word.galley), Color32::PLACEHOLDER).with_angle(box_.angle));
                // A bold the font hasn't got, drawn twice a hair apart.
                if let Some(apart) = word.bold {
                    painter.add(TextShape::new(top_left + vec2(cos, sin) * apart, Arc::clone(&word.galley), Color32::PLACEHOLDER).with_angle(box_.angle));
                }
            }
        }

        // The border, inside the box's edge, and the arrow out of it.
        let line = to_color32(style.stroke).gamma_multiply(style.opacity);
        if style.width > 0.0 {
            let stroke = Stroke::new((style.width as f32 * box_.per_point).max(1.0), line);
            if !line_style::paint_dashed(painter, &corners, true, stroke, &style.dash, box_.per_point) {
                measure::paint_joined(painter, &corners, true, stroke);
            }
        }
        if let Some(tip) = words.callout {
            let (tx, ty) = box_.frame.to_box(tip);
            let (sx, sy) = callout_start(w, h, (tx, ty));
            let (from, to) = (at(box_.frame.to_user(sx, sy)), at(tip));
            let width = (style.width.max(0.75) as f32 * box_.per_point).max(1.0);
            let direction = (to - from).normalized();
            let head = (width * 4.0).max(6.0 * box_.per_point.min(2.0));
            let base = to - direction * head;
            let side = vec2(-direction.y, direction.x) * head * 0.4;
            painter.line_segment([from, base], Stroke::new(width, line));
            painter.add(Shape::convex_polygon(vec![to, base + side, base - side], line, Stroke::NONE));
        }

        if active == Some(m.id) || picked.contains(&m.id) {
            let ring: Vec<Pos2> = corners.iter().copied().chain(corners.first().copied()).collect();
            painter.add(Shape::line(ring, Stroke::new(1.5, ACCENT)));
            // Its corners and sides are the frame's to take hold of (see
            // `reshape.rs`); its arrow's tip is its own.
            if active == Some(m.id) && editing != Some(m.id) {
                if let Some(tip) = words.callout {
                    painter.circle(at(tip), 5.0, Color32::WHITE, Stroke::new(1.5, ACCENT));
                }
            }
        }
    }
}

/// The text box on page `page` whose arrow's tip is within `slack` points
/// of `point`, in user space.
pub(super) fn tip_at(doc: &Doc, page: usize, (x, y): (f32, f32), slack: f32) -> Option<MarkupId> {
    let point = Pt::new(f64::from(x), f64::from(y));
    doc.session
        .measures()
        .iter()
        .filter(|(m, _)| m.page as usize == page && m.kind == MeasureKind::Text && doc.session.layers().is_editable(m))
        .find(|(m, _)| m.extras.text.as_ref().and_then(|t| t.callout).is_some_and(|tip| tip.dist(point) <= f64::from(slack)))
        .map(|(m, _)| m.id)
}

impl App {
    /// The text box whose arrow's tip is under `pos` on sheet `sheet`.
    pub(super) fn callout_tip_at(&self, sheet: usize, pos: Pos2) -> Option<MarkupId> {
        let doc = self.doc.as_ref()?;
        let point = self.pdf_point(sheet, pos)?;
        tip_at(doc, doc.sheet_page(sheet)?, point, (measure::PICK_SLACK + 3.0) * self.points_per_screen(sheet))
    }

    /// The text box under `pos` on sheet `sheet`.
    pub(super) fn text_box_at(&self, sheet: usize, pos: Pos2) -> Option<MarkupId> {
        let (id, _) = self.measurement_at(sheet, pos)?;
        let doc = self.doc.as_ref()?;
        doc.session.measures().get(id).is_some_and(|m| m.kind == MeasureKind::Text).then_some(id)
    }

    /// Takes up a text tool, putting down any other.
    pub(super) fn take_up_text(&mut self, arrow: bool) {
        self.set_measure_tool(None);
        self.tool = None;
        self.highlighter = false;
        self.put_down_clip();
        self.text_tool = Some(arrow);
        self.want_measurements();
    }

    /// Which way the sheet's right and up run in its page's user space, as
    /// unit vectors: a box is put down square to the sheet as it's seen.
    pub(super) fn sheet_axes(&self, sheet: usize) -> Option<(Pt, Pt)> {
        let doc = self.doc.as_ref()?;
        let g = doc.sheet_geometry(sheet)?;
        let (across, down) = if g.rotation % 2 == 1 { (g.bounds.height(), g.bounds.width()) } else { (g.bounds.width(), g.bounds.height()) };
        let user = |fx: f32, fy: f32| {
            let (x, y) = g.from_view(fx, fy);
            Pt::new(f64::from(x), f64::from(y))
        };
        let origin = user(0.5, 0.5);
        let right = user(0.5 + 1.0 / across, 0.5) - origin;
        let up = user(0.5, 0.5 - 1.0 / down) - origin;
        Some((right * (1.0 / right.len().max(f64::EPSILON)), up * (1.0 / up.len().max(f64::EPSILON))))
    }

    /// Starts dragging out a text box, or with an arrow, its arrow from the
    /// tip to where the box goes.
    pub(super) fn start_text(&mut self, sheet: usize, pos: Pos2) {
        if let (Some(arrow), Some(point)) = (self.text_tool, self.pdf_point(sheet, pos)) {
            self.drag = Some(Drag::PutText { sheet, start: point, end: point, arrow });
            self.popup = None;
        }
    }

    /// Puts a text box down: dragged out from `start` to `end` -- or with an
    /// arrow, pointing at `start` from a box at `end` -- and opens it to be
    /// typed into. A click, going nowhere, puts one down a usual size.
    pub(super) fn put_down_text(&mut self, sheet: usize, start: (f32, f32), end: (f32, f32), arrow: bool) {
        let Some((right, up)) = self.sheet_axes(sheet) else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(page) = doc.sheet_page(sheet) else { return };
        let (start, end) = (Pt::new(f64::from(start.0), f64::from(start.1)), Pt::new(f64::from(end.0), f64::from(end.1)));
        let (clicked_w, clicked_h) = (f64::from(CLICKED_BOX.0), f64::from(CLICKED_BOX.1));
        let (origin, width, height, tip) = if arrow {
            // The box beside where the drag ended, on the far side from the
            // tip, a usual size.
            let end = if start.dist(end) < 4.0 { start + right * f64::from(CLICKED_ARROW.0) + up * f64::from(CLICKED_ARROW.1) } else { end };
            let d = end - start;
            let (dx, dy) = (d.dot(right), d.dot(up));
            let x = if dx >= 0.0 { 0.0 } else { -clicked_w };
            let y = if dy >= 0.0 { 0.0 } else { -clicked_h };
            (end + right * x + up * y, clicked_w, clicked_h, Some(start))
        } else {
            let d = end - start;
            let (dx, dy) = (d.dot(right), d.dot(up));
            if dx.abs() < LEAST_BOX || dy.abs() < LEAST_BOX {
                // A click: a usual box, its top left where it was clicked.
                (start - up * clicked_h, clicked_w, clicked_h, None)
            } else {
                (start + right * dx.min(0.0) + up * dy.min(0.0), dx.abs(), dy.abs(), None)
            }
        };
        let corners = vec![origin, origin + right * width, origin + right * width + up * height, origin + up * height];
        let key = tools::ToolKey::Text { arrow };
        let settings = self.tools.settings(key);
        let mut markup = MeasureMarkup::new(page as u32, MeasureKind::Text, Geometry::Polygon { pts: corners, holes: Vec::new() });
        settings.apply(&mut markup);
        if let Some(text) = markup.extras.text.as_mut() {
            text.callout = tip;
        }
        let now = chrono::Utc::now().timestamp_millis();
        markup.meta.author = self.author_name();
        markup.meta.created_ms = Some(now);
        markup.meta.modified_ms = Some(now);
        let id = markup.id;
        let Some(doc) = self.doc.as_mut() else { return };
        markup.layer = doc.session.layer_for_new(&settings.defaults.layer, settings.defaults.layer_colour);
        doc.session.apply(Command::AddMeasure(Box::new(markup)));
        self.pick(&[RowId::Measure(id)]);
        self.text_editing = Some(Editing::new(id, String::new(), true));
    }

    /// Opens text box `id` to be typed into.
    pub(super) fn edit_text(&mut self, id: MarkupId) {
        let text = self.doc.as_ref().and_then(|d| d.session.measures().get(id)).and_then(|m| m.extras.text.as_ref()).map(TextBox::text);
        if let Some(text) = text {
            self.finish_text_edit();
            self.pick(&[RowId::Measure(id)]);
            self.text_editing = Some(Editing::new(id, text, false));
        }
    }

    /// Closes the box being typed into. One put down and left empty goes
    /// again, as if it never was.
    pub(super) fn finish_text_edit(&mut self) {
        let Some(editing) = self.text_editing.take() else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        doc.session.end_merge();
        if editing.fresh && editing.text.trim().is_empty() {
            if editing.typed {
                doc.session.apply(Command::RemoveMeasure(editing.id));
            } else {
                // Only the putting down to take back.
                doc.session.undo();
            }
            self.pick(&[]);
        }
    }

    /// The box being typed into, and its words, if it's still there.
    fn edited_box(&self) -> Option<(MarkupId, TextBox)> {
        let editing = self.text_editing.as_ref()?;
        let m = self.doc.as_ref()?.session.measures().get(editing.id)?;
        Some((m.id, m.extras.text.clone()?))
    }

    /// The formats of what's picked out in the box being typed into -- or
    /// with nothing picked out, the one typing takes -- the first of them
    /// what the details panel shows. `None` unless box `id` is being typed
    /// into.
    pub(super) fn editing_formats(&self, id: MarkupId) -> Option<Vec<RunFormat>> {
        let editing = self.text_editing.as_ref().filter(|e| e.id == id)?;
        let (_, words) = self.edited_box()?;
        if editing.selection.is_empty() {
            return Some(vec![editing.pending.clone().unwrap_or_else(|| words.format_at(editing.selection.start))]);
        }
        Some(words.formats_in(editing.selection.clone()))
    }

    /// Restyles what's picked out in the box being typed into, as one step
    /// to undo; with nothing picked out, what's typed next.
    pub(super) fn restyle_editing(&mut self, change: impl Fn(&mut RunFormat)) {
        let Some((id, words)) = self.edited_box() else { return };
        let Some(editing) = self.text_editing.as_mut() else { return };
        if editing.selection.is_empty() {
            let mut typing = editing.pending.clone().unwrap_or_else(|| words.format_at(editing.selection.start));
            change(&mut typing);
            editing.pending = Some(typing);
            return;
        }
        let restyled = words.restyled_range(editing.selection.clone(), change);
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(mut markup) = doc.session.measures().get(id).cloned() else { return };
        markup.extras.text = Some(restyled);
        markup.meta.modified_ms = Some(chrono::Utc::now().timestamp_millis());
        doc.session.apply(Command::ChangeMeasure(Box::new(markup)));
    }

    /// Restyles all of text box `id` by `change`, as one step to undo.
    pub(super) fn restyle_box(&mut self, id: MarkupId, change: impl Fn(&mut RunFormat)) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(mut markup) = doc.session.measures().get(id).cloned() else { return };
        let Some(words) = markup.extras.text.as_ref() else { return };
        markup.extras.text = Some(words.restyled_range(0..words.char_len().max(1), change));
        markup.meta.modified_ms = Some(chrono::Utc::now().timestamp_millis());
        doc.session.apply(Command::ChangeMeasure(Box::new(markup)));
    }

    /// Bold, italic or underline, as `which` picks it out of a format,
    /// turned on for what's picked out -- or off, if it's all on already.
    fn toggle_editing(&mut self, which: fn(&mut RunFormat) -> &mut bool) {
        let Some(formats) = self.text_editing.as_ref().and_then(|e| self.editing_formats(e.id)) else { return };
        let on = !formats.into_iter().all(|mut f| *which(&mut f));
        self.restyle_editing(move |f| *which(f) = on);
    }

    /// The editor over the box being typed into, where the box is, at the
    /// zoom in view: its words in their own fonts, sizes and colours, sized
    /// as the box fits them. Each change goes to the box as one step to
    /// undo. Esc, Ctrl+Enter, or a press on the page elsewhere closes it;
    /// the details panel, or Ctrl+B, I or U, styles what's picked out in it.
    pub(super) fn text_editor(&mut self, ctx: &egui::Context) {
        let Some((id, words)) = self.edited_box() else {
            self.text_editing = None;
            return;
        };
        let edit_id = Id::new(("text-editor", id.to_nm()));
        let focused = ctx.memory(|m| m.has_focus(edit_id));
        if focused {
            let (done, bold, italic, underline) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Escape) | i.consume_key(Modifiers::COMMAND, Key::Enter),
                    i.consume_key(Modifiers::COMMAND, Key::B),
                    i.consume_key(Modifiers::COMMAND, Key::I),
                    i.consume_key(Modifiers::COMMAND, Key::U),
                )
            });
            if done {
                return self.finish_text_edit();
            }
            if bold {
                self.toggle_editing(|f| &mut f.bold);
            }
            if italic {
                self.toggle_editing(|f| &mut f.italic);
            }
            if underline {
                self.toggle_editing(|f| &mut f.underline);
            }
        }
        // Styled from the panel, what was picked out stays as it was.
        let Some((_, words)) = self.edited_box().or(Some((id, words))) else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(m) = doc.session.measures().get(id) else { return };
        let Geometry::Polygon { pts, .. } = &m.geometry else { return };
        let Some(sheet) = doc.first_sheet_showing(m.page as usize) else { return };
        let (Some(&rect), Some(g)) = (self.page_rects.get(&sheet), doc.sheet_geometry(sheet)) else { return };
        let at = screen(rect, &g);
        let Some(box_) = placed(pts, &at) else { return };
        let pad = words.padding.max(0.0);
        let (w, h) = (box_.frame.width, box_.frame.height);
        let laid = self.text_fonts.laid(id, &words, w - 2.0 * pad, h - 2.0 * pad);
        let px = laid.scale * box_.per_point;
        let corners: Vec<Pos2> = pts.iter().map(|&p| at(p)).collect();
        // Upright, the box's own size, over its middle: a turned box is
        // typed into square to the screen.
        let area = Rect::from_center_size(Rect::from_points(&corners).center(), vec2(w as f32, h as f32) * box_.per_point);
        let inner = area.shrink(pad as f32 * box_.per_point);
        let align = match words.align() {
            markup_model::HAlign::Centre => Align::Center,
            markup_model::HAlign::Right => Align::Max,
            _ => Align::Min,
        };
        // Where the box sets its first line: down from the top as far as its
        // text is aligned, in the room the text leaves.
        let room = ((h - 2.0 * pad) as f32 - laid.height).max(0.0);
        let down = match words.valign {
            markup_model::VAlign::Top => 0.0,
            markup_model::VAlign::Middle => room / 2.0,
            markup_model::VAlign::Bottom => room,
        } * box_.per_point;
        let clip = area.expand(2.0).intersect(self.viewer_rect);
        let Some(editing) = self.text_editing.as_mut() else { return };
        let fonts = &mut self.text_fonts;
        editing.screen = area;
        // Nothing has the keyboard, as after a click in the details panel:
        // it comes back here, where the typing is -- once the panel's done
        // with the pointer and any colour picker it opened is shut. What was
        // picked out is picked out again, since an editor without the
        // keyboard lets go of it.
        let away = ctx.memory(|m| m.focused().is_none()) && !egui::Popup::is_any_open(ctx) && !ctx.input(|i| i.pointer.any_down());
        if std::mem::take(&mut editing.focus) || away {
            let mut state = egui::text_edit::TextEditState::load(ctx, edit_id).unwrap_or_default();
            let (start, end) = (egui::text::CCursor::new(editing.selection.start), egui::text::CCursor::new(editing.selection.end));
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(start, end)));
            state.store(ctx, edit_id);
            ctx.memory_mut(|m| m.request_focus(edit_id));
        }
        let pending = editing.pending.clone();
        let mut output = None;
        // Where the box is, however far off screen; not to be dragged about.
        egui::Area::new(Id::new(("text-editor-area", id.to_nm())))
            .fixed_pos(pos2(inner.min.x, inner.min.y + down))
            .constrain(false)
            .movable(false)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                ui.set_clip_rect(clip);
                ui.set_width(inner.width().max(20.0));
                // Laid out from the box as typing leaves it, so what's typed
                // shows in the format it's typed in.
                let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                    let shown = if text.as_str() == words.text() { words.clone() } else { words.edited(text.as_str(), pending.as_ref()) };
                    let job = job_of(ui.ctx(), fonts, &shown, pending.as_ref(), px, wrap, align);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let edit = egui::TextEdit::multiline(&mut editing.text)
                    .id(edit_id)
                    .frame(egui::Frame::NONE)
                    .background_color(Color32::TRANSPARENT)
                    .desired_width(inner.width().max(20.0))
                    .desired_rows(1)
                    .horizontal_align(align)
                    .margin(Margin::ZERO)
                    .layouter(&mut layouter);
                output = Some(edit.show(ui));
            });
        // The box being typed into, marked as it is.
        let ring: Vec<Pos2> = corners.iter().copied().chain(corners.first().copied()).collect();
        ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("text-editor-frame"))).add(Shape::dashed_line(&ring, Stroke::new(1.0, ACCENT), 4.0, 3.0));
        let Some(output) = output else { return };
        // Without the keyboard the editor shows nothing picked out, so what
        // was is shown here, to be seen while it's styled from the panel.
        if !output.response.has_focus() && !editing.selection.is_empty() {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("text-editor-picked"))).with_clip_rect(clip);
            paint_picked_out(&painter, &output.galley, output.galley_pos, editing.selection.clone());
        }
        let changed = output.response.changed();
        if changed {
            editing.typed = true;
            let typed = words.edited(&editing.text, editing.pending.as_ref());
            let mut changed_markup = m.clone();
            changed_markup.extras.text = Some(typed);
            changed_markup.meta.modified_ms = Some(chrono::Utc::now().timestamp_millis());
            if let Some(doc) = self.doc.as_mut() {
                doc.session.apply_merged(Command::ChangeMeasure(Box::new(changed_markup)));
            }
        }
        // What's picked out, while it has the keyboard; moving the caret lets
        // go of a format set for typing at it.
        if let (true, Some(range), Some(editing)) = (output.response.has_focus(), output.cursor_range, self.text_editing.as_mut()) {
            let range = range.as_sorted_char_range();
            let selection = range.start.0..range.end.0;
            if selection != editing.selection {
                if !changed {
                    editing.pending = None;
                }
                editing.selection = selection;
            }
        }
    }

    /// Resizes text box `id` by corner `corner`, to the pointer at `pos`.
    pub(super) fn drag_text_corner(&mut self, sheet: usize, id: MarkupId, corner: usize, pos: Pos2) {
        let Some(point) = self.pdf_point(sheet, pos) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let Geometry::Polygon { pts, .. } = &markup.geometry else { return };
        let Some(corners) = markup_model::box_resized(pts, corner, Pt::new(f64::from(point.0), f64::from(point.1)), LEAST_BOX) else { return };
        let mut resized = markup.clone();
        resized.geometry = Geometry::Polygon { pts: corners, holes: Vec::new() };
        doc.session.apply_merged(Command::ChangeMeasure(Box::new(resized)));
    }

    /// Points text box `id`'s arrow at the pointer at `pos`.
    pub(super) fn drag_callout_tip(&mut self, sheet: usize, id: MarkupId, pos: Pos2) {
        let Some(point) = self.pdf_point(sheet, pos) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let mut pointed = markup.clone();
        if let Some(text) = pointed.extras.text.as_mut() {
            text.callout = Some(Pt::new(f64::from(point.0), f64::from(point.1)));
        }
        doc.session.apply_merged(Command::ChangeMeasure(Box::new(pointed)));
    }

    /// Gives text box `id` an arrow, or takes its arrow away. A new arrow
    /// points down and left of the box, to be dragged where it's wanted.
    pub(super) fn set_callout(&mut self, id: MarkupId, on: bool) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(markup) = doc.session.measures().get(id) else { return };
        let Geometry::Polygon { pts, .. } = &markup.geometry else { return };
        let Some(frame) = Frame::of(pts) else { return };
        let mut changed = markup.clone();
        if let Some(text) = changed.extras.text.as_mut() {
            text.callout = on.then(|| frame.to_user(-36.0, -36.0));
        }
        doc.session.apply(Command::ChangeMeasure(Box::new(changed)));
    }
}

/// Shades the characters in `picked` of `galley`, laid out at `at`, as a
/// selection is shaded.
fn paint_picked_out(painter: &egui::Painter, galley: &egui::Galley, at: Pos2, picked: Range<usize>) {
    let shade = ACCENT.gamma_multiply(0.3);
    let mut index = 0;
    for row in &galley.rows {
        let top = at.y + row.pos.y;
        let bottom = top + row.row.size.y;
        let mut span: Option<(f32, f32)> = None;
        for glyph in &row.row.glyphs {
            if picked.contains(&index) {
                let left = at.x + row.pos.x + glyph.pos.x;
                let right = left + glyph.advance_width.max(1.0);
                span = Some(span.map_or((left, right), |(l, r)| (l.min(left), r.max(right))));
            }
            index += 1;
        }
        if let Some((left, right)) = span {
            painter.rect_filled(Rect::from_min_max(pos2(left, top), pos2(right, bottom)), CornerRadius::ZERO, shade);
        }
        if row.ends_with_newline {
            index += 1;
        }
    }
}

/// A box's words as egui lays them out in the editor: each run in its own
/// font, size and colour, at `px` screen points to a point of size, wrapped
/// at `wrap`. An empty box is laid out in the format typing will take, so
/// the caret is its height.
fn job_of(ctx: &egui::Context, fonts: &mut Fonts, words: &TextBox, typing: Option<&RunFormat>, px: f32, wrap: f32, align: Align) -> LayoutJob {
    let mut format_of = |f: &RunFormat| {
        let size = (f.size as f32 * px).clamp(2.0, 600.0);
        let (font_id, slant) = fonts.font(ctx, f, size);
        let colour = to_color32(f.colour);
        let underline = if f.underline { Stroke::new((size * 0.06).max(1.0), colour) } else { Stroke::NONE };
        TextFormat { font_id, color: colour, italics: slant, underline, ..Default::default() }
    };
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.halign = align;
    let last = words.paragraphs.len().saturating_sub(1);
    for (i, paragraph) in words.paragraphs.iter().enumerate() {
        for run in paragraph.runs.iter().filter(|r| !r.text.is_empty()) {
            job.append(&run.text, 0.0, format_of(&run.format));
        }
        if i < last {
            let own = paragraph.runs.last().map_or_else(RunFormat::default, |r| r.format.clone());
            job.append("\n", 0.0, format_of(&own));
        }
    }
    if job.text.is_empty() {
        let typing = typing.cloned().unwrap_or_else(|| words.format());
        job.append("", 0.0, format_of(&typing));
    }
    job
}

/// Draws the text box being dragged out: its box, and an arrow's line.
pub(super) fn paint_text_drag(painter: &egui::Painter, rect: Rect, g: &PageGeometry, start: (f32, f32), end: (f32, f32), arrow: bool) {
    let at = |(x, y): (f32, f32)| {
        let (fx, fy) = g.to_view(x, y);
        pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height())
    };
    let (from, to) = (at(start), at(end));
    let stroke = Stroke::new(1.5, ACCENT);
    if arrow {
        painter.line_segment([from, to], stroke);
        painter.circle_filled(from, 3.0, ACCENT);
    } else {
        let area = Rect::from_two_pos(from, to);
        painter.rect_filled(area, CornerRadius::ZERO, ACCENT.gamma_multiply(0.06));
        painter.rect_stroke(area, CornerRadius::ZERO, stroke, StrokeKind::Middle);
    }
}

#[cfg(test)]
mod tests {
    use super::super::quantities::tests::Table;
    use super::*;

    /// The test file, its first page upright and 600 by 800.
    fn upright() -> Table {
        let mut table = Table::named(&[]);
        let doc = table.app.doc.as_mut().unwrap();
        doc.geometry[0] = Some(PageGeometry { rotation: 0, bounds: crate::model::PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } });
        table
    }

    fn boxes(app: &App) -> Vec<MeasureMarkup> {
        app.doc.as_ref().unwrap().session.measures().iter().filter(|(m, _)| m.kind == MeasureKind::Text).map(|(m, _)| m.clone()).collect()
    }

    #[test]
    fn a_box_is_dragged_out_typed_into_and_one_left_empty_goes_again() {
        let mut table = upright();
        let app = &mut table.app;
        app.take_up_text(false);
        assert_eq!(app.held_tool(), Some(tools::ToolKey::Text { arrow: false }));
        assert!(!app.selecting());

        app.put_down_text(0, (100.0, 700.0), (300.0, 600.0), false);
        let [put] = &boxes(app)[..] else { panic!("one box") };
        let frame = Frame::of(match &put.geometry { Geometry::Polygon { pts, .. } => pts, _ => panic!("a polygon") }).unwrap();
        assert_eq!((frame.width.round(), frame.height.round()), (200.0, 100.0), "the size it was dragged out");
        assert_eq!(frame.to_user(0.0, 0.0), Pt::new(100.0, 600.0), "its bottom left first, upright");
        let id = put.id;

        // Typed into, it keeps the words, and they undo as one step.
        let editing = app.text_editing.as_mut().unwrap();
        editing.text = "Existing kerb".into();
        editing.typed = true;
        let retyped = put.extras.text.as_ref().unwrap().retyped("Existing kerb");
        let mut changed = put.clone();
        changed.extras.text = Some(retyped);
        app.doc.as_mut().unwrap().session.apply_merged(Command::ChangeMeasure(Box::new(changed)));
        app.finish_text_edit();
        assert_eq!(boxes(app)[0].extras.text.as_ref().unwrap().text(), "Existing kerb");

        // Put down and left: gone again, and nothing to undo for it.
        app.put_down_text(0, (100.0, 400.0), (100.0, 400.0), false);
        assert_eq!(boxes(app).len(), 2, "a click puts one down a usual size");
        app.finish_text_edit();
        assert_eq!(boxes(app).iter().map(|m| m.id).collect::<Vec<_>>(), vec![id]);
    }

    #[test]
    fn what_s_picked_out_in_a_box_being_typed_into_is_styled_alone() {
        let mut table = upright();
        let app = &mut table.app;
        app.put_down_text(0, (100.0, 700.0), (300.0, 600.0), false);
        let id = boxes(app)[0].id;
        let typed = boxes(app)[0].extras.text.as_ref().unwrap().edited("Existing kerb", None);
        let mut changed = boxes(app)[0].clone();
        changed.extras.text = Some(typed);
        app.doc.as_mut().unwrap().session.apply(Command::ChangeMeasure(Box::new(changed)));

        // "kerb" picked out and made bold and red.
        app.text_editing.as_mut().unwrap().selection = 9..13;
        app.toggle_editing(|f| &mut f.bold);
        app.restyle_editing(|f| f.colour = [1.0, 0.0, 0.0]);
        let words = boxes(app)[0].extras.text.clone().unwrap();
        let runs: Vec<(&str, bool, f32)> = words.paragraphs[0].runs.iter().map(|r| (r.text.as_str(), r.format.bold, r.format.colour[0])).collect();
        assert_eq!(runs, vec![("Existing ", false, 0.0), ("kerb", true, 1.0)]);
        assert_eq!(app.editing_formats(id).unwrap().len(), 1, "what's picked out is all one format");

        // Picked out across both, bold is mixed: a toggle makes it all bold.
        app.text_editing.as_mut().unwrap().selection = 0..13;
        assert_eq!(app.editing_formats(id).unwrap().len(), 2);
        app.toggle_editing(|f| &mut f.bold);
        assert!(boxes(app)[0].extras.text.as_ref().unwrap().paragraphs[0].runs.iter().all(|r| r.format.bold));

        // Nothing picked out: italic is for what's typed next, and the box
        // is as it was until then.
        app.text_editing.as_mut().unwrap().selection = 13..13;
        app.toggle_editing(|f| &mut f.italic);
        assert!(app.editing_formats(id).unwrap()[0].italic);
        assert!(!boxes(app)[0].extras.text.as_ref().unwrap().paragraphs[0].runs.iter().any(|r| r.format.italic));
    }

    #[test]
    fn a_box_s_words_are_laid_out_once_until_it_or_egui_s_glyphs_change() {
        let mut table = upright();
        let g = PageGeometry { rotation: 0, bounds: crate::model::PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } };
        table.app.put_down_text(0, (100.0, 700.0), (300.0, 600.0), false);
        let mut m = boxes(&table.app).remove(0);
        m.extras.text = Some(m.extras.text.as_ref().unwrap().edited("Existing kerb", None));
        table.app.doc.as_mut().unwrap().session.apply(Command::ChangeMeasure(Box::new(m.clone())));
        table.app.text_editing = None;
        let ctx = table.ctx.clone();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 800.0));
        let mut frame = |zoom: f32| {
            ctx.set_zoom_factor(zoom);
            let mut first = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let app = &mut table.app;
                let doc = app.doc.as_ref().unwrap();
                let items = measure::stack_runs(doc, 0).into_iter().flat_map(|(_, items)| items).collect::<Vec<_>>();
                paint_text_boxes(&ui.ctx().layer_painter(LayerId::background()), &mut app.text_fonts, rect, &g, None, &[], None, &items);
                first = app.text_fonts.drawn.get(&m.id).and_then(|d| d.words.first()).map(|w| Arc::clone(&w.galley));
            });
            output.textures_delta.clear();
            first.expect("the box's words laid out")
        };
        // The first frames load the font; then the same galleys each frame.
        frame(1.0);
        frame(1.0);
        let (a, b) = (frame(1.0), frame(1.0));
        assert!(Arc::ptr_eq(&a, &b), "laid out once, drawn again");
        // The screen's scale changed, egui starts its glyphs again: laid
        // out again, not drawn from glyphs that have gone.
        frame(2.0);
        let c = frame(2.0);
        assert!(!Arc::ptr_eq(&b, &c));
    }

    #[test]
    fn a_box_with_an_arrow_points_where_the_drag_started() {
        let mut table = upright();
        let app = &mut table.app;
        app.take_up_text(true);
        app.put_down_text(0, (100.0, 100.0), (200.0, 200.0), true);
        let put = boxes(app).remove(0);
        assert_eq!(put.extras.text.as_ref().unwrap().callout, Some(Pt::new(100.0, 100.0)));
        let Geometry::Polygon { pts, .. } = &put.geometry else { panic!("a polygon") };
        assert_eq!(pts[0], Pt::new(200.0, 200.0), "the box beyond where the drag ended, away from the tip");
        app.finish_text_edit();
        assert!(boxes(app).is_empty(), "an arrow left with no words goes too");
    }
}
