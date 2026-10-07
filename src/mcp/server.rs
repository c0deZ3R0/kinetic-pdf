//! Local-only authenticated transport and its lifetime.
use super::Handler;
use crate::control::Client;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::{
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio_util::sync::CancellationToken;

pub struct Server {
    url: String,
    token: String,
    cancel: CancellationToken,
    running: Arc<AtomicBool>,
}

impl Server {
    /// Port zero chooses a free port; occupied explicit ports report an error.
    pub fn start(client: Client, port: u16) -> Result<Self, String> {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let cancel = CancellationToken::new();
        let ct = cancel.clone();
        let running = Arc::new(AtomicBool::new(true));
        let alive = running.clone();
        let client = client.scoped(running.clone());
        let auth = format!("Bearer {token}");
        let host = format!("127.0.0.1:{port}");
        std::thread::Builder::new()
            .name("kinetic-mcp".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let mut config =
                        StreamableHttpServerConfig::default().with_cancellation_token(ct.clone());
                    config.legacy_session_mode = false;
                    config.json_response = true;
                    config.max_request_body_bytes = 2 * 1024 * 1024;
                    config.allowed_hosts = vec![host.clone()];
                    config.allowed_origins = vec![format!("http://{host}")];
                    let service = StreamableHttpService::new(
                        move || {
                            Ok(Handler {
                                client: client.clone(),
                            })
                        },
                        Arc::new(LocalSessionManager::default()),
                        config,
                    );
                    let router = axum::Router::new().nest_service("/mcp", service).layer(
                        middleware::from_fn_with_state(
                            Auth {
                                header: auth,
                                cancel: ct.clone(),
                            },
                            authenticate,
                        ),
                    );
                    if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                        let _ = axum::serve(listener, router)
                            .with_graceful_shutdown(ct.cancelled_owned())
                            .await;
                    }
                });
                alive.store(false, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token,
            cancel,
            running,
        })
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    /// Explicitly copied by the user; never logged or included in tool results.
    pub fn codex_config(&self) -> String {
        format!("[mcp_servers.kinetic_pdf]\nurl = {:?}\nhttp_headers = {{ Authorization = \"Bearer {}\" }}\n", self.url, self.token)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.cancel.cancel();
    }
}

#[derive(Clone)]
struct Auth {
    header: String,
    cancel: CancellationToken,
}
async fn authenticate(
    State(auth): State<Auth>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if auth.cancel.is_cancelled() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let value = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !equal(value.as_bytes(), auth.header.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (&a, &b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn dropping_the_server_revokes_requests_already_waiting_in_the_app_queue() {
        let (client, inbox) = crate::control::channel(|| {});
        let server = Server::start(client, 0).unwrap();
        let address = server.url.clone();
        let token = server.token.clone();
        let sender = std::thread::spawn(move || {
            post_url(
                &address,
                &format!("Authorization: Bearer {token}\r\n"),
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kinetic_inspect","arguments":{}}}"#,
            )
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let envelope = loop {
            if let Some(envelope) = inbox.next() {
                break envelope;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert!(envelope.can_start());
        drop(server);
        assert!(
            !envelope.can_start(),
            "revocation must not wait for HTTP cancellation delivery"
        );
        drop(envelope);
        let _ = sender.join().unwrap();
    }
    fn post(server: &Server, headers: &str, body: &str) -> String {
        post_url(server.url(), headers, body)
    }
    fn post_url(url: &str, headers: &str, body: &str) -> String {
        let address = url.trim_start_matches("http://").trim_end_matches("/mcp");
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
    #[test]
    fn authenticated_http_negotiates_and_publishes_tools_but_rejects_untrusted_origins() {
        let (client, inbox) = crate::control::channel(|| {});
        let server = Server::start(client, 0).unwrap();
        let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
        assert!(post(&server, "", initialize).starts_with("HTTP/1.1 401"));
        let auth = format!("Authorization: Bearer {}\r\n", server.token);
        let response = post(&server, &auth, initialize);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("protocolVersion"));
        let tools = post(
            &server,
            &auth,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        assert!(
            tools.contains("kinetic_inspect") && tools.contains("kinetic_control"),
            "{tools}"
        );
        assert!(post(
            &server,
            &format!("{auth}Origin: https://untrusted.example\r\n"),
            initialize
        )
        .starts_with("HTTP/1.1 403"));
        assert!(
            inbox.next().is_none(),
            "protocol and rejected requests never touch the app"
        );
    }
    #[test]
    fn every_connection_has_a_fresh_credential_and_bound_port() {
        let (client, _inbox) = crate::control::channel(|| {});
        let first = Server::start(client.clone(), 0).unwrap();
        let second = Server::start(client.clone(), 0).unwrap();
        assert_ne!(first.token, second.token);
        assert!(first.url.starts_with("http://127.0.0.1:"));
        let port = first
            .url
            .split(':')
            .nth(2)
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(Server::start(client, port).is_err());
    }
}
