//! Executes control requests on the UI thread using the normal app operations.
use super::*;
use crate::control::{self as api, DocumentRevision, DocumentTarget, Error, ErrorCode};

pub(super) struct Control {
    #[cfg(feature = "mcp")]
    pub(super) server: Option<crate::mcp::Server>,
    #[cfg(feature = "mcp")]
    pub(super) port: u16,
    pub(super) client: api::Client,
    inbox: api::Inbox,
    instance: String,
}

impl Control {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        let (client, inbox) = api::channel(move || ctx.request_repaint());
        static INSTANCE: AtomicU64 = AtomicU64::new(1);
        let instance = format!("{}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_micros(), INSTANCE.fetch_add(1, Ordering::Relaxed));
        Self { client, inbox, instance,
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
        }
    }

    pub(super) fn drain_control(&mut self) {
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
        if let Some(target) = &request.target {
            if self.control_state().document.as_ref() != Some(target) {
                return Err(Error::new(ErrorCode::StaleDocument, "The document or its contents changed; inspect it again"));
            }
        }
        match request.command {
            api::Command::Inspect => {},
            api::Command::GoToPage { page } => {
                let doc = self.doc.as_ref().ok_or_else(|| Error::new(ErrorCode::Unavailable, "No document is open"))?;
                if self.lifecycle.status() != Status::Idle { return Err(Error::new(ErrorCode::Busy, "The document is opening or saving")); }
                if page == 0 || page as usize > doc.arrange.len() { return Err(Error::new(ErrorCode::InvalidParameters, "Page is outside the displayed document")); }
                self.go_to_page(page as usize - 1);
            }
        }
        self.ctx.request_repaint();
        Ok(self.control_state())
    }
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
        assert_eq!(state.current_page, 3);
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
}
