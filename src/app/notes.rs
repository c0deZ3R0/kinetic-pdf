//! Highlight colours, the popup that edits a highlight and its note, the notes
//! panel, and the author name kept between runs.

use super::*;

pub(super) const COLORS: [(&str, Rgb); 4] = [
    ("Yellow", [1.0, 0.93, 0.25]),
    ("Green", [0.56, 0.93, 0.45]),
    ("Blue", [0.45, 0.76, 1.0]),
    ("Pink", [1.0, 0.62, 0.8]),
];

pub(super) const POPUP_WIDTH: f32 = 300.0;

/* ------------------------------------------------------------------ *
 * The author name, remembered between runs
 * ------------------------------------------------------------------ */

pub(super) fn author_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("pdf-annotate").join("author.txt"))
}

pub(super) fn load_author() -> String {
    author_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

pub(super) fn save_author(name: &str) {
    let Some(path) = author_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, name.trim());
}

impl App {
    pub(super) fn author_name(&self) -> String {
        match self.author.trim() {
            "" => "me".to_owned(),
            name => name.to_owned(),
        }
    }

    /* -------------------------------------------------------------- *
     * The popup: create a highlight, or edit an existing one
     * -------------------------------------------------------------- */

    pub(super) fn open_edit_popup(&mut self, uid: u64, anchor: Anchor) {
        let Some(entry) = self.entry(uid) else { return };
        self.popup = Some(Popup {
            mode: PopupMode::Edit(uid),
            anchor,
            color: entry.hl.color,
            note: entry.hl.comment.clone(),
            just_opened: true,
            height: 250.0,
        });
        self.active = Some(uid);
    }

    /// Scroll a highlight into view and open its note.
    pub(super) fn reveal(&mut self, uid: u64) {
        let Some(entry) = self.entry(uid) else { return };
        let Some(q) = entry.hl.quads.first().copied() else { return };
        let page = entry.hl.page;
        self.scroll_to_box(page, &q);
        let anchor = Anchor { page, x: (q.left + q.right) / 2.0, y: q.bottom };
        self.open_edit_popup(uid, anchor);
    }

    pub(super) fn show_popup(&mut self, ctx: &egui::Context) {
        let Some(popup) = &self.popup else { return };
        let Some(doc) = &self.doc else { return };
        // Its page scrolled out of view: hide it until the page comes back.
        let Some(rect) = self.page_rects.get(&popup.anchor.page).copied() else { return };
        let Some(geometry) = doc.geometry[popup.anchor.page] else { return };

        // What the popup is about.
        let (title, quote, meta, create, color_locked) = match &popup.mode {
            PopupMode::Create(pending) => {
                let quote = pending.iter().map(|p| p.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
                let first = pending.first().map_or(0, |p| p.page) + 1;
                let last = pending.last().map_or(0, |p| p.page) + 1;
                let meta = if first == last { format!("Page {first}") } else { format!("Pages {first}-{last}") };
                ("New highlight", quote, meta, true, false)
            }
            PopupMode::Edit(uid) => {
                let Some(entry) = self.entry(*uid) else { return };
                let mut meta = format!("Page {}", entry.hl.page + 1);
                if !entry.hl.author.is_empty() {
                    meta += &format!(" · {}", entry.hl.author);
                }
                ("Note", entry.hl.snippet.clone(), meta, false, !entry.is_new())
            }
        };
        let (fx, fy) = geometry.to_view(popup.anchor.x, popup.anchor.y);
        let point = pos2(rect.min.x + fx * rect.width(), rect.min.y + fy * rect.height());

        // Read before the text box sees them: Esc cancels, Ctrl+Enter saves.
        let (escape, commit_key) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::COMMAND, Key::Enter))
        });

        // Below the text if it fits, otherwise above it.
        let screen = ctx.content_rect();
        let gap = 14.0;
        let below = point.y + gap + popup.height <= screen.max.y - 8.0 || point.y - gap - popup.height < screen.min.y + 8.0;
        let x = (point.x - POPUP_WIDTH / 2.0)
            .clamp(screen.min.x + 8.0, (screen.max.x - POPUP_WIDTH - 8.0).max(screen.min.x + 8.0));
        let y = if below { point.y + gap } else { point.y - gap - popup.height };

        let Some(popup) = self.popup.as_mut() else { return };
        let mut action = PopupAction::None;
        let area_id = Id::new("highlight-popup");

        let area = egui::Area::new(area_id).order(Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
            Frame::NONE
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(14))
                .shadow(soft_shadow())
                .show(ui, |ui| {
                    ui.set_width(POPUP_WIDTH - 30.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 10.0);

                    // Header: the colour, what this is, where it is, and a close button.
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 6.0, to_color32(popup.color));
                        ui.painter().circle_stroke(dot.center(), 6.0, Stroke::new(1.0, Color32::from_black_alpha(30)));
                        ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let close = paint_button(ui, "×", FontId::proportional(17.0), Tone::Ghost, false, vec2(26.0, 26.0));
                            if close.on_hover_text("Close (Esc)").clicked() {
                                action = PopupAction::Cancel;
                            }
                            ui.label(RichText::new(&meta).size(12.0).color(MUTED));
                        });
                    });

                    if !quote.is_empty() {
                        quote_card(ui, &quote, to_color32(popup.color));
                    }

                    if color_locked {
                        ui.label(RichText::new("Saved highlights keep their colour.").size(12.0).color(MUTED));
                    } else {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.label(RichText::new("Colour").size(12.0).color(MUTED));
                            ui.add_space(8.0);
                            for (name, rgb) in COLORS {
                                let selected = rgb.iter().zip(popup.color).all(|(a, b)| (a - b).abs() < 0.02);
                                if swatch(ui, rgb, selected).on_hover_text(name).clicked() {
                                    popup.color = rgb;
                                }
                            }
                        });
                    }

                    let note = ui.add(
                        TextEdit::multiline(&mut popup.note)
                            .hint_text("Add a note (optional)…")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(10, 8)),
                    );
                    if popup.just_opened {
                        note.request_focus();
                        popup.just_opened = false;
                    }

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if !create && styled_button(ui, "Delete", Tone::Danger, false).clicked() {
                            action = PopupAction::Delete;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if styled_button(ui, if create { "Highlight" } else { "Save" }, Tone::Primary, false).clicked() {
                                action = PopupAction::Commit;
                            }
                            if styled_button(ui, "Cancel", Tone::Secondary, false).clicked() {
                                action = PopupAction::Cancel;
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        keycap(ui, "Ctrl+Enter");
                        ui.label(RichText::new("to save").size(11.5).color(MUTED));
                        ui.add_space(6.0);
                        keycap(ui, "Esc");
                        ui.label(RichText::new("to cancel").size(11.5).color(MUTED));
                    });
                });
        });
        popup.height = area.response.rect.height();

        // A small pointer from the card to the text it's about, drawn on the
        // card's own layer so it sits on top of the card's border.
        let card = area.response.rect;
        let nub_x = point.x.clamp(card.min.x + 22.0, card.max.x - 22.0);
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, area_id));
        let (edge, tip, fill_base) = if below {
            (card.min.y, card.min.y - 7.0, card.min.y + 1.5)
        } else {
            (card.max.y, card.max.y + 7.0, card.max.y - 1.5)
        };
        // Clockwise on screen, for clean anti-aliasing.
        let points = if below {
            vec![pos2(nub_x - 8.0, fill_base), pos2(nub_x, tip), pos2(nub_x + 8.0, fill_base)]
        } else {
            vec![pos2(nub_x + 8.0, fill_base), pos2(nub_x, tip), pos2(nub_x - 8.0, fill_base)]
        };
        painter.add(Shape::convex_polygon(points, SURFACE, Stroke::NONE));
        let border = Stroke::new(1.0, BORDER);
        painter.line_segment([pos2(nub_x - 8.0, edge), pos2(nub_x, tip)], border);
        painter.line_segment([pos2(nub_x, tip), pos2(nub_x + 8.0, edge)], border);

        let action = if escape {
            PopupAction::Cancel
        } else if commit_key {
            PopupAction::Commit
        } else {
            action
        };
        match action {
            PopupAction::Commit => self.commit_popup(),
            PopupAction::Cancel => self.popup = None,
            PopupAction::Delete => {
                if let Some(PopupMode::Edit(uid)) = self.popup.as_ref().map(|p| &p.mode) {
                    let uid = *uid;
                    self.remove(uid);
                }
            }
            PopupAction::None => {}
        }
    }

    pub(super) fn commit_popup(&mut self) {
        let Some(popup) = self.popup.take() else { return };
        let author = self.author_name();
        let Some(doc) = self.doc.as_mut() else { return };

        match popup.mode {
            PopupMode::Create(pending) => {
                for p in pending {
                    doc.highlights.push(Entry {
                        uid: next_uid(),
                        hl: Highlight {
                            key: None,
                            page: p.page,
                            quads: p.quads,
                            color: popup.color,
                            comment: popup.note.clone(),
                            author: author.clone(),
                            snippet: p.text,
                        },
                    });
                }
            }
            PopupMode::Edit(uid) => {
                let Some(entry) = doc.highlights.iter_mut().find(|e| e.uid == uid) else { return };
                entry.hl.comment = popup.note.clone();
                match entry.hl.key {
                    // A colour change only applies to highlights not yet in the
                    // file. A saved one may carry an appearance stream written
                    // by another viewer, which would keep showing the old colour.
                    None => entry.hl.color = popup.color,
                    Some(key) => {
                        doc.edits.insert(key, popup.note);
                    }
                }
            }
        }
        doc.dirty = true;
    }

    pub(super) fn remove(&mut self, uid: u64) {
        self.popup = None;
        if self.active == Some(uid) {
            self.active = None;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(index) = doc.highlights.iter().position(|e| e.uid == uid) else { return };
        let entry = doc.highlights.remove(index);
        if let Some(key) = entry.hl.key {
            doc.deletes.push(key);
            doc.edits.remove(&key);
        }
        doc.dirty = true;
    }

    /* -------------------------------------------------------------- *
     * Side panels
     * -------------------------------------------------------------- */

    /// A white panel on the right with a fixed header, and optionally a fixed
    /// footer, around a body that fills the rest.
    pub(super) fn side_panel(
        &mut self,
        ui: &mut Ui,
        id: &str,
        header: impl FnOnce(&mut Self, &mut Ui),
        footer: Option<fn(&mut Self, &mut Ui)>,
        body: impl FnOnce(&mut Self, &mut Ui),
    ) {
        egui::Panel::right(Id::new(id))
            .frame(Frame::NONE.fill(SURFACE))
            .default_size(320.0)
            .min_size(220.0)
            .show(ui, |ui| {
                let margin = Frame::NONE.inner_margin(Margin::symmetric(14, 12));
                egui::Panel::top(Id::new((id, "head"))).frame(margin).show(ui, |ui| header(self, ui));
                if let Some(footer) = footer {
                    egui::Panel::bottom(Id::new((id, "foot"))).frame(margin).show(ui, |ui| footer(self, ui));
                }
                egui::CentralPanel::default().frame(Frame::NONE).show(ui, |ui| body(self, ui));
            });
    }

    pub(super) fn notes_panel(&mut self, ui: &mut Ui) {
        self.side_panel(
            ui,
            "notes",
            |app, ui| {
                let (count, done) = app.doc.as_ref().map_or((0, true), |d| (d.highlights.len(), d.highlights_done));
                let detail = match (count, done) {
                    (0, true) => String::new(),
                    (_, true) => format!("{count} in this file"),
                    (_, false) => format!("{count} so far, still reading…"),
                };
                panel_heading(ui, "Notes", detail);
            },
            Some(|app, ui| {
                ui.label(RichText::new("Your name on new notes").size(12.0).color(MUTED));
                let name = ui.add(
                    TextEdit::singleline(&mut app.author)
                        .hint_text("me")
                        .char_limit(60)
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(8, 6)),
                );
                if name.changed() {
                    save_author(&app.author);
                }
            }),
            |app, ui| {
                egui::ScrollArea::vertical().id_salt("notes-list").auto_shrink(false).show(ui, |ui| {
                    app.note_rows(ui);
                });
            },
        );
    }

    pub(super) fn note_rows(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else {
            empty_note(ui, "Open a PDF to see its highlights.");
            return;
        };
        if doc.highlights.is_empty() {
            let message = if doc.highlights_done {
                "No highlights yet. Drag across some text to make one."
            } else {
                "Reading highlights…"
            };
            empty_note(ui, message);
            return;
        }

        let mut open = None;
        let mut delete = None;
        let clicked = ui.input(|i| i.pointer.primary_clicked());

        for e in &doc.highlights {
            let fill = if self.active == Some(e.uid) { NOTE_ACTIVE } else { SURFACE };
            let row = Frame::NONE.fill(fill).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let del = ui
                    .horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, to_color32(e.hl.color));
                        let mut label = format!("Page {}", e.hl.page + 1);
                        if !e.hl.author.is_empty() {
                            label += &format!(" · {}", e.hl.author);
                        }
                        if e.is_new() {
                            label += " · unsaved";
                        }
                        ui.label(RichText::new(label).size(12.0).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            paint_button(ui, "×", FontId::proportional(16.0), Tone::Ghost, false, vec2(24.0, 24.0))
                                .on_hover_text("Delete this highlight")
                        })
                        .inner
                    })
                    .inner;
                if !e.hl.snippet.is_empty() {
                    quote_block(ui, &e.hl.snippet, 110);
                }
                if e.hl.comment.is_empty() {
                    ui.label(RichText::new("No note").italics().color(SUBTLE));
                } else {
                    ui.label(RichText::new(e.hl.comment.as_str()).color(TEXT));
                }
                del
            });

            let rect = row.response.rect;
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, ROW_RULE));
            if row.inner.clicked() {
                delete = Some(e.uid);
            } else if clicked && ui.rect_contains_pointer(rect) && !row.inner.hovered() {
                open = Some(e.uid);
            }
        }

        if let Some(uid) = delete {
            self.remove(uid);
        } else if let Some(uid) = open {
            self.reveal(uid);
        }
    }
}
