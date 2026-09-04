//! Bundle manifest (v1): manifest.json + content-addressed blobs.
//!
//! No file content is embedded in JSON. Every file lives at
//! `<bundle>/blobs/<sha256hex>` and is referenced by hash.

use crate::scope::{check_duplicates, validate_rel, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const BUNDLE_VERSION: u32 = 1;
pub const ADAPTER_ID: &str = "codex";
pub const OPENCODE_ADAPTER_ID: &str = "opencode";
pub const ADAPTER_SCHEMA_VERSION: u32 = 1;

/// Exact live-tested Codex versions for S1. Expanded only after evidence.
pub const SUPPORTED_CODEX_VERSIONS: &[&str] = &["0.150.1"];

/// Exact live-tested OpenCode versions for S2. Expanded only after evidence.
pub const SUPPORTED_OPENCODE_VERSIONS: &[&str] = &["1.18.27"];

pub fn unsupported_adapter_error(adapter: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "unsupported bundle adapter '{}'. S2 supports 'codex' and 'opencode' only (no cross-agent translation)",
        adapter
    )
}

pub fn supported_versions_for(adapter: &str) -> Result<&'static [&'static str]> {
    match adapter {
        ADAPTER_ID => Ok(SUPPORTED_CODEX_VERSIONS),
        OPENCODE_ADAPTER_ID => Ok(SUPPORTED_OPENCODE_VERSIONS),
        other => Err(unsupported_adapter_error(other)),
    }
}

pub fn mcp_approval_target(adapter: &str, id: &str) -> Result<(Scope, String)> {
    match adapter {
        ADAPTER_ID => Ok((Scope::CodexHome, format!("config.toml#mcp_servers.{}", id))),
        OPENCODE_ADAPTER_ID => Ok((Scope::OpenCodeHome, format!("opencode.json#mcp.{}", id))),
        other => Err(unsupported_adapter_error(other)),
    }
}

pub fn allowed_settings_for(adapter: &str) -> Result<&'static [&'static str]> {
    match adapter {
        ADAPTER_ID => Ok(crate::codex::ALLOWED_SETTINGS),
        OPENCODE_ADAPTER_ID => Ok(crate::opencode::ALLOWED_SETTINGS),
        other => Err(unsupported_adapter_error(other)),
    }
}

/// Bundle size caps (fail closed).
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_FILES: usize = 256;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_MCP_SERVERS: usize = 64;
pub const MAX_ENV_NAMES: usize = 256;
pub const MAX_HELD_ITEMS: usize = 512;

/// Disclosure: bundles are NOT categorically secret-free.
pub const SECRET_DISCLOSURE: &str = "Known secret locations and formats were excluded; explicitly selected prompt/skill files were copied verbatim and may contain unrecognized secrets. Review bundle contents before transfer.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum FileKind {
    PassiveData,
    PromptContent,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFile {
    pub scope: Scope,
    pub rel: String,
    pub sha256: String,
    pub size: u64,
    /// Unix mode as string: "0644", "0600", or "0755" (executable only).
    pub mode: String,
    pub kind: FileKind,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeedsApproval {
    pub scope: Scope,
    pub rel: String,
    pub sha256: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldItem {
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpEntry {
    pub transport: String,
    /// S1: always false on export and apply. Enabling is a separate
    /// protected `enable` mutation.
    pub enabled: bool,
    #[serde(default)]
    pub needs_env: Vec<String>,
    /// Hash-bound approval fingerprint over the sanitized command descriptor.
    /// Apply requires `--approve <command_sha256>` per server.
    #[serde(default)]
    pub command_sha256: Option<String>,
    /// Sanitized command descriptor (structure + env key NAMES only; secret
    /// values are never stored).
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env_keys: Vec<String>,
}

/// Canonical v1 manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub bundle_version: u32,
    pub adapter: String,
    pub adapter_schema_version: u32,
    pub source_agent_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    pub files: Vec<BundleFile>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpEntry>,
    #[serde(default)]
    pub needs_env: Vec<String>,
    #[serde(default)]
    pub needs_approval: Vec<NeedsApproval>,
    #[serde(default)]
    pub held: Vec<HeldItem>,
    pub disclosure: String,
}

impl Manifest {
    pub fn new(source_agent_version: String, created: Option<String>) -> Self {
        Self::new_for(ADAPTER_ID, source_agent_version, created)
    }

    pub fn new_for(adapter: &str, source_agent_version: String, created: Option<String>) -> Self {
        Self {
            bundle_version: BUNDLE_VERSION,
            adapter: adapter.to_string(),
            adapter_schema_version: ADAPTER_SCHEMA_VERSION,
            source_agent_version,
            created,
            files: Vec::new(),
            settings: BTreeMap::new(),
            mcp: BTreeMap::new(),
            needs_env: Vec::new(),
            needs_approval: Vec::new(),
            held: Vec::new(),
            disclosure: SECRET_DISCLOSURE.to_string(),
        }
    }

    /// Strict structural validation (no I/O).
    pub fn validate(&self) -> Result<()> {
        if self.bundle_version != BUNDLE_VERSION {
            anyhow::bail!(
                "unsupported bundle version {}. Only version {} is supported",
                self.bundle_version,
                BUNDLE_VERSION
            );
        }
        let supported_versions = supported_versions_for(&self.adapter)?;
        if self.adapter_schema_version != ADAPTER_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported adapter schema version {}. Only version {} is supported",
                self.adapter_schema_version,
                ADAPTER_SCHEMA_VERSION
            );
        }
        if !supported_versions.contains(&self.source_agent_version.as_str()) {
            anyhow::bail!(
                "unsupported source agent version '{}' for adapter '{}'. Supported exact versions: {:?}",
                self.source_agent_version,
                self.adapter,
                supported_versions
            );
        }
        if self.files.len() > MAX_FILES {
            anyhow::bail!(
                "invalid bundle: {} files exceeds limit of {}",
                self.files.len(),
                MAX_FILES
            );
        }
        if self.mcp.len() > MAX_MCP_SERVERS {
            anyhow::bail!(
                "invalid bundle: {} MCP servers exceeds limit of {}",
                self.mcp.len(),
                MAX_MCP_SERVERS
            );
        }
        if self.needs_env.len() > MAX_ENV_NAMES {
            anyhow::bail!("invalid bundle: too many required environment variables");
        }
        if self.held.len() > MAX_HELD_ITEMS {
            anyhow::bail!("invalid bundle: too many held-item records");
        }
        if self
            .created
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128 || s.contains('\0') || s.contains('\n'))
        {
            anyhow::bail!("invalid bundle: malformed created metadata");
        }
        if self.disclosure != SECRET_DISCLOSURE {
            anyhow::bail!("invalid bundle: disclosure string must match exactly");
        }
        let mut total: u64 = 0;
        let mut targets: Vec<(Scope, String)> = Vec::with_capacity(self.files.len());
        for f in &self.files {
            validate_rel(&f.rel)?;
            if !valid_bundle_file_target(f.scope, &f.rel) {
                anyhow::bail!(
                    "invalid bundle: file target '{}:{}' is outside the adapter v1 allowlist",
                    f.scope.as_str(),
                    f.rel
                );
            }
            match (self.adapter.as_str(), f.scope) {
                (ADAPTER_ID, Scope::OpenCodeHome) => {
                    anyhow::bail!(
                        "invalid bundle: Codex bundles must not contain OpenCodeHome files"
                    );
                }
                (OPENCODE_ADAPTER_ID, Scope::CodexHome) => {
                    anyhow::bail!(
                        "invalid bundle: OpenCode bundles must not contain CodexHome files"
                    );
                }
                _ => {}
            }
            if !is_canonical_sha256(&f.sha256) {
                anyhow::bail!("invalid bundle: malformed sha256 for '{}'", f.rel);
            }
            if f.size > MAX_FILE_BYTES {
                anyhow::bail!(
                    "invalid bundle: file '{}' ({} bytes) exceeds per-file limit",
                    f.rel,
                    f.size
                );
            }
            total = total
                .checked_add(f.size)
                .context("invalid bundle: total size overflow")?;
            match f.mode.as_str() {
                "0644" | "0600" => {}
                "0755" => {
                    if !f.executable {
                        anyhow::bail!(
                            "invalid bundle: mode 0755 requires executable=true for '{}'",
                            f.rel
                        );
                    }
                }
                other => {
                    anyhow::bail!(
                        "invalid bundle: unsupported mode '{}' for '{}'",
                        other,
                        f.rel
                    );
                }
            }
            if f.executable != (f.kind == FileKind::Executable) {
                anyhow::bail!(
                    "invalid bundle: executable and kind disagree for '{}'",
                    f.rel
                );
            }
            if f.executable != (f.mode == "0755") {
                anyhow::bail!(
                    "invalid bundle: executable and mode disagree for '{}'",
                    f.rel
                );
            }
            targets.push((f.scope, f.rel.clone()));
        }
        if total > MAX_TOTAL_BYTES {
            anyhow::bail!(
                "invalid bundle: total {} bytes exceeds limit of {}",
                total,
                MAX_TOTAL_BYTES
            );
        }
        check_duplicates(&targets)?;
        let mut expected_approvals: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut expected_env = BTreeSet::new();

        // MCP invariants (authenticated — recomputed, never trusted).
        for (id, entry) in &self.mcp {
            if !valid_mcp_server_id(id) {
                anyhow::bail!("invalid bundle: malformed MCP server id '{}'", id);
            }
            if entry.enabled {
                anyhow::bail!(
                    "invalid bundle: MCP server '{}' must be disabled (enabled=false) in bundle v1",
                    id
                );
            }
            if entry.transport != "stdio" {
                anyhow::bail!(
                    "invalid bundle: MCP server '{}' has unsupported transport '{}' (stdio only in v1)",
                    id,
                    entry.transport
                );
            }
            if entry.command.is_empty() || entry.command.len() > 1 {
                anyhow::bail!(
                    "invalid bundle: MCP server '{}' must declare exactly one command",
                    id
                );
            }
            for part in entry.command.iter().chain(entry.args.iter()) {
                if part.is_empty()
                    || part.len() > 1024
                    || part.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
                {
                    anyhow::bail!(
                        "invalid bundle: MCP server '{}' has malformed command/args",
                        id
                    );
                }
            }
            if entry
                .args
                .iter()
                .any(|arg| looks_like_literal_secret_argument(arg))
            {
                anyhow::bail!(
                    "invalid bundle: MCP server '{}' has a suspicious secret-bearing argument",
                    id
                );
            }
            if entry.args.len() > 64 {
                anyhow::bail!("invalid bundle: MCP server '{}' has too many args", id);
            }
            if entry.env_keys.len() > MAX_ENV_NAMES || entry.needs_env.len() > MAX_ENV_NAMES {
                anyhow::bail!("invalid bundle: MCP server '{}' has too many env names", id);
            }
            if !valid_mcp_cwd(&entry.cwd) {
                anyhow::bail!("invalid bundle: MCP server '{}' has invalid cwd", id);
            }
            for key in entry.env_keys.iter().chain(entry.needs_env.iter()) {
                if !valid_env_name(key) {
                    anyhow::bail!(
                        "invalid bundle: MCP server '{}' has invalid env name '{}'",
                        id,
                        key
                    );
                }
            }
            // needs_env must equal the env_keys set exactly (no smuggling).
            {
                let mut a = entry.env_keys.clone();
                let mut b = entry.needs_env.clone();
                a.sort();
                b.sort();
                if has_adjacent_duplicate(&a)
                    || has_adjacent_duplicate(&b)
                    || a != entry.env_keys
                    || b != entry.needs_env
                    || a != b
                {
                    anyhow::bail!(
                        "invalid bundle: MCP server '{}' env names must be sorted, unique, and match exactly",
                        id
                    );
                }
                expected_env.extend(a);
            }
            // Fingerprint is mandatory and must recompute exactly. A
            // handcrafted manifest that omits command_sha256 or changes any
            // field without recomputing fails here.
            let recomputed =
                mcp_command_hash(&entry.command, &entry.args, &entry.cwd, &entry.env_keys);
            match &entry.command_sha256 {
                Some(claimed) if claimed == &recomputed => (),
                _ => {
                    anyhow::bail!(
                        "invalid bundle: MCP server '{}' has missing or mismatched command_sha256",
                        id
                    );
                }
            }
            let (scope, rel) = mcp_approval_target(&self.adapter, id)?;
            expected_approvals.insert((scope.as_str().to_string(), rel, recomputed));
        }
        // Settings allowlist (keys + short plain-string values).
        let allowed_settings = allowed_settings_for(&self.adapter)?;
        for (key, val) in &self.settings {
            if !allowed_settings.contains(&key.as_str()) {
                anyhow::bail!("invalid bundle: settings key '{}' is not allowlisted", key);
            }
            match val {
                serde_json::Value::String(s)
                    if s.len() <= 256 && !s.contains('\0') && !s.contains('\n') => {}
                _ => {
                    anyhow::bail!("invalid bundle: settings key '{}' has invalid value", key);
                }
            }
        }
        // Executable files MUST have a matching needs_approval entry binding
        // the exact hash (global boolean approval is forbidden).
        for f in self.files.iter().filter(|f| f.executable) {
            expected_approvals.insert((
                f.scope.as_str().to_string(),
                f.rel.clone(),
                f.sha256.clone(),
            ));
        }

        let mut actual_approvals = BTreeSet::new();
        for approval in &self.needs_approval {
            if approval.reason.is_empty()
                || approval.reason.len() > 512
                || approval.reason.contains('\0')
                || approval.reason.contains('\n')
                || !is_canonical_sha256(&approval.sha256)
            {
                anyhow::bail!("invalid bundle: malformed needs_approval record");
            }
            let key = (
                approval.scope.as_str().to_string(),
                approval.rel.clone(),
                approval.sha256.clone(),
            );
            if !actual_approvals.insert(key) {
                anyhow::bail!("invalid bundle: duplicate needs_approval record");
            }
        }
        if actual_approvals != expected_approvals {
            anyhow::bail!(
                "invalid bundle: needs_approval must exactly match executable files and MCP commands"
            );
        }

        let actual_env: BTreeSet<String> = self.needs_env.iter().cloned().collect();
        let sorted_env: Vec<String> = actual_env.iter().cloned().collect();
        if actual_env.len() != self.needs_env.len()
            || sorted_env != self.needs_env
            || actual_env != expected_env
        {
            anyhow::bail!(
                "invalid bundle: needs_env must be the sorted unique union of MCP environment references"
            );
        }
        for held in &self.held {
            if held.source.is_empty()
                || held.source.len() > 1024
                || held.reason.is_empty()
                || held.reason.len() > 1024
                || held.source.contains('\0')
                || held.reason.contains('\0')
            {
                anyhow::bail!("invalid bundle: malformed held-item record");
            }
        }
        Ok(())
    }
}

fn has_adjacent_duplicate(values: &[String]) -> bool {
    values.windows(2).any(|pair| pair[0] == pair[1])
}

pub fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn is_canonical_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Portable MCP ids are intentionally narrower than arbitrary TOML keys.
pub fn valid_mcp_server_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    let bytes = id.as_bytes();
    bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && !id.contains("..")
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub(crate) fn valid_bundle_file_target(scope: Scope, rel: &str) -> bool {
    match scope {
        Scope::CodexHome => rel == "AGENTS.md",
        Scope::OpenCodeHome => {
            if rel == "AGENTS.md" {
                return true;
            }
            let mut parts = rel.split('/');
            let Some("skills") = parts.next() else {
                return false;
            };
            let Some(skill) = parts.next() else {
                return false;
            };
            let Some(_) = parts.next() else {
                return false;
            };
            valid_skill_component(skill)
        }
        Scope::SharedSkills => {
            let mut parts = rel.split('/');
            let Some(skill) = parts.next() else {
                return false;
            };
            let Some(_) = parts.next() else {
                return false;
            };
            valid_skill_component(skill)
        }
    }
}

fn valid_skill_component(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.starts_with('-') || name.ends_with('-') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.contains("--")
}

/// Defense-in-depth only: secret references belong in `env_vars`, never in
/// command arguments. This intentionally rejects common literal credential
/// forms rather than trying to prove arbitrary text is secret-free.
pub fn looks_like_literal_secret_argument(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    const FLAGS: &[&str] = &[
        "--api-key",
        "--apikey",
        "--token",
        "--access-token",
        "--password",
        "--passwd",
        "--secret",
        "--authorization",
    ];
    const ASSIGNMENTS: &[&str] = &[
        "--api-key=",
        "--apikey=",
        "--token=",
        "--access-token=",
        "--password=",
        "--passwd=",
        "--secret=",
        "authorization=",
    ];
    FLAGS.contains(&lower.as_str())
        || ASSIGNMENTS.iter().any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|value| !value.is_empty())
        })
        || lower.starts_with("bearer ")
        || lower.contains("authorization: bearer ")
        || lower.starts_with("sk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || (lower.contains("://") && lower.contains('@'))
}

/// Canonical MCP command descriptor bytes. Single construction site shared by
/// export (fingerprint), manifest validation (recompute-and-compare), and
/// enable (live-descriptor binding). Field order is fixed.
pub fn canonical_mcp_descriptor(
    command: &[String],
    args: &[String],
    cwd: &Option<String>,
    env_keys: &[String],
) -> Vec<u8> {
    let descriptor = serde_json::json!({
        "command": command,
        "args": args,
        "cwd": cwd,
        "env_keys": env_keys,
    });
    serde_json::to_vec(&descriptor).unwrap_or_default()
}

pub fn mcp_command_hash(
    command: &[String],
    args: &[String],
    cwd: &Option<String>,
    env_keys: &[String],
) -> String {
    root_lockfile::compute_sha256(&canonical_mcp_descriptor(command, args, cwd, env_keys))
}

/// Valid environment-variable reference name (`[A-Za-z_][A-Za-z0-9_]{0,127}`).
pub fn valid_env_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate a portable MCP `cwd`: absent, `"."`, or a safe relative path.
/// Absolute and home-expanded paths are machine-local and rejected in v1.
pub fn valid_mcp_cwd(cwd: &Option<String>) -> bool {
    match cwd {
        None => true,
        Some(c) => {
            if c.is_empty() || c.len() > 1024 || c.contains('\0') {
                return false;
            }
            if c == "." {
                return true;
            }
            !Path::new(c).is_absolute() && crate::scope::validate_rel(c).is_ok()
        }
    }
}

/// Canonical bundle hash: sha256 over canonical JSON of the manifest.
pub fn manifest_hash(manifest: &Manifest) -> Result<String> {
    let bytes = serde_json::to_vec(manifest).context("Failed to serialize manifest")?;
    Ok(root_lockfile::compute_sha256(&bytes))
}

/// Load + validate a bundle directory (`manifest.json` + `blobs/`).
/// Enforces: manifest size cap, no symlinks anywhere in the bundle dir,
/// per-file hash/size match, caps, structural validity.
pub fn load_bundle(bundle_dir: &Path) -> Result<Manifest> {
    let manifest_path = bundle_dir.join("manifest.json");
    let meta = std::fs::symlink_metadata(&manifest_path)
        .with_context(|| format!("Cannot stat {}", manifest_path.display()))?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("invalid bundle: symlinks are rejected in bundle v1");
    }
    if !meta.is_file() {
        anyhow::bail!("invalid bundle: manifest.json must be a regular file");
    }
    if meta.len() > MAX_MANIFEST_BYTES {
        anyhow::bail!("invalid bundle: manifest exceeds size limit");
    }
    let content = std::fs::read(&manifest_path).context("Failed to read bundle manifest")?;
    reject_symlinks_in_dir(bundle_dir)?;
    let manifest: Manifest =
        serde_json::from_slice(&content).context("Failed to parse bundle manifest")?;
    manifest.validate()?;
    // Verify blobs.
    let mut total: u64 = 0;
    for f in &manifest.files {
        let blob_path = bundle_dir.join("blobs").join(&f.sha256);
        let bmeta = std::fs::symlink_metadata(&blob_path)
            .with_context(|| format!("Missing blob for '{}'", f.rel))?;
        if bmeta.file_type().is_symlink() {
            anyhow::bail!("invalid bundle: symlinks are rejected in bundle v1");
        }
        if !bmeta.is_file() {
            anyhow::bail!("invalid bundle: blobs must be regular files");
        }
        if bmeta.len() != f.size {
            anyhow::bail!("invalid bundle: size mismatch for '{}'", f.rel);
        }
        total += bmeta.len();
        // Streamed hash (bounded read).
        let digest = hash_file_capped(&blob_path, MAX_FILE_BYTES + 1)?;
        if digest != f.sha256.to_lowercase() {
            anyhow::bail!("invalid bundle: hash mismatch for '{}'", f.rel);
        }
    }
    if total > MAX_TOTAL_BYTES {
        anyhow::bail!("invalid bundle: total blob size exceeds limit");
    }
    Ok(manifest)
}

/// Reject any symlink under a directory (bundle v1: no symlinks).
pub fn reject_symlinks_in_dir(dir: &Path) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    let mut count = 0usize;
    while let Some(cur) = stack.pop() {
        let entries =
            std::fs::read_dir(&cur).with_context(|| format!("Cannot list {}", cur.display()))?;
        for entry in entries {
            let entry = entry?;
            count += 1;
            if count > MAX_FILES + 16 {
                anyhow::bail!("invalid bundle: too many entries");
            }
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                anyhow::bail!(
                    "invalid bundle: symlinks are rejected in bundle v1 ({})",
                    entry.path().display()
                );
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if !ft.is_file() {
                anyhow::bail!(
                    "invalid bundle: non-regular file {} is rejected in bundle v1",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

/// Hash a file with a byte cap (streaming, no unbounded read).
pub fn hash_file_capped(path: &Path, cap: u64) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let file = std::fs::File::open(path).context("Failed to open file for hashing")?;
    let mut capped = file.take(cap + 1);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    let mut total: u64 = 0;
    loop {
        let n = capped.read(&mut buf).context("Failed to read file")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > cap {
            anyhow::bail!("File {} exceeds size cap", path.display());
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest() -> Manifest {
        Manifest::new("0.150.1".to_string(), None)
    }

    #[test]
    fn version_gates() {
        let mut m = minimal_manifest();
        m.validate().unwrap();
        m.bundle_version = 2;
        assert!(m.validate().is_err());
        let mut m = minimal_manifest();
        m.adapter = "claude".to_string();
        assert!(m.validate().is_err());
        let mut m = minimal_manifest();
        m.source_agent_version = "0.153.0".to_string();
        assert!(m.validate().is_err());
    }

    fn opencode_manifest() -> Manifest {
        Manifest::new_for(OPENCODE_ADAPTER_ID, "1.18.27".to_string(), None)
    }

    #[test]
    fn adapter_version_gate_is_adapter_specific() {
        let m = opencode_manifest();
        m.validate().unwrap();

        let mut other_opencode = opencode_manifest();
        other_opencode.source_agent_version = "1.18.26".to_string();
        assert!(other_opencode.validate().is_err());

        let mut codex_ok = minimal_manifest();
        codex_ok.source_agent_version = "0.150.1".to_string();
        codex_ok.validate().unwrap();

        let mut codex_opencode_ver = minimal_manifest();
        codex_opencode_ver.source_agent_version = "1.18.27".to_string();
        assert!(codex_opencode_ver.validate().is_err());

        let mut opencode_codex_ver = opencode_manifest();
        opencode_codex_ver.source_agent_version = "0.150.1".to_string();
        assert!(opencode_codex_ver.validate().is_err());
    }

    #[test]
    fn opencode_home_rel_allowlist() {
        assert!(valid_bundle_file_target(Scope::OpenCodeHome, "AGENTS.md"));
        assert!(valid_bundle_file_target(
            Scope::OpenCodeHome,
            "skills/docs-writer/SKILL.md"
        ));
        assert!(valid_bundle_file_target(
            Scope::OpenCodeHome,
            "skills/docs-writer/nested/run.sh"
        ));
        assert!(!valid_bundle_file_target(
            Scope::OpenCodeHome,
            "opencode.json"
        ));
        assert!(!valid_bundle_file_target(Scope::OpenCodeHome, "auth.json"));
        assert!(!valid_bundle_file_target(
            Scope::OpenCodeHome,
            "skills/docs-writer"
        ));
        assert!(!valid_bundle_file_target(
            Scope::OpenCodeHome,
            "skills/Docs-writer/SKILL.md"
        ));
        assert!(!valid_bundle_file_target(Scope::CodexHome, "skills/x/y"));

        let mut ok = opencode_manifest();
        ok.files.push(BundleFile {
            scope: Scope::OpenCodeHome,
            rel: "AGENTS.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        ok.validate().unwrap();

        let mut native_skill = opencode_manifest();
        native_skill.files.push(BundleFile {
            scope: Scope::OpenCodeHome,
            rel: "skills/docs-writer/SKILL.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        native_skill.validate().unwrap();

        let mut shared = opencode_manifest();
        shared.files.push(BundleFile {
            scope: Scope::SharedSkills,
            rel: "docs-writer/SKILL.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        shared.validate().unwrap();

        let mut codex_home_in_opencode = opencode_manifest();
        codex_home_in_opencode.files.push(BundleFile {
            scope: Scope::CodexHome,
            rel: "AGENTS.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        assert!(codex_home_in_opencode.validate().is_err());

        let mut opencode_home_in_codex = minimal_manifest();
        opencode_home_in_codex.files.push(BundleFile {
            scope: Scope::OpenCodeHome,
            rel: "AGENTS.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        assert!(opencode_home_in_codex.validate().is_err());
    }

    #[test]
    fn opencode_mcp_approval_target_is_opencode_home() {
        let mut m = opencode_manifest();
        let command = vec!["npx".to_string()];
        let hash = mcp_command_hash(&command, &[], &None, &[]);
        m.mcp.insert(
            "github".to_string(),
            McpEntry {
                transport: "stdio".to_string(),
                enabled: false,
                needs_env: vec![],
                command_sha256: Some(hash.clone()),
                command,
                args: vec![],
                cwd: None,
                env_keys: vec![],
            },
        );
        m.needs_approval.push(NeedsApproval {
            scope: Scope::CodexHome,
            rel: "config.toml#mcp_servers.github".to_string(),
            sha256: hash.clone(),
            reason: "MCP command".to_string(),
        });
        assert!(m.validate().is_err());
        m.needs_approval[0].scope = Scope::OpenCodeHome;
        m.needs_approval[0].rel = "opencode.json#mcp.github".to_string();
        m.validate().unwrap();
    }

    #[test]
    fn mcp_must_be_disabled_and_executable_needs_approval() {
        let mut m = minimal_manifest();
        m.mcp.insert(
            "x".to_string(),
            McpEntry {
                transport: "stdio".to_string(),
                enabled: true,
                needs_env: vec![],
                command_sha256: None,
                command: vec![],
                args: vec![],
                cwd: None,
                env_keys: vec![],
            },
        );
        assert!(m.validate().is_err());

        let mut m = minimal_manifest();
        m.files.push(BundleFile {
            scope: Scope::CodexHome,
            rel: "AGENTS.md".to_string(),
            sha256: "a".repeat(64),
            size: 10,
            mode: "0755".to_string(),
            kind: FileKind::Executable,
            executable: true,
        });
        // Missing needs_approval -> invalid.
        assert!(m.validate().is_err());
    }

    #[test]
    fn disclosure_exact_match_required() {
        let mut m = minimal_manifest();
        m.disclosure = "custom".to_string();
        assert!(m.validate().is_err());
    }

    fn valid_mcp_manifest() -> Manifest {
        let mut m = minimal_manifest();
        let command = vec!["npx".to_string()];
        let args = vec!["-y".to_string(), "server-package".to_string()];
        let env = vec!["API_TOKEN".to_string()];
        let hash = mcp_command_hash(&command, &args, &None, &env);
        m.mcp.insert(
            "server-one".to_string(),
            McpEntry {
                transport: "stdio".to_string(),
                enabled: false,
                needs_env: env.clone(),
                command_sha256: Some(hash.clone()),
                command,
                args,
                cwd: None,
                env_keys: env.clone(),
            },
        );
        m.needs_env = env;
        m.needs_approval.push(NeedsApproval {
            scope: Scope::CodexHome,
            rel: "config.toml#mcp_servers.server-one".to_string(),
            sha256: hash,
            reason: "MCP command".to_string(),
        });
        m
    }

    #[test]
    fn mcp_approvals_and_environment_are_exact() {
        let m = valid_mcp_manifest();
        m.validate().unwrap();

        let mut extra = m.clone();
        extra.needs_approval.push(NeedsApproval {
            scope: Scope::CodexHome,
            rel: "AGENTS.md".to_string(),
            sha256: "a".repeat(64),
            reason: "unrelated".to_string(),
        });
        assert!(extra.validate().is_err());

        let mut unsorted = m.clone();
        unsorted.needs_env = vec!["Z_TOKEN".to_string(), "API_TOKEN".to_string()];
        assert!(unsorted.validate().is_err());

        let mut duplicate = m.clone();
        duplicate
            .needs_approval
            .push(duplicate.needs_approval[0].clone());
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn mcp_rejects_ambiguous_ids_and_literal_secret_args() {
        let mut bad_id = valid_mcp_manifest();
        let entry = bad_id.mcp.remove("server-one").unwrap();
        bad_id.mcp.insert("../server".to_string(), entry);
        assert!(bad_id.validate().is_err());

        let mut secret = valid_mcp_manifest();
        let entry = secret.mcp.get_mut("server-one").unwrap();
        entry.args.push("--api-key=literal-value".to_string());
        let new_hash = mcp_command_hash(&entry.command, &entry.args, &entry.cwd, &entry.env_keys);
        entry.command_sha256 = Some(new_hash.clone());
        secret.needs_approval[0].sha256 = new_hash;
        assert!(secret.validate().is_err());
    }

    #[test]
    fn settings_and_approval_hashes_are_canonical() {
        let mut unknown = minimal_manifest();
        unknown
            .settings
            .insert("notify".to_string(), serde_json::json!(["evil"]));
        assert!(unknown.validate().is_err());

        let mut opencode_extra = opencode_manifest();
        opencode_extra.settings.insert(
            "model_reasoning_effort".to_string(),
            serde_json::json!("high"),
        );
        assert!(opencode_extra.validate().is_err());
        opencode_extra.settings.clear();
        opencode_extra
            .settings
            .insert("model".to_string(), serde_json::json!("gpt-x"));
        opencode_extra.validate().unwrap();

        let mut upper = valid_mcp_manifest();
        upper.needs_approval[0].sha256 = upper.needs_approval[0].sha256.to_ascii_uppercase();
        assert!(upper.validate().is_err());
    }

    #[test]
    fn manifest_file_targets_are_allowlisted() {
        let mut auth = minimal_manifest();
        auth.files.push(BundleFile {
            scope: Scope::CodexHome,
            rel: "auth.json".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0600".to_string(),
            kind: FileKind::PassiveData,
            executable: false,
        });
        assert!(auth.validate().is_err());

        let mut skill = minimal_manifest();
        skill.files.push(BundleFile {
            scope: Scope::SharedSkills,
            rel: "safe-skill/SKILL.md".to_string(),
            sha256: "a".repeat(64),
            size: 2,
            mode: "0644".to_string(),
            kind: FileKind::PromptContent,
            executable: false,
        });
        skill.validate().unwrap();
    }
}
