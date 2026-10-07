//! Presentation controls reuse the same navigation and action helpers as the UI.
use super::*;

impl App {
    pub(super) fn control_invoke(&mut self, action: api::Action) -> std::result::Result<(), Error> {
        use api::Action as A;
        use actions::Action as Internal;
        let internal = match action {
            A::QuickAccess => { self.palette.show(palette::Mode::KeptTools); return Ok(()); },
            A::CommandPalette => { self.palette.show(palette::Mode::Commands); return Ok(()); },
            A::CloseMenus => { self.palette.hide(); self.popup = None; self.tool_creator = None; egui::Popup::close_all(&self.ctx); return Ok(()); },
            A::Layers => { self.show_tool_panel(tool_panel::Tab::Layers); return Ok(()); },
            A::OpenDialog => Internal::Open, A::SaveAsDialog => Internal::SaveAs,
            A::Find => Internal::Find, A::GoTo => Internal::GoToPage,
            A::ZoomIn => Internal::ZoomIn, A::ZoomOut => Internal::ZoomOut,
            A::FitPage => Internal::FitPage, A::FitWidth => Internal::FitWidth,
            A::NextPage => Internal::NextPage, A::PreviousPage => Internal::PreviousPage,
            A::FirstPage => Internal::FirstPage, A::LastPage => Internal::LastPage,
            A::FindNext => Internal::FindNext, A::FindPrevious => Internal::FindPrevious,
            A::Undo => Internal::Undo, A::Redo => Internal::Redo, A::Select => Internal::Select,
            A::Details => Internal::Details, A::Tools => Internal::KeptTools, A::Quantities => Internal::Quantities,
        };
        if !self.action_enabled(internal) { return Err(Error::new(ErrorCode::Unavailable, "Action is unavailable in the current state")); }
        if matches!(action, A::Undo | A::Redo) {
            self.control_editable()?;
            let doc = self.doc.as_ref().unwrap();
            let available = match action { A::Undo => doc.session.can_undo() || doc.arrange.can_undo(), _ => doc.session.can_redo() || doc.arrange.can_redo() };
            if !available { return Err(Error::new(ErrorCode::Unavailable, "No history step is available")); }
        }
        self.run_action(internal);
        Ok(())
    }

    pub(super) fn control_centre(&mut self, page: u32, point: [f32; 2]) -> std::result::Result<(), Error> {
        let sheet = self.control_page(page)?;
        if !point.iter().all(|v| v.is_finite()) { return Err(invalid("Point coordinates must be finite")); }
        let doc = self.doc.as_ref().unwrap();
        let geometry = doc.sheet_geometry(sheet).ok_or_else(|| Error::new(ErrorCode::Busy, "Page geometry has not loaded; navigate to this page first"))?;
        let (x, y) = geometry.to_view(point[0], point[1]);
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) { return Err(invalid("Point is outside the visible page bounds")); }
        let view = self.viewer_rect.size();
        if !self.viewer_rect.is_positive() { return Err(Error::new(ErrorCode::Unavailable, "The viewer has not been laid out")); }
        let layout = self.layout(doc);
        let size = arrange::sheet_size(doc, sheet).unwrap() * layout.scales[sheet];
        let width = content_width(view.x, layout.widest);
        self.zoom_anchor = None;
        self.scroll_x = Some(layout.x(width, sheet) + x * size.x - view.x / 2.0);
        self.scroll_y = Some(layout.tops[sheet] + y * size.y - view.y / 2.0);
        self.current_page = sheet;
        self.picked_page = Some(sheet);
        Ok(())
    }

    pub(super) fn control_region(&mut self, page: u32, region: api::PdfRect) -> std::result::Result<(), Error> {
        let sheet = self.control_page(page)?;
        let bounds = checked_rect(region)?;
        let doc = self.doc.as_ref().unwrap();
        let geometry = doc.sheet_geometry(sheet).ok_or_else(|| Error::new(ErrorCode::Busy, "Page geometry has not loaded"))?;
        if !geometry.bounds.contains(bounds.left, bounds.bottom) || !geometry.bounds.contains(bounds.right, bounds.top) { return Err(invalid("Region is outside the page")); }
        let (left, top, right, bottom) = geometry.box_to_view(&bounds);
        let size = arrange::sheet_size(doc, sheet).unwrap();
        let view = self.viewer_rect.size();
        if view.x <= 0.0 || view.y <= 0.0 { return Err(Error::new(ErrorCode::Unavailable, "The viewer has not been laid out")); }
        let shrink = if self.shrink_wide && size.x > doc.usual_size.x * OVERSIZED { doc.usual_size.x / size.x } else { 1.0 };
        let zoom = ((view.x - 40.0) / ((right - left) * size.x * shrink)).min((view.y - 40.0) / ((bottom - top) * size.y * shrink)).clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
        self.change_zoom(zoom, None);
        self.zoom_mode = ZoomMode::Custom;
        self.fit_requested = false;
        self.control_centre(page, [(bounds.left + bounds.right) / 2.0, (bounds.bottom + bounds.top) / 2.0])
    }
}

pub(super) fn checked_rect(rect: api::PdfRect) -> std::result::Result<PdfBox, Error> {
    if ![rect.left, rect.bottom, rect.right, rect.top].iter().all(|v| v.is_finite()) || rect.left >= rect.right || rect.bottom >= rect.top { return Err(invalid("Rectangle must have finite coordinates and positive width and height")); }
    Ok(PdfBox { left: rect.left, bottom: rect.bottom, right: rect.right, top: rect.top })
}
