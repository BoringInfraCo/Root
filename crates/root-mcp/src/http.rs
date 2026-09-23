//! Opt-in Streamable HTTP on loopback.
//!
//! This is the 2025-03-26 POST/session shape over the same JSON-RPC session as
//! stdio (`2024-11-05`). It is not the 2026-07-28 per-request protocol.
//! `Authorization: Bearer` is required. The token is never accepted in the URL.
//! GET is refused (no server-initiated stream). DELETE ends a session.

use crate::auth;
use crate::jsonrpc;
use crate::server::{handle_message, open_session};
use crate::session::ServerState;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_BODY: usize = 1024 * 1024;
const PATH: &str = "/mcp";

pub fn parse_loopback(addr: &str) -> Result<SocketAddr> {
    let addr = addr.trim();
    let socket = if let Some(port) = addr.strip_prefix("localhost:") {
        let port: u16 = port
            .parse()
            .with_context(|| format!("invalid HTTP port in {addr}"))?;
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    } else {
        addr.parse()
            .with_context(|| format!("invalid HTTP address {addr}"))?
    };
    match socket.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => Ok(socket),
        _ => anyhow::bail!("HTTP must bind to 127.0.0.1, not {socket}"),
    }
}

pub fn serve(listener: TcpListener, cwd: PathBuf, token: String) -> Result<()> {
    let bound = listener.local_addr()?;
    let mut sessions: HashMap<String, ServerState> = HashMap::new();
    for accepted in listener.incoming() {
        let Ok(mut stream) = accepted else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        if let Ok(request) = read_request(&mut stream) {
            let response = dispatch(&mut sessions, &cwd, &token, bound.port(), &request);
            let _ = write_response(&mut stream, &response);
        }
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    reason: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn dispatch(
    sessions: &mut HashMap<String, ServerState>,
    cwd: &Path,
    token: &str,
    port: u16,
    request: &Request,
) -> Response {
    if request.path != PATH {
        return text(404, "Not Found", "not found");
    }
    if query_has_token(&request.query) {
        return json_error(
            400,
            "Bad Request",
            jsonrpc::INVALID_REQUEST,
            Value::Null,
            "tokens must be sent in the Authorization header",
        );
    }
    let presented = request.headers.get("authorization").map(String::as_str);
    if !auth::bearer_matches(presented, token) {
        return Response {
            status: 401,
            reason: "Unauthorized",
            headers: vec![("WWW-Authenticate".into(), "Bearer realm=\"rootd\"".into())],
            body: Vec::new(),
        };
    }
    if let Some(origin) = request.headers.get("origin") {
        if !origin_allowed(origin, port) {
            return json_error(
                403,
                "Forbidden",
                jsonrpc::INTERNAL_ERROR,
                Value::Null,
                "invalid origin",
            );
        }
    }
    match request.method.as_str() {
        "POST" => post(sessions, cwd, request),
        "DELETE" => delete(sessions, request),
        _ => {
            let mut response = text(405, "Method Not Allowed", "method not allowed");
            response
                .headers
                .push(("Allow".into(), "POST, DELETE".into()));
            response
        }
    }
}

fn post(sessions: &mut HashMap<String, ServerState>, cwd: &Path, request: &Request) -> Response {
    if let Some(version) = request.headers.get("mcp-protocol-version") {
        if version == "2026-07-28" {
            return json_error(
                400,
                "Bad Request",
                -32022,
                Value::Null,
                "unsupported protocol version 2026-07-28; this listener speaks 2024-11-05",
            );
        }
    }
    if let Some(accept) = request.headers.get("accept") {
        if !accept.to_ascii_lowercase().contains("application/json") {
            return text(
                406,
                "Not Acceptable",
                "Accept must include application/json",
            );
        }
    }
    let body: Value = match serde_json::from_slice(&request.body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => {
            return json_error(
                400,
                "Bad Request",
                jsonrpc::INVALID_REQUEST,
                Value::Null,
                "POST body must be one JSON object",
            );
        }
        Err(_) => {
            return json_error(
                400,
                "Bad Request",
                jsonrpc::PARSE_ERROR,
                Value::Null,
                "parse error",
            );
        }
    };
    let rpc_id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "initialize" {
        let mut state = match open_session(cwd) {
            Ok(state) => state,
            Err(error) => {
                return json_error(
                    400,
                    "Bad Request",
                    jsonrpc::INTERNAL_ERROR,
                    rpc_id,
                    error.to_string(),
                );
            }
        };
        let response = handle_message(&mut state, body);
        if response
            .as_ref()
            .and_then(|value| value.get("error"))
            .is_some()
        {
            return json_ok(response, None);
        }
        let id = match crate::auth::random_hex(16) {
            Ok(id) => id,
            Err(error) => {
                return json_error(
                    500,
                    "Internal Server Error",
                    jsonrpc::INTERNAL_ERROR,
                    rpc_id,
                    error.to_string(),
                );
            }
        };
        sessions.insert(id.clone(), state);
        return json_ok(response, Some(id));
    }
    let Some(session_id) = request.headers.get("mcp-session-id") else {
        return json_error(
            400,
            "Bad Request",
            jsonrpc::INVALID_REQUEST,
            body.get("id").cloned().unwrap_or(Value::Null),
            "Mcp-Session-Id is required",
        );
    };
    let Some(state) = sessions.get_mut(session_id) else {
        return json_error(
            404,
            "Not Found",
            jsonrpc::INVALID_REQUEST,
            body.get("id").cloned().unwrap_or(Value::Null),
            "unknown session",
        );
    };
    let response = handle_message(state, body);
    if response.is_none() {
        return Response {
            status: 202,
            reason: "Accepted",
            headers: Vec::new(),
            body: Vec::new(),
        };
    }
    json_ok(response, None)
}

fn delete(sessions: &mut HashMap<String, ServerState>, request: &Request) -> Response {
    let Some(session_id) = request.headers.get("mcp-session-id") else {
        return json_error(
            400,
            "Bad Request",
            jsonrpc::INVALID_REQUEST,
            Value::Null,
            "Mcp-Session-Id is required",
        );
    };
    if sessions.remove(session_id).is_none() {
        return json_error(
            404,
            "Not Found",
            jsonrpc::INVALID_REQUEST,
            Value::Null,
            "unknown session",
        );
    }
    Response {
        status: 204,
        reason: "No Content",
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn query_has_token(query: &str) -> bool {
    query.split('&').any(|pair| {
        let name = pair.split('=').next().unwrap_or("");
        name == "token" || name == "access_token"
    })
}

pub fn origin_allowed(origin: &str, port: u16) -> bool {
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

fn json_ok(message: Option<Value>, session: Option<String>) -> Response {
    let body = message.unwrap_or(Value::Null);
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    let mut headers = vec![("Content-Type".into(), "application/json".into())];
    if let Some(session) = session {
        headers.push(("Mcp-Session-Id".into(), session));
    }
    Response {
        status: 200,
        reason: "OK",
        headers,
        body: bytes,
    }
}

fn json_error(
    status: u16,
    reason: &'static str,
    code: i64,
    id: Value,
    message: impl Into<String>,
) -> Response {
    let payload = jsonrpc::error(id, code, message);
    let body = serde_json::to_vec(&payload).unwrap_or_default();
    Response {
        status,
        reason,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body,
    }
}

fn text(status: u16, reason: &'static str, message: &str) -> Response {
    Response {
        status,
        reason,
        headers: vec![("Content-Type".into(), "text/plain".into())],
        body: message.as_bytes().to_vec(),
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut header_buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if header_buf.len() > 64 * 1024 {
            anyhow::bail!("HTTP headers are too large");
        }
        let n = stream.read(&mut byte)?;
        if n == 0 {
            anyhow::bail!("closed before headers");
        }
        header_buf.push(byte[0]);
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8(header_buf).context("HTTP headers are not UTF-8")?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        anyhow::bail!("HTTP body is too large");
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.reason,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_parse_rejects_other_interfaces() {
        assert_eq!(parse_loopback("127.0.0.1:8737").unwrap().port(), 8737);
        assert_eq!(parse_loopback("localhost:9").unwrap().port(), 9);
        assert!(parse_loopback("0.0.0.0:8737").is_err());
        assert!(parse_loopback("192.168.1.8:8737").is_err());
        assert!(parse_loopback("[::1]:8737").is_err());
    }

    #[test]
    fn origin_allows_only_this_listener() {
        assert!(origin_allowed("http://127.0.0.1:8737", 8737));
        assert!(origin_allowed("http://localhost:8737", 8737));
        assert!(!origin_allowed("http://127.0.0.1:1", 8737));
        assert!(!origin_allowed("https://127.0.0.1:8737", 8737));
        assert!(!origin_allowed("http://evil.example", 8737));
        assert!(!origin_allowed("null", 8737));
    }
}
