//! Ollama inspector and realizer.
//!
//! Inspect is read-only. Realize can POST /api/pull. Hosts must be loopback.

use crate::inventory::{
    looks_secret, REASON_ENDPOINT_UNREACHABLE, REASON_MALFORMED_OUTPUT, REASON_NOT_FOUND,
    REASON_PROTOCOL_UNSUPPORTED, REASON_TIMED_OUT,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const HTTP_BODY_LIMIT: usize = 1_048_576;
const DEFAULT_OLLAMA_HOST: &str = "127.0.0.1";
const DEFAULT_OLLAMA_PORT: u16 = 11434;
const INSPECT_TIMEOUT: Duration = Duration::from_secs(2);
const PULL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Idle timeout between NDJSON bytes during POST /api/pull. No total pull timeout.
pub const OLLAMA_PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub const REASON_REMOTE_OR_CLOUD_UNSUPPORTED: &str = "remote_or_cloud_unsupported";
pub const REASON_PULL_FAILED: &str = "pull_failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProtocol {
    Ready,
    Unreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProbe {
    pub version: Option<String>,
    pub protocol: RuntimeProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedModel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectError {
    EndpointUnreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
}

impl InspectError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::EndpointUnreachable => REASON_ENDPOINT_UNREACHABLE,
            Self::TimedOut => REASON_TIMED_OUT,
            Self::Malformed => REASON_MALFORMED_OUTPUT,
            Self::ProtocolUnsupported => REASON_PROTOCOL_UNSUPPORTED,
        }
    }
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for InspectError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizeError {
    EndpointUnreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
    NotFound,
    Failed,
    RemoteOrCloudUnsupported,
}

impl RealizeError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::EndpointUnreachable => REASON_ENDPOINT_UNREACHABLE,
            Self::TimedOut => REASON_TIMED_OUT,
            Self::Malformed => REASON_MALFORMED_OUTPUT,
            Self::ProtocolUnsupported => REASON_PROTOCOL_UNSUPPORTED,
            Self::NotFound => REASON_NOT_FOUND,
            Self::Failed => REASON_PULL_FAILED,
            Self::RemoteOrCloudUnsupported => REASON_REMOTE_OR_CLOUD_UNSUPPORTED,
        }
    }
}

impl std::fmt::Display for RealizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for RealizeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullProgress {
    pub status: String,
    pub digest: Option<String>,
    pub total: Option<u64>,
    pub completed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullOutcome {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_completed: Option<u64>,
}

pub trait OllamaInspector {
    fn inspect_runtime(&self) -> RuntimeProbe;
    fn list_models(&self) -> Result<Vec<ListedModel>, InspectError>;
}

pub trait OllamaRealizer {
    fn pull_tag(
        &self,
        name: &str,
        progress: &mut dyn FnMut(PullProgress),
    ) -> Result<PullOutcome, RealizeError>;
}

pub fn resolve_model_tag(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("{name}:latest")
    }
}

pub fn model_matches(declared: &str, listed_name: &str, listed_model: Option<&str>) -> bool {
    let resolved = resolve_model_tag(declared);
    let candidate_matches = |candidate: &str| {
        candidate == declared || candidate == resolved || resolve_model_tag(candidate) == resolved
    };
    candidate_matches(listed_name) || listed_model.is_some_and(candidate_matches)
}

pub fn is_remote_or_cloud(declared: &str, listed: Option<&ListedModel>) -> bool {
    let lower = declared.to_ascii_lowercase();
    if lower.ends_with("-cloud") || lower.ends_with(":cloud") {
        return true;
    }
    listed.is_some_and(|model| {
        model.remote_host.as_deref().is_some_and(|s| !s.is_empty())
            || model.remote_model.as_deref().is_some_and(|s| !s.is_empty())
    })
}

pub fn canonicalize_digest(input: &str) -> Option<String> {
    if looks_secret(input) {
        return None;
    }
    let hex = input
        .strip_prefix("sha256:")
        .or_else(|| input.strip_prefix("sha256-"))
        .unwrap_or(input);
    if hex.len() != 64 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    if looks_secret(hex) {
        return None;
    }
    Some(format!("sha256:{}", hex.to_ascii_lowercase()))
}

pub fn digests_equal(left: &str, right: &str) -> bool {
    match (canonicalize_digest(left), canonicalize_digest(right)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn pull_request_json(resolved_name: &str) -> String {
    serde_json::json!({
        "model": resolved_name,
        "stream": true,
    })
    .to_string()
}

fn inspect_to_realize(err: InspectError) -> RealizeError {
    match err {
        InspectError::EndpointUnreachable => RealizeError::EndpointUnreachable,
        InspectError::TimedOut => RealizeError::TimedOut,
        InspectError::Malformed => RealizeError::Malformed,
        InspectError::ProtocolUnsupported => RealizeError::ProtocolUnsupported,
    }
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
}

fn nonempty_json_str(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .filter(|s| !looks_secret(s))
        .map(str::to_string)
}

fn listed_from_json(model: &serde_json::Value) -> Result<ListedModel, InspectError> {
    let name = model
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or(InspectError::ProtocolUnsupported)?;
    Ok(ListedModel {
        name: name.to_string(),
        model: model
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        digest: model
            .get("digest")
            .and_then(|v| v.as_str())
            .filter(|digest| !looks_secret(digest))
            .map(str::to_string),
        size: model.get("size").and_then(json_u64),
        remote_host: nonempty_json_str(model.get("remote_host")),
        remote_model: nonempty_json_str(model.get("remote_model")),
    })
}

fn progress_from_event(event: &serde_json::Value) -> Option<PullProgress> {
    let status = event.get("status")?.as_str()?;
    if looks_secret(status) {
        return None;
    }
    Some(PullProgress {
        status: status.to_string(),
        digest: event
            .get("digest")
            .and_then(|v| v.as_str())
            .filter(|digest| !looks_secret(digest))
            .map(str::to_string),
        total: event.get("total").and_then(json_u64),
        completed: event.get("completed").and_then(json_u64),
    })
}

fn event_is_error(event: &serde_json::Value) -> bool {
    event
        .get("error")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

fn record_completed(completed: &mut BTreeMap<String, u64>, event: &serde_json::Value) {
    let Some(digest) = event.get("digest").and_then(|v| v.as_str()) else {
        return;
    };
    if digest.is_empty() || looks_secret(digest) {
        return;
    }
    let Some(bytes) = event.get("completed").and_then(json_u64) else {
        return;
    };
    completed.insert(digest.to_string(), bytes);
}

fn bytes_completed_from_map(completed: &BTreeMap<String, u64>) -> Option<u64> {
    if completed.is_empty() {
        None
    } else {
        Some(completed.values().copied().sum())
    }
}

fn outcome_from_list(
    resolved: &str,
    models: &[ListedModel],
    bytes_completed: Option<u64>,
) -> Result<PullOutcome, RealizeError> {
    let Some(found) = models
        .iter()
        .find(|model| model_matches(resolved, &model.name, model.model.as_deref()))
    else {
        return Err(RealizeError::Failed);
    };
    if is_remote_or_cloud(resolved, Some(found)) {
        return Err(RealizeError::RemoteOrCloudUnsupported);
    }
    Ok(PullOutcome {
        name: resolved.to_string(),
        digest: found.digest.as_deref().and_then(canonicalize_digest),
        bytes_completed,
    })
}

#[derive(Debug, Clone)]
pub enum MockPullScript {
    Stream {
        status: u16,
        events: Vec<serde_json::Value>,
    },
    Stall,
}

impl Default for MockPullScript {
    fn default() -> Self {
        Self::Stream {
            status: 200,
            events: vec![serde_json::json!({"status": "success"})],
        }
    }
}

pub struct MockOllama {
    runtime: Mutex<RuntimeProbe>,
    models: Mutex<Vec<ListedModel>>,
    pull_script: Mutex<MockPullScript>,
    models_after_pull: Mutex<Option<Vec<ListedModel>>>,
    captured_pull_body: Mutex<Option<String>>,
}

impl Default for MockOllama {
    fn default() -> Self {
        Self {
            runtime: Mutex::new(RuntimeProbe {
                version: Some("0.11.0".into()),
                protocol: RuntimeProtocol::Ready,
            }),
            models: Mutex::new(Vec::new()),
            pull_script: Mutex::new(MockPullScript::default()),
            models_after_pull: Mutex::new(None),
            captured_pull_body: Mutex::new(None),
        }
    }
}

impl MockOllama {
    fn lock<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn with_runtime(self, runtime: RuntimeProbe) -> Self {
        *Self::lock(&self.runtime) = runtime;
        self
    }

    pub fn with_models(self, models: Vec<ListedModel>) -> Self {
        *Self::lock(&self.models) = models;
        self
    }

    pub fn with_pull_script(self, script: MockPullScript) -> Self {
        *Self::lock(&self.pull_script) = script;
        self
    }

    pub fn with_pull_status(self, status: u16) -> Self {
        *Self::lock(&self.pull_script) = MockPullScript::Stream {
            status,
            events: Vec::new(),
        };
        self
    }

    pub fn with_pull_events(self, events: Vec<serde_json::Value>) -> Self {
        *Self::lock(&self.pull_script) = MockPullScript::Stream {
            status: 200,
            events,
        };
        self
    }

    pub fn with_stall(self) -> Self {
        *Self::lock(&self.pull_script) = MockPullScript::Stall;
        self
    }

    pub fn with_models_after_pull(self, models: Vec<ListedModel>) -> Self {
        *Self::lock(&self.models_after_pull) = Some(models);
        self
    }

    pub fn captured_pull_body(&self) -> Option<String> {
        Self::lock(&self.captured_pull_body).clone()
    }
}

impl OllamaInspector for MockOllama {
    fn inspect_runtime(&self) -> RuntimeProbe {
        Self::lock(&self.runtime).clone()
    }

    fn list_models(&self) -> Result<Vec<ListedModel>, InspectError> {
        match Self::lock(&self.runtime).protocol {
            RuntimeProtocol::Ready => Ok(Self::lock(&self.models).clone()),
            RuntimeProtocol::Unreachable => Err(InspectError::EndpointUnreachable),
            RuntimeProtocol::TimedOut => Err(InspectError::TimedOut),
            RuntimeProtocol::Malformed => Err(InspectError::Malformed),
            RuntimeProtocol::ProtocolUnsupported => Err(InspectError::ProtocolUnsupported),
        }
    }
}

impl OllamaRealizer for MockOllama {
    fn pull_tag(
        &self,
        name: &str,
        progress: &mut dyn FnMut(PullProgress),
    ) -> Result<PullOutcome, RealizeError> {
        let resolved = resolve_model_tag(name);
        if is_remote_or_cloud(name, None) || is_remote_or_cloud(&resolved, None) {
            return Err(RealizeError::RemoteOrCloudUnsupported);
        }
        let body = pull_request_json(&resolved);
        *Self::lock(&self.captured_pull_body) = Some(body);

        let script = Self::lock(&self.pull_script).clone();
        match script {
            MockPullScript::Stall => Err(RealizeError::TimedOut),
            MockPullScript::Stream { status, events } => {
                if status == 404 {
                    return Err(RealizeError::NotFound);
                }
                if !(200..300).contains(&status) {
                    return Err(RealizeError::Failed);
                }
                let mut completed = BTreeMap::new();
                let mut saw_success = false;
                for event in &events {
                    if event_is_error(event) {
                        return Err(RealizeError::Failed);
                    }
                    record_completed(&mut completed, event);
                    if let Some(update) = progress_from_event(event) {
                        if update.status == "success" {
                            saw_success = true;
                        }
                        progress(update);
                    }
                }
                if !saw_success {
                    return Err(RealizeError::Failed);
                }
                if let Some(after) = Self::lock(&self.models_after_pull).clone() {
                    *Self::lock(&self.models) = after;
                }
                let models = self.list_models().map_err(inspect_to_realize)?;
                outcome_from_list(&resolved, &models, bytes_completed_from_map(&completed))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpOllama {
    host: String,
    port: u16,
    inspect_timeout: Duration,
    pull_connect_timeout: Duration,
    pull_idle_timeout: Duration,
}

impl Default for HttpOllama {
    fn default() -> Self {
        Self::new(DEFAULT_OLLAMA_HOST, DEFAULT_OLLAMA_PORT)
    }
}

impl HttpOllama {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            inspect_timeout: INSPECT_TIMEOUT,
            pull_connect_timeout: PULL_CONNECT_TIMEOUT,
            pull_idle_timeout: OLLAMA_PULL_IDLE_TIMEOUT,
        }
    }

    pub fn for_tests(
        host: impl Into<String>,
        port: u16,
        inspect_timeout: Duration,
        pull_idle_timeout: Duration,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            inspect_timeout,
            pull_connect_timeout: inspect_timeout,
            pull_idle_timeout,
        }
    }

    fn get_json(&self, path: &str) -> Result<serde_json::Value, InspectError> {
        match http_get_json(&self.host, self.port, path, self.inspect_timeout) {
            HttpOutcome::Json(value) => Ok(value),
            HttpOutcome::Unreachable => Err(InspectError::EndpointUnreachable),
            HttpOutcome::TimedOut => Err(InspectError::TimedOut),
            HttpOutcome::Malformed => Err(InspectError::Malformed),
            HttpOutcome::ProtocolUnsupported => Err(InspectError::ProtocolUnsupported),
        }
    }
}

impl OllamaInspector for HttpOllama {
    fn inspect_runtime(&self) -> RuntimeProbe {
        match self.get_json("/api/version") {
            Err(InspectError::EndpointUnreachable) => RuntimeProbe {
                version: None,
                protocol: RuntimeProtocol::Unreachable,
            },
            Err(InspectError::TimedOut) => RuntimeProbe {
                version: None,
                protocol: RuntimeProtocol::TimedOut,
            },
            Err(InspectError::Malformed) => RuntimeProbe {
                version: None,
                protocol: RuntimeProtocol::Malformed,
            },
            Err(InspectError::ProtocolUnsupported) => RuntimeProbe {
                version: None,
                protocol: RuntimeProtocol::ProtocolUnsupported,
            },
            Ok(value) => match value.get("version").and_then(|v| v.as_str()) {
                None => RuntimeProbe {
                    version: None,
                    protocol: RuntimeProtocol::ProtocolUnsupported,
                },
                Some(version) => RuntimeProbe {
                    version: Some(version.to_string()).filter(|v| !looks_secret(v)),
                    protocol: RuntimeProtocol::Ready,
                },
            },
        }
    }

    fn list_models(&self) -> Result<Vec<ListedModel>, InspectError> {
        let value = self.get_json("/api/tags")?;
        let models = value
            .get("models")
            .and_then(|v| v.as_array())
            .ok_or(InspectError::ProtocolUnsupported)?;
        let mut listed = Vec::with_capacity(models.len());
        for model in models {
            listed.push(listed_from_json(model)?);
        }
        Ok(listed)
    }
}

impl OllamaRealizer for HttpOllama {
    fn pull_tag(
        &self,
        name: &str,
        progress: &mut dyn FnMut(PullProgress),
    ) -> Result<PullOutcome, RealizeError> {
        let resolved = resolve_model_tag(name);
        if is_remote_or_cloud(name, None) || is_remote_or_cloud(&resolved, None) {
            return Err(RealizeError::RemoteOrCloudUnsupported);
        }
        let body = pull_request_json(&resolved);
        let events = http_post_pull_ndjson(
            &self.host,
            self.port,
            &body,
            self.pull_connect_timeout,
            self.pull_idle_timeout,
        )?;

        let mut completed = BTreeMap::new();
        let mut saw_success = false;
        for event in &events {
            if event_is_error(event) {
                return Err(RealizeError::Failed);
            }
            record_completed(&mut completed, event);
            if let Some(update) = progress_from_event(event) {
                if update.status == "success" {
                    saw_success = true;
                }
                progress(update);
            }
        }
        if !saw_success {
            return Err(RealizeError::Failed);
        }

        let models = self.list_models().map_err(inspect_to_realize)?;
        outcome_from_list(&resolved, &models, bytes_completed_from_map(&completed))
    }
}

enum HttpOutcome {
    Json(serde_json::Value),
    Unreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
}

fn http_get_json(host: &str, port: u16, path: &str, timeout: Duration) -> HttpOutcome {
    if port == 0 {
        return HttpOutcome::Unreachable;
    }
    let addr = match to_loopback_addr(host, port) {
        Some(addr) => addr,
        None => return HttpOutcome::Unreachable,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            return HttpOutcome::TimedOut;
        }
        Err(_) => return HttpOutcome::Unreachable,
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        return HttpOutcome::Unreachable;
    }

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return HttpOutcome::Unreachable;
    }

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let started = Instant::now();
    loop {
        if started.elapsed() >= timeout {
            return HttpOutcome::TimedOut;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > HTTP_BODY_LIMIT {
                    return HttpOutcome::Malformed;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                return HttpOutcome::TimedOut;
            }
            Err(_) => return HttpOutcome::Unreachable,
        }
    }

    parse_http_json(&buf)
}

fn to_loopback_addr(host: &str, port: u16) -> Option<SocketAddr> {
    if host != "127.0.0.1" && host != "localhost" {
        return None;
    }
    (host, port)
        .to_socket_addrs()
        .ok()?
        .find(|addr| addr.ip().is_loopback())
}

fn parse_http_json(response: &[u8]) -> HttpOutcome {
    let text = String::from_utf8_lossy(response);
    let Some((header, body)) = text.split_once("\r\n\r\n") else {
        return HttpOutcome::Malformed;
    };
    let status_line = header.lines().next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    if status == 404 || status == 405 {
        return HttpOutcome::ProtocolUnsupported;
    }
    if !(200..300).contains(&status) {
        return HttpOutcome::Malformed;
    }
    let decoded = match decode_http_body(header, body) {
        Some(decoded) => decoded,
        None => return HttpOutcome::Malformed,
    };
    match serde_json::from_str::<serde_json::Value>(&decoded) {
        Ok(value) => HttpOutcome::Json(value),
        Err(_) => HttpOutcome::Malformed,
    }
}

fn decode_http_body(header: &str, body: &str) -> Option<String> {
    let chunked = header.lines().any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    if !chunked {
        return Some(body.to_string());
    }
    decode_chunked(body)
}

fn decode_chunked(body: &str) -> Option<String> {
    let mut rest = body;
    let mut out = String::new();
    loop {
        let (size_line, after) = rest.split_once("\r\n")?;
        let size = usize::from_str_radix(size_line.trim(), 16).ok()?;
        if size == 0 {
            return Some(out);
        }
        let chunk = after.get(..size)?;
        out.push_str(chunk);
        rest = after.get(size..)?.strip_prefix("\r\n")?;
    }
}

fn http_post_pull_ndjson(
    host: &str,
    port: u16,
    body: &str,
    connect_timeout: Duration,
    idle_timeout: Duration,
) -> Result<Vec<serde_json::Value>, RealizeError> {
    if port == 0 {
        return Err(RealizeError::EndpointUnreachable);
    }
    let addr = to_loopback_addr(host, port).ok_or(RealizeError::EndpointUnreachable)?;
    let mut stream = match TcpStream::connect_timeout(&addr, connect_timeout) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            return Err(RealizeError::TimedOut);
        }
        Err(_) => return Err(RealizeError::EndpointUnreachable),
    };
    if stream.set_read_timeout(Some(idle_timeout)).is_err()
        || stream.set_write_timeout(Some(connect_timeout)).is_err()
    {
        return Err(RealizeError::EndpointUnreachable);
    }

    let header = format!(
        "POST /api/pull HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_err() || stream.write_all(body.as_bytes()).is_err() {
        return Err(RealizeError::EndpointUnreachable);
    }

    let mut reader = StreamBuf {
        stream: &mut stream,
        buf: Vec::new(),
    };
    let (status, header_text) = reader.read_headers()?;
    if status == 404 {
        return Err(RealizeError::NotFound);
    }
    if !(200..300).contains(&status) {
        return Err(RealizeError::Failed);
    }
    let chunked = header_text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    reader.read_ndjson_objects(chunked)
}

struct StreamBuf<'a> {
    stream: &'a mut TcpStream,
    buf: Vec<u8>,
}

impl StreamBuf<'_> {
    fn read_more(&mut self) -> Result<usize, RealizeError> {
        let mut tmp = [0u8; 4096];
        match self.stream.read(&mut tmp) {
            Ok(0) => Ok(0),
            Ok(n) => {
                self.buf.extend_from_slice(&tmp[..n]);
                Ok(n)
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                Err(RealizeError::TimedOut)
            }
            Err(_) => Err(RealizeError::EndpointUnreachable),
        }
    }

    fn read_headers(&mut self) -> Result<(u16, String), RealizeError> {
        loop {
            if let Some(pos) = find_double_crlf(&self.buf) {
                let header_bytes: Vec<u8> = self.buf.drain(..pos).collect();
                self.buf.drain(..4);
                if header_bytes.len() > HTTP_BODY_LIMIT {
                    return Err(RealizeError::Malformed);
                }
                let header = String::from_utf8_lossy(&header_bytes).into_owned();
                let status_line = header.lines().next().unwrap_or("");
                let status = status_line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|code| code.parse::<u16>().ok())
                    .unwrap_or(0);
                return Ok((status, header));
            }
            if self.buf.len() > HTTP_BODY_LIMIT {
                return Err(RealizeError::Malformed);
            }
            if self.read_more()? == 0 {
                return Err(RealizeError::Malformed);
            }
        }
    }

    fn read_ndjson_objects(
        &mut self,
        chunked: bool,
    ) -> Result<Vec<serde_json::Value>, RealizeError> {
        let mut events = Vec::new();
        let mut decoded = Vec::new();
        let mut incoming = Vec::new();
        incoming.append(&mut self.buf);
        let mut data_remaining: Option<usize> = None;
        let mut at_chunk_crlf = false;

        loop {
            if chunked {
                match feed_chunked(
                    &mut incoming,
                    &mut decoded,
                    &mut data_remaining,
                    &mut at_chunk_crlf,
                ) {
                    Ok(true) => {
                        flush_ndjson(&mut decoded, &mut events)?;
                        return Ok(events);
                    }
                    Ok(false) => {}
                    Err(()) => return Err(RealizeError::Malformed),
                }
            } else {
                decoded.append(&mut incoming);
            }

            drain_ndjson_lines(&mut decoded, &mut events)?;

            match self.read_more()? {
                0 => {
                    incoming.append(&mut self.buf);
                    if chunked {
                        match feed_chunked(
                            &mut incoming,
                            &mut decoded,
                            &mut data_remaining,
                            &mut at_chunk_crlf,
                        ) {
                            Ok(_) => {}
                            Err(()) => return Err(RealizeError::Malformed),
                        }
                    } else {
                        decoded.append(&mut incoming);
                    }
                    flush_ndjson(&mut decoded, &mut events)?;
                    return Ok(events);
                }
                _ => incoming.append(&mut self.buf),
            }
        }
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn flush_ndjson(
    decoded: &mut Vec<u8>,
    events: &mut Vec<serde_json::Value>,
) -> Result<(), RealizeError> {
    drain_ndjson_lines(decoded, events)?;
    if !decoded.is_empty() {
        let leftover = std::mem::take(decoded);
        push_ndjson_object(&leftover, events)?;
    }
    Ok(())
}

fn drain_ndjson_lines(
    decoded: &mut Vec<u8>,
    events: &mut Vec<serde_json::Value>,
) -> Result<(), RealizeError> {
    while let Some(idx) = decoded.iter().position(|b| *b == b'\n') {
        let mut line: Vec<u8> = decoded.drain(..=idx).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        push_ndjson_object(&line, events)?;
    }
    if decoded.len() > HTTP_BODY_LIMIT {
        return Err(RealizeError::Malformed);
    }
    Ok(())
}

fn push_ndjson_object(
    line: &[u8],
    events: &mut Vec<serde_json::Value>,
) -> Result<(), RealizeError> {
    if line.len() > HTTP_BODY_LIMIT {
        return Err(RealizeError::Malformed);
    }
    let value =
        serde_json::from_slice::<serde_json::Value>(line).map_err(|_| RealizeError::Malformed)?;
    events.push(value);
    Ok(())
}

fn feed_chunked(
    incoming: &mut Vec<u8>,
    decoded: &mut Vec<u8>,
    data_remaining: &mut Option<usize>,
    at_chunk_crlf: &mut bool,
) -> Result<bool, ()> {
    loop {
        if *at_chunk_crlf {
            if incoming.len() < 2 {
                return Ok(false);
            }
            if !incoming.starts_with(b"\r\n") {
                return Err(());
            }
            incoming.drain(..2);
            *at_chunk_crlf = false;
            continue;
        }
        match *data_remaining {
            None => {
                let Some(pos) = incoming.windows(2).position(|w| w == b"\r\n") else {
                    if incoming.len() > 64 {
                        return Err(());
                    }
                    return Ok(false);
                };
                let size_line = std::str::from_utf8(&incoming[..pos]).map_err(|_| ())?;
                let size = usize::from_str_radix(size_line.trim(), 16).map_err(|_| ())?;
                incoming.drain(..pos + 2);
                if size == 0 {
                    return Ok(true);
                }
                *data_remaining = Some(size);
            }
            Some(remaining) => {
                if remaining == 0 {
                    *data_remaining = None;
                    *at_chunk_crlf = true;
                    continue;
                }
                if incoming.is_empty() {
                    return Ok(false);
                }
                let take = remaining.min(incoming.len());
                decoded.extend_from_slice(&incoming[..take]);
                incoming.drain(..take);
                let next = remaining - take;
                if next == 0 {
                    *data_remaining = None;
                    *at_chunk_crlf = true;
                } else {
                    *data_remaining = Some(next);
                    return Ok(false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    const HEX64: &str = "c6eb81c2c3a4b5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e";
    const HEX64_UPPER: &str = "C6EB81C2C3A4B5D6E7F8091A2B3C4D5E6F708192A3B4C5D6E7F8091A2B3C4D5E";

    fn listed(name: &str, digest: Option<&str>) -> ListedModel {
        ListedModel {
            name: name.to_string(),
            model: Some(name.to_string()),
            digest: digest.map(str::to_string),
            size: None,
            remote_host: None,
            remote_model: None,
        }
    }

    fn sha(hex: &str) -> String {
        format!("sha256:{hex}")
    }

    #[test]
    fn canonicalize_plain_hex() {
        assert_eq!(
            canonicalize_digest(HEX64_UPPER).as_deref(),
            Some(sha(HEX64).as_str())
        );
    }

    #[test]
    fn canonicalize_sha256_colon() {
        assert_eq!(
            canonicalize_digest(&format!("sha256:{HEX64_UPPER}")).as_deref(),
            Some(sha(HEX64).as_str())
        );
    }

    #[test]
    fn canonicalize_sha256_dash() {
        assert_eq!(
            canonicalize_digest(&format!("sha256-{HEX64_UPPER}")).as_deref(),
            Some(sha(HEX64).as_str())
        );
    }

    #[test]
    fn canonicalize_mixed_case() {
        let mixed = "c6EB81c2C3a4b5D6e7F8091A2b3C4d5E6f708192A3b4C5d6E7f8091a2B3c4D5e";
        assert_eq!(mixed.len(), 64);
        assert_eq!(
            canonicalize_digest(mixed).as_deref(),
            Some(sha(HEX64).as_str())
        );
        assert!(digests_equal(&format!("sha256:{HEX64_UPPER}"), HEX64));
    }

    #[test]
    fn canonicalize_too_short_or_non_hex() {
        assert_eq!(canonicalize_digest("deadbeef"), None);
        assert_eq!(canonicalize_digest("sha256:deadbeef"), None);
        assert_eq!(canonicalize_digest(&format!("sha256:{HEX64}ff")), None);
        let mut bad = HEX64.to_string();
        bad.replace_range(0..1, "z");
        assert_eq!(canonicalize_digest(&bad), None);
    }

    #[test]
    fn canonicalize_rejects_secrets() {
        assert_eq!(canonicalize_digest("CANARY_SECRET_TOKEN"), None);
        assert_eq!(canonicalize_digest("sha256:token"), None);
        assert_eq!(canonicalize_digest("sha256-secret"), None);
        assert_eq!(canonicalize_digest("bearer-api_key"), None);
        assert_eq!(canonicalize_digest(&format!("sha256:{HEX64}canary")), None);
    }

    #[test]
    fn latest_tag_resolution_matches() {
        assert!(model_matches("qwen3", "qwen3:latest", Some("qwen3:latest")));
        assert!(model_matches("qwen3:latest", "qwen3:latest", None));
        assert!(model_matches("qwen3:latest", "qwen3", Some("qwen3")));
        assert!(model_matches("qwen3", "qwen3", None));
        assert!(!model_matches("qwen3:7b", "qwen3:latest", None));
        assert!(!model_matches("qwen3:7b", "qwen3", None));
        assert!(model_matches("qwen3:7b", "other", Some("qwen3:7b")));
    }

    #[test]
    fn remote_or_cloud_classification() {
        assert!(is_remote_or_cloud("gpt-oss:120b-cloud", None));
        assert!(is_remote_or_cloud("foo:cloud", None));
        assert!(!is_remote_or_cloud("qwen3:8b", None));
        let remote = ListedModel {
            name: "qwen3:8b".into(),
            model: None,
            digest: None,
            size: None,
            remote_host: Some("https://ollama.example".into()),
            remote_model: None,
        };
        assert!(is_remote_or_cloud("qwen3:8b", Some(&remote)));
        let remote_model = ListedModel {
            name: "qwen3:8b".into(),
            model: None,
            digest: None,
            size: None,
            remote_host: None,
            remote_model: Some("qwen3:8b".into()),
        };
        assert!(is_remote_or_cloud("qwen3:8b", Some(&remote_model)));
    }

    #[test]
    fn mock_untagged_list_matches_latest() {
        let mock = MockOllama::default().with_models(vec![listed("qwen3", Some("sha256:abc"))]);
        let models = mock.list_models().unwrap();
        assert!(model_matches(
            "qwen3:latest",
            &models[0].name,
            models[0].model.as_deref()
        ));
        assert!(model_matches(
            "qwen3",
            &models[0].name,
            models[0].model.as_deref()
        ));
    }

    #[test]
    fn mock_pull_404() {
        let mock = MockOllama::default().with_pull_status(404);
        let err = mock.pull_tag("missing", &mut |_| {}).unwrap_err();
        assert_eq!(err, RealizeError::NotFound);
        assert!(!err.to_string().contains("CANARY"));
    }

    #[test]
    fn mock_digest_after_pull_is_canonical() {
        let mock = MockOllama::default()
            .with_pull_events(vec![serde_json::json!({"status":"success"})])
            .with_models_after_pull(vec![listed(
                "qwen3:latest",
                Some(&format!("sha256-{HEX64_UPPER}")),
            )]);
        let outcome = mock.pull_tag("qwen3", &mut |_| {}).unwrap();
        assert_eq!(outcome.name, "qwen3:latest");
        assert_eq!(outcome.digest.as_deref(), Some(sha(HEX64).as_str()));
        assert_eq!(outcome.bytes_completed, None);
        let encoded = serde_json::to_value(&outcome).unwrap();
        assert!(encoded.get("bytes_completed").is_none());
    }

    #[test]
    fn mock_stall_is_idle_timeout() {
        let mock = MockOllama::default().with_stall();
        assert_eq!(
            mock.pull_tag("qwen3", &mut |_| {}).unwrap_err(),
            RealizeError::TimedOut
        );
    }

    #[test]
    fn mock_quote_in_name_json_body_escapes_and_omits_insecure() {
        let mock = MockOllama::default()
            .with_pull_events(vec![serde_json::json!({"status":"success"})])
            .with_models_after_pull(vec![listed("qwen\"3:latest", Some(&sha(HEX64)))]);
        mock.pull_tag("qwen\"3", &mut |_| {}).unwrap();
        let body = mock.captured_pull_body().unwrap();
        assert!(!body.contains("insecure"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["model"], "qwen\"3:latest");
        assert_eq!(parsed["stream"], true);
        assert!(parsed.get("insecure").is_none());
        assert!(body.contains("qwen\\\"3:latest") || body.contains("qwen\\u00223:latest"));
        assert!(!body.contains("format!"));
    }

    #[test]
    fn mock_bytes_completed_last_seen_per_digest() {
        let events = vec![
            serde_json::json!({"status":"downloading","digest":"sha256:aa","total":100,"completed":10}),
            serde_json::json!({"status":"downloading","digest":"sha256:aa","total":100,"completed":50}),
            serde_json::json!({"status":"downloading","digest":"sha256:bb","total":200,"completed":20}),
            serde_json::json!({"status":"downloading","digest":"sha256:aa","total":100,"completed":100}),
            serde_json::json!({"status":"success"}),
        ];
        let mock = MockOllama::default()
            .with_pull_events(events)
            .with_models_after_pull(vec![listed("qwen3:8b", Some(&sha(HEX64)))]);
        let outcome = mock.pull_tag("qwen3:8b", &mut |_| {}).unwrap();
        assert_eq!(outcome.bytes_completed, Some(120));
    }

    #[test]
    fn mock_cloud_name_is_unsupported_without_pull() {
        let mock = MockOllama::default();
        assert_eq!(
            mock.pull_tag("foo:cloud", &mut |_| {}).unwrap_err(),
            RealizeError::RemoteOrCloudUnsupported
        );
        assert!(mock.captured_pull_body().is_none());
    }

    #[test]
    fn inventory_probe_source_has_no_pull() {
        let src = include_str!("inventory.rs");
        assert!(
            !src.contains("fn pull"),
            "InventoryProbe must not grow a pull method"
        );
        assert!(
            !src.contains("pull_tag"),
            "InventoryProbe must not expose pull_tag"
        );
        assert!(
            !src.contains("/api/pull"),
            "InventoryProbe must not call /api/pull"
        );
    }

    struct PullFixture {
        version_body: String,
        tags_body: String,
        version_status: u16,
        tags_status: u16,
        pull_status: u16,
        pull_body: String,
        stall: bool,
        stall_delay: Duration,
    }

    impl PullFixture {
        fn success(tags: &str, pull_ndjson: &str) -> Self {
            Self {
                version_body: r#"{"version":"0.11.0"}"#.into(),
                tags_body: tags.into(),
                version_status: 200,
                tags_status: 200,
                pull_status: 200,
                pull_body: pull_ndjson.into(),
                stall: false,
                stall_delay: Duration::from_millis(400),
            }
        }
    }

    fn spawn_fixture(
        fixture: PullFixture,
    ) -> (u16, Arc<Mutex<Option<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(Mutex::new(None));
        let captured_thread = captured.clone();
        let handle = thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let request = read_request(&mut stream);
                if request.contains("POST /api/pull") {
                    if let Some((_, body)) = request.split_once("\r\n\r\n") {
                        *captured_thread.lock().unwrap() =
                            Some(body.trim_end_matches('\0').to_string());
                    }
                    if fixture.stall {
                        thread::sleep(fixture.stall_delay);
                        continue;
                    }
                    let body = &fixture.pull_body;
                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        fixture.pull_status,
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                } else if request.contains("/api/version") {
                    let body = &fixture.version_body;
                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        fixture.version_status,
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                } else {
                    let body = &fixture.tags_body;
                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        fixture.tags_status,
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        (port, captured, handle)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        for _ in 0..8 {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = find_double_crlf(&buf) {
                        let headers = String::from_utf8_lossy(&buf[..pos]);
                        let want = headers.lines().find_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            lower
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        });
                        let have = buf.len().saturating_sub(pos + 4);
                        if want.map(|n| have >= n).unwrap_or(true) {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn test_client(port: u16, idle: Duration) -> HttpOllama {
        HttpOllama::for_tests("127.0.0.1", port, Duration::from_secs(2), idle)
    }

    #[test]
    fn http_quote_in_name_on_wire_escapes_and_omits_insecure() {
        let tags =
            format!(r#"{{"models":[{{"name":"qwen\"3:latest","digest":"sha256:{HEX64}"}}]}}"#);
        let (port, captured, _handle) =
            spawn_fixture(PullFixture::success(&tags, "{\"status\":\"success\"}\n"));
        let client = test_client(port, Duration::from_secs(2));
        client.pull_tag("qwen\"3", &mut |_| {}).unwrap();
        let body = captured.lock().unwrap().clone().unwrap();
        assert!(!body.contains("insecure"));
        let parsed: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(parsed["model"].as_str(), Some("qwen\"3:latest"));
        assert_eq!(parsed["stream"], true);
        assert!(parsed.get("insecure").is_none());
        assert!(body.contains("qwen\\\"3:latest") || body.contains("qwen\\u00223:latest"));
    }

    #[test]
    fn http_insecure_absent_from_pull_body() {
        let tags = format!(r#"{{"models":[{{"name":"qwen3:latest","digest":"sha256:{HEX64}"}}]}}"#);
        let (port, captured, _handle) =
            spawn_fixture(PullFixture::success(&tags, "{\"status\":\"success\"}\n"));
        let client = test_client(port, Duration::from_secs(2));
        client.pull_tag("qwen3", &mut |_| {}).unwrap();
        let body = captured.lock().unwrap().clone().unwrap();
        assert!(!body.to_ascii_lowercase().contains("insecure"));
        let parsed: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(
            parsed.as_object().map(|o| o.len()),
            Some(2),
            "POST body must be only model+stream: {body}"
        );
    }

    #[test]
    fn http_idle_timeout_stall() {
        let mut fixture = PullFixture::success(r#"{"models":[]}"#, "");
        fixture.stall = true;
        fixture.stall_delay = Duration::from_millis(400);
        let (port, _, _handle) = spawn_fixture(fixture);
        let client = test_client(port, Duration::from_millis(150));
        let err = client.pull_tag("qwen3", &mut |_| {}).unwrap_err();
        assert_eq!(err, RealizeError::TimedOut);
    }

    #[test]
    fn http_404() {
        let mut fixture =
            PullFixture::success(r#"{"models":[]}"#, r#"{"error":"CANARY_SECRET_TOKEN"}"#);
        fixture.pull_status = 404;
        let (port, _, _handle) = spawn_fixture(fixture);
        let client = test_client(port, Duration::from_secs(2));
        let err = client.pull_tag("missing", &mut |_| {}).unwrap_err();
        assert_eq!(err, RealizeError::NotFound);
        assert!(!err.to_string().contains("CANARY_SECRET_TOKEN"));
        assert!(!format!("{err:?}").contains("CANARY_SECRET_TOKEN"));
    }

    #[test]
    fn http_bytes_completed_last_seen_per_digest() {
        let tags = format!(
            r#"{{"models":[{{"name":"qwen3:8b","model":"qwen3:8b","digest":"sha256:{HEX64}","size":123}}]}}"#
        );
        let pull = concat!(
            r#"{"status":"pulling manifest"}"#,
            "\n",
            r#"{"status":"downloading","digest":"sha256:aa","total":100,"completed":10}"#,
            "\n",
            r#"{"status":"downloading","digest":"sha256:aa","total":100,"completed":50}"#,
            "\n",
            r#"{"status":"downloading","digest":"sha256:bb","total":200,"completed":20}"#,
            "\n",
            r#"{"status":"downloading","digest":"sha256:aa","total":100,"completed":100}"#,
            "\n",
            r#"{"status":"success"}"#,
            "\n"
        );
        let (port, _, _handle) = spawn_fixture(PullFixture::success(&tags, pull));
        let client = test_client(port, Duration::from_secs(2));
        let mut seen = Vec::new();
        let outcome = client
            .pull_tag("qwen3:8b", &mut |p| seen.push(p.status.clone()))
            .unwrap();
        assert_eq!(outcome.bytes_completed, Some(120));
        assert_eq!(outcome.digest.as_deref(), Some(sha(HEX64).as_str()));
        assert!(seen.contains(&"success".to_string()));
    }

    #[test]
    fn http_non_loopback_does_not_connect() {
        let client = HttpOllama::for_tests(
            "8.8.8.8",
            11434,
            Duration::from_millis(200),
            Duration::from_millis(200),
        );
        let runtime = client.inspect_runtime();
        assert_eq!(runtime.protocol, RuntimeProtocol::Unreachable);
        assert_eq!(
            client.pull_tag("qwen3", &mut |_| {}).unwrap_err(),
            RealizeError::EndpointUnreachable
        );
    }

    #[test]
    fn http_inspect_lists_extra_fields_and_raw_digest() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for _ in 0..8 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let request = String::from_utf8_lossy(&buf);
                    let body = if request.contains("/api/version") {
                        r#"{"version":"0.11.0"}"#
                    } else {
                        r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","digest":"sha256:deadbeef","size":123,"modified_at":"2026-01-01T00:00:00Z","extra":"ignored"}]}"#
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        let client = test_client(port, Duration::from_secs(2));
        assert_eq!(client.inspect_runtime().protocol, RuntimeProtocol::Ready);
        let models = client.list_models().unwrap();
        assert_eq!(models[0].name, "qwen3:8b");
        assert_eq!(models[0].digest.as_deref(), Some("sha256:deadbeef"));
        assert_eq!(models[0].size, Some(123));
    }

    #[test]
    fn http_oversized_ndjson_line_is_malformed() {
        let huge = "x".repeat(HTTP_BODY_LIMIT + 8);
        let pull = format!("{huge}\n");
        let (port, _, _handle) = spawn_fixture(PullFixture::success(r#"{"models":[]}"#, &pull));
        let client = test_client(port, Duration::from_secs(2));
        assert_eq!(
            client.pull_tag("qwen3", &mut |_| {}).unwrap_err(),
            RealizeError::Malformed
        );
    }
}
