//! Local connector packages.
//!
//! A package is a content-addressed manifest plus an executable. Root stores
//! credential names, not values. Write and destructive tools wait on a separate
//! approval. The child process gets a scrubbed environment, an empty working
//! directory, a timeout, and an output cap. This is not a kernel sandbox:
//! network and filesystem grants other than `none` are rejected until they can
//! be enforced.

use crate::auth;
use crate::policy;
use crate::registry::RegisteredCapability;
use crate::tools::ToolError;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEMA: u32 = 1;
const RESERVED: &[&str] = &["workspace", "work", "continuity", "environment", "mcp"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    id: String,
    version: String,
    publisher: String,
    executable: String,
    executable_sha256: String,
    tools: Vec<ToolSpec>,
    events: Vec<String>,
    credentials: Vec<CredentialSpec>,
    network: Vec<String>,
    filesystem: Vec<String>,
    limits: Limits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSpec {
    name: String,
    description: String,
    risk: Risk,
    input_schema: Value,
    idempotent: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Risk {
    Read,
    Write,
    Destructive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialSpec {
    name: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    timeout_ms: u64,
    max_output_bytes: u64,
    max_calls: u64,
    window_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Index {
    connectors: Vec<Installed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Installed {
    id: String,
    enabled: bool,
    manifest_sha256: String,
    executable_sha256: String,
    credentials: Vec<StoredCredential>,
    recent_calls: Vec<u64>,
    results: Vec<StoredResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredential {
    name: String,
    scopes: Vec<String>,
    bound: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredResult {
    key: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ApprovalFile {
    approvals: Vec<Approval>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Approval {
    id: String,
    connector_id: String,
    tool: String,
    risk: String,
    args_sha256: String,
    status: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub struct InstallReport {
    pub id: String,
    pub enabled: bool,
    pub executable_sha256: String,
    pub manifest_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectorSummary {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub enabled: bool,
    pub tools: usize,
    pub credentials_bound: usize,
    pub credentials_required: usize,
}

#[derive(Debug, Serialize)]
pub struct ConnectorDetail {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub enabled: bool,
    pub executable_sha256: String,
    pub manifest_sha256: String,
    pub intact: bool,
    pub tools: Vec<ToolView>,
    pub events: Vec<String>,
    pub credentials: Vec<CredentialView>,
    pub network: Vec<String>,
    pub filesystem: Vec<String>,
    pub isolation: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ToolView {
    pub name: String,
    pub risk: String,
    pub idempotent: bool,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct CredentialView {
    pub name: String,
    pub scopes: Vec<String>,
    pub bound: bool,
}

#[derive(Debug, Serialize)]
pub struct AuthPlan {
    pub id: String,
    pub credentials: Vec<CredentialView>,
}

#[derive(Debug, Serialize)]
pub struct ApprovalView {
    pub id: String,
    pub connector_id: String,
    pub tool: String,
    pub risk: String,
    pub status: String,
    pub args_sha256: String,
    pub created_at: String,
}

pub fn install(manifest_path: &Path, package_dir: &Path) -> Result<InstallReport> {
    let _lock = lock_host()?;
    let manifest = load_manifest(manifest_path)?;
    let source = package_dir.join(&manifest.executable);
    let digest =
        sha256_file(&source).with_context(|| format!("could not hash {}", source.display()))?;
    if digest != manifest.executable_sha256 {
        anyhow::bail!(
            "executable digest mismatch for {}: manifest {} actual {digest}",
            manifest.id,
            manifest.executable_sha256
        );
    }
    let mut index = read_index()?;
    if index.connectors.iter().any(|item| item.id == manifest.id) {
        anyhow::bail!("connector {} is already installed", manifest.id);
    }
    let dir = package_dir_for(&manifest.id)?;
    fs::create_dir_all(dir.join("bin")).and_then(|_| fs::create_dir_all(dir.join("run")))?;
    let stored_manifest = dir.join("manifest.json");
    fs::copy(manifest_path, &stored_manifest)?;
    let stored_bin = dir.join("bin").join(&manifest.executable);
    fs::copy(&source, &stored_bin)?;
    let mut permissions = fs::metadata(&stored_bin)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&stored_bin, permissions)?;
    let manifest_sha = sha256_file(&stored_manifest)?;
    index.connectors.push(Installed {
        id: manifest.id.clone(),
        enabled: false,
        manifest_sha256: manifest_sha.clone(),
        executable_sha256: digest.clone(),
        credentials: manifest
            .credentials
            .iter()
            .map(|item| StoredCredential {
                name: item.name.clone(),
                scopes: item.scopes.clone(),
                bound: false,
            })
            .collect(),
        recent_calls: Vec::new(),
        results: Vec::new(),
    });
    write_index(&index)?;
    audit(&manifest.id, "", "install", &digest, "installed")?;
    Ok(InstallReport {
        id: manifest.id,
        enabled: false,
        executable_sha256: digest,
        manifest_sha256: manifest_sha,
    })
}

pub fn list() -> Result<Vec<ConnectorSummary>> {
    let _lock = lock_host()?;
    let index = read_index()?;
    index
        .connectors
        .iter()
        .map(|item| {
            let manifest = load_installed(&item.id)?;
            let bound = item.credentials.iter().filter(|cred| cred.bound).count();
            Ok(ConnectorSummary {
                id: item.id.clone(),
                version: manifest.version,
                publisher: manifest.publisher,
                enabled: item.enabled,
                tools: manifest.tools.len(),
                credentials_bound: bound,
                credentials_required: item.credentials.len(),
            })
        })
        .collect()
}

pub fn inspect(id: &str) -> Result<ConnectorDetail> {
    let _lock = lock_host()?;
    let index = read_index()?;
    let item = installed(&index, id)?;
    detail(item)
}

pub fn enable(id: &str) -> Result<ConnectorDetail> {
    let _lock = lock_host()?;
    let mut index = read_index()?;
    let position = position(&index, id)?;
    {
        let manifest = load_installed(id)?;
        ensure_intact(&index.connectors[position], &manifest)?;
        health_check(&index.connectors[position], &manifest)?;
    }
    index.connectors[position].enabled = true;
    write_index(&index)?;
    audit(id, "", "enable", "", "enabled")?;
    detail(&index.connectors[position])
}

pub fn disable(id: &str) -> Result<ConnectorDetail> {
    let _lock = lock_host()?;
    let mut index = read_index()?;
    let position = position(&index, id)?;
    index.connectors[position].enabled = false;
    write_index(&index)?;
    audit(id, "", "disable", "", "disabled")?;
    detail(&index.connectors[position])
}

pub fn remove(id: &str) -> Result<InstallReport> {
    let _lock = lock_host()?;
    let mut index = read_index()?;
    let position = position(&index, id)?;
    let removed = index.connectors.remove(position);
    write_index(&index)?;
    let _ = fs::remove_dir_all(package_dir_for(id)?);
    let mut approvals = read_approvals()?;
    approvals.approvals.retain(|item| item.connector_id != id);
    write_approvals(&approvals)?;
    audit(id, "", "remove", &removed.executable_sha256, "removed")?;
    Ok(InstallReport {
        id: id.to_string(),
        enabled: false,
        executable_sha256: removed.executable_sha256,
        manifest_sha256: removed.manifest_sha256,
    })
}

pub fn auth_plan(id: &str) -> Result<AuthPlan> {
    let _lock = lock_host()?;
    let index = read_index()?;
    let item = installed(&index, id)?;
    Ok(AuthPlan {
        id: id.to_string(),
        credentials: credential_views(item),
    })
}

pub fn auth_bind(id: &str, name: &str) -> Result<AuthPlan> {
    let _lock = lock_host()?;
    if std::env::var_os(name).is_none() {
        anyhow::bail!("credential {name} is not present in the environment");
    }
    let mut index = read_index()?;
    let position = position(&index, id)?;
    let Some(credential) = index.connectors[position]
        .credentials
        .iter_mut()
        .find(|item| item.name == name)
    else {
        anyhow::bail!("connector {id} does not require credential {name}");
    };
    credential.bound = true;
    write_index(&index)?;
    audit(id, "", "auth.bind", name, "bound")?;
    Ok(AuthPlan {
        id: id.to_string(),
        credentials: credential_views(&index.connectors[position]),
    })
}

pub fn auth_revoke(id: &str, name: &str) -> Result<AuthPlan> {
    let _lock = lock_host()?;
    let mut index = read_index()?;
    let position = position(&index, id)?;
    let Some(credential) = index.connectors[position]
        .credentials
        .iter_mut()
        .find(|item| item.name == name)
    else {
        anyhow::bail!("connector {id} does not require credential {name}");
    };
    credential.bound = false;
    write_index(&index)?;
    audit(id, "", "auth.revoke", name, "revoked")?;
    Ok(AuthPlan {
        id: id.to_string(),
        credentials: credential_views(&index.connectors[position]),
    })
}

pub fn approval_list() -> Result<Vec<ApprovalView>> {
    let _lock = lock_host()?;
    Ok(read_approvals()?
        .approvals
        .iter()
        .map(approval_view)
        .collect())
}

pub fn approval_decide(id: &str, approve: bool) -> Result<ApprovalView> {
    let _lock = lock_host()?;
    let mut file = read_approvals()?;
    let Some(item) = file.approvals.iter_mut().find(|item| item.id == id) else {
        anyhow::bail!("unknown approval {id}");
    };
    if item.status != "pending" {
        anyhow::bail!("approval {id} is already {}", item.status);
    }
    item.status = if approve { "approved" } else { "denied" }.to_string();
    let view = approval_view(item);
    write_approvals(&file)?;
    audit(
        &view.connector_id,
        &view.tool,
        "approval",
        &view.args_sha256,
        &view.status,
    )?;
    Ok(view)
}

pub fn enabled_capabilities() -> Result<Vec<RegisteredCapability>> {
    if host_dir_existing().is_none() {
        return Ok(Vec::new());
    }
    let Ok(_lock) = lock_host() else {
        return Ok(Vec::new());
    };
    let Ok(index) = read_index() else {
        return Ok(Vec::new());
    };
    let mut tools = Vec::new();
    for item in index.connectors.iter().filter(|item| item.enabled) {
        let Ok(manifest) = load_installed(&item.id) else {
            continue;
        };
        for tool in &manifest.tools {
            tools.push(RegisteredCapability {
                name: tool.name.clone(),
                namespace: tool
                    .name
                    .split('.')
                    .next()
                    .unwrap_or(&tool.name)
                    .to_string(),
                capability: risk_name(tool.risk).to_string(),
                description: tool.description.clone(),
            });
        }
    }
    Ok(tools)
}

pub fn mcp_tools() -> Vec<Value> {
    if host_dir_existing().is_none() {
        return Vec::new();
    }
    enabled_capabilities()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tool| {
            let manifest = load_installed(
                tool.name
                    .split('.')
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(".")
                    .as_str(),
            )
            .ok()?;
            let spec = manifest.tools.iter().find(|item| item.name == tool.name)?;
            Some(serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.input_schema,
            }))
        })
        .collect()
}

/// `None` when `name` is not an enabled connector tool.
pub fn invoke(name: &str, arguments: &Value) -> Option<Result<Value, ToolError>> {
    let outcome = invoke_inner(name, arguments);
    match outcome {
        Ok(None) => None,
        Ok(Some(payload)) => Some(Ok(payload)),
        Err(error) => Some(Err(ToolError::Invalid(error.to_string()))),
    }
}

fn invoke_inner(name: &str, arguments: &Value) -> Result<Option<Value>> {
    if host_dir_existing().is_none() {
        return Ok(None);
    }
    let _lock = match lock_host() {
        Ok(lock) => lock,
        Err(_) => return Ok(None),
    };
    let mut index = match read_index() {
        Ok(index) => index,
        Err(_) => return Ok(None),
    };
    let Some(position) = index.connectors.iter().position(|item| {
        item.enabled
            && load_installed(&item.id)
                .ok()
                .is_some_and(|manifest| manifest.tools.iter().any(|tool| tool.name == name))
    }) else {
        return Ok(None);
    };
    let manifest = load_installed(&index.connectors[position].id)?;
    let tool = manifest
        .tools
        .iter()
        .find(|tool| tool.name == name)
        .unwrap()
        .clone();
    ensure_intact(&index.connectors[position], &manifest)?;
    let args_hash = sha256_bytes(&canonical(arguments));
    let correlation = auth::random_hex(8)?;

    if tool.risk != Risk::Read {
        let gate = gate_approval(&manifest.id, &tool, &args_hash)?;
        if let Some(message) = gate {
            let status = if message.starts_with("approval denied") {
                "denied"
            } else {
                "approval_required"
            };
            audit(&manifest.id, name, "call", &args_hash, status)?;
            drop(_lock);
            anyhow::bail!("{message}");
        }
    }

    if tool.idempotent {
        if let Some(content) = index.connectors[position]
            .results
            .iter()
            .find(|item| item.key == args_hash)
            .map(|item| item.content.clone())
        {
            audit(&manifest.id, name, "call", &args_hash, "replay")?;
            return Ok(Some(call_payload(&correlation, &content)));
        }
    }

    enforce_rate(&mut index.connectors[position], &manifest.limits)?;
    write_index(&index)?;
    drop(_lock);

    let content = run_process(
        &manifest,
        &index.connectors[position],
        "call",
        name,
        arguments,
    )?;
    let content = redact_output(&content);

    let _lock = lock_host()?;
    let mut index = read_index()?;
    if let Some(item) = index
        .connectors
        .iter_mut()
        .find(|item| item.id == manifest.id)
    {
        if tool.idempotent {
            item.results.push(StoredResult {
                key: args_hash.clone(),
                content: content.clone(),
            });
            if item.results.len() > 32 {
                item.results.remove(0);
            }
            write_index(&index)?;
        }
    }
    audit(&manifest.id, name, "call", &args_hash, "ran")?;
    Ok(Some(call_payload(&correlation, &content)))
}

fn call_payload(correlation: &str, content: &str) -> Value {
    serde_json::json!({
        "status": "ok",
        "content": content,
        "correlation_id": correlation,
    })
}

fn gate_approval(connector_id: &str, tool: &ToolSpec, args_hash: &str) -> Result<Option<String>> {
    let mut file = read_approvals()?;
    if let Some(item) = file.approvals.iter().find(|item| {
        item.connector_id == connector_id
            && item.tool == tool.name
            && item.args_sha256 == args_hash
            && item.status == "denied"
    }) {
        return Ok(Some(format!("approval denied: {}", item.id)));
    }
    if let Some(item) = file.approvals.iter_mut().find(|item| {
        item.connector_id == connector_id
            && item.tool == tool.name
            && item.args_sha256 == args_hash
            && item.status == "approved"
    }) {
        item.status = "consumed".to_string();
        write_approvals(&file)?;
        return Ok(None);
    }
    if let Some(item) = file.approvals.iter().find(|item| {
        item.connector_id == connector_id
            && item.tool == tool.name
            && item.args_sha256 == args_hash
            && item.status == "pending"
    }) {
        return Ok(Some(format!("approval required: {}", item.id)));
    }
    let id = format!("root_appr_{}", auth::random_hex(8)?);
    file.approvals.push(Approval {
        id: id.clone(),
        connector_id: connector_id.to_string(),
        tool: tool.name.clone(),
        risk: risk_name(tool.risk).to_string(),
        args_sha256: args_hash.to_string(),
        status: "pending".to_string(),
        created_at: Utc::now().to_rfc3339(),
    });
    write_approvals(&file)?;
    Ok(Some(format!("approval required: {id}")))
}

fn run_process(
    manifest: &Manifest,
    installed: &Installed,
    op: &str,
    tool: &str,
    arguments: &Value,
) -> Result<String> {
    let dir = package_dir_for(&manifest.id)?;
    let exe = dir.join("bin").join(&manifest.executable);
    let run_dir = dir.join("run");
    fs::create_dir_all(&run_dir)?;
    let request = serde_json::json!({
        "op": op,
        "tool": tool,
        "arguments": arguments,
    });
    let mut command = std::process::Command::new(&exe);
    command
        .env_clear()
        .current_dir(&run_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for credential in &installed.credentials {
        if !credential.bound {
            continue;
        }
        if let Some(value) = std::env::var_os(&credential.name) {
            command.env(&credential.name, value);
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start {}", exe.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{request}")?;
    }
    let max = manifest.limits.max_output_bytes;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(max.saturating_add(1)).read_to_end(&mut buf);
        }
        buf
    });
    let timeout = Duration::from_millis(manifest.limits.timeout_ms);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "connector {} exceeded {} ms",
                manifest.id,
                manifest.limits.timeout_ms
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        anyhow::bail!("connector {} exited {status}", manifest.id);
    }
    let buf = reader.join().unwrap_or_default();
    if buf.len() as u64 > max {
        anyhow::bail!("connector {} exceeded output limit", manifest.id);
    }
    let text = String::from_utf8(buf).context("connector output is not UTF-8")?;
    let line = text.lines().next().unwrap_or("");
    let value: Value = serde_json::from_str(line).context("connector output is not JSON")?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("connector failed");
        anyhow::bail!("{message}");
    }
    Ok(value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

fn health_check(installed: &Installed, manifest: &Manifest) -> Result<()> {
    let content = run_process(
        manifest,
        installed,
        "health",
        "",
        &Value::Object(Map::new()),
    )?;
    let _ = content;
    Ok(())
}

fn enforce_rate(installed: &mut Installed, limits: &Limits) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let window = limits.window_seconds;
    installed
        .recent_calls
        .retain(|at| now.saturating_sub(*at) < window);
    if installed.recent_calls.len() as u64 >= limits.max_calls {
        anyhow::bail!("connector {} rate limit exceeded", installed.id);
    }
    installed.recent_calls.push(now);
    Ok(())
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    if let Some(label) = root_work::secrets::detect(&text) {
        anyhow::bail!("refusing connector manifest: it looks like a secret ({label})");
    }
    let manifest: Manifest = serde_json::from_str(&text).context("invalid connector manifest")?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<()> {
    if manifest.schema != SCHEMA {
        anyhow::bail!("unsupported connector schema {}", manifest.schema);
    }
    if !valid_id(&manifest.id) {
        anyhow::bail!("invalid connector id {}", manifest.id);
    }
    let namespace = manifest.id.split('.').next().unwrap_or("");
    if RESERVED.contains(&namespace) {
        anyhow::bail!("connector id {} uses a reserved namespace", manifest.id);
    }
    if manifest.executable.contains('/')
        || manifest.executable.contains("..")
        || manifest.executable.is_empty()
    {
        anyhow::bail!("executable must be a single file name");
    }
    if manifest.executable_sha256.len() != 64
        || !manifest
            .executable_sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        anyhow::bail!("executable_sha256 must be 64 hex characters");
    }
    if manifest.network != ["none"] || manifest.filesystem != ["none"] {
        anyhow::bail!(
            "only default-deny grants are accepted (network and filesystem must be [\"none\"])"
        );
    }
    if manifest.limits.timeout_ms == 0 || manifest.limits.timeout_ms > 60_000 {
        anyhow::bail!("timeout_ms must be 1..=60000");
    }
    if manifest.limits.max_output_bytes == 0 || manifest.limits.max_output_bytes > 1_048_576 {
        anyhow::bail!("max_output_bytes must be 1..=1048576");
    }
    if manifest.limits.max_calls == 0 || manifest.limits.window_seconds == 0 {
        anyhow::bail!("rate limit must be positive");
    }
    if manifest.tools.is_empty() {
        anyhow::bail!("connector must declare at least one tool");
    }
    for tool in &manifest.tools {
        if !tool.name.starts_with(&format!("{}.", manifest.id)) {
            anyhow::bail!("tool {} must be named under {}", tool.name, manifest.id);
        }
        if !tool.input_schema.is_object() {
            anyhow::bail!("tool {} input_schema must be an object", tool.name);
        }
    }
    for credential in &manifest.credentials {
        if !credential
            .name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            || credential.name.is_empty()
        {
            anyhow::bail!(
                "credential name {} must be an uppercase identifier",
                credential.name
            );
        }
    }
    for event in &manifest.events {
        if event.is_empty() || event.contains(' ') {
            anyhow::bail!("invalid event type {event}");
        }
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    let mut parts = id.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    parts.next().is_none() && ident(first) && ident(second)
}

fn ident(part: &str) -> bool {
    let mut chars = part.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn load_installed(id: &str) -> Result<Manifest> {
    load_manifest(&package_dir_for(id)?.join("manifest.json"))
}

fn ensure_intact(installed: &Installed, manifest: &Manifest) -> Result<()> {
    let dir = package_dir_for(&installed.id)?;
    let manifest_sha = sha256_file(&dir.join("manifest.json"))?;
    let executable_sha = sha256_file(&dir.join("bin").join(&manifest.executable))?;
    if manifest_sha != installed.manifest_sha256 || executable_sha != installed.executable_sha256 {
        anyhow::bail!("connector {} package changed on disk", installed.id);
    }
    Ok(())
}

fn detail(installed: &Installed) -> Result<ConnectorDetail> {
    let manifest = load_installed(&installed.id)?;
    let intact = ensure_intact(installed, &manifest).is_ok();
    Ok(ConnectorDetail {
        id: installed.id.clone(),
        version: manifest.version,
        publisher: manifest.publisher,
        enabled: installed.enabled,
        executable_sha256: installed.executable_sha256.clone(),
        manifest_sha256: installed.manifest_sha256.clone(),
        intact,
        tools: manifest
            .tools
            .iter()
            .map(|tool| ToolView {
                name: tool.name.clone(),
                risk: risk_name(tool.risk).to_string(),
                idempotent: tool.idempotent,
                description: tool.description.clone(),
            })
            .collect(),
        events: manifest.events,
        credentials: credential_views(installed),
        network: manifest.network,
        filesystem: manifest.filesystem,
        isolation: "separate process, scrubbed environment, empty working directory, timeout, output cap; not a kernel sandbox",
    })
}

fn credential_views(installed: &Installed) -> Vec<CredentialView> {
    installed
        .credentials
        .iter()
        .map(|item| CredentialView {
            name: item.name.clone(),
            scopes: item.scopes.clone(),
            bound: item.bound,
        })
        .collect()
}

fn approval_view(item: &Approval) -> ApprovalView {
    ApprovalView {
        id: item.id.clone(),
        connector_id: item.connector_id.clone(),
        tool: item.tool.clone(),
        risk: item.risk.clone(),
        status: item.status.clone(),
        args_sha256: item.args_sha256.clone(),
        created_at: item.created_at.clone(),
    }
}

fn risk_name(risk: Risk) -> &'static str {
    match risk {
        Risk::Read => "read",
        Risk::Write => "write",
        Risk::Destructive => "destructive",
    }
}

fn redact_output(content: &str) -> String {
    if root_work::secrets::detect(content).is_some() {
        "[redacted]".to_string()
    } else {
        content.to_string()
    }
}

fn host_dir_existing() -> Option<PathBuf> {
    let root = policy::root_dir().ok()?;
    let dir = root.join("connectors");
    dir.is_dir().then_some(dir)
}

fn host_dir() -> Result<PathBuf> {
    let root = policy::root_dir()?;
    let dir = root.join("connectors");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn package_dir_for(id: &str) -> Result<PathBuf> {
    Ok(host_dir()?.join(id))
}

struct HostLock {
    file: File,
}

fn lock_host() -> Result<HostLock> {
    let path = host_dir()?.join("host.lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    file.lock().context("could not lock connector host")?;
    Ok(HostLock { file })
}

impl Drop for HostLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn index_path() -> Result<PathBuf> {
    Ok(host_dir()?.join("index.json"))
}

fn read_index() -> Result<Index> {
    let path = index_path()?;
    if !path.exists() {
        return Ok(Index::default());
    }
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn write_index(index: &Index) -> Result<()> {
    let path = index_path()?;
    let bytes = serde_json::to_vec_pretty(index)?;
    atomic_write(&path, &bytes)
}

fn approvals_path() -> Result<PathBuf> {
    Ok(host_dir()?.join("approvals.json"))
}

fn read_approvals() -> Result<ApprovalFile> {
    let path = approvals_path()?;
    if !path.exists() {
        return Ok(ApprovalFile::default());
    }
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn write_approvals(file: &ApprovalFile) -> Result<()> {
    atomic_write(&approvals_path()?, &serde_json::to_vec_pretty(file)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn audit(
    connector_id: &str,
    tool: &str,
    action: &str,
    args_sha256: &str,
    status: &str,
) -> Result<()> {
    let path = host_dir()?.join("audit.jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::json!({
        "at": Utc::now().to_rfc3339(),
        "correlation_id": auth::random_hex(8).unwrap_or_else(|_| "unknown".to_string()),
        "connector_id": connector_id,
        "tool": tool,
        "action": action,
        "args_sha256": args_sha256,
        "status": status,
    });
    writeln!(file, "{line}")?;
    Ok(())
}

fn installed<'a>(index: &'a Index, id: &str) -> Result<&'a Installed> {
    index
        .connectors
        .iter()
        .find(|item| item.id == id)
        .context(format!("unknown connector {id}"))
}

fn position(index: &Index, id: &str) -> Result<usize> {
    index
        .connectors
        .iter()
        .position(|item| item.id == id)
        .context(format!("unknown connector {id}"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

fn canonical(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&canonicalize(value)).unwrap_or_default()
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                if let Some(child) = map.get(&key) {
                    out.insert(key, canonicalize(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}
