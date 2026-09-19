//! The drawing tools: choosing one, drawing markups with it, picking them out
//! again, and showing them until the page's own drawing does.

use super::*;
use crate::markup;
use crate::session::MarkupEntry;

pub(super) const MARKUP_COLORS: [(&str, Rgb); 4] =
    [("Red", markup::DEFAULT_COLOR), ("Blue", [0.15, 0.39, 0.92]), ("Green", [0.09, 0.6, 0.27]), ("Black", [0.1, 0.1, 0.1])];

/// Stroke widths in points.
pub(super) const WIDTHS: [(&str, f32); 3] = [("Thin", 1.0), ("Medium", 2.0), ("Thick", 4.0)];

/// The key choosing each of `MarkupKind::TOOLS`.
const TOOL_KEYS: [Key; 5] = [Key::P, Key::R, Key::E, Key::L, Key::A];

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

/// Draws a markup on the page drawn at `page`, `per_point` screen points to a
/// PDF point, the way its appearance in the file draws it.
fn paint_shape(painter: &egui::Painter, page: Rect, g: &PageGeometry, per_point: f32, m: &Markup) {
    let at = |[x, y]: [f32; 2]| {
        let (fx, fy) = g.to_view(x, y);
        pos2(page.min.x + fx * page.width(), page.min.y + fy * page.height())
    };
    let stroke = Stroke::new((m.width * per_point).max(1.0), to_color32(m.color));
    match (m.kind, &m.points[..]) {
        (MarkupKind::Rectangle, [a, b, ..]) => {
            painter.rect_stroke(to_screen(page, g, &PdfBox::spanning(*a, *b)), CornerRadius::ZERO, stroke, StrokeKind::Middle);
        }
        (MarkupKind::Ellipse, [a, b, ..]) => {
            let area = to_screen(page, g, &PdfBox::spanning(*a, *b));
            painter.add(Shape::ellipse_stroke(area.center(), area.size() / 2.0, stroke));
        }
        (MarkupKind::Arrow, [from, to, ..]) => {
            let [left, right] = markup::arrow_head(*from, *to, m.width);
            painter.line_segment([at(*from), at(*to)], stroke);
            painter.add(Shape::line(vec![at(left), at(*to), at(right)], stroke));
        }
        (_, points) => {
            painter.add(Shape::line(points.iter().map(|&p| at(p)).collect(), stroke));
        }
    }
}

/// Draws what of `page`'s markups its own drawing doesn't show: new ones,
/// ones just saved until the page is drawn again, the one being drawn, and
/// saved ones removed, crossed out. The selected one is outlined.
pub(super) fn paint_markups(painter: &egui::Painter, doc: &Doc, page: usize, rect: Rect, g: &PageGeometry, active: Option<u64>, drag: Option<&Drag>) {
    let per_point = rect.width() / doc.sizes[page].x;
    let redrawing = doc.redraw.contains(&page);
    for e in doc.session.markups().iter().filter(|e| e.markup.page == page) {
        if e.markup.key.is_none() || redrawing {
            paint_shape(painter, rect, g, per_point, &e.markup);
        }
        if active == Some(e.uid) {
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
    if let Some(Drag::Markup(m)) = drag {
        if m.page == page {
            paint_shape(painter, rect, g, per_point, m);
        }
    }
}

impl App {
    fn markup_entry(&self, uid: u64) -> Option<&MarkupEntry> {
        self.doc.as_ref()?.session.markup(uid)
    }

    /// PDF points to a screen point on `page` as it's drawn now.
    pub(super) fn points_per_screen(&self, page: usize) -> f32 {
        match (&self.doc, self.page_rects.get(&page)) {
            (Some(doc), Some(rect)) => doc.sizes[page].x / rect.width(),
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
                    // The two panels go together, and apart from the tools:
                    // neither draws anything, they just open something to
                    // read.
                    ui.separator();
                    let scaling = self.sidebar == Sidebar::Scale;
                    if tool_button(ui, Icon::Scale, Tone::Secondary, scaling).on_hover_text("Scale — what this page measures at").clicked() {
                        self.sidebar = if scaling { Sidebar::None } else { Sidebar::Scale };
                        // Closing the scale panel puts down the tools that
                        // belong to it, and only those: a measurement tool in
                        // hand has nothing to do with this panel, and
                        // dropping it would take the tool's own panel with it.
                        if scaling && matches!(self.measure_tool, Some(MeasureTool::Calibrate | MeasureTool::Verify)) {
                            self.measure_tool = None;
                        }
                    }
                    if tool_button(ui, Icon::Quantities, Tone::Secondary, self.quantities_open)
                        .on_hover_text("Quantities — the table of everything measured")
                        .clicked()
                    {
                        self.quantities_open = !self.quantities_open;
                    }
                    ui.separator();
                    self.measure_buttons(ui);
                    ui.separator();
                    if tool_button(ui, Icon::Select, Tone::Secondary, self.tool.is_none()).on_hover_text("Select text and open notes (V or Esc)").clicked() {
                        self.tool = None;
                        self.measure_tool = None;
                    }
                    for (kind, key) in MarkupKind::TOOLS.into_iter().zip(TOOL_KEYS) {
                        let hint = format!("{} — draw with the {} ({})", kind.label(), kind.label().to_lowercase(), key.name());
                        if tool_button(ui, tool_icon(kind), Tone::Secondary, self.tool == Some(kind)).on_hover_text(hint).clicked() {
                            self.tool = Some(kind);
                            self.measure_tool = None;
                        }
                    }
                    ui.separator();
                    for (name, rgb) in MARKUP_COLORS {
                        if swatch(ui, rgb, self.markup_color == rgb).on_hover_text(name).clicked() {
                            self.markup_color = rgb;
                        }
                    }
                    ui.separator();
                    for (name, width) in WIDTHS {
                        if tool_button(ui, Icon::Width(width), Tone::Secondary, self.markup_width == width).on_hover_text(name).clicked() {
                            self.markup_width = width;
                        }
                    }
                });
            });
        });
    }

    /// Letters choose tools, and Esc goes back to selecting, unless something
    /// is being typed or a popup is open.
    pub(super) fn tool_keys(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() || self.popup.is_some() || self.discarding.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (select, picked) = ctx.input_mut(|i| {
            let select = i.consume_key(Modifiers::NONE, Key::V) | i.consume_key(Modifiers::NONE, Key::Escape);
            (select, TOOL_KEYS.iter().position(|&key| i.consume_key(Modifiers::NONE, key)))
        });
        if select {
            self.tool = None;
        }
        if let Some(i) = picked {
            self.tool = Some(MarkupKind::TOOLS[i]);
        }
    }

    /// Starts drawing a markup with the tool in use at `pos` on `page`.
    pub(super) fn start_markup(&mut self, page: usize, pos: Pos2) {
        let (Some(kind), Some((x, y))) = (self.tool, self.pdf_point(page, pos)) else { return };
        self.drag = Some(Drag::Markup(Markup {
            key: None,
            page,
            kind,
            points: vec![[x, y]],
            bounds: PdfBox::spanning([x, y], [x, y]),
            color: self.markup_color,
            width: self.markup_width,
            comment: String::new(),
            author: self.author_name(),
        }));
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
