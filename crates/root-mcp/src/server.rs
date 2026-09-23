//! JSON-RPC request handling and the stdio transport.

use crate::jsonrpc;
use crate::policy::{self, Policy};
use crate::protocol;
use crate::session::ServerState;
use crate::tools::{self, ToolError};
use anyhow::{Context, Result};
use root_work::{Repository, WorkStore};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

#[derive(Debug, Serialize)]
pub struct McpWorkspaceSummary {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct McpStatusReport {
    pub success: bool,
    pub workspace: Option<McpWorkspaceSummary>,
    pub capabilities: policy::CapabilitiesView,
    pub policy_source: String,
    pub tools: Vec<String>,
    pub protocol_version: String,
}

enum DispatchError {
    InvalidParams(String),
    MethodNotFound(String),
    Internal(String),
}

/// Handle a single decoded JSON-RPC message. Returns `None` for notifications.
pub fn handle_message(state: &mut ServerState, message: Value) -> Option<Value> {
    let object = match message.as_object() {
        Some(object) => object,
        None => {
            return Some(jsonrpc::error(
                Value::Null,
                jsonrpc::INVALID_REQUEST,
                "Invalid Request: expected a JSON object.",
            ))
        }
    };

    let id_present = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);

    let method = match object.get("method") {
        Some(Value::String(method)) => method.clone(),
        _ => {
            if !id_present {
                return None;
            }
            return Some(jsonrpc::error(
                id,
                jsonrpc::INVALID_REQUEST,
                "Invalid Request: missing method.",
            ));
        }
    };

    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let outcome = dispatch(state, &method, &params);

    if !id_present {
        return None;
    }

    let response = match outcome {
        Ok(result) => jsonrpc::success(id, result),
        Err(DispatchError::InvalidParams(message)) => {
            jsonrpc::error(id, jsonrpc::INVALID_PARAMS, message)
        }
        Err(DispatchError::MethodNotFound(message)) => {
            jsonrpc::error(id, jsonrpc::METHOD_NOT_FOUND, message)
        }
        Err(DispatchError::Internal(message)) => {
            jsonrpc::error(id, jsonrpc::INTERNAL_ERROR, message)
        }
    };
    Some(response)
}

/// Parse a single newline-delimited message, then handle it.
pub fn handle_line(state: &mut ServerState, line: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(message) => handle_message(state, message),
        Err(_) => Some(jsonrpc::parse_error()),
    }
}

fn dispatch(state: &mut ServerState, method: &str, params: &Value) -> Result<Value, DispatchError> {
    match method {
        "initialize" => initialize(state, params),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list()),
        "tools/call" => call_tool(state, params),
        other => Err(DispatchError::MethodNotFound(format!(
            "Method not found: {other}"
        ))),
    }
}

fn initialize(state: &mut ServerState, params: &Value) -> Result<Value, DispatchError> {
    let object = params.as_object();
    let requested = object
        .and_then(|object| object.get("protocolVersion"))
        .and_then(Value::as_str);
    let client_name = object
        .and_then(|object| object.get("clientInfo"))
        .and_then(Value::as_object)
        .and_then(|client| client.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    state
        .initialize(client_name.clone(), client_name)
        .map_err(|error| DispatchError::Internal(error.to_string()))?;

    Ok(json!({
        "protocolVersion": protocol::negotiate_protocol_version(requested),
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": protocol::SERVER_NAME,
            "version": protocol::SERVER_VERSION,
        },
    }))
}

fn call_tool(state: &mut ServerState, params: &Value) -> Result<Value, DispatchError> {
    let object = params.as_object().ok_or_else(|| {
        DispatchError::InvalidParams("tools/call params must be an object.".into())
    })?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::InvalidParams("tools/call requires a string 'name'.".into()))?
        .to_string();

    let arguments = match object.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => {
            return Err(DispatchError::InvalidParams(
                "tools/call 'arguments' must be an object.".into(),
            ))
        }
    };

    let Some(tool) = tools::find(&name) else {
        return match crate::connector::invoke(&name, &arguments) {
            Some(Ok(payload)) => Ok(tool_success(payload)),
            Some(Err(ToolError::Invalid(message))) => Ok(tool_error(message)),
            Some(Err(ToolError::Internal(message))) => Err(DispatchError::Internal(message)),
            None => Err(DispatchError::InvalidParams(format!(
                "Unknown tool '{name}'."
            ))),
        };
    };

    if !state.policy.allowed(tool.capability) {
        return Ok(tool_error(format!(
            "Unauthorized: {} capability is denied by policy.",
            tool.capability
        )));
    }

    match tools::dispatch(state, &name, &arguments) {
        Ok(payload) => Ok(tool_success(payload)),
        Err(ToolError::Invalid(message)) => Ok(tool_error(message)),
        Err(ToolError::Internal(message)) => Err(DispatchError::Internal(message)),
    }
}

fn tool_success(payload: Value) -> Value {
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": false,
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    let message: String = message.into();
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

/// Serve newline-delimited JSON-RPC 2.0 over an already-bound session.
pub fn run_lines<I, W>(state: &mut ServerState, lines: I, mut output: W) -> Result<()>
where
    I: Iterator<Item = std::io::Result<String>>,
    W: Write,
{
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(state, &line) {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// Serve newline-delimited JSON-RPC 2.0 over stdin/stdout until EOF.
pub fn run_stdio(state: &mut ServerState) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_lines(state, stdin.lock().lines(), stdout.lock())
}

pub(crate) fn open_session(cwd: &std::path::Path) -> Result<ServerState> {
    let repository = Repository::discover(cwd)
        .with_context(|| format!("not a git repository: {}", cwd.display()))?;
    let store = WorkStore::open(repository.clone())?;
    let root_dir = policy::root_dir()?;
    let policy = Policy::load_at(&root_dir);
    Ok(ServerState::new(store, root_dir, repository, policy))
}

/// Stdio compatibility shim. Confirms the workspace the way v0.6 did, then
/// proxies newline-delimited JSON-RPC to rootd. A daemon is started for this
/// Root directory when one is not already listening.
pub fn serve() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repository = Repository::discover(&cwd)?;
    // Fail closed before proxying, with the same errors as the in-process server.
    drop(WorkStore::open(repository)?);

    let stream = match crate::daemon::connect_stdio_shim() {
        Ok(stream) => stream,
        Err(error) => {
            let response = jsonrpc::error(Value::Null, jsonrpc::INTERNAL_ERROR, error.to_string());
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            return Err(error);
        }
    };
    let root_dir = policy::root_dir()?;
    let token = crate::auth::read_existing(&root_dir)?;
    let mut writer = stream.try_clone()?;
    writeln!(
        writer,
        "{}",
        serde_json::json!({
            "cwd": cwd,
            "authorization": format!("Bearer {token}"),
        })
    )?;
    writer.flush()?;

    let copy_in = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let _ = std::io::copy(&mut stdin, &mut writer);
        let _ = writer.shutdown(std::net::Shutdown::Write);
    });

    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let mut flushing = FlushOnWrite(&mut stdout);
    let _ = std::io::copy(&mut &stream, &mut flushing);
    drop(copy_in);
    Ok(())
}

struct FlushOnWrite<W>(W);

impl<W: Write> Write for FlushOnWrite<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.0.write(buf)?;
        self.0.flush()?;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

pub fn status() -> Result<McpStatusReport> {
    let root_dir = policy::root_dir()?;
    let policy = Policy::load_at(&root_dir);
    let cwd = std::env::current_dir()?;
    let workspace = Repository::discover(&cwd)
        .ok()
        .and_then(|repository| WorkStore::open(repository).ok())
        .map(|store| McpWorkspaceSummary {
            id: store.workspace().id.clone(),
            name: store.workspace().name.clone(),
        });

    Ok(McpStatusReport {
        success: true,
        workspace,
        capabilities: policy.view(),
        policy_source: policy.source().to_string(),
        tools: crate::registry::registered()
            .into_iter()
            .map(|tool| tool.name)
            .collect(),
        protocol_version: protocol::PROTOCOL_VERSION.to_string(),
    })
}
