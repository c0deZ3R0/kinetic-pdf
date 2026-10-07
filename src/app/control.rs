//! Executes control requests on the UI thread using the normal app operations.
use super::*;
mod operations;
mod demo;
mod experiment;
mod tools;
mod views;
use operations::{Pending, Wait};
use views::checked_rect;
use crate::control::{self as api, DocumentRevision, DocumentTarget, Error, ErrorCode};

pub(super) struct Control {
    #[cfg(feature = "mcp")]
    pub(super) server: Option<crate::mcp::Server>,
    #[cfg(feature = "mcp")]
    pub(super) port: u16,
    pub(super) client: api::Client,
    inbox: api::Inbox,
    operations: Vec<Pending>,
    next_operation: u64,
    demo: Option<crate::demo::Player>,
    pub(in crate::app) experimental: bool,
    draft: Option<experiment::Draft>,
    next_preview: u64,
    instance: String,
}

impl Control {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        let (client, inbox) = api::channel(move || ctx.request_repaint());
        static INSTANCE: AtomicU64 = AtomicU64::new(1);
        let instance = format!("{}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_micros(), INSTANCE.fetch_add(1, Ordering::Relaxed));
        Self { client, inbox, instance, operations: Vec::new(), next_operation: 0, demo: None, experimental: false, draft: None, next_preview: 1,
            #[cfg(feature = "mcp")]
            server: None,
            #[cfg(feature = "mcp")]
            port: 47831,
        }
    }
}

impl App {
    #[cfg(feature = "mcp")]
    pub(super) fn start_mcp(&mut self) {
        match crate::mcp::Server::start(self.control_client(), self.control.port) {
            Ok(server) => self.control.server = Some(server),
            Err(error) => self.toast(format!("Could not enable MCP: {error}")),
        }
    }
    pub fn control_client(&self) -> api::Client { self.control.client.clone() }

    pub fn control_state(&self) -> api::State {
        let document = self.doc.as_ref().map(|doc| DocumentTarget {
            instance: self.control.instance.clone(), generation: doc.generation,
            revision: DocumentRevision { edits: doc.session.revision(), arrangement: doc.arrange.revision(), file: format!("{:016x}", doc.snapshot.file()) },
        });
        api::State {
            instance: self.control.instance.clone(), document,
            path: self.doc.as_ref().map(|d| d.path.to_string_lossy().into_owned()),
            pages: self.doc.as_ref().map_or(0, |d| d.arrange.len()),
            current_page: if self.doc.is_some() { self.current_page + 1 } else { 0 },
            zoom: self.zoom, scroll: [self.scroll_offset.x, self.scroll_offset.y],
            viewport: [self.viewer_rect.width().max(0.0), self.viewer_rect.height().max(0.0)],
            dirty: self.has_unsaved_work(), busy: self.lifecycle.status() != Status::Idle,
            view_ready: self.view_is_sharp() && self.scroll_x.is_none() && self.scroll_y.is_none() && self.zoom_anchor.is_none() && !self.fit_requested,
            palette_open: self.palette.open,
            active_tool: self.held_tool().map(|k| k.stored()),
            active_layer: self.doc.as_ref().map(|d| d.session.active_layer().to_nm()),
            search_query: self.search.query.clone(), search_complete: self.search.done,
            can_undo: self.doc.as_ref().is_some_and(|d| d.session.can_undo() || d.arrange.can_undo()),
            can_redo: self.doc.as_ref().is_some_and(|d| d.session.can_redo() || d.arrange.can_redo()),
        }
    }

    pub(super) fn drain_control(&mut self) {
        self.refresh_operations();
        // A bounded amount per frame leaves input and drawing responsive.
        for _ in 0..8 {
            let Some(envelope) = self.control.inbox.next() else { break };
            if envelope.can_start() {
                let result = self.execute_control(envelope.request.clone());
                envelope.finish(result);
            }
        }
    }

    pub(super) fn execute_control(&mut self, request: api::Request) -> api::Result {
        if self.control.demo.as_ref().is_some_and(crate::demo::Player::running) && !matches!(request.command, api::Command::Inspect | api::Command::DemoStatus | api::Command::CancelDemo | api::Command::PollOperation { .. }) { return Err(Error::new(ErrorCode::Busy, "Cancel the active demo before issuing commands")); }
        if let Some(target) = &request.target {
            if self.control_state().document.as_ref() != Some(target) {
                return Err(Error::new(ErrorCode::StaleDocument, "The document or its contents changed; inspect it again"));
            }
        }
        self.control_dispatch(request)
    }

    fn control_dispatch(&mut self, request: api::Request) -> api::Result {
        use api::Command as C;
        if requires_target(&request.command) && request.target.is_none() { return Err(invalid("A document target from inspect is required for this operation")); }
        let mut data = None;
        let mut operation = None;
        match request.command {
            C::Inspect => {},
            C::Experiment { request } => { data = self.control_experiment(request)?; },
            C::RunDemo { script } => { self.control_start_demo(script)?; data = Some(api::Data::Demo(self.control.demo.as_ref().map(|p| p.progress.clone()))); },
            C::CancelDemo => { if let Some(player) = &mut self.control.demo { player.cancel(); } data = Some(api::Data::Demo(self.control.demo.as_ref().map(|p| p.progress.clone()))); },
            C::DemoStatus => { data = Some(api::Data::Demo(self.control.demo.as_ref().map(|p| p.progress.clone()))); },
            C::PollOperation { id } => {
                self.refresh_operations();
                operation = Some(self.control.operations.iter().find(|p| p.operation.id == id).ok_or_else(|| Error::new(ErrorCode::NotFound, "Operation is no longer in the recent history"))?.operation.clone());
            },
            C::GoToPage { page } => { self.control_idle()?; self.control_page(page)?; self.go_to_page(page as usize - 1); },
            C::ListPages => {
                let doc = self.control_document()?;
                data = Some(api::Data::Pages(doc.arrange.sheets().iter().enumerate().map(|(position, sheet)| api::Page {
                    position: position + 1, file_page: sheet.page() + 1,
                    label: doc.session.page_labels().get(sheet.page()).cloned().flatten(),
                    size: arrange::sheet_size(doc, position).map_or([0.0, 0.0], |s| [s.x, s.y]), turns: sheet.turns(),
                }).collect()));
            },
            C::Open { path } => {
                self.control_idle()?;
                if self.has_unsaved_work() { return Err(Error::new(ErrorCode::NeedsConfirmation, "Save or resolve unsaved work in the app before opening another file")); }
                let path = checked_path(&path, true)?;
                self.control.operation_slot()?;
                self.open(path);
                operation = Some(self.control.begin(Wait::Open, self.generation));
            },
            C::Save | C::SaveAs { .. } => {
                self.control_editable()?;
                let generation = self.generation;
                let target = match request.command {
                    C::SaveAs { path, overwrite } => {
                        let path = checked_path(&path, false)?;
                        if path.exists() && !overwrite { return Err(Error::new(ErrorCode::NeedsConfirmation, "Destination exists; set overwrite explicitly or choose another path")); }
                        Some(path)
                    },
                    _ => {
                        if self.doc.as_ref().is_some_and(|d| self.is_untitled(&d.path)) { return Err(invalid("An untitled document needs save_as with an explicit destination")); }
                        None
                    },
                };
                self.control.operation_slot()?;
                self.save_to(target);
                if self.lifecycle.status() == Status::Saving { operation = Some(self.control.begin(Wait::Save, generation)); }
                else if self.has_unsaved_work() { return Err(Error::new(ErrorCode::Failed, "The save could not start")); }
            },
            C::ReadText { page } => {
                self.control_idle()?;
                let sheet = self.control_page(page)?;
                let doc = self.doc.as_ref().unwrap();
                let file_page = doc.sheet_page(sheet).unwrap();
                if !doc.in_file(file_page) {
                    data = Some(api::Data::Text { page: page as usize, text: String::new(), truncated: false });
                } else if let Some(chars) = doc.text.get(&file_page) {
                    let text = doc.selectable_text(file_page).map(|t| t.iter().map(|c| c.ch).take(100000).collect()).unwrap_or_else(|| chars.iter().map(|c| c.ch).take(100000).collect());
                    data = Some(api::Data::Text { page: page as usize, text, truncated: chars.len() > 100000 });
                } else {
                    self.control.operation_slot()?;
                    self.tx.send(Request::ExtractText { generation: doc.generation, page: file_page }).map_err(|_| Error::new(ErrorCode::Unavailable, "PDF worker has stopped"))?;
                    operation = Some(self.control.begin(Wait::Text(file_page), doc.generation));
                }
            },
            C::ListAnnotations => {
                self.want_measurements();
                let doc = self.control_document()?;
                let position = |page| doc.first_sheet_showing(page).map_or(0, |p| p + 1);
                let mut items: Vec<_> = doc.session.highlights().iter().map(|e| api::Annotation { id: e.uid.to_string(), page: position(e.hl.page), kind: "highlight".into(), comment: e.hl.comment.clone(), can_reshape: false }).collect();
                items.extend(doc.session.markups().iter().map(|e| api::Annotation { id: e.uid.to_string(), page: position(e.markup.page), kind: e.markup.kind.label().into(), comment: e.markup.comment.clone(), can_reshape: e.markup.key.is_none() }));
                items.extend(doc.session.measures().iter().map(|(m, _)| api::Annotation { id: m.id.to_nm(), page: position(m.page as usize), kind: format!("{:?}", m.kind), comment: m.meta.label.clone(), can_reshape: true }));
                data = Some(api::Data::Annotations { items, complete: doc.highlights_done && doc.measurements == MeasureRead::Ready });
            },
            C::ListLayers => { self.want_measurements(); data = Some(self.control_layers()?); },
            C::CreateLayer { name } => {
                checked_name(&name)?; self.control_measurements()?;
                let doc = self.doc.as_mut().unwrap();
                let (command, id) = crate::layering::add(&doc.session, &name);
                doc.session.apply(command);
                data = Some(api::Data::Identifiers(vec![id.to_nm()]));
            },
            C::RenameLayer { id, name } => {
                checked_name(&name)?; self.control_measurements()?;
                let id = self.control_layer(&id)?;
                let doc = self.doc.as_mut().unwrap();
                let mut layers = doc.session.layers().clone();
                if !layers.rename(id, &name) { return Err(invalid("Layer cannot be renamed to that name")); }
                doc.session.apply(Command::SetLayers(layers));
            },
            C::SelectLayer { id } => { self.control_measurements()?; let id = self.control_layer(&id)?; self.doc.as_mut().unwrap().session.set_active_layer(id); },
            C::SetPageLabel { page, label } => {
                self.control_editable()?;
                let sheet = self.control_page(page)?;
                if let Some(label) = &label { if label.len() > 1024 { return Err(invalid("Page label is too long")); } }
                let doc = self.doc.as_mut().unwrap();
                let file_page = doc.sheet_page(sheet).unwrap();
                let mut labels = doc.session.page_labels().to_vec();
                labels.resize(doc.sizes.len(), None);
                labels[file_page] = label;
                doc.session.apply(Command::SetPageLabels(labels));
            },
            C::InsertBlankPage { at, size } => {
                self.control_editable()?;
                if at == 0 || at as usize > self.control_document()?.arrange.len() + 1 || size.iter().any(|v| !v.is_finite() || !(1.0..=14400.0).contains(v)) { return Err(invalid("Invalid insertion position or page size in PDF points")); }
                self.sheets_mut(|a| { a.insert_blank(at as usize - 1, size); });
            },
            C::ListTools => data = Some(self.control_tools()),
            C::ToolSchema => data = Some(api::Data::ToolSchema(serde_json::from_str(&super::tools::Tools::schema_json()).map_err(|e| Error::new(ErrorCode::Failed, e.to_string()))?)),
            C::ConfigureTool { name, group, kind, settings, replace } => { self.control_configure_tool(name, group, kind, settings, replace)?; data = Some(self.control_tools()); },
            C::SelectTool { kind } => { self.control_idle()?; self.control_document()?; self.control_select_tool(&kind)?; },
            C::SelectSavedTool { name, group } => { self.control_idle()?; self.control_document()?; self.control_select_saved(&name, &group)?; },
            C::SetZoom { zoom } => {
                self.control_idle()?; self.control_document()?;
                if !zoom.is_finite() || !(ZOOMS[0]..=ZOOMS[ZOOMS.len() - 1]).contains(&zoom) { return Err(invalid("Zoom must be between 0.05 and 8.0")); }
                self.zoom_mode = ZoomMode::Custom; self.fit_requested = false; self.change_zoom(zoom, None);
            },
            C::Pan { delta } => {
                self.control_idle()?; self.control_document()?;
                if delta.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) { return Err(invalid("Pan delta must be finite logical pixels")); }
                self.zoom_anchor = None;
                self.scroll_x = Some(self.scroll_x.unwrap_or(self.scroll_offset.x) + delta[0]);
                self.scroll_y = Some(self.scroll_y.unwrap_or(self.scroll_offset.y) + delta[1]);
            },
            C::CentreOn { page, point } => { self.control_idle()?; self.control_centre(page, point)?; },
            C::ZoomToRegion { page, region } => { self.control_idle()?; self.control_region(page, region)?; },
            C::Find { query } => {
                self.control_idle()?; self.control_document()?;
                if query.is_empty() || query.len() > 4096 { return Err(invalid("Search needs between 1 and 4096 bytes of text")); }
                self.control.operation_slot()?;
                self.search.query = query;
                self.start_search();
                operation = Some(self.control.begin(Wait::Search(self.search.id), self.generation));
            },
            C::SearchResults => {
                let doc = self.control_document()?;
                data = Some(api::Data::Search { complete: self.search.done, results: self.search.hits.iter().enumerate().map(|(i, hit)| api::SearchResult {
                    index: i + 1, page: doc.first_sheet_showing(hit.page).map(|p| p + 1), matched: hit.matched.clone(), before: hit.before.clone(), after: hit.after.clone(),
                }).collect() });
            },
            C::GoToResult { result } => {
                self.control_idle()?;
                if result == 0 || result as usize > self.search.hits.len() { return Err(invalid("Search result is outside the result list")); }
                let hit = &self.search.hits[result as usize - 1];
                if self.control_document()?.first_sheet_showing(hit.page).is_none() { return Err(Error::new(ErrorCode::Unavailable, "The matching page is no longer displayed")); }
                self.go_to_hit(result as usize - 1);
            },
            C::Invoke { action } => self.control_invoke(action)?,
            C::PaletteQuery { query } => { if !self.palette.open || query.len() > 4096 { return Err(invalid("Open the palette first and use a query of at most 4096 bytes")); } self.palette.set_query(query); },
            C::PaletteChoose { label } => { self.control_editable()?; self.choose_palette_label(&label).map_err(invalid)?; },
            C::AddHighlight { page, quads, comment, color } => {
                self.control_editable()?;
                let sheet = self.control_page(page)?;
                if quads.is_empty() || quads.len() > 2048 || comment.len() > 100000 || color.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { return Err(invalid("Invalid highlight geometry, colour or note length")); }
                let quads: Vec<_> = quads.into_iter().map(checked_rect).collect::<std::result::Result<_, _>>()?;
                let doc = self.doc.as_mut().unwrap();
                let page = doc.sheet_page(sheet).unwrap();
                let bounds = doc.sheet_geometry(sheet).ok_or_else(|| Error::new(ErrorCode::Busy, "Page geometry has not loaded"))?.bounds;
                if quads.iter().any(|q| !bounds.contains(q.left, q.bottom) || !bounds.contains(q.right, q.top)) { return Err(invalid("Highlight is outside page bounds")); }
                let ids = doc.session.apply(Command::AddHighlights(vec![Highlight { key: None, page, quads, color, comment, author: self.author.clone(), snippet: String::new() }]));
                data = Some(api::Data::Identifiers(ids.iter().map(u64::to_string).collect()));
            },
            C::EditNote { id, comment } => {
                self.control_editable()?;
                if comment.len() > 100000 { return Err(invalid("Note is too long")); }
                let doc = self.doc.as_mut().unwrap();
                let color = doc.session.highlight(id).map(|h| h.hl.color).or_else(|| doc.session.markup(id).map(|m| m.markup.color)).ok_or_else(|| Error::new(ErrorCode::NotFound, "Annotation ID not found"))?;
                doc.session.apply(Command::EditNote { uid: id, comment, color });
            },
        }
        self.ctx.request_repaint();
        Ok(api::Response { state: self.control_state(), data, operation })
    }

    fn control_document(&self) -> std::result::Result<&Doc, Error> { self.doc.as_ref().ok_or_else(|| Error::new(ErrorCode::Unavailable, "No document is open")) }
    fn control_idle(&self) -> std::result::Result<(), Error> {
        if self.lifecycle.status() != Status::Idle || self.compare.is_some() { Err(Error::new(ErrorCode::Busy, "The app is opening, saving, unavailable or comparing documents")) } else { Ok(()) }
    }
    fn control_editable(&self) -> std::result::Result<(), Error> { self.control_idle()?; if !self.control_document()?.session.can_edit() { return Err(Error::new(ErrorCode::Busy, "Editing is currently blocked")); } Ok(()) }
    fn control_page(&self, page: u32) -> std::result::Result<usize, Error> {
        if page == 0 || page as usize > self.control_document()?.arrange.len() { Err(invalid("Page is outside the displayed document")) } else { Ok(page as usize - 1) }
    }
    fn control_measurements(&mut self) -> std::result::Result<(), Error> {
        self.control_editable()?; self.want_measurements();
        match &self.control_document()?.measurements { MeasureRead::Ready => Ok(()), MeasureRead::Failed(message) => Err(Error::new(ErrorCode::Failed, message.clone())), _ => Err(Error::new(ErrorCode::Busy, "Layers and measurements are loading; inspect and retry")) }
    }
    fn control_layer(&self, id: &str) -> std::result::Result<markup_model::LayerId, Error> {
        let id = markup_model::LayerId::from_nm(id).ok_or_else(|| invalid("Invalid layer ID"))?;
        if self.control_document()?.session.layers().get(id).is_none() { return Err(Error::new(ErrorCode::NotFound, "Layer ID not found")); }
        Ok(id)
    }
    fn control_layers(&self) -> std::result::Result<api::Data, Error> {
        let doc = self.control_document()?;
        Ok(api::Data::Layers(doc.session.layers().layers().iter().map(|l| api::Layer {
            id: l.id.to_nm(), name: l.name.clone(), path: doc.session.layers().path(l.id), visible: l.visible, locked: l.locked, active: doc.session.active_layer() == l.id,
        }).collect()))
    }
}

fn invalid(message: impl Into<String>) -> Error { Error::new(ErrorCode::InvalidParameters, message) }
fn checked_name(name: &str) -> std::result::Result<(), Error> { if name.trim().is_empty() || name.len() > 256 { Err(invalid("Name must be nonempty and at most 256 bytes")) } else { Ok(()) } }
fn checked_path(path: &str, existing: bool) -> std::result::Result<PathBuf, Error> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) { return Err(invalid("Use an absolute PDF path")); }
    if existing { return path.canonicalize().map_err(|e| Error::new(ErrorCode::NotFound, e.to_string())); }
    if !path.parent().is_some_and(|p| p.is_dir()) { return Err(invalid("Destination directory does not exist")); }
    Ok(path)
}
fn requires_target(command: &api::Command) -> bool {
    matches!(command, api::Command::Experiment { request: crate::experiment::Request::Commit { .. } } | api::Command::Save | api::Command::SaveAs { .. } | api::Command::CreateLayer { .. } | api::Command::RenameLayer { .. } | api::Command::SelectLayer { .. } | api::Command::SetPageLabel { .. } | api::Command::InsertBlankPage { .. } | api::Command::AddHighlight { .. } | api::Command::EditNote { .. } | api::Command::Invoke { action: api::Action::Undo | api::Action::Redo } | api::Command::PaletteChoose { .. })
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::app_with_a_document;
    #[test]
    fn navigation_uses_the_live_app_and_rejects_replaced_documents() {
        let (mut app, _) = app_with_a_document();
        let target = app.control_state().document;
        let state = app.execute_control(api::Request { command: api::Command::GoToPage { page: 3 }, target: target.clone() }).unwrap();
        assert_eq!(state.state.current_page, 3);
        app.doc.as_mut().unwrap().generation += 1;
        assert_eq!(app.execute_control(api::Request { command: api::Command::GoToPage { page: 1 }, target }).unwrap_err().code, ErrorCode::StaleDocument);
        assert_eq!(app.current_page, 2);
    }
    #[test]
    fn queued_work_is_cancelled_when_the_client_drops_it() {
        let (mut app, _) = app_with_a_document();
        let ticket = app.control_client().submit(api::Request { command: api::Command::GoToPage { page: 3 }, target: None }, Duration::from_secs(1)).unwrap();
        drop(ticket);
        app.drain_control();
        assert_eq!(app.current_page, 0);
    }

    fn run(app: &mut App, command: api::Command) -> api::Result {
        app.execute_control(api::Request { command, target: app.control_state().document })
    }

    #[test]
    fn labels_layers_and_blank_pages_use_normal_history_and_stale_guards() {
        let (mut app, _) = app_with_a_document();
        app.doc.as_mut().unwrap().measurements = MeasureRead::Ready;
        let old = app.control_state().document;
        run(&mut app, api::Command::SetPageLabel { page: 2, label: Some("A-101".into()) }).unwrap();
        assert_eq!(app.doc.as_ref().unwrap().session.page_labels()[1].as_deref(), Some("A-101"));
        assert!(app.has_unsaved_work());
        let stale = app.execute_control(api::Request { command: api::Command::SetPageLabel { page: 2, label: None }, target: old });
        assert_eq!(stale.unwrap_err().code, ErrorCode::StaleDocument);
        run(&mut app, api::Command::Invoke { action: api::Action::Undo }).unwrap();
        assert!(app.doc.as_ref().unwrap().session.page_labels()[1].is_none());
        assert!(!app.has_unsaved_work());
        run(&mut app, api::Command::Invoke { action: api::Action::Redo }).unwrap();
        let layer = run(&mut app, api::Command::CreateLayer { name: "Walls".into() }).unwrap();
        let Some(api::Data::Identifiers(ids)) = layer.data else { panic!("missing layer ID") };
        run(&mut app, api::Command::RenameLayer { id: ids[0].clone(), name: "Structure".into() }).unwrap();
        run(&mut app, api::Command::SelectLayer { id: ids[0].clone() }).unwrap();
        assert_eq!(app.doc.as_ref().unwrap().session.layers().name(app.doc.as_ref().unwrap().session.active_layer()), "Structure");
        run(&mut app, api::Command::InsertBlankPage { at: 2, size: [300.0, 400.0] }).unwrap();
        assert_eq!(app.doc.as_ref().unwrap().arrange.len(), 4);
        assert_eq!(arrange::sheet_size(app.doc.as_ref().unwrap(), 1), Some(vec2(300.0, 400.0)));
    }

    #[test]
    fn configured_tools_can_be_selected_through_the_visible_quick_access_menu() {
        let (mut app, _) = app_with_a_document();
        app.tools = super::super::tools::Tools::default();
        let config = || api::Command::ConfigureTool { name: "Demo pen".into(), group: "Demo".into(), kind: "draw.pen".into(), settings: Some(serde_json::json!({"style":{"width":3.0,"stroke":[0.1,0.2,0.3]}})), replace: false };
        run(&mut app, config()).unwrap();
        assert_eq!(run(&mut app, config()).unwrap_err().code, ErrorCode::NeedsConfirmation);
        run(&mut app, api::Command::Invoke { action: api::Action::QuickAccess }).unwrap();
        run(&mut app, api::Command::PaletteQuery { query: "Demo pen".into() }).unwrap();
        run(&mut app, api::Command::PaletteChoose { label: "Demo pen".into() }).unwrap();
        assert_eq!(app.control_state().active_tool.as_deref(), Some("draw.pen"));
        assert_eq!(app.tools.settings(super::super::tools::ToolKey::Draw(MarkupKind::Pen)).style.width, 3.0);
        assert!(!app.palette.open);
        assert!(run(&mut app, api::Command::ConfigureTool { name: "Bad".into(), group: String::new(), kind: "draw.pen".into(), settings: Some(serde_json::json!({"style":{"widht":1}})), replace: false }).is_err());
        assert!(run(&mut app, api::Command::ConfigureTool { name: "Bad".into(), group: String::new(), kind: "draw.pen".into(), settings: Some(serde_json::json!({"style":{"width":-1}})), replace: false }).is_err());
        assert_eq!(app.tools.saved_count(), 1);
    }

    #[test]
    fn navigation_handles_rotated_page_geometry_and_validates_before_changing_zoom() {
        let (mut app, _) = app_with_a_document();
        app.viewer_rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        app.fit_requested = false;
        for turn in 0..4 {
            app.doc.as_mut().unwrap().geometry[0] = Some(PageGeometry { rotation: turn, bounds: PdfBox { left: 0.0, bottom: 0.0, right: 600.0, top: 800.0 } });
            run(&mut app, api::Command::CentreOn { page: 1, point: [300.0, 400.0] }).unwrap();
            let region = api::PdfRect { left: 100.0, bottom: 100.0, right: 300.0, top: 300.0 };
            run(&mut app, api::Command::ZoomToRegion { page: 1, region }).unwrap();
            assert!(app.zoom >= 1.0);
            assert!(app.scroll_x.is_some() && app.scroll_y.is_some());
        }
        let zoom = app.zoom;
        assert!(run(&mut app, api::Command::SetZoom { zoom: f32::NAN }).is_err());
        assert_eq!(app.zoom, zoom);
        app.lifecycle.start_save(false);
        assert_eq!(run(&mut app, api::Command::Pan { delta: [1.0, 1.0] }).unwrap_err().code, ErrorCode::Busy);
    }

    #[test]
    fn a_save_operation_reports_worker_failure_instead_of_success_on_submission() {
        let (mut app, ctx) = app_with_a_document();
        let (tx, requests) = std::sync::mpsc::channel(); app.tx = tx;
        let (replies, rx) = std::sync::mpsc::channel(); app.rx = rx;
        run(&mut app, api::Command::SetPageLabel { page: 1, label: Some("Test".into()) }).unwrap();
        let operation = run(&mut app, api::Command::Save).unwrap().operation.unwrap();
        assert_eq!(operation.status, api::OperationStatus::Pending);
        assert!(matches!(requests.recv_timeout(Duration::from_secs(1)).unwrap(), Request::Save { .. }));
        replies.send(Reply::SaveFailed { generation: 1, error: "Read-only destination".into() }).unwrap();
        app.drain_replies(&ctx);
        let response = run(&mut app, api::Command::PollOperation { id: operation.id }).unwrap();
        assert_eq!(response.operation.unwrap().status, api::OperationStatus::Failed);
        assert!(app.has_unsaved_work());
    }
}
