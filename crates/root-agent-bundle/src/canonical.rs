//! Canonical `RootAgentEnvironment` schema (Sprint 011 part 1).
//!
//! Harness-independent canonical form. Read-only: no filesystem writes,
//! no env-var reads, no locks. All validation is fail-closed with
//! `invalid canonical env: ...` errors (adapter-unknown errors reuse the
//! bundle `unsupported bundle adapter ...` sentinel).

use crate::manifest::{
    is_canonical_sha256, looks_like_literal_secret_argument, supported_versions_for,
    unsupported_adapter_error, valid_env_name, valid_mcp_cwd, valid_mcp_server_id, ADAPTER_ID,
    CLAUDE_ADAPTER_ID, MAX_ENV_NAMES, MAX_FILES, MAX_HELD_ITEMS, MAX_MCP_SERVERS,
    OPENCODE_ADAPTER_ID, SECRET_DISCLOSURE,
};
use crate::scope::validate_rel;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Canonical schema version (v1 only).
pub const CANONICAL_SCHEMA_VERSION: u32 = 1;

/// Union settings allowlist: passive setting keys portable across harnesses.
/// Per-harness narrowing happens at translation time, never by deleting
/// canonical data.
pub const UNION_SETTINGS_ALLOWLIST: &[&str] = &["model", "model_reasoning_effort", "service_tier"];

/// Max canonical env JSON file size (64 KiB).
pub const MAX_CANONICAL_ENV_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalInstructions {
    pub main: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSkill {
    pub name: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTool {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMcp {
    pub id: String,
    pub transport: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub credential_refs: Vec<String>,
    pub environment: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalPolicy {
    pub key: String,
    pub disposition: String,
    /// Why the entry was held (sanitizer refusal, non-portable shape, ...).
    /// Empty when no further explanation is needed. Additive in Sprint 011:
    /// old canonical files without this field still parse via the default.
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalEnvironment {
    pub needs_env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalProvenance {
    pub captured_at: String,
    pub bundle_hash: String,
    pub disclosure: String,
}

/// Canonical agent environment. `mcp_servers` uses `BTreeMap` so
/// `serde_json::to_vec` emits canonical (sorted-key) JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalEnv {
    pub schema_version: u32,
    pub source_agent: String,
    pub source_agent_version: String,
    pub instructions: CanonicalInstructions,
    pub skills: Vec<CanonicalSkill>,
    pub tools: Vec<CanonicalTool>,
    pub mcp_servers: BTreeMap<String, CanonicalMcp>,
    pub policies: Vec<CanonicalPolicy>,
    pub environment: CanonicalEnvironment,
    pub provenance: CanonicalProvenance,
}

fn has_adjacent_duplicate(values: &[String]) -> bool {
    values.windows(2).any(|pair| pair[0] == pair[1])
}

fn valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.starts_with('-') || name.ends_with('-') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.contains("--")
}

fn contains_nul_or_newline(s: &str) -> bool {
    s.contains('\0') || s.contains('\n') || s.contains('\r')
}

/// Conservative guard rail: reject tool values shaped like secrets.
///
/// Canonical tools carry names only (e.g. model identifiers), never secret
/// values. Case-insensitive. Rejects bearer prefixes, well-known key
/// prefixes (`sk-`, `ghp_`, `github_pat_`, `xoxb-`/`xoxp-`/`xoxa-`/`xoxr-`),
/// PEM blocks, URL-with-credentials shapes, and assignment shapes whose key
/// looks credential-ish with a non-empty value. Ordinary model identifiers
/// (`gpt-5`, `sonnet`, `opus`, `haiku`, `o1`, `gemini-*`) are accepted.
pub fn looks_like_secret_value(v: &str) -> bool {
    let lower = v.trim().to_ascii_lowercase();
    if lower.starts_with("bearer ") {
        return true;
    }
    for prefix in [
        "sk-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
    ] {
        if lower.starts_with(prefix) {
            return true;
        }
    }
    if lower.contains("-----begin") {
        return true;
    }
    if lower.contains("://") && lower.contains('@') {
        return true;
    }
    // Assignment shapes (`key=value` / `key: value`) whose key looks
    // credential-ish with a non-empty value. The key is normalized (only
    // alphanumerics) so `api-key`, `api key`, and `api_key` all match.
    for sep in ['=', ':'] {
        if let Some((raw_key, raw_val)) = lower.split_once(sep) {
            if raw_val.trim().is_empty() {
                continue;
            }
            let key: String = raw_key
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            const HINTS: &[&str] = &["token", "secret", "passwd", "password", "apikey", "auth"];
            if HINTS.iter().any(|hint| key.contains(hint)) {
                return true;
            }
        }
    }
    false
}

impl CanonicalEnv {
    /// Strict structural validation (no I/O, no env reads, no locks).
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CANONICAL_SCHEMA_VERSION {
            anyhow::bail!(
                "invalid canonical env: unsupported schema_version {}. Only version {} is supported",
                self.schema_version,
                CANONICAL_SCHEMA_VERSION
            );
        }
        // Informational source harness; must name a known adapter.
        canonical_adapter_id(&self.source_agent)?;
        if self.source_agent_version.is_empty()
            || self.source_agent_version.len() > 64
            || contains_nul_or_newline(&self.source_agent_version)
        {
            anyhow::bail!("invalid canonical env: malformed source_agent_version");
        }
        // Instructions: role name only.
        if self.instructions.main != "AGENTS.md" {
            anyhow::bail!(
                "invalid canonical env: instructions.main must be \"AGENTS.md\" (role name only)"
            );
        }
        // Skills: unique names, valid names, files within caps.
        if self.skills.len() > MAX_FILES {
            anyhow::bail!(
                "invalid canonical env: {} skills exceeds limit of {}",
                self.skills.len(),
                MAX_FILES
            );
        }
        {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for skill in &self.skills {
                if !valid_skill_name(&skill.name) {
                    anyhow::bail!(
                        "invalid canonical env: malformed skill name '{}'",
                        skill.name
                    );
                }
                if !seen.insert(skill.name.as_str()) {
                    anyhow::bail!(
                        "invalid canonical env: duplicate skill name '{}'",
                        skill.name
                    );
                }
                if skill.files.is_empty() {
                    anyhow::bail!("invalid canonical env: skill '{}' has no files", skill.name);
                }
                if skill.files.len() > MAX_FILES {
                    anyhow::bail!(
                        "invalid canonical env: skill '{}' has too many files",
                        skill.name
                    );
                }
                let mut seen_files: BTreeSet<&str> = BTreeSet::new();
                for f in &skill.files {
                    validate_rel(f).map_err(|e| {
                        anyhow::anyhow!(
                            "invalid canonical env: skill '{}' has invalid file '{}': {:#}",
                            skill.name,
                            f,
                            e
                        )
                    })?;
                    if !seen_files.insert(f.as_str()) {
                        anyhow::bail!(
                            "invalid canonical env: skill '{}' has duplicate file '{}'",
                            skill.name,
                            f
                        );
                    }
                }
            }
        }
        // Tools: unique keys, allowlisted, short plain strings.
        {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for tool in &self.tools {
                if !UNION_SETTINGS_ALLOWLIST.contains(&tool.name.as_str()) {
                    anyhow::bail!(
                        "invalid canonical env: tool '{}' is not allowlisted",
                        tool.name
                    );
                }
                if !seen.insert(tool.name.as_str()) {
                    anyhow::bail!("invalid canonical env: duplicate tool '{}'", tool.name);
                }
                if tool.value.len() > 256 || contains_nul_or_newline(&tool.value) {
                    anyhow::bail!(
                        "invalid canonical env: tool '{}' has invalid value",
                        tool.name
                    );
                }
                if looks_like_secret_value(&tool.value) {
                    anyhow::bail!(
                        "invalid canonical env: tool '{}' looks like a secret value; canonical tools carry names only, never secret values",
                        tool.name
                    );
                }
            }
        }
        // MCP servers.
        if self.mcp_servers.len() > MAX_MCP_SERVERS {
            anyhow::bail!(
                "invalid canonical env: {} MCP servers exceeds limit of {}",
                self.mcp_servers.len(),
                MAX_MCP_SERVERS
            );
        }
        for (key, entry) in &self.mcp_servers {
            if key != &entry.id {
                anyhow::bail!(
                    "invalid canonical env: MCP map key '{}' does not match entry id '{}'",
                    key,
                    entry.id
                );
            }
            validate_canonical_mcp(entry)?;
        }
        // Policies.
        if self.policies.len() > MAX_HELD_ITEMS {
            anyhow::bail!(
                "invalid canonical env: {} policies exceeds limit of {}",
                self.policies.len(),
                MAX_HELD_ITEMS
            );
        }
        for policy in &self.policies {
            if policy.key.is_empty()
                || policy.key.len() > 256
                || contains_nul_or_newline(&policy.key)
            {
                anyhow::bail!("invalid canonical env: malformed policy key");
            }
            if policy.disposition != "held" && policy.disposition != "requires-review" {
                anyhow::bail!(
                    "invalid canonical env: policy '{}' has invalid disposition '{}' (held | requires-review)",
                    policy.key,
                    policy.disposition
                );
            }
            if policy.reason.len() > 1024 || contains_nul_or_newline(&policy.reason) {
                anyhow::bail!(
                    "invalid canonical env: policy '{}' has malformed reason",
                    policy.key
                );
            }
        }
        // Environment: sorted unique names, exact union of MCP refs.
        if self.environment.needs_env.len() > MAX_ENV_NAMES {
            anyhow::bail!("invalid canonical env: too many required environment variables");
        }
        for name in &self.environment.needs_env {
            if !valid_env_name(name) {
                anyhow::bail!("invalid canonical env: invalid env name '{}'", name);
            }
        }
        {
            let mut sorted = self.environment.needs_env.clone();
            sorted.sort();
            if has_adjacent_duplicate(&sorted) || sorted != self.environment.needs_env {
                anyhow::bail!("invalid canonical env: needs_env must be sorted and unique");
            }
            let mut union: BTreeSet<String> = BTreeSet::new();
            for entry in self.mcp_servers.values() {
                union.extend(entry.credential_refs.iter().cloned());
            }
            let expected: Vec<String> = union.into_iter().collect();
            if expected != self.environment.needs_env {
                anyhow::bail!(
                    "invalid canonical env: needs_env must be the sorted unique union of MCP credential references"
                );
            }
        }
        // Provenance.
        if self.provenance.captured_at.is_empty()
            || self.provenance.captured_at.len() > 128
            || contains_nul_or_newline(&self.provenance.captured_at)
        {
            anyhow::bail!("invalid canonical env: malformed captured_at");
        }
        if !self.provenance.bundle_hash.is_empty() {
            match self.provenance.bundle_hash.strip_prefix("sha256:") {
                Some(hex) if is_canonical_sha256(hex) => {}
                _ => {
                    anyhow::bail!("invalid canonical env: malformed bundle_hash");
                }
            }
        }
        if self.provenance.disclosure != SECRET_DISCLOSURE {
            anyhow::bail!("invalid canonical env: disclosure string must match exactly");
        }
        Ok(())
    }

    /// Stable content hash: sha256 over canonical JSON bytes.
    pub fn env_hash(&self) -> Result<String> {
        let bytes =
            serde_json::to_vec(self).context("invalid canonical env: failed to serialize")?;
        Ok(root_lockfile::compute_sha256(&bytes))
    }
}

fn validate_canonical_mcp(entry: &CanonicalMcp) -> Result<()> {
    let id = entry.id.as_str();
    if !valid_mcp_server_id(id) {
        anyhow::bail!("invalid canonical env: malformed MCP server id '{}'", id);
    }
    if entry.enabled {
        anyhow::bail!(
            "invalid canonical env: MCP server '{}' must be disabled (enabled=false) in canonical v1",
            id
        );
    }
    if entry.transport != "stdio" {
        anyhow::bail!(
            "invalid canonical env: MCP server '{}' has unsupported transport '{}' (stdio only in v1)",
            id,
            entry.transport
        );
    }
    if entry.command.is_empty() || entry.command.len() > 1 {
        anyhow::bail!(
            "invalid canonical env: MCP server '{}' must declare exactly one command",
            id
        );
    }
    for part in entry.command.iter().chain(entry.args.iter()) {
        if part.is_empty()
            || part.len() > 1024
            || part.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
        {
            anyhow::bail!(
                "invalid canonical env: MCP server '{}' has malformed command/args",
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
            "invalid canonical env: MCP server '{}' has a suspicious secret-bearing argument",
            id
        );
    }
    if entry.args.len() > 64 {
        anyhow::bail!(
            "invalid canonical env: MCP server '{}' has too many args",
            id
        );
    }
    if entry.credential_refs.len() > MAX_ENV_NAMES || entry.environment.len() > MAX_ENV_NAMES {
        anyhow::bail!(
            "invalid canonical env: MCP server '{}' has too many env names",
            id
        );
    }
    if !valid_mcp_cwd(&entry.cwd) {
        anyhow::bail!("invalid canonical env: MCP server '{}' has invalid cwd", id);
    }
    for key in entry.credential_refs.iter().chain(entry.environment.iter()) {
        if !valid_env_name(key) {
            anyhow::bail!(
                "invalid canonical env: MCP server '{}' has invalid env name '{}'",
                id,
                key
            );
        }
    }
    // credential_refs must equal environment as sorted unique sets (no smuggling).
    {
        let mut a = entry.credential_refs.clone();
        let mut b = entry.environment.clone();
        a.sort();
        b.sort();
        if has_adjacent_duplicate(&a)
            || has_adjacent_duplicate(&b)
            || a != entry.credential_refs
            || b != entry.environment
            || a != b
        {
            anyhow::bail!(
                "invalid canonical env: MCP server '{}' env names must be sorted, unique, and match exactly",
                id
            );
        }
    }
    Ok(())
}

/// Normalize a harness id to its canonical adapter id.
/// Lowercase + trim; unknown ids return `unsupported_adapter_error`.
pub fn canonical_adapter_id(id: &str) -> Result<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        "codex" => Ok(ADAPTER_ID),
        "opencode" => Ok(OPENCODE_ADAPTER_ID),
        "claude" => Ok(CLAUDE_ADAPTER_ID),
        other => Err(unsupported_adapter_error(other)),
    }
}

/// Exact version support: membership in `supported_versions_for(adapter)`.
/// Returns false for unknown adapters (callers surface the error separately).
pub fn version_supported(adapter: &str, version: &str) -> bool {
    match supported_versions_for(adapter) {
        Ok(list) => list.contains(&version),
        Err(_) => false,
    }
}

/// Load + validate a canonical env JSON file.
/// Rejects symlinks, enforces the 64 KiB cap, parses JSON, validates.
pub fn load_canonical_file(path: &Path) -> Result<CanonicalEnv> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("invalid canonical env: cannot stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("invalid canonical env: symlinks are rejected");
    }
    if !meta.is_file() {
        anyhow::bail!("invalid canonical env: path must be a regular file");
    }
    if meta.len() > MAX_CANONICAL_ENV_BYTES {
        anyhow::bail!("invalid canonical env: file exceeds size limit");
    }
    let bytes = std::fs::read(path).context("invalid canonical env: failed to read file")?;
    if bytes.len() as u64 > MAX_CANONICAL_ENV_BYTES {
        anyhow::bail!("invalid canonical env: file exceeds size limit");
    }
    let env: CanonicalEnv =
        serde_json::from_slice(&bytes).context("invalid canonical env: failed to parse JSON")?;
    env.validate()?;
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "root-canonical-011-{}-{}-{}-{}",
                label,
                std::process::id(),
                nanos,
                id
            ));
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(&path)
                    .unwrap();
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&path).unwrap();
            }
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn minimal_env() -> CanonicalEnv {
        CanonicalEnv {
            schema_version: 1,
            source_agent: "codex".to_string(),
            source_agent_version: "0.150.1".to_string(),
            instructions: CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: vec![],
            tools: vec![],
            mcp_servers: BTreeMap::new(),
            policies: vec![],
            environment: CanonicalEnvironment { needs_env: vec![] },
            provenance: CanonicalProvenance {
                captured_at: "1234567890s-since-epoch".to_string(),
                bundle_hash: String::new(),
                disclosure: SECRET_DISCLOSURE.to_string(),
            },
        }
    }

    fn full_env() -> CanonicalEnv {
        let mut env = minimal_env();
        env.skills.push(CanonicalSkill {
            name: "docs-writer".to_string(),
            files: vec!["SKILL.md".to_string()],
        });
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "gpt-5".to_string(),
        });
        env.mcp_servers.insert(
            "github".to_string(),
            CanonicalMcp {
                id: "github".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec!["-y".to_string(), "server-package".to_string()],
                cwd: Some(".".to_string()),
                credential_refs: vec!["GITHUB_TOKEN".to_string()],
                environment: vec!["GITHUB_TOKEN".to_string()],
                enabled: false,
            },
        );
        env.environment.needs_env = vec!["GITHUB_TOKEN".to_string()];
        env.policies.push(CanonicalPolicy {
            key: "shell_environment_policy".to_string(),
            disposition: "held".to_string(),
            reason: String::new(),
        });
        env
    }

    #[test]
    fn minimal_valid_env_passes() {
        minimal_env().validate().unwrap();
        full_env().validate().unwrap();
    }

    #[test]
    fn unknown_top_level_key_rejected() {
        let mut value = serde_json::to_value(minimal_env()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("extra_key".to_string(), serde_json::json!(1));
        let err = serde_json::from_value::<CanonicalEnv>(value).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    #[test]
    fn unknown_nested_key_rejected() {
        let json = r#"{
            "schema_version": 1,
            "source_agent": "codex",
            "source_agent_version": "0.150.1",
            "instructions": {"main": "AGENTS.md", "bogus": 1},
            "skills": [], "tools": [], "mcp_servers": {},
            "policies": [],
            "environment": {"needs_env": []},
            "provenance": {"captured_at": "x", "bundle_hash": "", "disclosure": "DISC"}
        }"#;
        assert!(serde_json::from_str::<CanonicalEnv>(json).is_err());
    }

    #[test]
    fn credential_value_in_refs_rejected() {
        // Env values / non-names are not valid env names.
        for bad in [
            "sk-abc123",
            "SECRET VALUE",
            "has space",
            "lower-with-dash",
            "--token=x",
            "Bearer abc",
            "",
        ] {
            let mut env = minimal_env();
            env.mcp_servers.insert(
                "github".to_string(),
                CanonicalMcp {
                    id: "github".to_string(),
                    transport: "stdio".to_string(),
                    command: vec!["npx".to_string()],
                    args: vec![],
                    cwd: None,
                    credential_refs: vec![bad.to_string()],
                    environment: vec![bad.to_string()],
                    enabled: false,
                },
            );
            env.environment.needs_env = vec![bad.to_string()];
            assert!(
                env.validate().is_err(),
                "credential_refs should reject '{}'",
                bad
            );
        }
    }

    #[test]
    fn secret_args_rejected() {
        for bad in [
            "sk-abc123",
            "ghp_something",
            "--token=abc",
            "--api-key=xyz",
            "Bearer sometoken",
            "github_pat_abc",
            "xoxb-123",
        ] {
            let mut env = full_env();
            let entry = env.mcp_servers.get_mut("github").unwrap();
            entry.args = vec![bad.to_string()];
            assert!(env.validate().is_err(), "args should reject '{}'", bad);
        }
    }

    #[test]
    fn enabled_must_be_false() {
        let mut env = full_env();
        env.mcp_servers.get_mut("github").unwrap().enabled = true;
        let err = env.validate().unwrap_err().to_string();
        assert!(err.contains("disabled"), "got: {err}");
    }

    #[test]
    fn non_stdio_transport_rejected() {
        let mut env = full_env();
        env.mcp_servers.get_mut("github").unwrap().transport = "sse".to_string();
        assert!(env.validate().is_err());
    }

    #[test]
    fn empty_and_multi_command_rejected() {
        let mut empty = full_env();
        empty.mcp_servers.get_mut("github").unwrap().command = vec![];
        assert!(empty.validate().is_err());

        let mut two = full_env();
        two.mcp_servers.get_mut("github").unwrap().command =
            vec!["npx".to_string(), "extra".to_string()];
        assert!(two.validate().is_err());
    }

    #[test]
    fn malformed_command_parts_rejected() {
        for bad in ["", "has\nnewline", "has\0nul", "has\rcr", &"x".repeat(1025)] {
            let mut env = full_env();
            env.mcp_servers.get_mut("github").unwrap().command = vec![bad.to_string()];
            assert!(
                env.validate().is_err(),
                "command should reject len={}",
                bad.len()
            );
        }
        let mut env = full_env();
        env.mcp_servers.get_mut("github").unwrap().args = vec!["ok".to_string(), String::new()];
        assert!(env.validate().is_err());
    }

    #[test]
    fn unsorted_dupe_credential_refs_rejected() {
        // Unsorted.
        let mut env = full_env();
        {
            let entry = env.mcp_servers.get_mut("github").unwrap();
            entry.credential_refs = vec!["Z_TOKEN".to_string(), "A_TOKEN".to_string()];
            entry.environment = vec!["Z_TOKEN".to_string(), "A_TOKEN".to_string()];
        }
        env.environment.needs_env = vec!["A_TOKEN".to_string(), "Z_TOKEN".to_string()];
        assert!(env.validate().is_err());

        // Duplicate.
        let mut env = full_env();
        {
            let entry = env.mcp_servers.get_mut("github").unwrap();
            entry.credential_refs = vec!["A_TOKEN".to_string(), "A_TOKEN".to_string()];
            entry.environment = vec!["A_TOKEN".to_string(), "A_TOKEN".to_string()];
        }
        env.environment.needs_env = vec!["A_TOKEN".to_string()];
        assert!(env.validate().is_err());

        // credential_refs != environment.
        let mut env = full_env();
        {
            let entry = env.mcp_servers.get_mut("github").unwrap();
            entry.environment = vec!["OTHER_TOKEN".to_string()];
        }
        assert!(env.validate().is_err());
    }

    #[test]
    fn needs_env_must_equal_union() {
        // Extra entry.
        let mut env = full_env();
        env.environment.needs_env = vec!["EXTRA".to_string(), "GITHUB_TOKEN".to_string()];
        assert!(env.validate().is_err());

        // Missing entry.
        let env = minimal_env();
        let mut missing = full_env();
        missing.environment.needs_env = vec![];
        let _ = env;
        assert!(missing.validate().is_err());

        // Unsorted needs_env.
        let mut env = minimal_env();
        env.mcp_servers.insert(
            "a".to_string(),
            CanonicalMcp {
                id: "a".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec!["A_TOKEN".to_string()],
                environment: vec!["A_TOKEN".to_string()],
                enabled: false,
            },
        );
        env.mcp_servers.insert(
            "b".to_string(),
            CanonicalMcp {
                id: "b".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec!["Z_TOKEN".to_string()],
                environment: vec!["Z_TOKEN".to_string()],
                enabled: false,
            },
        );
        env.environment.needs_env = vec!["Z_TOKEN".to_string(), "A_TOKEN".to_string()];
        assert!(env.validate().is_err());
        env.environment.needs_env = vec!["A_TOKEN".to_string(), "Z_TOKEN".to_string()];
        env.validate().unwrap();
    }

    #[test]
    fn unknown_tool_key_rejected() {
        let mut env = minimal_env();
        env.tools.push(CanonicalTool {
            name: "notify".to_string(),
            value: "x".to_string(),
        });
        assert!(env.validate().is_err());
    }

    #[test]
    fn duplicate_tool_rejected() {
        let mut env = minimal_env();
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "a".to_string(),
        });
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "b".to_string(),
        });
        assert!(env.validate().is_err());
    }

    #[test]
    fn oversized_tool_value_rejected() {
        let mut env = minimal_env();
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "x".repeat(257),
        });
        assert!(env.validate().is_err());

        let mut env = minimal_env();
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "has\nnewline".to_string(),
        });
        assert!(env.validate().is_err());

        let mut env = minimal_env();
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "has\0nul".to_string(),
        });
        assert!(env.validate().is_err());
    }

    #[test]
    fn secret_shaped_tool_values_rejected() {
        // Every rejected shape from the Sprint 011 guard rail.
        let bad_values = [
            // Bearer prefix (case-insensitive).
            "Bearer abc123",
            "bearer xyz",
            "BEARER token-value",
            // Key prefixes (case-insensitive).
            "sk-abc123",
            "SK-ABC123",
            "ghp_something",
            "GHP_SOMETHING",
            "github_pat_abc123",
            "GITHUB_PAT_ABC123",
            "xoxb-12345",
            "xoxp-12345",
            "xoxa-12345",
            "xoxr-12345",
            // PEM blocks (case-insensitive).
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----begin pgp private key block-----",
            // URL-with-credentials.
            "https://user:pass@example.com",
            "postgres://bob:s3cret@db.internal:5432/app",
            // Assignment shapes with credential-ish keys (case-insensitive,
            // separator-insensitive).
            "token=abc123",
            "TOKEN=ABC123",
            "api_key=xyz",
            "apikey=xyz",
            "api-key=xyz",
            "auth=secrettoken",
            "password=hunter2",
            "PASSWORD=hunter2",
            "passwd=x",
            "secret=shh",
            "my-secret = value",
            "access_token: xyz",
        ];
        for bad in bad_values {
            assert!(looks_like_secret_value(bad), "guard should reject '{bad}'");
            let mut env = minimal_env();
            env.tools.push(CanonicalTool {
                name: "model".to_string(),
                value: bad.to_string(),
            });
            let err = env.validate().unwrap_err().to_string();
            assert!(
                err.contains("looks like a secret"),
                "tool value '{bad}' should fail closed, got: {err}"
            );
        }
    }

    #[test]
    fn ordinary_model_identifiers_accepted() {
        for good in [
            "gpt-5",
            "sonnet",
            "opus",
            "haiku",
            "o1",
            "gemini-2.5-flash",
            "gemini-1.5-pro",
            "high",
            "other-model",
        ] {
            assert!(
                !looks_like_secret_value(good),
                "guard must accept model identifier '{good}'"
            );
            let mut env = minimal_env();
            env.tools.push(CanonicalTool {
                name: "model".to_string(),
                value: good.to_string(),
            });
            assert!(
                env.validate().is_ok(),
                "model identifier '{good}' should validate"
            );
        }
    }

    #[test]
    fn bad_skill_name_rejected() {
        for bad in [
            "Docs-Writer",
            "-lead",
            "trail-",
            "has--dbl",
            "",
            &"a".repeat(65),
        ] {
            let mut env = minimal_env();
            env.skills.push(CanonicalSkill {
                name: bad.to_string(),
                files: vec!["SKILL.md".to_string()],
            });
            assert!(
                env.validate().is_err(),
                "skill name should reject '{}'",
                bad
            );
        }
        // Duplicate names.
        let mut env = minimal_env();
        env.skills.push(CanonicalSkill {
            name: "docs-writer".to_string(),
            files: vec!["SKILL.md".to_string()],
        });
        env.skills.push(CanonicalSkill {
            name: "docs-writer".to_string(),
            files: vec!["OTHER.md".to_string()],
        });
        assert!(env.validate().is_err());
        // Bad rel file.
        let mut env = minimal_env();
        env.skills.push(CanonicalSkill {
            name: "docs-writer".to_string(),
            files: vec!["../escape.md".to_string()],
        });
        assert!(env.validate().is_err());
    }

    #[test]
    fn bad_mcp_id_rejected() {
        let mut env = minimal_env();
        env.mcp_servers.insert(
            "../bad".to_string(),
            CanonicalMcp {
                id: "../bad".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec![],
                environment: vec![],
                enabled: false,
            },
        );
        assert!(env.validate().is_err());

        // Map key must equal entry id.
        let mut env = minimal_env();
        env.mcp_servers.insert(
            "github".to_string(),
            CanonicalMcp {
                id: "other".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec![],
                environment: vec![],
                enabled: false,
            },
        );
        assert!(env.validate().is_err());
    }

    #[test]
    fn wrong_disclosure_rejected() {
        let mut env = minimal_env();
        env.provenance.disclosure = "custom".to_string();
        assert!(env.validate().is_err());
    }

    #[test]
    fn wrong_schema_version_rejected() {
        let mut env = minimal_env();
        env.schema_version = 2;
        assert!(env.validate().is_err());
    }

    #[test]
    fn bad_source_agent_rejected() {
        let mut env = minimal_env();
        env.source_agent = "gemini".to_string();
        assert!(env.validate().is_err());
    }

    #[test]
    fn bad_provenance_rejected() {
        let mut env = minimal_env();
        env.provenance.captured_at = String::new();
        assert!(env.validate().is_err());

        let mut env = minimal_env();
        env.provenance.bundle_hash = "not-a-hash".to_string();
        assert!(env.validate().is_err());

        let mut env = minimal_env();
        env.provenance.bundle_hash = format!("sha256:{}", "a".repeat(64));
        env.validate().unwrap();

        let mut env = minimal_env();
        env.provenance.bundle_hash = format!("sha256:{}", "A".repeat(64));
        assert!(env.validate().is_err());
    }

    #[test]
    fn bad_policy_rejected() {
        let mut env = minimal_env();
        env.policies.push(CanonicalPolicy {
            key: "k".to_string(),
            disposition: "allow".to_string(),
            reason: String::new(),
        });
        assert!(env.validate().is_err());

        let mut env = minimal_env();
        env.policies.push(CanonicalPolicy {
            key: String::new(),
            disposition: "held".to_string(),
            reason: String::new(),
        });
        assert!(env.validate().is_err());
    }

    #[test]
    fn policy_reason_validated_and_backward_compatible() {
        // Malformed reasons fail closed.
        let mut env = minimal_env();
        env.policies.push(CanonicalPolicy {
            key: "mcp:github".to_string(),
            disposition: "held".to_string(),
            reason: "has\nnewline".to_string(),
        });
        assert!(env.validate().is_err());

        // Pre-Sprint-011 canonical files without `reason` still parse.
        let legacy = serde_json::json!({
            "schema_version": 1,
            "source_agent": "codex",
            "source_agent_version": "0.150.1",
            "instructions": {"main": "AGENTS.md"},
            "skills": [], "tools": [], "mcp_servers": {},
            "policies": [{"key": "mcp:github", "disposition": "held"}],
            "environment": {"needs_env": []},
            "provenance": {
                "captured_at": "1234567890",
                "bundle_hash": "",
                "disclosure": SECRET_DISCLOSURE
            }
        });
        let parsed: CanonicalEnv = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.policies[0].reason, "");
        parsed.validate().unwrap();
    }

    #[test]
    fn env_hash_stability() {
        let env = full_env();
        let h1 = env.env_hash().unwrap();
        let h2 = env.env_hash().unwrap();
        assert_eq!(h1, h2);
        let mut changed = env.clone();
        changed.tools[0].value = "other-model".to_string();
        let h3 = changed.env_hash().unwrap();
        assert_ne!(h1, h3);
    }

    #[test]
    fn load_canonical_file_round_trip() {
        let _env = crate::lock_env();
        let tmp = TempDir::new("roundtrip");
        let path = tmp.path().join("env.json");
        let env = full_env();
        let bytes = serde_json::to_vec_pretty(&env).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let loaded = load_canonical_file(&path).unwrap();
        assert_eq!(loaded, env);
    }

    #[cfg(unix)]
    #[test]
    fn load_canonical_file_symlink_rejected() {
        let _env = crate::lock_env();
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new("symlink");
        let target = tmp.path().join("env.json");
        std::fs::write(&target, serde_json::to_vec(&minimal_env()).unwrap()).unwrap();
        let link = tmp.path().join("link.json");
        symlink(&target, &link).unwrap();
        assert!(load_canonical_file(&link).is_err());
    }

    #[test]
    fn load_canonical_file_oversize_rejected() {
        let _env = crate::lock_env();
        let tmp = TempDir::new("oversize");
        let path = tmp.path().join("big.json");
        let big = vec![b'x'; (MAX_CANONICAL_ENV_BYTES + 1) as usize];
        std::fs::write(&path, &big).unwrap();
        assert!(load_canonical_file(&path).is_err());
    }

    #[test]
    fn version_supported_gates() {
        assert!(version_supported("codex", "0.150.1"));
        assert!(!version_supported("codex", "1.18.27"));
        assert!(!version_supported("codex", "0.999.0"));
        assert!(version_supported("opencode", "1.18.27"));
        assert!(!version_supported("opencode", "0.150.1"));
        assert!(version_supported("claude", "2.1.260"));
        assert!(!version_supported("claude", "2.1.259"));
        assert!(!version_supported("gemini", "1.0.0"));
    }

    #[test]
    fn canonical_adapter_id_mapping() {
        assert_eq!(canonical_adapter_id("codex").unwrap(), ADAPTER_ID);
        assert_eq!(
            canonical_adapter_id("opencode").unwrap(),
            OPENCODE_ADAPTER_ID
        );
        assert_eq!(canonical_adapter_id("claude").unwrap(), CLAUDE_ADAPTER_ID);
        assert_eq!(canonical_adapter_id(" Codex ").unwrap(), ADAPTER_ID);
        assert_eq!(
            canonical_adapter_id("OPencode").unwrap(),
            OPENCODE_ADAPTER_ID
        );
        assert_eq!(canonical_adapter_id("Claude\n").unwrap(), CLAUDE_ADAPTER_ID);
        assert!(canonical_adapter_id("gemini").is_err());
    }
}
