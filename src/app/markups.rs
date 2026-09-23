//! The drawing tools: choosing one, drawing markups with it, picking them out
//! again, and showing them until the page's own drawing does.

use super::*;
use markup_model::markup::WidthUnit;

use super::tools::{ToolKey, ToolSettings};
use crate::markup;
use crate::session::MarkupEntry;

pub(super) const MARKUP_COLORS: [(&str, Rgb); 4] =
    [("Red", markup::DEFAULT_COLOR), ("Blue", [0.15, 0.39, 0.92]), ("Green", [0.09, 0.6, 0.27]), ("Black", [0.1, 0.1, 0.1])];

/// The three thicknesses on the toolbar, in pixels on screen, as the details
/// panel's slider sets them: a line stays the thickness it was set to however
/// far the drawing is zoomed.
pub(super) const WIDTHS: [(&str, f32); 3] = [("Thin", 1.0), ("Medium", 2.0), ("Thick", 4.0)];

/// The key choosing each of `MarkupKind::TOOLS`.
const TOOL_KEYS: [Key; 5] = [Key::P, Key::R, Key::E, Key::L, Key::A];

/// The key taking up the highlighter.
const HIGHLIGHTER_KEY: Key = Key::H;

/// The picture on a drawing tool's button.
pub(super) fn tool_icon(kind: MarkupKind) -> Icon {
    match kind {
        MarkupKind::Pen => Icon::Pen,
        MarkupKind::Rectangle => Icon::Rectangle,
        MarkupKind::Ellipse => Icon::Ellipse,
        MarkupKind::Line => Icon::Line,
        MarkupKind::Arrow | MarkupKind::Other => Icon::Arrow,
    }
}

/// Screen points from a markup's box that still pick it.
const PICK_SLACK: f32 = 3.0;

/// Screen points a pen moves before its stroke takes another point.
pub(super) const PEN_SPACING: f32 = 1.5;

/// Screen points a drag has to reach to make a markup rather than a click.
const LEAST_DRAWN: f32 = 3.0;

/// The markup on `page`, drawn at `rect`, at `(x, y)` in PDF user space: the
/// smallest whose box is within reach, so one inside another can be picked.
pub(super) fn markup_at(doc: &Doc, page: usize, rect: Rect, (x, y): (f32, f32)) -> Option<u64> {
    let slack = PICK_SLACK * doc.sizes[page].x / rect.width();
    let area = |b: &PdfBox| b.width() * b.height();
    doc.session
        .markups()
        .iter()
        .filter(|e| e.markup.page == page)
        .filter(|e| {
            let b = e.markup.bounds;
            x >= b.left - slack && x <= b.right + slack && y >= b.bottom - slack && y <= b.top + slack
        })
        .min_by(|a, b| area(&a.markup.bounds).total_cmp(&area(&b.markup.bounds)))
        .map(|e| e.uid)
}

/// Follows the pointer to `point`: a pen stroke takes it once it's `spacing`
/// points from the last; the other tools move their second corner or end.
pub(super) fn follow(markup: &mut Markup, point: [f32; 2], spacing: f32) {
    match markup.kind {
        MarkupKind::Pen => {
            if markup.points.last().is_none_or(|last| (last[0] - point[0]).hypot(last[1] - point[1]) >= spacing) {
                markup.points.push(point);
            }
        }
        _ => {
            markup.points.truncate(1);
            markup.points.push(point);
        }
    }
}

/// After a save that changed how `pages` are drawn: their images are drawn
/// again, the old ones standing in meanwhile, and until they're done the
/// markups just saved show as drawn here.
pub(super) fn redraw_pages(doc: &mut Doc, pages: &[usize]) {
    for &page in pages {
        doc.redraw.insert(page);
        if let Some(texture) = doc.textures.get_mut(&page) {
            texture.complete = false;
        }
    }
    doc.tiles.retain(|key, _| !pages.contains(&key.page));
    doc.spares.retain(|(page, _), _| !pages.contains(page));
}

/// A rectangle's outline as a ring of points, for hatching it.
fn corners(area: Rect) -> Vec<Pos2> {
    vec![area.left_top(), area.right_top(), area.right_bottom(), area.left_bottom()]
}

/// An ellipse's outline as a ring of points. Enough of them that a hatch
/// stops on the curve rather than on a visible chord.
fn oval(area: Rect) -> Vec<Pos2> {
    const STEPS: usize = 64;
    let (c, r) = (area.center(), area.size() / 2.0);
    (0..STEPS)
        .map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / STEPS as f32;
            pos2(c.x + r.x * angle.cos(), c.y + r.y * angle.sin())
        })
        .collect()
}

/// The fill and whatever is ruled over it, under the outline -- the order the
/// file paints them in, so the screen and the page agree. `rings` is the
/// shape's outline in screen points.
fn paint_inside(painter: &egui::Painter, rings: &[Vec<Pos2>], m: &Markup, per_point: f32) {
    let Some(rgb) = m.style.fill.filter(|_| m.kind.fills()) else { return };
    let ring = &rings[0];
    let colour = to_color32(rgb).gamma_multiply(m.style.fill_opacity);
    painter.add(Shape::convex_polygon(ring.clone(), colour, Stroke::NONE));
    if !m.style.pattern.is_ruled() {
        return;
    }
    let ruling = to_color32(m.style.pattern_colour.unwrap_or(m.color)).gamma_multiply(m.style.pattern_opacity);
    let fill = measure::Fill { colour, pattern: m.style.pattern, ruling, cell: f64::from(m.style.pattern_size) };
    measure::paint_pattern(painter, rings, fill, per_point);
}

/// Draws a markup on the page drawn at `page`, `per_point` screen points to a
/// PDF point, the way its appearance in the file draws it.
fn paint_shape(painter: &egui::Painter, page: Rect, g: &PageGeometry, per_point: f32, m: &Markup) {
    let at = |[x, y]: [f32; 2]| {
        let (fx, fy) = g.to_view(x, y);
        pos2(page.min.x + fx * page.width(), page.min.y + fy * page.height())
    };
    let stroke = Stroke::new((m.width * per_point).max(1.0), to_color32(m.color).gamma_multiply(m.style.opacity));
    let dashed = |points: &[Pos2], closed: bool| line_style::paint_dashed(painter, points, closed, stroke, &m.style.dash, per_point);
    match (m.kind, &m.points[..]) {
        (MarkupKind::Rectangle, [a, b, ..]) => {
            let area = to_screen(page, g, &PdfBox::spanning(*a, *b));
            let ring = corners(area);
            paint_inside(painter, &[ring.clone()], m, per_point);
            if !dashed(&ring, true) { painter.rect_stroke(area, CornerRadius::ZERO, stroke, StrokeKind::Middle); }
        }
        (MarkupKind::Ellipse, [a, b, ..]) => {
            let area = to_screen(page, g, &PdfBox::spanning(*a, *b));
            let ring = oval(area);
            paint_inside(painter, &[ring.clone()], m, per_point);
            if !dashed(&ring, true) { painter.add(Shape::ellipse_stroke(area.center(), area.size() / 2.0, stroke)); }
        }
        (MarkupKind::Arrow, [from, to, ..]) => {
            let [left, right] = markup::arrow_head(*from, *to, m.width);
            if !dashed(&[at(*from), at(*to)], false) { painter.line_segment([at(*from), at(*to)], stroke); }
            painter.add(Shape::line(vec![at(left), at(*to), at(right)], stroke));
        }
        (_, points) => {
            let path: Vec<_> = points.iter().map(|&p| at(p)).collect();
            if !dashed(&path, false) { painter.add(Shape::line(path, stroke)); }
        }
    }
}

/// Draws what of `page`'s markups its own drawing doesn't show: new ones,
/// ones just saved until the page is drawn again, the one being drawn, and
/// saved ones removed, crossed out. Those picked out are outlined.
pub(super) fn paint_markups(painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, g: &PageGeometry, picked: &[u64], drag: Option<&Drag>) {
    let per_point = rect.width() / doc.sizes[page].x;
    let redrawing = doc.redraw.contains(&page);
    for e in doc.session.markups().iter().filter(|e| e.markup.page == page) {
        if e.markup.key.is_none() || redrawing {
            paint_shape(painter, rect, g, per_point, &e.markup);
        }
        if picked.contains(&e.uid) {
            let area = to_screen(rect, g, &e.markup.bounds).expand(PICK_SLACK);
            painter.rect_stroke(area, CornerRadius::same(2), Stroke::new(1.5, ACCENT), StrokeKind::Outside);
        }
    }
    for m in doc.session.erased().iter().filter(|m| m.page == page) {
        let area = to_screen(rect, g, &m.bounds).expand(PICK_SLACK);
        let stroke = Stroke::new(1.5, DANGER);
        painter.rect_stroke(area, CornerRadius::same(2), stroke, StrokeKind::Outside);
        painter.line_segment([area.left_top(), area.right_bottom()], stroke);
        painter.line_segment([area.right_top(), area.left_bottom()], stroke);
    }
    if let Some(Drag::Markup { markup: m, .. }) = drag {
        if m.page == page {
            paint_shape(painter, rect, g, per_point, m);
        }
    }
}

impl App {
    fn markup_entry(&self, uid: u64) -> Option<&MarkupEntry> {
        self.doc.as_ref()?.session.markup(uid)
    }

    /// PDF points to a screen point on sheet `sheet` as it's drawn now. The
    /// sheet's own size, not its page's: a turned sheet is as wide as its page
    /// is tall, and a screen point on it is worth that much less across.
    pub(super) fn points_per_screen(&self, sheet: usize) -> f32 {
        match (&self.doc, self.page_rects.get(&sheet)) {
            (Some(doc), Some(rect)) => arrange::sheet_size(doc, sheet).map_or(1.0, |size| size.x) / rect.width(),
            _ => 1.0,
        }
    }

    /// The row of drawing tools under the toolbar, with the colour and width
    /// new markups take.
    pub(super) fn tool_strip(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE.fill(SURFACE).inner_margin(Margin::symmetric(12, 6));
        egui::Panel::top("tools").frame(frame).show(ui, |ui| {
            ui.add_enabled_ui(self.doc.is_some(), |ui| {
                // Wrapped, not one line: there are two dozen buttons here, and
                // in a narrow window the row used to run off the right-hand
                // edge, leaving the colours and widths past it with no way to
                // reach them.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    let (can_undo, can_redo) = (self.can_undo(false), self.can_undo(true));
                    let undo_hint = if self.placing.is_some() { "Take back the last point (Ctrl+Z)" } else { "Undo (Ctrl+Z)" };
                    if ui.add_enabled_ui(can_undo, |ui| tool_button(ui, Icon::Undo, Tone::Secondary, false).on_hover_text(undo_hint)).inner.clicked() {
                        self.undo_step(false);
                    }
                    if ui.add_enabled_ui(can_redo, |ui| tool_button(ui, Icon::Redo, Tone::Secondary, false).on_hover_text("Redo (Ctrl+Y)")).inner.clicked() {
                        self.undo_step(true);
                    }
                    // The scale is on the rail down the left with the other
                    // panels, so it isn't among the tools here.
                    ui.separator();
                    self.measure_buttons(ui);
                    ui.separator();
                    let hint = "Select — pick out markups, measurements and highlights (V or Esc)";
                    if tool_button(ui, Icon::Select, Tone::Secondary, self.selecting()).on_hover_text(hint).clicked() {
                        self.take_up_select();
                    }
                    for (kind, key) in MarkupKind::TOOLS.into_iter().zip(TOOL_KEYS) {
                        let hint = format!("{} — draw with the {} ({})", kind.label(), kind.label().to_lowercase(), key.name());
                        if tool_button(ui, tool_icon(kind), Tone::Secondary, self.tool == Some(kind)).on_hover_text(hint).clicked() {
                            self.take_up_drawing(kind);
                        }
                    }
                    let hint = "Highlighter — drag across text to highlight it, or hold Ctrl and drag a box round it (H)";
                    if tool_button(ui, Icon::Highlighter, Tone::Secondary, self.highlighting()).on_hover_text(hint).clicked() {
                        self.take_up_highlighter();
                    }
                    // The quick way to set a colour or a thickness: what the
                    // details panel does one setting at a time, in one click,
                    // on whatever is picked out. What they are lit against is
                    // what they would change, so they read as that thing's
                    // colour and thickness rather than as a mode.
                    ui.separator();
                    let (shown_colour, shown_width) = self.quick_shown();
                    for (name, rgb) in MARKUP_COLORS {
                        let hint = format!("{name} — the colour of whatever is picked out, or of the next one drawn");
                        if swatch(ui, rgb, shown_colour == rgb).on_hover_text(hint).clicked() {
                            self.markup_color = rgb;
                            self.set_quick_colour(rgb);
                        }
                    }
                    ui.separator();
                    for (name, width) in WIDTHS {
                        let hint = format!("{name} — {} px, on whatever is picked out, or on the next one drawn", width);
                        if tool_button(ui, Icon::Width(width), Tone::Secondary, shown_width == width).on_hover_text(hint).clicked() {
                            self.markup_width = width;
                            self.set_quick_width(width);
                        }
                    }
                });
            });
        });
    }

    /// Letters choose tools, and Esc goes back to the Select tool, unless
    /// something is being typed or a popup is open.
    pub(super) fn tool_keys(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() || self.popup.is_some() || self.discarding.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (select, highlight, all, picked) = ctx.input_mut(|i| {
            let select = i.consume_key(Modifiers::NONE, Key::V) | i.consume_key(Modifiers::NONE, Key::Escape);
            let highlight = i.consume_key(Modifiers::NONE, HIGHLIGHTER_KEY);
            // Before the tool letters, so Ctrl+A isn't taken for the arrow.
            let all = i.consume_key(Modifiers::COMMAND, Key::A);
            (select, highlight, all, TOOL_KEYS.iter().position(|&key| i.consume_key(Modifiers::NONE, key)))
        });
        if all {
            self.take_up_select();
            self.pick_everything_on_page();
        }
        if select {
            self.take_up_select();
        }
        if highlight {
            self.take_up_highlighter();
        }
        if let Some(i) = picked {
            self.take_up_drawing(MarkupKind::TOOLS[i]);
        }
    }

    /// Whether the Select tool is in hand, which it is whenever nothing else
    /// is: a press picks out what is under it rather than drawing anything.
    pub(super) fn selecting(&self) -> bool {
        self.tool.is_none() && self.measure_tool.is_none() && !self.highlighter
    }

    /// Whether the highlighter is in hand. Any other tool taken up puts it
    /// down, so it is only ever in hand on its own.
    pub(super) fn highlighting(&self) -> bool {
        self.highlighter && self.tool.is_none() && self.measure_tool.is_none()
    }

    /// Puts down whatever is in hand, which leaves the Select tool.
    pub(super) fn take_up_select(&mut self) {
        self.set_measure_tool(None);
        self.tool = None;
        self.highlighter = false;
    }

    /// Takes up the highlighter, putting down any other tool.
    pub(super) fn take_up_highlighter(&mut self) {
        self.set_measure_tool(None);
        self.tool = None;
        self.highlighter = true;
    }

    /// Takes up a drawing tool, putting down any other.
    pub(super) fn take_up_drawing(&mut self, kind: MarkupKind) {
        self.set_measure_tool(None);
        self.tool = Some(kind);
        self.highlighter = false;
    }

    /// What the toolbar's swatches and widths would change: the measurement
    /// or markup picked out, or failing that the tool in hand. `None` when
    /// there is nothing to change, which is when nothing is picked out and
    /// nothing is in hand.
    fn quick_subject(&self) -> Option<(ToolKey, ToolSettings)> {
        let doc = self.doc.as_ref();
        if let Some(id) = self.active_measure {
            let markup = doc.and_then(|d| d.session.measures().get(id))?;
            return Some((ToolKey::of_measurement(markup.kind)?, ToolSettings::of_markup(markup)));
        }
        if let Some(entry) = self.active.and_then(|uid| doc?.session.markup(uid)) {
            return Some((ToolKey::Draw(entry.markup.kind), ToolSettings::of_drawing(&entry.markup)));
        }
        let key = self.held_tool()?;
        Some((key, self.tools.settings(key)))
    }

    /// The colour and thickness the toolbar lights up: those of whatever the
    /// buttons would change, so they read as that thing's own settings.
    fn quick_shown(&self) -> (Rgb, f32) {
        match self.quick_subject() {
            Some((_, settings)) => (settings.style.stroke, settings.style.width as f32),
            None => (self.markup_color, self.markup_width),
        }
    }

    /// Puts changed settings where the toolbar's quick buttons put them.
    fn quick_change(&mut self, change: impl Fn(&mut ToolSettings)) {
        // Several picked out all take it, as one step to undo.
        let picked = self.picked_rows();
        if picked.len() > 1 {
            self.change_each(&picked, change);
            return;
        }
        let Some((key, mut settings)) = self.quick_subject() else { return };
        change(&mut settings);
        match (self.active_measure, self.active) {
            (Some(id), _) => self.change_measurement(id, &settings),
            // A highlight is picked out by the same field and isn't ours to
            // restyle, so only a markup the session knows counts.
            (None, Some(uid)) if self.doc.as_ref().is_some_and(|d| d.session.markup(uid).is_some()) => {
                self.change_drawing(uid, &settings);
            }
            _ => self.tools.set(key, settings),
        }
    }

    /// A colour from the toolbar's swatches. The fill and what is ruled over
    /// it follow the line when they were the same colour as it, which is how
    /// a tool starts out: changing one colour shouldn't leave a blue outline
    /// round a red fill.
    fn set_quick_colour(&mut self, rgb: Rgb) {
        self.quick_change(|s| {
            let was = s.style.stroke;
            let follows = |c: &mut Option<Rgb>| {
                if *c == Some(was) {
                    *c = Some(rgb);
                }
            };
            follows(&mut s.style.fill);
            follows(&mut s.style.pattern_colour);
            follows(&mut s.style.label_colour);
            s.style.stroke = rgb;
        });
    }

    /// A thickness from the toolbar, in pixels on screen, as the details
    /// panel's own slider sets it.
    fn set_quick_width(&mut self, width: f32) {
        self.quick_change(|s| {
            s.style.width = f64::from(width);
            s.style.width_unit = WidthUnit::ScreenPixels;
        });
    }

    /// Starts drawing a markup with the tool in use at `pos` on sheet `sheet`.
    /// The markup belongs to the page that sheet shows; the pointer is
    /// followed on the sheet, which is where the user can see it.
    pub(super) fn start_markup(&mut self, sheet: usize, pos: Pos2) {
        let Some(page) = self.doc.as_ref().and_then(|doc| doc.sheet_page(sheet)) else { return };
        let (Some(kind), Some((x, y))) = (self.tool, self.pdf_point(sheet, pos)) else { return };
        let mut markup = Markup {
            key: None,
            page,
            kind,
            points: vec![[x, y]],
            bounds: PdfBox::spanning([x, y], [x, y]),
            color: self.markup_color,
            width: self.markup_width,
            style: Default::default(),
            name: String::new(),
            comment: String::new(),
            author: self.author_name(),
        };
        // Drawn with what the tool is set to, so the shape kept is the shape
        // shown while it was being drawn (see `pages.rs`).
        self.tools.settings(ToolKey::Draw(kind)).apply_to_drawing(&mut markup);
        self.drag = Some(Drag::Markup { markup, sheet });
        self.popup = None;
        self.active = None;
    }

    /// Keeps a markup just drawn, unless it was only a click.
    pub(super) fn add_markup(&mut self, mut markup: Markup) {
        let least = LEAST_DRAWN * self.points_per_screen(markup.page);
        let Some(doc) = self.doc.as_mut() else { return };
        if !markup::is_drawn(markup.kind, &markup.points, least) {
            return;
        }
        markup.bounds = markup::bounds(markup.kind, &markup.points, markup.width);
        doc.session.apply(Command::AddMarkup(markup));
    }

    /// Selects a markup and opens its note under it.
    pub(super) fn open_markup_popup(&mut self, uid: u64) {
        let Some(m) = self.markup_entry(uid).map(|e| &e.markup) else { return };
        let b = m.bounds;
        self.popup = Some(Popup {
            mode: PopupMode::Markup(uid),
            anchor: Anchor { page: m.page, x: (b.left + b.right) / 2.0, y: b.bottom },
            color: m.color,
            note: m.comment.clone(),
            just_opened: true,
            height: 250.0,
        });
        self.active = Some(uid);
    }

    /// What the popup says about a markup: its title, where it is and who
    /// made it, and whether its colour is fixed.
    pub(super) fn markup_popup_heading(&self, uid: u64) -> Option<(&'static str, String, bool)> {
        let m = &self.markup_entry(uid)?.markup;
        Some((m.kind.label(), byline(m.page, &m.author), m.key.is_some()))
    }
}
