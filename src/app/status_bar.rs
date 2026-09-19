//! The thin bar across the bottom of the window, above the quantities: the
//! zoom, how the page is fitted, and which page the view is on.

use super::*;

impl App {
    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        let frame = Frame::NONE
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(10, 3))
            .stroke(Stroke::new(1.0, INPUT_BORDER));
        egui::Panel::bottom("status-bar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.set_height(SLIM_HEIGHT);
                // The whole bar, taken before anything is laid out in it: the
                // page group is centred on the window, not on what is left of
                // the bar once the zoom controls have had their side.
                let bar = ui.max_rect();

                let has_doc = self.doc.is_some();
                ui.add_enabled_ui(has_doc, |ui| {
                    if slim_icon_button(ui, "−").on_hover_text("Zoom out (Ctrl+-)").clicked() {
                        self.zoom_by(-1);
                    }
                    let zoom = RichText::new(format!("{:.0}%", self.zoom * 100.0)).size(12.0).color(MUTED);
                    ui.add_sized(vec2(42.0, SLIM_HEIGHT), egui::Label::new(zoom));
                    if slim_icon_button(ui, "+").on_hover_text("Zoom in (Ctrl++, or Ctrl + mouse wheel)").clicked() {
                        self.zoom_by(1);
                    }
                    ui.add_space(2.0);
                    let fit_width = self.zoom_mode == ZoomMode::FitWidth;
                    if slim_button(ui, "Fit width", Tone::Secondary, fit_width).on_hover_text("Fit the page width (Ctrl+0)").clicked() {
                        self.zoom_mode = ZoomMode::FitWidth;
                    }
                    let fit_page = self.zoom_mode == ZoomMode::FitPage;
                    if slim_button(ui, "Fit page", Tone::Secondary, fit_page).on_hover_text("Fit a whole page").clicked() {
                        self.zoom_mode = ZoomMode::FitPage;
                    }
                });

                // The page and its sheet name sit on the centre line of the
                // window, so they are measured first and the gap before them
                // made to put their middle there. A group too wide for what is
                // left simply follows the zoom controls instead of sliding
                // underneath them.
                let width = self.page_group_width(ui);
                let here = ui.available_rect_before_wrap();
                let left = (bar.center().x - width / 2.0).max(here.min.x);
                ui.add_space(left - here.min.x);
                self.page_group(ui);
            });
        });
    }

    /// What the sheet name is on the page the view is on, if the file names
    /// it. Most PDFs label nothing, and then nothing is shown.
    fn sheet_name(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        doc.labels.get(self.current_page)?.clone()
    }

    /// How wide `page_group` will come out, so it can be centred. The pieces
    /// are the same ones it draws, in the same fonts.
    fn page_group_width(&self, ui: &Ui) -> f32 {
        let Some(pages) = self.doc.as_ref().map(|d| d.sizes.len()) else {
            return 12.0;
        };
        let text = |s: String, size: f32| ui.painter().layout_no_wrap(s, FontId::proportional(size), Color32::PLACEHOLDER).size().x;
        let spacing = ui.spacing().item_spacing.x;
        // The sheet name is left out on purpose: it trails off to the right so
        // that the page number itself stays on the centre line however long
        // the name is, rather than shifting with every sheet.
        PAGE_BOX_WIDTH + spacing + text(format!("of {pages}"), 12.0)
    }

    /// The page number is a box to type in: a number and Enter goes to that
    /// page. It shows where the view is the rest of the time, so it follows
    /// scrolling unless it's being typed in. The sheet name, where the file
    /// gives one, follows after a bar.
    fn page_group(&mut self, ui: &mut Ui) {
        let Some(pages) = self.doc.as_ref().map(|d| d.sizes.len()) else {
            ui.label(RichText::new("—").size(12.0).color(MUTED));
            return;
        };
        let box_id = Id::new("page-number");
        let typing = ui.memory(|m| m.has_focus(box_id));
        if !typing {
            self.page_box = (self.current_page + 1).to_string();
        }
        let field = TextEdit::singleline(&mut self.page_box)
            .id(box_id)
            .desired_width(30.0)
            .horizontal_align(Align::Center)
            // The box is taller than a line of text, which otherwise sits at
            // its top.
            .vertical_align(Align::Center)
            .font(FontId::proportional(12.0));
        let response = ui
            .add_sized(vec2(PAGE_BOX_WIDTH, SLIM_HEIGHT), field)
            .on_hover_text("Go to a page (Ctrl+G): type a number and press Enter");
        if std::mem::take(&mut self.page_box_focus) {
            self.page_box.clear();
            response.request_focus();
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            match self.page_box.trim().parse::<usize>() {
                Ok(page) if (1..=pages).contains(&page) => self.go_to_page(page - 1),
                // Anything else goes back to where the view is.
                _ => self.page_box = (self.current_page + 1).to_string(),
            }
        }
        ui.label(RichText::new(format!("of {pages}")).size(12.0).color(MUTED));
        if let Some(sheet) = self.sheet_name() {
            ui.label(RichText::new(format!("| {sheet}")).size(12.0).color(TEXT));
        }
    }
}

/// The page box is as wide as four figures, so the number sits in the middle
/// of it rather than against an edge.
const PAGE_BOX_WIDTH: f32 = 34.0;
