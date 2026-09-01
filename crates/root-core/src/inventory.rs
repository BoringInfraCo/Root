//! Read-only declared-environment inventory.
//!
//! Inspects Rootfile `[agents]` and `[models]` declarations. Probes never
//! install, pull, remove, resolve, or restore resources.

use root_lockfile::Rootfile;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 4096;
const HTTP_BODY_LIMIT: usize = 1_048_576;
const DEFAULT_OLLAMA_HOST: &str = "127.0.0.1";
const DEFAULT_OLLAMA_PORT: u16 = 11434;

pub const SUPPORTED_AGENTS: &[&str] = &["claude", "codex", "opencode", "pi"];
pub const SUPPORTED_MODEL_RUNTIME: &str = "ollama";

pub const REASON_NOT_SUPPORTED: &str = "not_supported_by_this_release";
pub const REASON_PROTOCOL_UNSUPPORTED: &str = "protocol_unsupported";
pub const REASON_NOT_FOUND: &str = "not_found";
pub const REASON_INVOCATION_FAILED: &str = "invocation_failed";
pub const REASON_TIMED_OUT: &str = "timed_out";
pub const REASON_PERMISSION_DENIED: &str = "permission_denied";
pub const REASON_MALFORMED_OUTPUT: &str = "malformed_output";
pub const REASON_ENDPOINT_UNREACHABLE: &str = "endpoint_unreachable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Agent,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Present,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationState {
    Satisfied,
    Missing,
    Drifted,
    Unknown,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    ExecutableLookup,
    VersionCommand,
    OllamaApiTags,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub presence: Presence,
    pub observed_version: Option<String>,
    pub observed_digest: Option<String>,
    pub evidence_source: EvidenceSource,
    pub reason: Option<String>,
}

impl ProbeResult {
    fn present_version(version: String, source: EvidenceSource) -> Self {
        Self {
            presence: Presence::Present,
            observed_version: Some(version),
            observed_digest: None,
            evidence_source: source,
            reason: None,
        }
    }

    fn present_model(version: Option<String>, digest: Option<String>) -> Self {
        Self {
            presence: Presence::Present,
            observed_version: version,
            observed_digest: digest,
            evidence_source: EvidenceSource::OllamaApiTags,
            reason: None,
        }
    }

    fn absent(source: EvidenceSource, reason: &str) -> Self {
        Self {
            presence: Presence::Absent,
            observed_version: None,
            observed_digest: None,
            evidence_source: source,
            reason: Some(reason.to_string()),
        }
    }

    fn unknown(source: EvidenceSource, reason: &str) -> Self {
        Self {
            presence: Presence::Unknown,
            observed_version: None,
            observed_digest: None,
            evidence_source: source,
            reason: Some(reason.to_string()),
        }
    }
}

/// Read-only inspection. Implementors must not mutate agents, models, or files.
pub trait InventoryProbe {
    fn inspect_agent(&self, name: &str) -> ProbeResult;
    fn inspect_model(&self, name: &str, runtime: &str) -> ProbeResult;
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InventoryItem {
    pub name: String,
    pub kind: ResourceKind,
    pub desired: String,
    pub observation: Presence,
    pub evaluation: EvaluationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_digest: Option<String>,
    pub evidence_source: EvidenceSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct InventoryReport {
    pub agents: Vec<InventoryItem>,
    pub models: Vec<InventoryItem>,
}

impl InventoryReport {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.models.is_empty()
    }

    pub fn evaluations(&self) -> impl Iterator<Item = EvaluationState> + '_ {
        self.agents
            .iter()
            .chain(self.models.iter())
            .map(|item| item.evaluation)
    }
}

#[derive(Default)]
pub struct MockInventoryProbe {
    pub agents: BTreeMap<String, ProbeResult>,
    pub models: BTreeMap<String, ProbeResult>,
    pub agent_calls: Mutex<Vec<String>>,
    pub model_calls: Mutex<Vec<(String, String)>>,
}

impl InventoryProbe for MockInventoryProbe {
    fn inspect_agent(&self, name: &str) -> ProbeResult {
        self.agent_calls.lock().unwrap().push(name.to_string());
        self.agents.get(name).cloned().unwrap_or_else(|| {
            ProbeResult::absent(EvidenceSource::ExecutableLookup, REASON_NOT_FOUND)
        })
    }

    fn inspect_model(&self, name: &str, runtime: &str) -> ProbeResult {
        self.model_calls
            .lock()
            .unwrap()
            .push((name.to_string(), runtime.to_string()));
        self.models
            .get(name)
            .cloned()
            .unwrap_or_else(|| ProbeResult::absent(EvidenceSource::OllamaApiTags, REASON_NOT_FOUND))
    }
}

pub struct SystemInventoryProbe {
    path_dirs: Option<Vec<PathBuf>>,
    ollama_host: String,
    ollama_port: u16,
    timeout: Duration,
    ollama_cache: Mutex<Option<OllamaSnapshot>>,
}

impl Default for SystemInventoryProbe {
    fn default() -> Self {
        Self {
            path_dirs: None,
            ollama_host: DEFAULT_OLLAMA_HOST.to_string(),
            ollama_port: DEFAULT_OLLAMA_PORT,
            timeout: PROBE_TIMEOUT,
            ollama_cache: Mutex::new(None),
        }
    }
}

impl SystemInventoryProbe {
    pub fn for_tests(path_dirs: Vec<PathBuf>, ollama_port: u16, timeout: Duration) -> Self {
        Self {
            path_dirs: Some(path_dirs),
            ollama_host: DEFAULT_OLLAMA_HOST.to_string(),
            ollama_port,
            timeout,
            ollama_cache: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone)]
enum OllamaSnapshot {
    Unreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
    Ready { models: Vec<OllamaListedModel> },
}

#[derive(Debug, Clone)]
struct OllamaListedModel {
    name: String,
    model: Option<String>,
    digest: Option<String>,
}

struct AgentDescriptor {
    name: &'static str,
    executable: &'static str,
    version_args: &'static [&'static [&'static str]],
    extra_env: &'static [(&'static str, &'static str)],
}

const AGENT_DESCRIPTORS: &[AgentDescriptor] = &[
    AgentDescriptor {
        name: "codex",
        executable: "codex",
        version_args: &[&["--version"]],
        extra_env: &[],
    },
    AgentDescriptor {
        name: "claude",
        executable: "claude",
        version_args: &[&["--version"], &["-v"]],
        extra_env: &[],
    },
    AgentDescriptor {
        name: "opencode",
        executable: "opencode",
        version_args: &[&["--version"], &["-v"]],
        extra_env: &[("OPENCODE_DISABLE_AUTOUPDATE", "1")],
    },
    AgentDescriptor {
        name: "pi",
        executable: "pi",
        version_args: &[&["--version"], &["-v"]],
        extra_env: &[("PI_OFFLINE", "1"), ("PI_SKIP_VERSION_CHECK", "1")],
    },
];

fn supported_agent(name: &str) -> Option<&'static AgentDescriptor> {
    let canonical = name.to_ascii_lowercase();
    AGENT_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.name == canonical)
}

pub fn is_supported_agent(name: &str) -> bool {
    let canonical = name.to_ascii_lowercase();
    SUPPORTED_AGENTS.contains(&canonical.as_str())
}

pub fn inspect_declarations(rootfile: &Rootfile, probe: &impl InventoryProbe) -> InventoryReport {
    let mut agents = Vec::new();
    for (name, desired) in &rootfile.agents {
        agents.push(evaluate_agent(name, desired, probe));
    }
    let mut models = Vec::new();
    for (name, declaration) in &rootfile.models {
        models.push(evaluate_model(name, &declaration.runtime, probe));
    }
    InventoryReport { agents, models }
}

fn evaluate_agent(name: &str, desired: &str, probe: &impl InventoryProbe) -> InventoryItem {
    if !is_supported_agent(name) {
        return InventoryItem {
            name: name.to_string(),
            kind: ResourceKind::Agent,
            desired: desired.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unsupported,
            observed_version: None,
            observed_digest: None,
            evidence_source: EvidenceSource::None,
            reason: Some(REASON_NOT_SUPPORTED.to_string()),
        };
    }

    let probed = probe.inspect_agent(name);
    item_from_probe(
        name,
        ResourceKind::Agent,
        desired,
        probed,
        EvaluationState::Satisfied,
    )
}

fn evaluate_model(name: &str, runtime: &str, probe: &impl InventoryProbe) -> InventoryItem {
    if runtime.to_ascii_lowercase() != SUPPORTED_MODEL_RUNTIME {
        return InventoryItem {
            name: name.to_string(),
            kind: ResourceKind::Model,
            desired: runtime.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unsupported,
            observed_version: None,
            observed_digest: None,
            evidence_source: EvidenceSource::None,
            reason: Some(REASON_NOT_SUPPORTED.to_string()),
        };
    }

    let probed = probe.inspect_model(name, runtime);
    item_from_probe(
        name,
        ResourceKind::Model,
        runtime,
        probed,
        EvaluationState::Satisfied,
    )
}

fn item_from_probe(
    name: &str,
    kind: ResourceKind,
    desired: &str,
    probed: ProbeResult,
    present_evaluation: EvaluationState,
) -> InventoryItem {
    let (observation, evaluation) = match probed.presence {
        Presence::Present => (Presence::Present, present_evaluation),
        Presence::Absent => (Presence::Absent, EvaluationState::Missing),
        Presence::Unknown => {
            if probed.reason.as_deref() == Some(REASON_PROTOCOL_UNSUPPORTED) {
                (Presence::Unknown, EvaluationState::Unsupported)
            } else {
                (Presence::Unknown, EvaluationState::Unknown)
            }
        }
    };
    InventoryItem {
        name: name.to_string(),
        kind,
        desired: desired.to_string(),
        observation,
        evaluation,
        observed_version: probed.observed_version,
        observed_digest: probed.observed_digest,
        evidence_source: probed.evidence_source,
        reason: probed.reason,
    }
}

/// Combine package status with inventory evaluations. NeedsAttention wins.
pub fn combine_environment_state(
    package_state: &str,
    evaluations: impl IntoIterator<Item = EvaluationState>,
) -> (bool, String) {
    let mut inventory_attention = false;
    let mut inventory_drifted = false;
    for state in evaluations {
        match state {
            EvaluationState::Unknown | EvaluationState::Unsupported => inventory_attention = true,
            EvaluationState::Missing | EvaluationState::Drifted => inventory_drifted = true,
            EvaluationState::Satisfied => {}
        }
    }

    let state = if package_state == "NeedsAttention" || inventory_attention {
        "NeedsAttention"
    } else if package_state == "Drifted" || inventory_drifted {
        "Drifted"
    } else {
        "Healthy"
    };
    (state == "Healthy", state.to_string())
}

pub fn drift_category_for(item: &InventoryItem) -> Option<&'static str> {
    match (item.kind, item.evaluation, item.reason.as_deref()) {
        (_, EvaluationState::Satisfied, _) => None,
        (ResourceKind::Agent, EvaluationState::Missing, _) => Some("agent-missing"),
        (ResourceKind::Agent, EvaluationState::Unknown, _) => Some("agent-observation-unknown"),
        (ResourceKind::Agent, EvaluationState::Unsupported, _) => {
            Some("agent-not-supported-by-this-release")
        }
        (ResourceKind::Agent, EvaluationState::Drifted, _) => None,
        (ResourceKind::Model, EvaluationState::Missing, _) => Some("model-missing"),
        (ResourceKind::Model, EvaluationState::Unknown, _) => Some("model-observation-unknown"),
        (ResourceKind::Model, EvaluationState::Unsupported, Some(REASON_PROTOCOL_UNSUPPORTED)) => {
            Some("model-runtime-protocol-unsupported")
        }
        (ResourceKind::Model, EvaluationState::Unsupported, _) => {
            Some("model-runtime-not-supported-by-this-release")
        }
        (ResourceKind::Model, EvaluationState::Drifted, _) => None,
    }
}

pub fn drift_suggestion_for(category: &str) -> &'static str {
    match category {
        "agent-missing" => {
            "Install the declared agent on this machine. Root inspects agents only and does not install them."
        }
        "agent-observation-unknown" => {
            "Re-run `root status` after the agent is invokable. A failed probe is not treated as missing."
        }
        "agent-not-supported-by-this-release" => {
            "This Root release cannot inspect that agent. Presence is not reported as missing."
        }
        "model-missing" => {
            "The declared model was not listed by Ollama. Root does not pull or restore models in v0.2.5."
        }
        "model-observation-unknown" => {
            "Ensure the Ollama daemon is reachable at 127.0.0.1:11434, then re-run `root status`."
        }
        "model-runtime-not-supported-by-this-release" => {
            "This Root release only inspects the Ollama runtime."
        }
        "model-runtime-protocol-unsupported" => {
            "The reachable endpoint did not implement the Ollama /api/version and /api/tags contract Root tests."
        }
        _ => "Re-run `root status` after addressing the declared environment issue.",
    }
}

pub fn reason_phrase(reason: &str) -> &str {
    match reason {
        REASON_NOT_SUPPORTED => "not supported by this release",
        REASON_PROTOCOL_UNSUPPORTED => "runtime protocol unsupported",
        REASON_NOT_FOUND => "not found",
        REASON_INVOCATION_FAILED => "invocation failed",
        REASON_TIMED_OUT => "timed out",
        REASON_PERMISSION_DENIED => "permission denied",
        REASON_MALFORMED_OUTPUT => "malformed output",
        REASON_ENDPOINT_UNREACHABLE => "endpoint unreachable",
        other => other,
    }
}

impl InventoryProbe for SystemInventoryProbe {
    fn inspect_agent(&self, name: &str) -> ProbeResult {
        let Some(descriptor) = supported_agent(name) else {
            return ProbeResult::unknown(EvidenceSource::None, REASON_NOT_SUPPORTED);
        };
        inspect_agent_descriptor(descriptor, self.path_dirs.as_deref(), self.timeout)
    }

    fn inspect_model(&self, name: &str, runtime: &str) -> ProbeResult {
        if runtime.to_ascii_lowercase() != SUPPORTED_MODEL_RUNTIME {
            return ProbeResult::unknown(EvidenceSource::None, REASON_NOT_SUPPORTED);
        }
        let snapshot = self.ollama_snapshot();
        match snapshot {
            OllamaSnapshot::Unreachable => {
                ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_ENDPOINT_UNREACHABLE)
            }
            OllamaSnapshot::TimedOut => {
                ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_TIMED_OUT)
            }
            OllamaSnapshot::Malformed => {
                ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_MALFORMED_OUTPUT)
            }
            OllamaSnapshot::ProtocolUnsupported => {
                ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_PROTOCOL_UNSUPPORTED)
            }
            OllamaSnapshot::Ready { models } => {
                if let Some(found) = models.iter().find(|model| model_matches(name, model)) {
                    ProbeResult::present_model(None, found.digest.clone())
                } else {
                    ProbeResult::absent(EvidenceSource::OllamaApiTags, REASON_NOT_FOUND)
                }
            }
        }
    }
}

impl SystemInventoryProbe {
    fn ollama_snapshot(&self) -> OllamaSnapshot {
        let mut cache = self
            .ollama_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = cache.clone() {
            return existing;
        }
        let snapshot = probe_ollama(&self.ollama_host, self.ollama_port, self.timeout);
        *cache = Some(snapshot.clone());
        snapshot
    }
}

fn model_matches(declared: &str, listed: &OllamaListedModel) -> bool {
    listed.name == declared || listed.model.as_deref() == Some(declared)
}

fn inspect_agent_descriptor(
    descriptor: &AgentDescriptor,
    path_dirs: Option<&[PathBuf]>,
    timeout: Duration,
) -> ProbeResult {
    let Some(executable) = find_executable(descriptor.executable, path_dirs) else {
        return ProbeResult::absent(EvidenceSource::ExecutableLookup, REASON_NOT_FOUND);
    };

    let mut last_reason = REASON_INVOCATION_FAILED;
    for args in descriptor.version_args {
        match run_version_command(&executable, args, descriptor.extra_env, timeout) {
            VersionOutcome::TimedOut => {
                return ProbeResult::unknown(EvidenceSource::VersionCommand, REASON_TIMED_OUT);
            }
            VersionOutcome::PermissionDenied => {
                return ProbeResult::unknown(
                    EvidenceSource::VersionCommand,
                    REASON_PERMISSION_DENIED,
                );
            }
            VersionOutcome::Failed => last_reason = REASON_INVOCATION_FAILED,
            VersionOutcome::Malformed => {
                return ProbeResult::unknown(
                    EvidenceSource::VersionCommand,
                    REASON_MALFORMED_OUTPUT,
                );
            }
            VersionOutcome::Parsed(version) => {
                return ProbeResult::present_version(version, EvidenceSource::VersionCommand);
            }
        }
    }
    ProbeResult::unknown(EvidenceSource::VersionCommand, last_reason)
}

fn find_executable(name: &str, path_dirs: Option<&[PathBuf]>) -> Option<PathBuf> {
    let owned_dirs: Vec<PathBuf>;
    let dirs: &[PathBuf] = if let Some(dirs) = path_dirs {
        dirs
    } else {
        let path_var = std::env::var_os("PATH")?;
        owned_dirs = std::env::split_paths(&path_var).collect();
        &owned_dirs
    };
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

enum VersionOutcome {
    Parsed(String),
    Malformed,
    Failed,
    TimedOut,
    PermissionDenied,
}

fn run_version_command(
    executable: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> VersionOutcome {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return VersionOutcome::PermissionDenied;
        }
        Err(_) => return VersionOutcome::Failed,
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => match child.wait_with_output() {
                Ok(output) => {
                    if !output.status.success() {
                        return VersionOutcome::Failed;
                    }
                    let stdout = truncate_bytes(&output.stdout);
                    return match normalize_version(&stdout) {
                        Some(version) => VersionOutcome::Parsed(version),
                        None => VersionOutcome::Malformed,
                    };
                }
                Err(_) => return VersionOutcome::Failed,
            },
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return VersionOutcome::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return VersionOutcome::Failed;
            }
        }
    }
}

fn truncate_bytes(bytes: &[u8]) -> String {
    let slice = if bytes.len() > OUTPUT_LIMIT {
        &bytes[..OUTPUT_LIMIT]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).trim().to_string()
}

fn normalize_version(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect();
    let re_match = cleaned.split_whitespace().find_map(|token| {
        let trimmed = token.trim_matches(|ch: char| {
            !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-' && ch != '+'
        });
        is_version_token(trimmed).then(|| trimmed.to_string())
    });
    re_match.filter(|value| !looks_secret(value))
}

fn is_version_token(token: &str) -> bool {
    let mut parts = token.split('.');
    let first = parts.next().unwrap_or("");
    let second = parts.next();
    first.chars().any(|ch| ch.is_ascii_digit())
        && second
            .map(|part| part.chars().any(|ch| ch.is_ascii_digit()))
            .unwrap_or(false)
}

fn looks_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("canary")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("bearer")
        || lower.contains("api_key")
}

fn probe_ollama(host: &str, port: u16, timeout: Duration) -> OllamaSnapshot {
    match http_get_json(host, port, "/api/version", timeout) {
        HttpOutcome::Unreachable => return OllamaSnapshot::Unreachable,
        HttpOutcome::TimedOut => return OllamaSnapshot::TimedOut,
        HttpOutcome::Malformed => return OllamaSnapshot::Malformed,
        HttpOutcome::ProtocolUnsupported => return OllamaSnapshot::ProtocolUnsupported,
        HttpOutcome::Json(value) => {
            if value.get("version").and_then(|v| v.as_str()).is_none() {
                return OllamaSnapshot::ProtocolUnsupported;
            }
        }
    }
    match http_get_json(host, port, "/api/tags", timeout) {
        HttpOutcome::Unreachable => OllamaSnapshot::Unreachable,
        HttpOutcome::TimedOut => OllamaSnapshot::TimedOut,
        HttpOutcome::Malformed => OllamaSnapshot::Malformed,
        HttpOutcome::ProtocolUnsupported => OllamaSnapshot::ProtocolUnsupported,
        HttpOutcome::Json(value) => match value.get("models").and_then(|v| v.as_array()) {
            None => OllamaSnapshot::ProtocolUnsupported,
            Some(models) => {
                let mut listed = Vec::new();
                for model in models {
                    let Some(name) = model.get("name").and_then(|v| v.as_str()) else {
                        return OllamaSnapshot::ProtocolUnsupported;
                    };
                    listed.push(OllamaListedModel {
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
                    });
                }
                OllamaSnapshot::Ready { models: listed }
            }
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use root_lockfile::ModelDeclaration;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn agent_rootfile(entries: &[(&str, &str)]) -> Rootfile {
        let mut rootfile = Rootfile::default();
        for (name, constraint) in entries {
            rootfile
                .agents
                .insert((*name).to_string(), (*constraint).to_string());
        }
        rootfile
    }

    fn model_rootfile(entries: &[(&str, &str)]) -> Rootfile {
        let mut rootfile = Rootfile::default();
        for (name, runtime) in entries {
            rootfile.models.insert(
                (*name).to_string(),
                ModelDeclaration {
                    runtime: (*runtime).to_string(),
                },
            );
        }
        rootfile
    }

    fn present_agent(version: &str) -> ProbeResult {
        ProbeResult::present_version(version.to_string(), EvidenceSource::VersionCommand)
    }

    #[test]
    fn supported_agent_present_is_satisfied() {
        let mut probe = MockInventoryProbe::default();
        probe.agents.insert("codex".into(), present_agent("0.42.0"));
        let report = inspect_declarations(&agent_rootfile(&[("codex", "*")]), &probe);
        assert_eq!(report.agents.len(), 1);
        assert_eq!(report.agents[0].observation, Presence::Present);
        assert_eq!(report.agents[0].evaluation, EvaluationState::Satisfied);
        assert_eq!(report.agents[0].observed_version.as_deref(), Some("0.42.0"));
        assert_eq!(
            report.agents[0].evidence_source,
            EvidenceSource::VersionCommand
        );
        assert_eq!(probe.agent_calls.lock().unwrap().as_slice(), ["codex"]);
    }

    #[test]
    fn supported_agent_absent_is_missing() {
        let probe = MockInventoryProbe::default();
        let report = inspect_declarations(&agent_rootfile(&[("claude", "*")]), &probe);
        assert_eq!(report.agents[0].observation, Presence::Absent);
        assert_eq!(report.agents[0].evaluation, EvaluationState::Missing);
        assert_eq!(report.agents[0].reason.as_deref(), Some(REASON_NOT_FOUND));
    }

    #[test]
    fn version_failure_is_unknown_never_missing() {
        let mut probe = MockInventoryProbe::default();
        probe.agents.insert(
            "opencode".into(),
            ProbeResult::unknown(EvidenceSource::VersionCommand, REASON_TIMED_OUT),
        );
        let report = inspect_declarations(&agent_rootfile(&[("opencode", "*")]), &probe);
        assert_eq!(report.agents[0].observation, Presence::Unknown);
        assert_eq!(report.agents[0].evaluation, EvaluationState::Unknown);
        assert_ne!(report.agents[0].evaluation, EvaluationState::Missing);
    }

    #[test]
    fn unshipped_agent_is_unsupported_without_probe() {
        let probe = MockInventoryProbe::default();
        let report = inspect_declarations(&agent_rootfile(&[("gemini", "*")]), &probe);
        assert_eq!(report.agents[0].evaluation, EvaluationState::Unsupported);
        assert_eq!(
            report.agents[0].reason.as_deref(),
            Some(REASON_NOT_SUPPORTED)
        );
        assert!(probe.agent_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn ollama_present_model_is_satisfied() {
        let mut probe = MockInventoryProbe::default();
        probe.models.insert(
            "qwen3:8b".into(),
            ProbeResult::present_model(Some("0.11.0".into()), Some("sha256:abc".into())),
        );
        let report = inspect_declarations(&model_rootfile(&[("qwen3:8b", "ollama")]), &probe);
        assert_eq!(report.models[0].observation, Presence::Present);
        assert_eq!(report.models[0].evaluation, EvaluationState::Satisfied);
        assert_eq!(
            report.models[0].observed_digest.as_deref(),
            Some("sha256:abc")
        );
    }

    #[test]
    fn ollama_missing_model_is_missing() {
        let mut probe = MockInventoryProbe::default();
        probe.models.insert(
            "qwen3:8b".into(),
            ProbeResult::absent(EvidenceSource::OllamaApiTags, REASON_NOT_FOUND),
        );
        let report = inspect_declarations(&model_rootfile(&[("qwen3:8b", "ollama")]), &probe);
        assert_eq!(report.models[0].observation, Presence::Absent);
        assert_eq!(report.models[0].evaluation, EvaluationState::Missing);
    }

    #[test]
    fn ollama_unreachable_is_unknown() {
        let mut probe = MockInventoryProbe::default();
        probe.models.insert(
            "qwen3:8b".into(),
            ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_ENDPOINT_UNREACHABLE),
        );
        let report = inspect_declarations(&model_rootfile(&[("qwen3:8b", "ollama")]), &probe);
        assert_eq!(report.models[0].observation, Presence::Unknown);
        assert_eq!(report.models[0].evaluation, EvaluationState::Unknown);
    }

    #[test]
    fn ollama_protocol_mismatch_is_unsupported() {
        let mut probe = MockInventoryProbe::default();
        probe.models.insert(
            "qwen3:8b".into(),
            ProbeResult::unknown(EvidenceSource::OllamaApiTags, REASON_PROTOCOL_UNSUPPORTED),
        );
        let report = inspect_declarations(&model_rootfile(&[("qwen3:8b", "ollama")]), &probe);
        assert_eq!(report.models[0].evaluation, EvaluationState::Unsupported);
        assert_eq!(
            report.models[0].reason.as_deref(),
            Some(REASON_PROTOCOL_UNSUPPORTED)
        );
    }

    #[test]
    fn unshipped_runtime_is_unsupported_without_probe() {
        let probe = MockInventoryProbe::default();
        let report = inspect_declarations(&model_rootfile(&[("qwen3:8b", "lmstudio")]), &probe);
        assert_eq!(report.models[0].evaluation, EvaluationState::Unsupported);
        assert_eq!(
            report.models[0].reason.as_deref(),
            Some(REASON_NOT_SUPPORTED)
        );
        assert!(probe.model_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn combine_state_maps_inventory_outcomes() {
        assert_eq!(
            combine_environment_state("Healthy", [EvaluationState::Satisfied]),
            (true, "Healthy".into())
        );
        assert_eq!(
            combine_environment_state("Healthy", [EvaluationState::Missing]),
            (false, "Drifted".into())
        );
        assert_eq!(
            combine_environment_state("Healthy", [EvaluationState::Unknown]),
            (false, "NeedsAttention".into())
        );
        assert_eq!(
            combine_environment_state("Healthy", [EvaluationState::Unsupported]),
            (false, "NeedsAttention".into())
        );
        assert_eq!(
            combine_environment_state(
                "Drifted",
                [EvaluationState::Unknown, EvaluationState::Missing]
            ),
            (false, "NeedsAttention".into())
        );
        assert_eq!(
            combine_environment_state("NeedsAttention", [EvaluationState::Missing]),
            (false, "NeedsAttention".into())
        );
    }

    #[test]
    fn inventory_ordering_follows_btreemap() {
        let mut probe = MockInventoryProbe::default();
        probe.agents.insert("pi".into(), present_agent("1.0.0"));
        probe.agents.insert("codex".into(), present_agent("0.1.0"));
        let report = inspect_declarations(&agent_rootfile(&[("pi", "*"), ("codex", "*")]), &probe);
        let names: Vec<_> = report
            .agents
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, ["codex", "pi"]);
    }

    #[test]
    fn mock_probe_has_no_mutation_surface() {
        fn assert_read_only<T: InventoryProbe>(_: &T) {}
        let probe = MockInventoryProbe::default();
        assert_read_only(&probe);
        assert!(probe.agent_calls.lock().unwrap().is_empty());
        assert!(probe.model_calls.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_codex_present_parses_prefixed_version() {
        let dir = std::env::temp_dir().join(format!("root_inv_codex_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(
            &dir,
            "codex",
            "#!/bin/sh\necho 'codex-cli 0.42.0'\nexit 0\n",
        );
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("codex");
        assert_eq!(result.presence, Presence::Present);
        assert_eq!(result.observed_version.as_deref(), Some("0.42.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_claude_falls_back_to_short_flag() {
        let dir = std::env::temp_dir().join(format!("root_inv_claude_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(
            &dir,
            "claude",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 1; fi\nif [ \"$1\" = \"-v\" ]; then echo '1.2.3'; exit 0; fi\nexit 1\n",
        );
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("claude");
        assert_eq!(result.presence, Presence::Present);
        assert_eq!(result.observed_version.as_deref(), Some("1.2.3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_opencode_and_pi_version_flags() {
        let dir = std::env::temp_dir().join(format!("root_inv_agents_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(
            &dir,
            "opencode",
            "#!/bin/sh\necho 'opencode 2.0.1'\nexit 0\n",
        );
        write_script(&dir, "pi", "#!/bin/sh\necho 'pi 0.5.0'\nexit 0\n");
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let opencode = probe.inspect_agent("opencode");
        let pi = probe.inspect_agent("pi");
        assert_eq!(opencode.observed_version.as_deref(), Some("2.0.1"));
        assert_eq!(pi.observed_version.as_deref(), Some("0.5.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_absent_executable_is_absent() {
        let dir = std::env::temp_dir().join(format!("root_inv_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("codex");
        assert_eq!(result.presence, Presence::Absent);
        assert_eq!(result.reason.as_deref(), Some(REASON_NOT_FOUND));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_timeout_is_unknown() {
        let dir = std::env::temp_dir().join(format!("root_inv_timeout_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "codex", "#!/bin/sh\nsleep 30\n");
        let probe =
            SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_millis(150));
        let result = probe.inspect_agent("codex");
        assert_eq!(result.presence, Presence::Unknown);
        assert_eq!(result.reason.as_deref(), Some(REASON_TIMED_OUT));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_nonzero_version_flags_are_unknown_not_absent() {
        let dir = std::env::temp_dir().join(format!("root_inv_failver_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "claude", "#!/bin/sh\nexit 1\n");
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("claude");
        assert_eq!(result.presence, Presence::Unknown);
        assert_ne!(result.presence, Presence::Absent);
        assert_eq!(result.reason.as_deref(), Some(REASON_INVOCATION_FAILED));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_malformed_version_is_unknown() {
        let dir = std::env::temp_dir().join(format!("root_inv_badver_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "codex", "#!/bin/sh\necho 'not-a-version'\nexit 0\n");
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("codex");
        assert_eq!(result.presence, Presence::Unknown);
        assert_eq!(result.reason.as_deref(), Some(REASON_MALFORMED_OUTPUT));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn system_probe_does_not_leak_canary_from_version_output() {
        let dir = std::env::temp_dir().join(format!("root_inv_canary_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_script(
            &dir,
            "codex",
            "#!/bin/sh\necho 'codex-cli CANARY_SECRET_TOKEN 0.9.0'\nexit 0\n",
        );
        let probe = SystemInventoryProbe::for_tests(vec![dir.clone()], 1, Duration::from_secs(2));
        let result = probe.inspect_agent("codex");
        let encoded = serde_json::to_string(&InventoryItem {
            name: "codex".into(),
            kind: ResourceKind::Agent,
            desired: "*".into(),
            observation: result.presence,
            evaluation: EvaluationState::Satisfied,
            observed_version: result.observed_version.clone(),
            observed_digest: None,
            evidence_source: result.evidence_source,
            reason: result.reason.clone(),
        })
        .unwrap();
        assert!(
            !encoded.contains("CANARY_SECRET_TOKEN"),
            "canary leaked: {encoded}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn spawn_json_server(
        version_body: &'static str,
        tags_body: &'static str,
        version_status: u16,
        tags_status: u16,
    ) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            for _ in 0..8 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let request = String::from_utf8_lossy(&buf);
                    let (status, body) = if request.contains("/api/version") {
                        (version_status, version_body)
                    } else {
                        (tags_status, tags_body)
                    };
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        (port, handle)
    }

    #[test]
    fn ollama_http_present_and_extra_fields_ignored() {
        let tags = r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","digest":"sha256:deadbeef","size":123,"modified_at":"2026-01-01T00:00:00Z","extra":"ignored"}]}"#;
        let (port, _handle) = spawn_json_server(r#"{"version":"0.11.0"}"#, tags, 200, 200);
        let probe = SystemInventoryProbe::for_tests(Vec::new(), port, Duration::from_secs(2));
        let present = probe.inspect_model("qwen3:8b", "ollama");
        assert_eq!(present.presence, Presence::Present);
        assert_eq!(present.observed_digest.as_deref(), Some("sha256:deadbeef"));
        let missing = probe.inspect_model("other:7b", "ollama");
        assert_eq!(missing.presence, Presence::Absent);
    }

    #[test]
    fn ollama_http_malformed_and_protocol() {
        let (bad_port, _) = spawn_json_server("not-json", "not-json", 200, 200);
        let bad = SystemInventoryProbe::for_tests(Vec::new(), bad_port, Duration::from_secs(2));
        let malformed = bad.inspect_model("qwen3:8b", "ollama");
        assert_eq!(malformed.presence, Presence::Unknown);
        assert_eq!(malformed.reason.as_deref(), Some(REASON_MALFORMED_OUTPUT));

        let (proto_port, _) = spawn_json_server("{}", r#"{"models":[]}"#, 200, 200);
        let proto = SystemInventoryProbe::for_tests(Vec::new(), proto_port, Duration::from_secs(2));
        let unsupported = proto.inspect_model("qwen3:8b", "ollama");
        assert_eq!(
            unsupported.reason.as_deref(),
            Some(REASON_PROTOCOL_UNSUPPORTED)
        );

        let unreachable =
            SystemInventoryProbe::for_tests(Vec::new(), 1, Duration::from_millis(200));
        let down = unreachable.inspect_model("qwen3:8b", "ollama");
        assert_eq!(down.presence, Presence::Unknown);
        assert!(
            down.reason.as_deref() == Some(REASON_ENDPOINT_UNREACHABLE)
                || down.reason.as_deref() == Some(REASON_TIMED_OUT)
        );
    }

    #[test]
    fn ollama_http_does_not_leak_canary_header_or_body() {
        let tags = r#"{"models":[{"name":"qwen3:8b","digest":"sha256:abc","token":"CANARY_SECRET_TOKEN"}]}"#;
        let (port, _) = spawn_json_server(r#"{"version":"0.1.0"}"#, tags, 200, 200);
        let probe = SystemInventoryProbe::for_tests(Vec::new(), port, Duration::from_secs(2));
        let result = probe.inspect_model("qwen3:8b", "ollama");
        let encoded = serde_json::to_string(&result.observed_digest).unwrap();
        assert!(!encoded.contains("CANARY_SECRET_TOKEN"));
        assert_eq!(result.observed_digest.as_deref(), Some("sha256:abc"));
    }
}
