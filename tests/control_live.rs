//! Real HTTP -> bounded app queue -> egui UI -> PDF worker -> saved PDF roundtrip.
#![cfg(feature = "mcp")]
mod common;
use eframe::{egui, App as _};
use kinetic_pdf::{
    app::App,
    control::{Command, Data, OperationStatus, Request, Response},
    mcp::Server,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct Live {
    app: App,
    ctx: egui::Context,
    frame: eframe::Frame,
    server: Server,
    auth: String,
    clock: f64,
}
impl Live {
    fn tick(&mut self) {
        self.clock += 0.02;
        let app = &mut self.app;
        let frame = &mut self.frame;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.clock),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| app.ui(ui, frame),
        );
        output.textures_delta.clear();
    }
    fn rpc(&mut self, request: serde_json::Value) -> serde_json::Value {
        let url = self.server.url().to_string();
        let auth = self.auth.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let body = request.to_string();
            let response = ureq::post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .send(body.as_str())
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap();
            tx.send(serde_json::from_str::<serde_json::Value>(&response).unwrap())
                .unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.tick();
            if let Ok(result) = rx.try_recv() {
                return result;
            }
            assert!(Instant::now() < deadline, "HTTP request never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn call(&mut self, command: Command, targeted: bool) -> Response {
        let target = targeted
            .then(|| self.app.control_state().document)
            .flatten();
        let request = Request { command, target };
        let result = self.rpc(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"kinetic_control","arguments":request}}));
        let result = &result["result"];
        assert_ne!(result["isError"], true, "{}", result["structuredContent"]);
        serde_json::from_value(result["structuredContent"].clone()).unwrap()
    }
    fn wait(&mut self, response: Response) {
        let Some(mut operation) = response.operation else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while operation.status == OperationStatus::Pending {
            assert!(
                Instant::now() < deadline,
                "Worker operation never completed"
            );
            operation = self
                .call(Command::PollOperation { id: operation.id }, false)
                .operation
                .unwrap();
        }
        assert_eq!(
            operation.status,
            OperationStatus::Complete,
            "{:?}",
            operation.error
        );
    }
}

#[test]
fn live_mcp_controls_read_search_annotate_insert_label_save_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    // This test binary has one test. Preferences/cache never touch the user's folders.
    std::env::set_var("APPDATA", directory.path());
    std::env::set_var("LOCALAPPDATA", directory.path());
    for key in [
        "KINETIC_PDF_CACHE",
        "KINETIC_PDF_HELPERS",
        "KINETIC_PDF_UPDATE",
    ] {
        std::env::set_var(key, "0");
    }
    let path = directory.path().join("input.pdf");
    std::fs::write(&path, common::build_pdf_rotated(&[90, 0], &[])).unwrap();
    let ctx = egui::Context::default();
    let app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
    let server = Server::start(app.control_client(), 0).unwrap();
    let config = server.codex_config();
    let auth = config
        .split("Authorization = \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let mut live = Live {
        app,
        ctx,
        frame: eframe::Frame::_new_kittest(),
        server,
        auth,
        clock: 0.0,
    };
    let initialized = live.rpc(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"roundtrip-test","version":"1"}}}));
    assert!(initialized["result"]["protocolVersion"].is_string());
    let opened = live.call(
        Command::Open {
            path: path.to_string_lossy().into_owned(),
        },
        false,
    );
    live.wait(opened);
    assert_eq!(live.app.control_state().pages, 2);
    let text = live.call(Command::ReadText { page: 1 }, true);
    live.wait(text);
    let text = live.call(Command::ReadText { page: 1 }, true);
    let Some(Data::Text { text, .. }) = text.data else {
        panic!("missing text")
    };
    assert!(text.contains("Page 1 line 1"));
    let search = live.call(
        Command::Find {
            query: "line 3".into(),
        },
        true,
    );
    live.wait(search);
    let results = live.call(Command::SearchResults, true);
    let Some(Data::Search { results, complete }) = results.data else {
        panic!("missing search")
    };
    assert!(complete);
    assert_eq!(results.len(), 2);
    live.call(Command::GoToResult { result: 2 }, true);
    assert_eq!(live.app.control_state().current_page, 2);
    live.call(Command::GoToPage { page: 1 }, true);
    live.call(
        Command::AddHighlight {
            page: 1,
            quads: vec![kinetic_pdf::control::PdfRect {
                left: 70.0,
                bottom: 715.0,
                right: 300.0,
                top: 735.0,
            }],
            comment: "MCP roundtrip".into(),
            color: [1.0, 0.9, 0.2],
        },
        true,
    );
    live.call(
        Command::InsertBlankPage {
            at: 2,
            size: [300.0, 400.0],
        },
        true,
    );
    live.call(
        Command::SetPageLabel {
            page: 2,
            label: Some("測定 – blank".into()),
        },
        true,
    );
    let destination = directory.path().join("output.pdf");
    let saved = live.call(
        Command::SaveAs {
            path: destination.to_string_lossy().into_owned(),
            overwrite: false,
        },
        true,
    );
    live.wait(saved);
    assert!(destination.is_file());
    assert!(!live.app.control_state().dirty);
    let pages = live.call(Command::ListPages, true);
    let Some(Data::Pages(pages)) = pages.data else {
        panic!("missing pages")
    };
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[1].label.as_deref(), Some("測定 – blank"));
    let opened = live.call(
        Command::Open {
            path: destination.to_string_lossy().into_owned(),
        },
        false,
    );
    live.wait(opened);
    assert_eq!(live.app.control_state().pages, 3);
    // Exercise the standalone PowerShell runner against this same live MCP endpoint.
    let script_path = directory.path().join("demo.json");
    std::fs::write(
        &script_path,
        serde_json::to_vec(&serde_json::json!({
            "version":1,"window":null,"steps":[
                {"step":"command","command":{"command":"go_to_page","page":2}},
                {"step":"cue","caption":"測定 demo","shortcut":"Ctrl+K","pointer":[0.5,0.5]},
                {"step":"command","command":{"command":"invoke","action":"quick_access"}},
                {"step":"pause","ms":300}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let runner = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/play-demo.ps1");
    let quoted = |path: &std::path::Path| path.to_string_lossy().replace('\'', "''");
    let code = format!("& '{}' -Endpoint '{}' -ScriptPath '{}' -Token (ConvertTo-SecureString $env:KINETIC_TEST_DEMO_TOKEN -AsPlainText -Force)",quoted(&runner),live.server.url(),quoted(&script_path));
    let token = live.auth.strip_prefix("Bearer ").unwrap().to_string();
    let (finished, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &code,
            ])
            .env("KINETIC_TEST_DEMO_TOKEN", token)
            .env_remove("PSModulePath")
            .output()
            .unwrap();
        finished.send(output).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let output = loop {
        live.tick();
        if let Ok(output) = rx.try_recv() {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell demo runner did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Demo complete"));
    assert_eq!(live.app.control_state().current_page, 2);
    assert!(live.app.control_state().palette_open);
    // Read an actual file independently of the UI's session state.
    let pdf = pdf_content::lopdf::Document::load(&destination).unwrap();
    assert_eq!(pdf.get_pages().len(), 3);
    let annotations: Vec<_> = pdf
        .get_pages()
        .values()
        .flat_map(|&id| pdf.get_page_annotations(id).unwrap())
        .filter(|d| {
            pdf_io::values::read_text(&pdf, d, b"Contents").as_deref() == Some("MCP roundtrip")
        })
        .collect();
    assert_eq!(annotations.len(), 1);
}
