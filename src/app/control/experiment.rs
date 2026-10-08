//! Resolves an experimental proposal without editing; commits through normal session history.
use super::super::tools::{ToolKey, ToolSettings};
use super::*;
use crate::experiment::{self, Anchor, Preview};

pub(super) struct Draft {
    preview: Preview,
    target: DocumentTarget,
    placement: experiment::Placement,
    settings: ToolSettings,
    comment: String,
    file_page: usize,
    active_layer: markup_model::LayerId,
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::app_with_a_document;
    use super::*;
    fn run(app: &mut App, request: experiment::Request) -> api::Result {
        app.execute_control(api::Request {
            command: api::Command::Experiment { request },
            target: app.control_state().document,
        })
    }
    fn preview(app: &mut App, kind: &str, anchor: Anchor) -> Preview {
        let response = run(
            app,
            experiment::Request::Preview {
                version: 1,
                page: 1,
                kind: kind.into(),
                anchor,
                saved_tool: None,
                comment: "Demo note".into(),
            },
        )
        .unwrap();
        let Some(api::Data::Preview(preview)) = response.data else {
            panic!("missing preview")
        };
        preview
    }
    fn setup() -> App {
        let (mut app, _) = app_with_a_document();
        app.control.experimental = true;
        app.doc.as_mut().unwrap().measurements = MeasureRead::Ready;
        app.tools = super::super::super::tools::Tools::default();
        app.doc.as_mut().unwrap().geometry[0] = Some(PageGeometry {
            rotation: 0,
            bounds: PdfBox {
                left: 30.0,
                bottom: 50.0,
                right: 430.0,
                top: 650.0,
            },
        });
        app
    }
    #[test]
    fn commit_undo_redo_restores_preset_layers_and_colours_together() {
        for existing in [false, true] {
            let mut app = setup();
            if existing {
                let doc = app.doc.as_mut().unwrap();
                let mut layers = doc.session.layers().clone();
                layers.named_path("Structure/Walls");
                doc.session.load_layers(layers);
            }
            let before = app.doc.as_ref().unwrap().session.layers().clone();
            app.control_configure_tool(
                "Wall".into(),
                "Tests".into(),
                "draw.line".into(),
                Some(serde_json::json!({"defaults": {
                    "layer": "Structure/Walls", "layer_colour": [0.2, 0.4, 0.6]
                }})),
                false,
            )
            .unwrap();
            let response = run(
                &mut app,
                experiment::Request::Preview {
                    version: 1,
                    page: 1,
                    kind: "draw.line".into(),
                    anchor: Anchor::PdfPoints {
                        points: vec![[60.0, 100.0], [200.0, 300.0]],
                    },
                    saved_tool: Some(experiment::SavedTool {
                        name: "Wall".into(),
                        group: "Tests".into(),
                    }),
                    comment: String::new(),
                },
            )
            .unwrap();
            let Some(api::Data::Preview(proposal)) = response.data else {
                panic!("missing preview")
            };
            run(
                &mut app,
                experiment::Request::Commit {
                    preview: proposal.id,
                },
            )
            .unwrap();
            let doc = app.doc.as_mut().unwrap();
            let committed = doc.session.layers().clone();
            let shape = doc.session.measures().iter().next().unwrap().0;
            assert_eq!(
                committed.get(shape.layer).unwrap().colour,
                Some([0.2, 0.4, 0.6])
            );
            assert_ne!(committed, before);
            doc.session.undo();
            assert_eq!(doc.session.layers(), &before);
            assert_eq!(doc.session.measures().iter().count(), 0);
            assert!(!doc.session.is_dirty());
            doc.session.redo();
            assert_eq!(doc.session.layers(), &committed);
            assert_eq!(doc.session.measures().iter().count(), 1);
            assert!(doc.session.is_dirty());
        }
    }
    #[test]
    fn preview_freezes_saved_tool_settings_and_locked_layers_reject_commit() {
        let mut app = setup();
        let configure = |width, replace| api::Command::ConfigureTool {
            name: "Demo line".into(),
            group: "Tests".into(),
            kind: "draw.line".into(),
            settings: Some(serde_json::json!({"style":{"width":width}})),
            replace,
        };
        app.execute_control(api::Request {
            command: configure(9.0, false),
            target: None,
        })
        .unwrap();
        let response = run(
            &mut app,
            experiment::Request::Preview {
                version: 1,
                page: 1,
                kind: "draw.line".into(),
                anchor: Anchor::PdfPoints {
                    points: vec![[60.0, 100.0], [200.0, 300.0]],
                },
                saved_tool: Some(experiment::SavedTool {
                    name: "Demo line".into(),
                    group: "Tests".into(),
                }),
                comment: String::new(),
            },
        )
        .unwrap();
        let Some(api::Data::Preview(proposal)) = response.data else {
            panic!("missing preview")
        };
        app.execute_control(api::Request {
            command: configure(3.0, true),
            target: None,
        })
        .unwrap();
        run(
            &mut app,
            experiment::Request::Commit {
                preview: proposal.id,
            },
        )
        .unwrap();
        assert_eq!(
            app.doc
                .as_ref()
                .unwrap()
                .session
                .measures()
                .iter()
                .next()
                .unwrap()
                .0
                .style
                .width,
            9.0
        );
        let doc = app.doc.as_mut().unwrap();
        let mut layers = doc.session.layers().clone();
        layers.set_locked(doc.session.active_layer(), true);
        doc.session.apply(Command::SetLayers(layers));
        let proposal = preview(
            &mut app,
            "draw.rectangle",
            Anchor::PdfPoints {
                points: vec![[60.0, 100.0], [200.0, 300.0]],
            },
        );
        assert_eq!(
            run(
                &mut app,
                experiment::Request::Commit {
                    preview: proposal.id
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::Unavailable
        );
        assert_eq!(
            app.doc.as_ref().unwrap().session.measures().iter().count(),
            1
        );
    }
    #[test]
    fn all_drawing_proposals_use_the_normal_model_and_undo_history() {
        for rotation in 0..4 {
            for kind in [
                "draw.rectangle",
                "draw.ellipse",
                "draw.line",
                "draw.arrow",
                "draw.pen",
            ] {
                let mut app = setup();
                app.doc.as_mut().unwrap().geometry[0]
                    .as_mut()
                    .unwrap()
                    .rotation = rotation;
                let proposal = preview(
                    &mut app,
                    kind,
                    Anchor::PageFractions {
                        points: vec![[0.2, 0.3], [0.8, 0.9]],
                    },
                );
                assert!(
                    !app.has_unsaved_work(),
                    "preview must not mutate the document"
                );
                run(
                    &mut app,
                    experiment::Request::Commit {
                        preview: proposal.id.clone(),
                    },
                )
                .unwrap();
                let doc = app.doc.as_ref().unwrap();
                assert_eq!(doc.session.measures().iter().count(), 1);
                assert_eq!(
                    doc.session.measures().iter().next().unwrap().0.meta.label,
                    "Demo note"
                );
                assert!(doc.session.is_dirty());
                assert_eq!(
                    run(
                        &mut app,
                        experiment::Request::Commit {
                            preview: proposal.id
                        }
                    )
                    .unwrap_err()
                    .code,
                    ErrorCode::NotFound
                );
                app.execute_control(api::Request {
                    command: api::Command::Invoke {
                        action: api::Action::Undo,
                    },
                    target: app.control_state().document,
                })
                .unwrap();
                assert_eq!(
                    app.doc.as_ref().unwrap().session.measures().iter().count(),
                    0
                );
                assert!(!app.has_unsaved_work());
            }
        }
    }
    #[test]
    fn proposals_require_opt_in_and_a_current_target_and_can_be_discarded() {
        let mut app = setup();
        app.control.experimental = false;
        assert_eq!(
            run(&mut app, experiment::Request::Discard)
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        app.control.experimental = true;
        let proposal = preview(
            &mut app,
            "draw.rectangle",
            Anchor::PdfPoints {
                points: vec![[60.0, 100.0], [200.0, 300.0]],
            },
        );
        app.doc
            .as_mut()
            .unwrap()
            .session
            .apply(Command::SetPageLabels(vec![
                Some("Changed".into()),
                None,
                None,
            ]));
        assert_eq!(
            run(
                &mut app,
                experiment::Request::Commit {
                    preview: proposal.id
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::StaleDocument
        );
        assert_eq!(
            app.doc.as_ref().unwrap().session.measures().iter().count(),
            0
        );
        run(&mut app, experiment::Request::Discard).unwrap();
        assert!(app.control.draft.is_none());
    }
    #[test]
    fn text_anchors_highlight_selectable_glyphs_and_invalid_geometry_never_edits() {
        let mut app = setup();
        let chars = "wall"
            .chars()
            .enumerate()
            .map(|(i, ch)| TextChar {
                ch,
                bounds: Some(PdfBox {
                    left: 60.0 + i as f32 * 12.0,
                    bottom: 100.0,
                    right: 70.0 + i as f32 * 12.0,
                    top: 120.0,
                }),
                ink_bounds: None,
            })
            .collect();
        app.doc.as_mut().unwrap().text.insert(0, chars);
        let proposal = preview(
            &mut app,
            "highlight",
            Anchor::Text {
                quote: "wall".into(),
                occurrence: 1,
            },
        );
        assert!(!proposal.quads.is_empty());
        run(
            &mut app,
            experiment::Request::Commit {
                preview: proposal.id,
            },
        )
        .unwrap();
        assert_eq!(app.doc.as_ref().unwrap().session.highlights().len(), 1);
        let invalid = run(
            &mut app,
            experiment::Request::Preview {
                version: 1,
                page: 1,
                kind: "draw.rectangle".into(),
                anchor: Anchor::PdfPoints {
                    points: vec![[60.0, 100.0], [60.0, 300.0]],
                },
                saved_tool: None,
                comment: String::new(),
            },
        );
        assert_eq!(invalid.unwrap_err().code, ErrorCode::InvalidParameters);
        assert_eq!(
            app.doc.as_ref().unwrap().session.measures().iter().count(),
            0
        );
    }
}

impl App {
    pub(super) fn control_experiment(
        &mut self,
        request: experiment::Request,
    ) -> std::result::Result<Option<api::Data>, Error> {
        if !self.control.experimental {
            return Err(Error::new(
                ErrorCode::Unavailable,
                "Enable experimental markup access in this window's Settings first",
            ));
        }
        match request {
            experiment::Request::Discard => {
                self.control.draft = None;
                Ok(None)
            }
            experiment::Request::Preview {
                version,
                page,
                kind,
                anchor,
                saved_tool,
                comment,
            } => {
                if version != 1 || comment.len() > 100000 {
                    return Err(invalid("Unsupported experiment version or note length"));
                }
                self.control_measurements()?;
                let key =
                    ToolKey::from_stored(&kind).ok_or_else(|| invalid("Unknown markup kind"))?;
                if !matches!(key, ToolKey::Draw(_) | ToolKey::Highlight) {
                    return Err(Error::new(
                        ErrorCode::Unsupported,
                        "This experiment supports drawing tools and highlights",
                    ));
                }
                let sheet = self.control_page(page)?;
                let doc = self.doc.as_ref().unwrap();
                let geometry = doc
                    .sheet_geometry(sheet)
                    .ok_or_else(|| Error::new(ErrorCode::Busy, "Page geometry has not loaded"))?;
                let file_page = doc.sheet_page(sheet).unwrap();
                let text = doc.selectable_text(file_page);
                let mut placement = experiment::resolve(&anchor, geometry, text.as_deref())?;
                if key == ToolKey::Highlight {
                    if placement.quads.is_empty() {
                        if placement.points.len() != 2 {
                            return Err(invalid("Geometric highlights require two box corners"));
                        }
                        let [a, b] = [placement.points[0], placement.points[1]];
                        let rect = checked_rect(api::PdfRect {
                            left: a[0].min(b[0]),
                            bottom: a[1].min(b[1]),
                            right: a[0].max(b[0]),
                            top: a[1].max(b[1]),
                        })?;
                        placement.quads.push(rect);
                    }
                } else {
                    if matches!(anchor, Anchor::Text { .. }) {
                        return Err(Error::new(
                            ErrorCode::Unsupported,
                            "Text anchors currently propose highlights only",
                        ));
                    }
                    if placement.points.len() < 2
                        || (!matches!(key, ToolKey::Draw(MarkupKind::Pen))
                            && placement.points.len() != 2)
                    {
                        return Err(invalid(
                            "Drawing needs two corners/endpoints, or a pen path",
                        ));
                    }
                    let points = &placement.points;
                    if points
                        .windows(2)
                        .all(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) < 0.01)
                    {
                        return Err(invalid("Drawing has no extent"));
                    }
                    if matches!(
                        key,
                        ToolKey::Draw(MarkupKind::Rectangle | MarkupKind::Ellipse)
                    ) && ((points[0][0] - points[1][0]).abs() < 0.01
                        || (points[0][1] - points[1][1]).abs() < 0.01)
                    {
                        return Err(invalid("Box needs positive width and height"));
                    }
                }
                let settings = match saved_tool {
                    Some(tool) => {
                        let saved = (0..self.tools.saved_count())
                            .filter_map(|i| self.tools.saved_tool(i))
                            .find(|t| t.name == tool.name && t.group == tool.group)
                            .ok_or_else(|| {
                                Error::new(ErrorCode::NotFound, "Saved tool not found")
                            })?;
                        if saved.key != kind {
                            return Err(invalid("Saved tool kind does not match the proposal"));
                        }
                        saved.settings.clone()
                    }
                    None => self.tools.settings(key),
                };
                let id = format!("{}-{}", self.control.instance, self.control.next_preview);
                self.control.next_preview += 1;
                let preview = Preview {
                    id,
                    page,
                    kind,
                    points: placement.points.clone(),
                    quads: placement
                        .quads
                        .iter()
                        .map(|b| api::PdfRect {
                            left: b.left,
                            bottom: b.bottom,
                            right: b.right,
                            top: b.top,
                        })
                        .collect(),
                    settings: serde_json::to_value(&settings)
                        .map_err(|e| invalid(e.to_string()))?,
                };
                let active_layer = doc.session.active_layer();
                self.control.draft = Some(Draft {
                    target: self.control_state().document.unwrap(),
                    preview: preview.clone(),
                    placement,
                    settings,
                    comment,
                    file_page,
                    active_layer,
                });
                Ok(Some(api::Data::Preview(preview)))
            }
            experiment::Request::Commit { preview } => {
                self.control_measurements()?;
                let draft = self
                    .control
                    .draft
                    .as_ref()
                    .ok_or_else(|| Error::new(ErrorCode::NotFound, "No current preview"))?;
                if draft.preview.id != preview {
                    return Err(Error::new(
                        ErrorCode::NotFound,
                        "Preview was replaced; inspect the new preview",
                    ));
                }
                if self.control_state().document.as_ref() != Some(&draft.target) {
                    return Err(Error::new(
                        ErrorCode::StaleDocument,
                        "Document changed after preview; preview again",
                    ));
                }
                let layer = self.doc.as_ref().unwrap().session.active_layer();
                let layers = self.doc.as_ref().unwrap().session.layers();
                if layer != draft.active_layer {
                    return Err(Error::new(
                        ErrorCode::StaleDocument,
                        "Active layer changed after preview; preview again",
                    ));
                }
                if layers.is_locked(layer) || !layers.is_visible(layer) {
                    return Err(Error::new(
                        ErrorCode::Unavailable,
                        "Select a visible unlocked layer before committing",
                    ));
                }
                let draft = self.control.draft.take().unwrap();
                let doc = self.doc.as_mut().unwrap();
                if draft.preview.kind == "highlight" {
                    let ids = doc.session.apply(Command::AddHighlights(vec![Highlight {
                        key: None,
                        page: draft.file_page,
                        quads: draft.placement.quads,
                        color: draft.settings.style.stroke,
                        comment: draft.comment,
                        author: self.author.clone(),
                        snippet: String::new(),
                    }]));
                    Ok(Some(api::Data::Identifiers(
                        ids.into_iter().map(|id| id.to_string()).collect(),
                    )))
                } else {
                    let Some(ToolKey::Draw(kind)) = ToolKey::from_stored(&draft.preview.kind)
                    else {
                        unreachable!()
                    };
                    let (kind, geometry) =
                        super::super::markups::shape_of_points(kind, &draft.placement.points)
                            .ok_or_else(|| invalid("Shape could not be constructed"))?;
                    let mut shape =
                        markup_model::Markup::new(draft.file_page as u32, kind, geometry);
                    draft.settings.apply(&mut shape);
                    shape.meta.label = draft.comment;
                    shape.meta.author = self.author.clone();
                    shape.meta.created_ms = Some(chrono::Utc::now().timestamp_millis());
                    shape.meta.modified_ms = shape.meta.created_ms;
                    let mut layers = doc.session.layers().clone();
                    shape.layer = crate::layering::target_for_new(
                        &mut layers,
                        doc.session.active_layer(),
                        &draft.settings.defaults.layer,
                        draft.settings.defaults.layer_colour,
                    );
                    if layers.is_locked(shape.layer) || !layers.is_visible(shape.layer) {
                        return Err(Error::new(
                            ErrorCode::Unavailable,
                            "Tool's layer is locked or hidden",
                        ));
                    }
                    let id = shape.id.to_nm();
                    doc.session.apply(Command::Batch(vec![
                        Command::SetLayers(layers),
                        Command::AddMeasure(Box::new(shape)),
                    ]));
                    Ok(Some(api::Data::Identifiers(vec![id])))
                }
            }
        }
    }

    pub(in crate::app) fn paint_control_preview(&self) {
        let Some(draft) = &self.control.draft else {
            return;
        };
        if !self.control.experimental
            || self.control_state().document.as_ref() != Some(&draft.target)
        {
            return;
        }
        let Some(doc) = &self.doc else {
            return;
        };
        let Some(sheet) = doc.first_sheet_showing(draft.file_page) else {
            return;
        };
        let (Some(rect), Some(g)) = (self.page_rects.get(&sheet), doc.sheet_geometry(sheet)) else {
            return;
        };
        let painter = self
            .ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("markup-proposal"),
            ))
            .with_clip_rect(self.viewer_rect);
        let at = |[x, y]: [f32; 2]| {
            let (x, y) = g.to_view(x, y);
            rect.min + rect.size() * vec2(x, y)
        };
        let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 190, 50));
        for quad in &draft.placement.quads {
            painter.rect_stroke(
                Rect::from_two_pos(at([quad.left, quad.bottom]), at([quad.right, quad.top])),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        let points: Vec<_> = draft.placement.points.iter().copied().map(at).collect();
        if points.len() == 2
            && matches!(
                draft.preview.kind.as_str(),
                "draw.rectangle" | "draw.ellipse"
            )
        {
            painter.rect_stroke(
                Rect::from_two_pos(points[0], points[1]),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        } else if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
