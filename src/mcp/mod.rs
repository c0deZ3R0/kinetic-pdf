//! MCP is an adapter to `control`, not an owner of application state.
mod server;
pub use server::Server;
use crate::control;
use rmcp::{model::*, service::RequestContext, ErrorData, RoleServer, ServerHandler};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
struct Handler { client: control::Client }

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        let schema = rmcp::schemars::schema_for!(control::Request).to_value();
        let mut inspect = Tool::new("kinetic_inspect", "Inspect the connected Kinetic PDF window and get the document target for subsequent commands.", Arc::new(serde_json::from_value(serde_json::json!({"type":"object","properties":{},"additionalProperties":false})).unwrap()));
        inspect.annotations = Some(ToolAnnotations::new().read_only(true));
        let control = Tool::new("kinetic_control", "Control this Kinetic PDF window. Pages are one-based displayed sheets. Supply the target returned by inspect to guard against a changed document. Inspect after errors before retrying writes.", Arc::new(serde_json::from_value(schema).map_err(|e| ErrorData::internal_error(e.to_string(), None))?));
        Ok(ListToolsResult { tools: vec![inspect, control], ..Default::default() })
    }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        let command = match request.name.as_ref() {
            "kinetic_inspect" => {
                if !arguments.is_empty() { return Err(ErrorData::invalid_params("Inspect takes no arguments", None)); }
                control::Request { command: control::Command::Inspect, target: None }
            },
            "kinetic_control" => serde_json::from_value(serde_json::Value::Object(arguments)).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?,
            _ => return Err(ErrorData::invalid_params("Unknown tool", None)),
        };
        let result = match self.client.submit(command, Duration::from_secs(30)) {
            Ok(ticket) => {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(result) = ticket.try_result() { break result; }
                    tokio::select! {
                        _ = context.ct.cancelled() => break Err(control::Error::new(control::ErrorCode::Cancelled, "Request cancelled; inspect before retrying an operation that may have started")),
                        _ = tokio::time::sleep_until(deadline) => break Err(control::Error::new(control::ErrorCode::Timeout, "App did not answer; queued work is cancelled. Inspect before retrying")),
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
                    }
                }
            },
            Err(error) => Err(error),
        };
        let (value, is_error) = match result {
            Ok(state) => (serde_json::json!({"state":state}), false),
            Err(error) => (serde_json::json!({"error":error}), true),
        };
        let response = if is_error { CallToolResult::structured_error(value) } else { CallToolResult::structured(value) };
        Ok(response.into())
    }
}
