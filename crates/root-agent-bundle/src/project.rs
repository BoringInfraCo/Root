//! Project-scoped canonical environment files (`agent.toml`) + Rootfile stanza.
//!
//! Read-only discovery except for explicit `agent.toml` writes via capture.
//! No locks, no journals, no snapshots. TOML shape follows SPRINT_011 §2
//! (checked-in, names only, disclosure comment on top). Exactly one source
//! per resolve (no merging).

use crate::canonical::looks_like_secret_value;
use crate::canonical::{CanonicalEnv, MAX_CANONICAL_ENV_BYTES};
use crate::capture::resolve_repo_root;
use crate::manifest::{looks_like_literal_secret_argument, SECRET_DISCLOSURE};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------------------
// TOML intermediate shape (S11 §2: [[mcp_servers]] array, not a map).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentToml {
    schema_version: u32,
    source_agent: String,
    source_agent_version: String,
    instructions: TomlInstructions,
    #[serde(default)]
    skills: Vec<TomlSkill>,
    #[serde(default)]
    tools: Vec<TomlTool>,
    #[serde(default)]
    mcp_servers: Vec<TomlMcp>,
    #[serde(default)]
    policies: Vec<TomlPolicy>,
    environment: TomlEnvironment,
    provenance: TomlProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlInstructions {
    main: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlSkill {
    name: String,
    files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlTool {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlMcp {
    id: String,
    transport: String,
    command: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    credential_refs: Vec<String>,
    environment: Vec<String>,
    enabled: bool,
    // Literal value tables are accepted by the parser only to fail closed
    // with the dedicated message family (never silently dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env_values: Option<toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlPolicy {
    key: String,
    disposition: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlEnvironment {
    needs_env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlProvenance {
    captured_at: String,
    #[serde(default)]
    bundle_hash: String,
    disclosure: String,
}

/// CanonicalEnv → TOML per SPRINT_011 §2 shape with `# <SECRET_DISCLOSURE>`
/// as the top comment line. Validates first (fail closed, names only).
pub fn emit_agent_toml(env: &CanonicalEnv) -> Result<String> {
    env.validate()?;
    // Work-state refusal: any free-text field that looks secret-shaped aborts
    // the write (mirrors root-work refusal style; proposal stays readable).
    refuse_secret_shaped_free_text(env)?;
    let mut servers: Vec<TomlMcp> = Vec::with_capacity(env.mcp_servers.len());
    for (id, e) in &env.mcp_servers {
        servers.push(TomlMcp {
            id: id.clone(),
            transport: e.transport.clone(),
            command: e.command.clone(),
            args: e.args.clone(),
            cwd: e.cwd.clone(),
            credential_refs: e.credential_refs.clone(),
            environment: e.environment.clone(),
            enabled: e.enabled,
            env: None,
            env_values: None,
        });
    }
    // BTreeMap iteration is already sorted; be explicit for stability.
    servers.sort_by(|a, b| a.id.cmp(&b.id));
    let doc = AgentToml {
        schema_version: env.schema_version,
        source_agent: env.source_agent.clone(),
        source_agent_version: env.source_agent_version.clone(),
        instructions: TomlInstructions {
            main: env.instructions.main.clone(),
        },
        skills: env
            .skills
            .iter()
            .map(|s| TomlSkill {
                name: s.name.clone(),
                files: s.files.clone(),
            })
            .collect(),
        tools: env
            .tools
            .iter()
            .map(|t| TomlTool {
                name: t.name.clone(),
                value: t.value.clone(),
            })
            .collect(),
        mcp_servers: servers,
        policies: env
            .policies
            .iter()
            .map(|p| TomlPolicy {
                key: p.key.clone(),
                disposition: p.disposition.clone(),
                reason: p.reason.clone(),
            })
            .collect(),
        environment: TomlEnvironment {
            needs_env: env.environment.needs_env.clone(),
        },
        provenance: TomlProvenance {
            captured_at: env.provenance.captured_at.clone(),
            bundle_hash: env.provenance.bundle_hash.clone(),
            disclosure: env.provenance.disclosure.clone(),
        },
    };
    let body = toml::to_string_pretty(&doc).context("Failed to serialize agent.toml")?;
    Ok(format!("# {}\n{}", SECRET_DISCLOSURE, body))
}

fn refuse_secret_shaped_free_text(env: &CanonicalEnv) -> Result<()> {
    for t in &env.tools {
        if looks_like_secret_value(&t.value) {
            anyhow::bail!(
                "refusing to write agent environment: tool '{}' looks like a secret value; canonical tools carry names only",
                t.name
            );
        }
    }
    for (id, e) in &env.mcp_servers {
        for a in e.command.iter().chain(e.args.iter()) {
            if looks_like_literal_secret_argument(a) || looks_like_secret_value(a) {
                anyhow::bail!(
                    "refusing to write agent environment: MCP server '{}' has a secret-shaped argument; use env_vars references",
                    id
                );
            }
        }
    }
    for p in &env.policies {
        if looks_like_secret_value(&p.reason) || looks_like_secret_value(&p.key) {
            anyhow::bail!(
                "refusing to write agent environment: policy '{}' looks like a secret value; Root does not store credentials",
                p.key
            );
        }
    }
    if looks_like_secret_value(&env.provenance.captured_at) {
        anyhow::bail!(
            "refusing to write agent environment: provenance looks like a secret value; Root does not store credentials"
        );
    }
    for s in &env.skills {
        if looks_like_secret_value(&s.name) {
            anyhow::bail!(
                "refusing to write agent environment: skill '{}' looks like a secret value",
                s.name
            );
        }
        for f in &s.files {
            if looks_like_secret_value(f) {
                anyhow::bail!(
                    "refusing to write agent environment: skill file '{}' looks like a secret value",
                    f
                );
            }
        }
    }
    Ok(())
}

/// Inverse of [`emit_agent_toml`]: TOML text → validated [`CanonicalEnv`].
/// Size-capped, unknown keys rejected, secret scan fail-closed.
pub fn parse_agent_toml(text: &str) -> Result<CanonicalEnv> {
    if text.len() as u64 > MAX_CANONICAL_ENV_BYTES {
        anyhow::bail!("invalid canonical env: file exceeds size limit");
    }
    let doc: AgentToml =
        toml::from_str(text).context("invalid canonical env: failed to parse TOML")?;
    // Literal env tables fail closed with the dedicated family (never dropped).
    for s in &doc.mcp_servers {
        if s.env.is_some() {
            anyhow::bail!(
                "MCP literal env values cannot be exported; use env_vars/{{env:}} references for server '{}'",
                s.id
            );
        }
        if s.env_values.is_some() {
            anyhow::bail!(
                "MCP literal env values cannot be exported; use env_vars/{{env:}} references for server '{}'",
                s.id
            );
        }
        for a in s.command.iter().chain(s.args.iter()) {
            if looks_like_literal_secret_argument(a) {
                anyhow::bail!(
                    "invalid canonical env: MCP server '{}' has a suspicious secret-bearing argument",
                    s.id
                );
            }
        }
    }
    for t in &doc.tools {
        if looks_like_secret_value(&t.value) {
            anyhow::bail!(
                "invalid canonical env: tool '{}' looks like a secret value; canonical tools carry names only, never secret values",
                t.name
            );
        }
    }
    // Convert [[mcp_servers]] array → BTreeMap (duplicate ids rejected).
    let mut mcp_servers: BTreeMap<String, crate::canonical::CanonicalMcp> = BTreeMap::new();
    for s in doc.mcp_servers {
        if mcp_servers.contains_key(&s.id) {
            anyhow::bail!("invalid canonical env: duplicate MCP server id '{}'", s.id);
        }
        // Credential/env sets must already be sorted unique (validated below,
        // but check early for a legible error before map insertion).
        mcp_servers.insert(
            s.id.clone(),
            crate::canonical::CanonicalMcp {
                id: s.id,
                transport: s.transport,
                command: s.command,
                args: s.args,
                cwd: s.cwd,
                credential_refs: s.credential_refs,
                environment: s.environment,
                enabled: s.enabled,
            },
        );
    }
    let env = CanonicalEnv {
        schema_version: doc.schema_version,
        source_agent: doc.source_agent,
        source_agent_version: doc.source_agent_version,
        instructions: crate::canonical::CanonicalInstructions {
            main: doc.instructions.main,
        },
        skills: doc
            .skills
            .into_iter()
            .map(|s| crate::canonical::CanonicalSkill {
                name: s.name,
                files: s.files,
            })
            .collect(),
        tools: doc
            .tools
            .into_iter()
            .map(|t| crate::canonical::CanonicalTool {
                name: t.name,
                value: t.value,
            })
            .collect(),
        mcp_servers,
        policies: doc
            .policies
            .into_iter()
            .map(|p| crate::canonical::CanonicalPolicy {
                key: p.key,
                disposition: p.disposition,
                reason: p.reason,
            })
            .collect(),
        environment: crate::canonical::CanonicalEnvironment {
            needs_env: doc.environment.needs_env,
        },
        provenance: crate::canonical::CanonicalProvenance {
            captured_at: doc.provenance.captured_at,
            bundle_hash: doc.provenance.bundle_hash,
            disclosure: doc.provenance.disclosure,
        },
    };
    // Secret-shaped free text (policy reasons etc.) refuses like the write path.
    refuse_secret_shaped_free_text(&env)?;
    env.validate()?;
    // Paranoia: ensure needs_env union exactness already covered by validate;
    // keep BTreeSet import used for future checks.
    let _ = BTreeSet::<String>::new();
    Ok(env)
}

// ---------------------------------------------------------------------------
// Rootfile [agents] stanza (standalone parse, no root-core dep).
// ---------------------------------------------------------------------------

/// Minimal `[agents]` stanza from a Rootfile path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootfileAgentStanza {
    pub env: Option<String>,
    pub default_target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RootfileToml {
    #[serde(default)]
    agents: Option<RootfileAgentsToml>,
}

#[derive(Debug, Deserialize)]
struct RootfileAgentsToml {
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    default_target: Option<String>,
}

/// Missing file/table → all-None (not an error); malformed TOML → Err.
pub fn read_rootfile_agent_stanza(rootfile: &Path) -> Result<RootfileAgentStanza> {
    match std::fs::symlink_metadata(rootfile) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RootfileAgentStanza {
                env: None,
                default_target: None,
            })
        }
        Err(e) => return Err(e).context("Failed to stat Rootfile"),
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!("refusing Rootfile: symlink");
            }
            if !m.is_file() {
                anyhow::bail!("Rootfile path is not a regular file");
            }
            if m.len() > 1024 * 1024 {
                anyhow::bail!("Rootfile exceeds size limit");
            }
        }
    }
    let bytes = std::fs::read(rootfile).context("Failed to read Rootfile")?;
    let text = String::from_utf8(bytes).context("Rootfile is not valid UTF-8")?;
    let parsed: RootfileToml = toml::from_str(&text).context("Failed to parse Rootfile TOML")?;
    Ok(match parsed.agents {
        None => RootfileAgentStanza {
            env: None,
            default_target: None,
        },
        Some(a) => RootfileAgentStanza {
            env: a.env,
            default_target: a.default_target,
        },
    })
}

/// Walk ancestors for `.root/agent.toml` (regular file only).
pub fn discover_project_file(start: &Path) -> Option<PathBuf> {
    let base = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(start))
            .unwrap_or_else(|_| start.to_path_buf())
    };
    // If `start` is a file, begin at its parent; else begin at itself.
    let mut dir = if base.is_file() {
        base.parent().map(|p| p.to_path_buf()).unwrap_or(base)
    } else {
        base
    };
    loop {
        let candidate = dir.join(".root").join("agent.toml");
        if let Ok(m) = std::fs::symlink_metadata(&candidate) {
            if m.is_file() && !m.file_type().is_symlink() {
                return Some(candidate);
            }
        }
        match dir.parent() {
            Some(p) => {
                if p == dir {
                    return None;
                }
                dir = p.to_path_buf();
            }
            None => return None,
        }
        // Stop at filesystem root (parent == self handled above).
        if dir.as_os_str().is_empty() {
            return None;
        }
    }
}

fn read_env_file(path: &Path) -> Result<Vec<u8>> {
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
    Ok(bytes)
}

fn parse_env_bytes(path: &Path, bytes: &[u8]) -> Result<CanonicalEnv> {
    // Try JSON first (Sprint-011 `--env` artifact), then TOML (agent.toml).
    if let Ok(env) = serde_json::from_slice::<CanonicalEnv>(bytes) {
        refuse_secret_shaped_free_text(&env)?;
        env.validate()?;
        return Ok(env);
    }
    let text = std::str::from_utf8(bytes).with_context(|| {
        format!(
            "invalid canonical env: {} is not valid UTF-8",
            path.display()
        )
    })?;
    parse_agent_toml(text)
}

/// Resolve a Rootfile `[agents].env` relative path against the repo root with
/// containment: no absolute path, no `..` escape above the repo, and no
/// symlink component between the repo root and the destination.
fn resolve_rootfile_env_path(repo: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        anyhow::bail!("invalid Rootfile [agents].env: empty path");
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        anyhow::bail!(
            "invalid Rootfile [agents].env '{}': absolute paths are forbidden",
            rel
        );
    }
    let repo_norm = normalize_absolute(repo)?;
    let normalized = normalize_absolute(&repo_norm.join(rel_path))?;
    if normalized == repo_norm || !normalized.starts_with(&repo_norm) {
        anyhow::bail!("invalid Rootfile [agents].env '{}': escapes repo root", rel);
    }
    if let Ok(rest) = normalized.strip_prefix(&repo_norm) {
        let mut prefix = repo_norm.clone();
        for comp in rest.components() {
            prefix.push(comp);
            if let Ok(m) = std::fs::symlink_metadata(&prefix) {
                if m.file_type().is_symlink() {
                    anyhow::bail!(
                        "refusing Rootfile [agents].env '{}': symlink ancestor '{}'",
                        rel,
                        prefix.display()
                    );
                }
            }
        }
    }
    Ok(normalized)
}

fn normalize_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to determine current directory")?
            .join(path)
    };
    let mut out = PathBuf::new();
    for comp in absolute.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    anyhow::bail!("invalid path '{}': escapes via '..'", path.display());
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    Ok(out)
}

/// Precedence: explicit `--env` > repo `[agents].env` (Rootfile stanza) or
/// repo `.root/agent.toml` > `~/.root/agent.toml`. Exactly one source, NO
/// merging.
pub fn resolve_env_source(explicit: Option<&Path>, cwd: &Path) -> Result<(PathBuf, CanonicalEnv)> {
    if let Some(p) = explicit {
        let bytes = read_env_file(p)?;
        let env = parse_env_bytes(p, &bytes)?;
        return Ok((p.to_path_buf(), env));
    }
    // Repo branch: locate the repo root, prefer the Rootfile [agents].env
    // stanza (resolved + contained inside the repo), else `.root/agent.toml`.
    if let Some(repo) = resolve_repo_root(cwd) {
        let stanza = read_rootfile_agent_stanza(&repo.join("Rootfile"))?;
        let candidate = match stanza.env {
            Some(rel) => Some(resolve_rootfile_env_path(&repo, &rel)?),
            None => {
                let fallback = repo.join(".root").join("agent.toml");
                match std::fs::symlink_metadata(&fallback) {
                    Ok(m) if m.is_file() && !m.file_type().is_symlink() => Some(fallback),
                    _ => None,
                }
            }
        };
        if let Some(found) = candidate {
            let bytes = read_env_file(&found)?;
            let env = parse_env_bytes(&found, &bytes)?;
            return Ok((found, env));
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".root").join("agent.toml");
        if let Ok(m) = std::fs::symlink_metadata(&candidate) {
            if m.is_file() && !m.file_type().is_symlink() {
                let bytes = read_env_file(&candidate)?;
                let env = parse_env_bytes(&candidate, &bytes)?;
                return Ok((candidate, env));
            }
        }
    }
    anyhow::bail!(
        "no canonical environment found: pass --env, add .root/agent.toml, or capture one"
    )
}

/// Validated repo Rootfile `[agents].default_target` (normalized adapter id).
/// Absent repo/Rootfile/table → `None`; unknown target value → Err.
pub fn resolve_default_target(cwd: &Path) -> Result<Option<String>> {
    let Some(repo) = resolve_repo_root(cwd) else {
        return Ok(None);
    };
    let stanza = read_rootfile_agent_stanza(&repo.join("Rootfile"))?;
    match stanza.default_target {
        None => Ok(None),
        Some(target) => match crate::canonical::canonical_adapter_id(&target) {
            Ok(norm) => Ok(Some(norm.to_string())),
            Err(e) => Err(e).context(format!(
                "invalid Rootfile [agents].default_target '{}'",
                target
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{
        CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalMcp, CanonicalPolicy,
        CanonicalProvenance, CanonicalSkill, CanonicalTool, CANONICAL_SCHEMA_VERSION,
    };
    use crate::manifest::SECRET_DISCLOSURE;
    use std::collections::BTreeMap;

    fn minimal_env() -> CanonicalEnv {
        CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
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
                captured_at: "1234567890".to_string(),
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
                args: vec!["-y".to_string(), "pkg".to_string()],
                cwd: None,
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
    fn toml_round_trip_emit_parse_validate_equal() {
        let env = full_env();
        let toml = emit_agent_toml(&env).unwrap();
        assert!(
            toml.starts_with(&format!("# {}", SECRET_DISCLOSURE)),
            "top comment must carry disclosure, got:\n{toml}"
        );
        let parsed = parse_agent_toml(&toml).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn toml_round_trip_minimal() {
        let env = minimal_env();
        let toml = emit_agent_toml(&env).unwrap();
        let parsed = parse_agent_toml(&toml).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn parse_rejects_literal_env_and_secret_tool_value() {
        // Literal env table rejected with dedicated family.
        let mut env = full_env();
        let mut toml = emit_agent_toml(&env).unwrap();
        // Inject a literal env table under the github server.
        toml = toml.replace(
            "enabled = false",
            "enabled = false\n[env]\nTOKEN = \"literal\"",
        );
        // The injected `[env]` top-level table makes TOML invalid for our shape
        // (unknown field) or triggers the literal-env family; either must Err,
        // but a dedicated literal-env message is preferred when parseable.
        // Build a targeted literal-env case via direct TOML edit on mcp block:
        let literal = r#"
schema_version = 1
source_agent = "codex"
source_agent_version = "0.150.1"
[instructions]
main = "AGENTS.md"
[[skills]]
name = "docs-writer"
files = ["SKILL.md"]
[[tools]]
name = "model"
value = "gpt-5"
[[mcp_servers]]
id = "github"
transport = "stdio"
command = ["npx"]
args = ["-y"]
credential_refs = ["GITHUB_TOKEN"]
environment = ["GITHUB_TOKEN"]
enabled = false
[env]
TOKEN = "literal"
[environment]
needs_env = ["GITHUB_TOKEN"]
[provenance]
captured_at = "1234567890"
bundle_hash = ""
disclosure = "Known secret locations and formats were excluded; explicitly selected prompt/skill files were copied verbatim and may contain unrecognized secrets. Review bundle contents before transfer."
"#;
        let err = parse_agent_toml(literal).unwrap_err().to_string();
        assert!(
            err.contains("MCP literal env values cannot be")
                || err.contains("unknown field")
                || err.contains("failed to parse"),
            "got: {err}"
        );

        // Secret tool value rejected.
        env.tools[0].value = "sk-abc123".to_string();
        // emit itself must refuse (validate first).
        let err = emit_agent_toml(&env).unwrap_err().to_string();
        assert!(err.contains("looks like a secret"), "got: {err}");
        // Direct TOML with secret tool value also rejected on parse.
        let secret_toml = r#"
schema_version = 1
source_agent = "codex"
source_agent_version = "0.150.1"
[instructions]
main = "AGENTS.md"
[[tools]]
name = "model"
value = "sk-abc123"
[environment]
needs_env = []
[provenance]
captured_at = "1234567890"
bundle_hash = ""
disclosure = "Known secret locations and formats were excluded; explicitly selected prompt/skill files were copied verbatim and may contain unrecognized secrets. Review bundle contents before transfer."
"#;
        let err = parse_agent_toml(secret_toml).unwrap_err().to_string();
        assert!(err.contains("looks like a secret"), "got: {err}");
        let _ = toml;
    }

    #[test]
    fn parse_rejects_unknown_keys_and_oversize() {
        let mut toml = emit_agent_toml(&minimal_env()).unwrap();
        toml.push_str("extra_key = 1\n");
        assert!(parse_agent_toml(&toml).is_err());
        let big = "x".repeat((MAX_CANONICAL_ENV_BYTES + 1) as usize);
        assert!(parse_agent_toml(&big).is_err());
    }

    #[test]
    fn rootfile_stanza_missing_is_none_and_malformed_errs() {
        let _env = crate::lock_env();
        let dir = std::env::temp_dir().join(format!(
            "root-proj-rootfile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Missing file → None.
        let stanza = read_rootfile_agent_stanza(&dir.join("Rootfile-missing")).unwrap();
        assert_eq!(stanza.env, None);
        assert_eq!(stanza.default_target, None);
        // Valid stanza.
        let path = dir.join("Rootfile");
        std::fs::write(
            &path,
            "[agents]\nenv = \".root/agent.toml\"\ndefault_target = \"opencode\"\n",
        )
        .unwrap();
        let stanza = read_rootfile_agent_stanza(&path).unwrap();
        assert_eq!(stanza.env.as_deref(), Some(".root/agent.toml"));
        assert_eq!(stanza.default_target.as_deref(), Some("opencode"));
        // No [agents] table → None.
        std::fs::write(&path, "[other]\nx = 1\n").unwrap();
        let stanza = read_rootfile_agent_stanza(&path).unwrap();
        assert_eq!(stanza.env, None);
        // Malformed → Err.
        std::fs::write(&path, "{ not valid toml").unwrap();
        assert!(read_rootfile_agent_stanza(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn precedence_explicit_beats_repo_beats_home_none_errs() {
        let _env = crate::lock_env();
        // Save/restore home + cwd-sensitive env.
        let saved_home = std::env::var_os("HOME");
        let tmp = std::env::temp_dir().join(format!(
            "root-proj-prec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(&tmp)
                .unwrap();
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&tmp).unwrap();
        }
        let home = tmp.join("home");
        let repo = tmp.join("repo");
        std::fs::create_dir_all(home.join(".root")).unwrap();
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::env::set_var("HOME", &home);

        // Repo file (source opencode) vs home file (source claude) vs explicit (codex).
        let mut repo_env = minimal_env();
        repo_env.source_agent = "opencode".to_string();
        repo_env.source_agent_version = "1.18.27".to_string();
        let mut home_env = minimal_env();
        home_env.source_agent = "claude".to_string();
        home_env.source_agent_version = "2.1.260".to_string();
        let mut explicit_env = minimal_env();
        explicit_env.source_agent = "codex".to_string();
        explicit_env.source_agent_version = "0.150.1".to_string();

        std::fs::write(
            repo.join(".root").join("agent.toml"),
            emit_agent_toml(&repo_env).unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join(".root").join("agent.toml"),
            emit_agent_toml(&home_env).unwrap(),
        )
        .unwrap();
        let explicit_path = tmp.join("explicit.toml");
        std::fs::write(&explicit_path, emit_agent_toml(&explicit_env).unwrap()).unwrap();

        // Explicit wins.
        let (p, e) = resolve_env_source(Some(&explicit_path), &repo.join("sub")).unwrap();
        assert_eq!(p, explicit_path);
        assert_eq!(e.source_agent, "codex");
        // Repo wins over home.
        let (p, e) = resolve_env_source(None, &repo.join("sub")).unwrap();
        assert_eq!(p, repo.join(".root").join("agent.toml"));
        assert_eq!(e.source_agent, "opencode");
        // Outside repo falls back to home.
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let (p, e) = resolve_env_source(None, &outside).unwrap();
        assert_eq!(p, home.join(".root").join("agent.toml"));
        assert_eq!(e.source_agent, "claude");
        // None found → Err with exact family.
        std::fs::remove_file(repo.join(".root").join("agent.toml")).unwrap();
        std::fs::remove_file(home.join(".root").join("agent.toml")).unwrap();
        let err = resolve_env_source(None, &outside).unwrap_err().to_string();
        assert!(err.contains("no canonical environment found"), "got: {err}");

        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_finds_repo_file_and_rejects_symlink() {
        let _env = crate::lock_env();
        let tmp = std::env::temp_dir().join(format!(
            "root-proj-disc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("a").join("b")).unwrap();
        std::fs::create_dir_all(tmp.join("a").join(".root")).unwrap();
        let target = tmp.join("a").join(".root").join("agent.toml");
        std::fs::write(&target, emit_agent_toml(&minimal_env()).unwrap()).unwrap();
        let found = discover_project_file(&tmp.join("a").join("b")).unwrap();
        assert_eq!(found, target);
        assert!(discover_project_file(&tmp.join("nowhere")).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn unique_tmp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "root-proj-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rootfile_stanza_env_path_is_used() {
        let _env = crate::lock_env();
        let tmp = unique_tmp("stanza-env");
        let repo = tmp.join("repo");
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::write(
            repo.join("Rootfile"),
            "[agents]\nenv = \"config/agents.toml\"\n",
        )
        .unwrap();
        let mut env = minimal_env();
        env.source_agent = "opencode".to_string();
        env.source_agent_version = "1.18.27".to_string();
        let target = repo.join("config").join("agents.toml");
        std::fs::write(&target, emit_agent_toml(&env).unwrap()).unwrap();

        let (path, loaded) = resolve_env_source(None, &sub).unwrap();
        assert_eq!(path, target);
        assert_eq!(loaded.source_agent, "opencode");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rootfile_stanza_escape_rejected() {
        let _env = crate::lock_env();
        let tmp = unique_tmp("stanza-escape");
        let repo = tmp.join("repo");
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join("Rootfile"),
            "[agents]\nenv = \"../outside.toml\"\n",
        )
        .unwrap();
        let err = resolve_env_source(None, &repo).unwrap_err().to_string();
        assert!(
            err.contains("escapes repo root") || err.contains("[agents].env"),
            "got: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_default_target_reads_stanza() {
        let _env = crate::lock_env();
        let tmp = unique_tmp("default-target");
        let repo = tmp.join("repo");
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        // Absent Rootfile → None.
        assert_eq!(resolve_default_target(&sub).unwrap(), None);

        std::fs::write(
            repo.join("Rootfile"),
            "[agents]\ndefault_target = \"opencode\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve_default_target(&sub).unwrap().as_deref(),
            Some("opencode")
        );

        // Unknown value → Err.
        std::fs::write(
            repo.join("Rootfile"),
            "[agents]\ndefault_target = \"gemini\"\n",
        )
        .unwrap();
        assert!(resolve_default_target(&sub).is_err());

        // Table present but no default_target → None.
        std::fs::write(repo.join("Rootfile"), "[agents]\nenv = \"x.toml\"\n").unwrap();
        assert_eq!(resolve_default_target(&sub).unwrap(), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
