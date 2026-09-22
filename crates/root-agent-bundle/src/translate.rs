//! Cross-harness translation engine (Sprint 011 part 2).
//!
//! Read-only: never writes files (outside hermetic test temp dirs), never
//! acquires [`crate::lock::GlobalMutationLock`], never mutates live config.
//! Probes reuse the existing capped (`4096` bytes / `10`s) version probes;
//! secret values are never read, logged, or stored (names only). Claude MCP
//! is always `Unsupported` with the exact
//! [`crate::claude::CLAUDE_MCP_HELD_ERROR`] sentinel.

use crate::canonical::{
    canonical_adapter_id, version_supported, CanonicalEnv, CanonicalEnvironment,
    CanonicalInstructions, CanonicalMcp, CanonicalPolicy, CanonicalProvenance, CanonicalSkill,
    CanonicalTool, CANONICAL_SCHEMA_VERSION,
};
use crate::manifest::{
    allowed_settings_for, mcp_approval_target, mcp_command_hash, supported_versions_for,
    unsupported_adapter_error, SECRET_DISCLOSURE,
};
use crate::plan::{target_precondition, ApprovalOut, HeldOut};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Public verdicts / reports (CLI worker builds on these — do not rename).
// ---------------------------------------------------------------------------

/// Per-item verdict (informational taxonomy; reports carry categorized lists).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Portable,
    RequiresReview,
    Unsupported,
    SecretsRequired,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    pub item: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranslationPlan {
    pub from: String,
    pub to: String,
    pub source_version: String,
    pub source_supported: bool,
    pub target_version: Option<String>,
    pub target_supported: bool,
    /// Version/support warnings (Sprint 011, additive). Human rendering is
    /// done by the caller; this is data only.
    pub warnings: Vec<String>,
    pub plan_hash: String,
    pub portable: Vec<String>,
    pub requires_review: Vec<ReviewItem>,
    pub unsupported: Vec<ReviewItem>,
    pub secrets_required: Vec<String>,
    pub needs_approval: Vec<crate::plan::ApprovalOut>,
    pub held: Vec<crate::plan::HeldOut>,
    pub mutated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranslationDiff {
    pub a: String,
    pub b: String,
    /// Version/support visibility (Sprint 011, additive).
    pub a_version: Option<String>,
    pub a_supported: bool,
    pub b_version: Option<String>,
    pub b_supported: bool,
    /// Version/support warnings (Sprint 011, additive). Human rendering is
    /// done by the caller; this is data only.
    pub warnings: Vec<String>,
    pub portable: Vec<String>,
    pub requires_review: Vec<ReviewItem>,
    pub unsupported: Vec<ReviewItem>,
    pub secrets_required: Vec<String>,
    pub held: Vec<crate::plan::HeldOut>,
    pub mutated: bool,
}

// ---------------------------------------------------------------------------
// Small helpers (read-only).
// ---------------------------------------------------------------------------

fn harness_main(adapter: &str) -> &'static str {
    match adapter {
        crate::manifest::CLAUDE_ADAPTER_ID => "CLAUDE.md",
        _ => "AGENTS.md",
    }
}

fn harness_scope_str(adapter: &str) -> &'static str {
    match adapter {
        crate::manifest::ADAPTER_ID => "codex_home",
        crate::manifest::OPENCODE_ADAPTER_ID => "opencode_home",
        crate::manifest::CLAUDE_ADAPTER_ID => "claude_home",
        _ => "unknown_home",
    }
}

fn seconds_since_epoch() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn valid_skill_name_local(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.starts_with('-') || name.ends_with('-') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.contains("--")
}

/// Version/support warnings (data only; human rendering is done by the
/// caller). `side` is `source`/`target` for plans and `a`/`b` for diffs.
/// Supported versions emit nothing; present-but-ungated versions and absent
/// binaries each emit one warning naming the supported exact versions and
/// the live version (or its absence).
fn version_warnings_for(side: &str, adapter: &str, live: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(supported) = supported_versions_for(adapter) else {
        return out;
    };
    match live {
        Some(v) if supported.contains(&v) => {}
        Some(v) => out.push(format!(
            "{side} {adapter} version '{v}' is not in supported exact versions {supported:?}; portable items demoted to Requires review"
        )),
        None => out.push(format!(
            "{side} {adapter} binary absent; live version unknown; supported exact versions {supported:?}; version-gated items held for review"
        )),
    }
    out
}

/// Single-line, capped copy of a sanitizer error for `CanonicalPolicy.reason`
/// (which must stay short plain text so canonical validation keeps passing).
fn sanitize_reason(e: &anyhow::Error) -> String {
    let flat: String = format!("{e:#}")
        .chars()
        .map(|c| match c {
            '\0' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect();
    let trimmed = flat.trim();
    if trimmed.is_empty() {
        return "sanitizer refused entry".to_string();
    }
    const MAX_REASON_CHARS: usize = 512;
    if trimmed.chars().count() > MAX_REASON_CHARS {
        trimmed.chars().take(MAX_REASON_CHARS).collect()
    } else {
        trimmed.to_string()
    }
}

/// Secret-location held entries (names only, never values). Read-only.
/// Avoids the literal `.claude.json` filename so plan-to-claude output never
/// contains a `.claude.json` target string; the global-state file is described
/// without its dotfile literal.
fn secret_held_for(adapter: &str) -> Vec<HeldOut> {
    match adapter {
        crate::manifest::ADAPTER_ID => vec![
            HeldOut {
                source: "auth.json".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "sessions/, *.sqlite*, history.jsonl, cache/, logs/".to_string(),
                reason: "volatile: never inspected or exported".to_string(),
            },
        ],
        crate::manifest::OPENCODE_ADAPTER_ID => vec![
            HeldOut {
                source: "auth tokens, mcp-auth.json".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "sessions/, *.sqlite*, logs/, share state".to_string(),
                reason: "volatile: never inspected or exported".to_string(),
            },
        ],
        crate::manifest::CLAUDE_ADAPTER_ID => vec![
            HeldOut {
                source: ".credentials.json, macOS Keychain, ANTHROPIC_* tokens".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "claude global state (sign-in, OAuth, projects, MCP)".to_string(),
                reason: crate::claude::CLAUDE_MCP_HELD_ERROR.to_string(),
            },
        ],
        _ => vec![],
    }
}

fn skill_held_for(skills: &[CanonicalSkill]) -> Vec<HeldOut> {
    skills
        .iter()
        .map(|s| HeldOut {
            source: format!("skill:{}", s.name),
            reason:
                "skill content requires hash-bound approval at apply; content-approval deferred"
                    .to_string(),
        })
        .collect()
}

/// Live target version probe (read-only, capped via existing probe fns).
/// Absent binary → `None`, not an error. Present-but-unparseable → `None`.
fn probe_target_version(adapter: &str) -> Option<String> {
    match adapter {
        crate::manifest::ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::codex::CODEX_BINARY)?;
            crate::codex::probe_codex_version(&bin).ok()
        }
        crate::manifest::OPENCODE_ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::opencode::OPENCODE_BINARY)?;
            crate::opencode::probe_opencode_version(&bin).ok()
        }
        crate::manifest::CLAUDE_ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::claude::CLAUDE_BINARY)?;
            crate::claude::probe_claude_version(&bin).ok()
        }
        _ => None,
    }
}

/// Filesystem render targets for `to` (read-only stat via `target_precondition`).
fn render_targets_for(to: &str) -> Result<Vec<(crate::scope::Scope, String)>> {
    match to {
        crate::manifest::ADAPTER_ID => Ok(vec![
            (crate::scope::Scope::CodexHome, "AGENTS.md".to_string()),
            (crate::scope::Scope::CodexHome, "config.toml".to_string()),
        ]),
        crate::manifest::OPENCODE_ADAPTER_ID => {
            let rel = crate::opencode::config_rel().unwrap_or_else(|_| "opencode.json".to_string());
            Ok(vec![
                (crate::scope::Scope::OpenCodeHome, "AGENTS.md".to_string()),
                (crate::scope::Scope::OpenCodeHome, rel),
            ])
        }
        crate::manifest::CLAUDE_ADAPTER_ID => Ok(vec![
            (crate::scope::Scope::ClaudeHome, "CLAUDE.md".to_string()),
            (crate::scope::Scope::ClaudeHome, "settings.json".to_string()),
        ]),
        other => Err(unsupported_adapter_error(other)),
    }
}

fn compute_render_preconditions(to: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (scope, rel) in render_targets_for(to)? {
        let (key, state) = target_precondition(scope, &rel)?;
        // Mirror `plan::compute_plan`: symlinks and directories abort
        // planning instead of being hashed into a successful plan.
        if state == "symlink:held" {
            anyhow::bail!(
                "Refusing to plan translation write through symlink target '{}'; bundle v1 rejects symlinks",
                key
            );
        }
        if state == "dir:present" {
            anyhow::bail!(
                "Refusing to replace directory target '{}' with a rendered translation file",
                key
            );
        }
        out.insert(key, state);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// build_canonical_from_inspect
// ---------------------------------------------------------------------------

/// Build a validated [`CanonicalEnv`] from live inspect (read-only).
///
/// Maps instructions to the canonical role name (`AGENTS.md` — the canonical
/// v1 role; harness-specific `CLAUDE.md` rendering happens at plan time),
/// skills to names-only entries (`files=["SKILL.md"]` refs; content hashed at
/// apply time in Sprint 012), tools via `read_allowed_settings` (string values
/// only; non-strings never exported), MCP via `sanitize_mcp_server` (codex /
/// opencode; claude MCP is held and never sanitized), `needs_env` as the
/// sorted unique union, and provenance with seconds-since-epoch capture.
pub fn build_canonical_from_inspect(adapter: &str) -> Result<CanonicalEnv> {
    let norm = canonical_adapter_id(adapter)?;
    match norm {
        crate::manifest::ADAPTER_ID => build_from_codex(),
        crate::manifest::OPENCODE_ADAPTER_ID => build_from_opencode(),
        crate::manifest::CLAUDE_ADAPTER_ID => build_from_claude(),
        other => Err(unsupported_adapter_error(other)),
    }
}

fn build_from_codex() -> Result<CanonicalEnv> {
    let report = crate::codex::inspect()?;
    let version = report.version.unwrap_or_else(|| "unknown".to_string());
    let home = crate::codex::codex_home()?;
    let config_path = home.join("config.toml");
    // Absent config is ok (empty); unreadable/malformed config propagates.
    // `read_allowed_settings` already returns `Ok(empty)` when the file is
    // absent, so `?` alone distinguishes the two cases here.
    let settings = crate::codex::read_allowed_settings(&config_path)?;
    let mut tools = Vec::new();
    let mut policies: Vec<CanonicalPolicy> = Vec::new();
    for (k, v) in settings {
        match v {
            serde_json::Value::String(s) => tools.push(CanonicalTool { name: k, value: s }),
            _ => policies.push(CanonicalPolicy {
                key: format!("tools:{k}"),
                disposition: "held".to_string(),
                reason: format!("setting '{k}' has a non-string value; held, not exported"),
            }),
        }
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let mut skills = Vec::new();
    for name in report.skills {
        if !valid_skill_name_local(&name) {
            policies.push(CanonicalPolicy {
                key: format!("skill:{name}"),
                disposition: "held".to_string(),
                reason: format!("invalid skill name '{name}'; held, not exported"),
            });
            continue;
        }
        // Names only; content hashed at apply time. `SKILL.md` is a ref
        // placeholder satisfying the canonical non-empty-files invariant.
        skills.push(CanonicalSkill {
            name,
            files: vec!["SKILL.md".to_string()],
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    let mut mcp_servers: BTreeMap<String, CanonicalMcp> = BTreeMap::new();
    // Absent config is ok (empty); unreadable/malformed config propagates.
    // `mcp_server_names` reads unconditionally, so absence is checked first.
    let names = if !config_path.exists() {
        Vec::new()
    } else {
        crate::codex::mcp_server_names(&config_path)?
    };
    for id in names {
        match crate::codex::sanitize_mcp_server(&config_path, &id) {
            Ok(san) => {
                let mut refs = san.env_keys.clone();
                refs.sort();
                let entry = CanonicalMcp {
                    id: id.clone(),
                    transport: "stdio".to_string(),
                    command: san.command.clone(),
                    args: san.args.clone(),
                    cwd: san.cwd.clone(),
                    credential_refs: refs.clone(),
                    environment: refs,
                    enabled: false,
                };
                mcp_servers.insert(id, entry);
            }
            Err(e) => policies.push(CanonicalPolicy {
                key: format!("mcp:{id}"),
                disposition: "held".to_string(),
                reason: sanitize_reason(&e),
            }),
        }
    }

    let mut union = BTreeSet::new();
    for e in mcp_servers.values() {
        union.extend(e.credential_refs.iter().cloned());
    }
    let needs_env: Vec<String> = union.into_iter().collect();

    let env = CanonicalEnv {
        schema_version: CANONICAL_SCHEMA_VERSION,
        source_agent: crate::manifest::ADAPTER_ID.to_string(),
        source_agent_version: version,
        instructions: CanonicalInstructions {
            main: "AGENTS.md".to_string(),
        },
        skills,
        tools,
        mcp_servers,
        policies,
        environment: CanonicalEnvironment { needs_env },
        provenance: CanonicalProvenance {
            captured_at: seconds_since_epoch(),
            bundle_hash: String::new(),
            disclosure: SECRET_DISCLOSURE.to_string(),
        },
    };
    env.validate()?;
    Ok(env)
}

fn build_from_opencode() -> Result<CanonicalEnv> {
    let report = crate::opencode::inspect()?;
    let version = report.version.unwrap_or_else(|| "unknown".to_string());
    // A refused/unresolvable live config path is a source failure, not an
    // absent config: propagate instead of falling back to a default path.
    // (`live_config_path` already resolves absent files to the default
    // `opencode.json` path, so absence still yields empty success below.)
    let config_path = crate::opencode::live_config_path()?;
    // Absent config is ok (empty); unreadable/malformed config propagates.
    let settings = crate::opencode::read_allowed_settings(&config_path)?;
    let mut tools = Vec::new();
    let mut policies: Vec<CanonicalPolicy> = Vec::new();
    for (k, v) in settings {
        match v {
            serde_json::Value::String(s) => tools.push(CanonicalTool { name: k, value: s }),
            _ => policies.push(CanonicalPolicy {
                key: format!("tools:{k}"),
                disposition: "held".to_string(),
                reason: format!("setting '{k}' has a non-string value; held, not exported"),
            }),
        }
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let mut skills = Vec::new();
    for name in report.skills {
        if !valid_skill_name_local(&name) {
            policies.push(CanonicalPolicy {
                key: format!("skill:{name}"),
                disposition: "held".to_string(),
                reason: format!("invalid skill name '{name}'; held, not exported"),
            });
            continue;
        }
        skills.push(CanonicalSkill {
            name,
            files: vec!["SKILL.md".to_string()],
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    let mut mcp_servers: BTreeMap<String, CanonicalMcp> = BTreeMap::new();
    // Absent config is ok (empty); unreadable/malformed config propagates.
    let names = if !config_path.exists() {
        Vec::new()
    } else {
        crate::opencode::mcp_server_names(&config_path)?
    };
    for id in names {
        match crate::opencode::sanitize_mcp_server(&config_path, &id) {
            Ok(san) => {
                let mut refs = san.env_keys.clone();
                refs.sort();
                let entry = CanonicalMcp {
                    id: id.clone(),
                    transport: "stdio".to_string(),
                    command: san.command.clone(),
                    args: san.args.clone(),
                    cwd: san.cwd.clone(),
                    credential_refs: refs.clone(),
                    environment: refs,
                    enabled: false,
                };
                mcp_servers.insert(id, entry);
            }
            Err(e) => policies.push(CanonicalPolicy {
                key: format!("mcp:{id}"),
                disposition: "held".to_string(),
                reason: sanitize_reason(&e),
            }),
        }
    }

    let mut union = BTreeSet::new();
    for e in mcp_servers.values() {
        union.extend(e.credential_refs.iter().cloned());
    }
    let needs_env: Vec<String> = union.into_iter().collect();

    let env = CanonicalEnv {
        schema_version: CANONICAL_SCHEMA_VERSION,
        source_agent: crate::manifest::OPENCODE_ADAPTER_ID.to_string(),
        source_agent_version: version,
        instructions: CanonicalInstructions {
            main: "AGENTS.md".to_string(),
        },
        skills,
        tools,
        mcp_servers,
        policies,
        environment: CanonicalEnvironment { needs_env },
        provenance: CanonicalProvenance {
            captured_at: seconds_since_epoch(),
            bundle_hash: String::new(),
            disclosure: SECRET_DISCLOSURE.to_string(),
        },
    };
    env.validate()?;
    Ok(env)
}

fn build_from_claude() -> Result<CanonicalEnv> {
    let report = crate::claude::inspect()?;
    let version = report.version.unwrap_or_else(|| "unknown".to_string());
    let settings_path = crate::claude::settings_path()?;
    // Absent settings are ok (empty); unreadable/malformed settings
    // propagate. `read_allowed_settings` already returns `Ok(empty)` when
    // the file is absent, so `?` alone distinguishes the two cases here.
    let settings = crate::claude::read_allowed_settings(&settings_path)?;
    let mut tools = Vec::new();
    let mut policies: Vec<CanonicalPolicy> = Vec::new();
    for (k, v) in settings {
        match v {
            serde_json::Value::String(s) => tools.push(CanonicalTool { name: k, value: s }),
            _ => policies.push(CanonicalPolicy {
                key: format!("tools:{k}"),
                disposition: "held".to_string(),
                reason: format!("setting '{k}' has a non-string value; held, not exported"),
            }),
        }
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let mut skills = Vec::new();
    for name in report.skills {
        if !valid_skill_name_local(&name) {
            policies.push(CanonicalPolicy {
                key: format!("skill:{name}"),
                disposition: "held".to_string(),
                reason: format!("invalid skill name '{name}'; held, not exported"),
            });
            continue;
        }
        skills.push(CanonicalSkill {
            name,
            files: vec!["SKILL.md".to_string()],
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    // Claude MCP is held: never sanitize values. Names only inform held
    // policies; canonical `mcp_servers` stays empty so no descriptor (and no
    // `.claude.json` target) is ever emitted. An unreadable global-state
    // file propagates (`list_user_mcp_names` already returns `Ok(empty)`
    // when it is absent).
    let names = crate::claude::list_user_mcp_names()?;
    for id in names {
        policies.push(CanonicalPolicy {
            key: format!("mcp:{id}"),
            disposition: "held".to_string(),
            reason: crate::claude::CLAUDE_MCP_HELD_ERROR.to_string(),
        });
    }

    let env = CanonicalEnv {
        schema_version: CANONICAL_SCHEMA_VERSION,
        source_agent: crate::manifest::CLAUDE_ADAPTER_ID.to_string(),
        source_agent_version: version,
        instructions: CanonicalInstructions {
            main: "AGENTS.md".to_string(),
        },
        skills,
        tools,
        mcp_servers: BTreeMap::new(),
        policies,
        environment: CanonicalEnvironment { needs_env: vec![] },
        provenance: CanonicalProvenance {
            captured_at: seconds_since_epoch(),
            bundle_hash: String::new(),
            disclosure: SECRET_DISCLOSURE.to_string(),
        },
    };
    env.validate()?;
    Ok(env)
}

// ---------------------------------------------------------------------------
// plan_translation
// ---------------------------------------------------------------------------

/// Read-only translation plan from a canonical env to a target harness.
pub fn plan_translation(from: &str, env: &CanonicalEnv, to: &str) -> Result<TranslationPlan> {
    let from_norm = canonical_adapter_id(from)?;
    let to_norm = canonical_adapter_id(to)?;
    env.validate()?;

    let source_supported = version_supported(from_norm, &env.source_agent_version);
    let target_version = probe_target_version(to_norm);
    let target_supported = target_version
        .as_deref()
        .map(|v| version_supported(to_norm, v))
        .unwrap_or(false);

    // Version/support warnings (data only; no categorization change).
    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(version_warnings_for(
        "source",
        from_norm,
        Some(&env.source_agent_version),
    ));
    warnings.extend(version_warnings_for(
        "target",
        to_norm,
        target_version.as_deref(),
    ));

    let mut portable: Vec<String> = Vec::new();
    let mut requires_review: Vec<ReviewItem> = Vec::new();
    let mut unsupported: Vec<ReviewItem> = Vec::new();
    let mut needs_approval: Vec<ApprovalOut> = Vec::new();

    // Instructions → always portable (role-mapped filename per harness).
    {
        let from_main = harness_main(from_norm);
        let to_main = harness_main(to_norm);
        portable.push(format!(
            "{}:{} -> {}:{}",
            harness_scope_str(from_norm),
            from_main,
            harness_scope_str(to_norm),
            to_main
        ));
    }

    // Skills → portable when source is supported; otherwise demoted.
    for skill in &env.skills {
        if source_supported {
            portable.push(format!("skill:{}", skill.name));
        } else {
            requires_review.push(ReviewItem {
                item: format!("skill:{}", skill.name),
                reason: format!(
                    "unsupported source version '{}'; skill content requires review",
                    env.source_agent_version
                ),
            });
        }
    }

    // Tools → portable iff allowlisted on target.
    let allowed = allowed_settings_for(to_norm)?;
    for tool in &env.tools {
        if allowed.contains(&tool.name.as_str()) {
            portable.push(format!("tools:{}", tool.name));
        } else {
            requires_review.push(ReviewItem {
                item: format!("tools:{}", tool.name),
                reason: format!(
                    "{} allowlist is [{}]; held, not dropped",
                    to_norm,
                    allowed.join(", ")
                ),
            });
        }
    }

    // MCP servers.
    let is_claude_target = to_norm == crate::manifest::CLAUDE_ADAPTER_ID;
    // Deterministic order (BTreeMap already sorted, but be explicit).
    let mut mcp_ids: Vec<&String> = env.mcp_servers.keys().collect();
    mcp_ids.sort();
    for id in mcp_ids {
        let entry = &env.mcp_servers[id];
        if is_claude_target {
            unsupported.push(ReviewItem {
                item: format!("mcp:{id}"),
                reason: crate::claude::CLAUDE_MCP_HELD_ERROR.to_string(),
            });
            continue;
        }
        if source_supported {
            requires_review.push(ReviewItem {
                item: format!("mcp:{id}"),
                reason: "MCP command descriptor requires hash-bound approval at apply".to_string(),
            });
        } else {
            requires_review.push(ReviewItem {
                item: format!("mcp:{id}"),
                reason: format!(
                    "MCP command descriptor requires hash-bound approval at apply; unsupported source version '{}'",
                    env.source_agent_version
                ),
            });
        }
        let (scope, rel) = mcp_approval_target(to_norm, id)?;
        let sha = mcp_command_hash(
            &entry.command,
            &entry.args,
            &entry.cwd,
            &entry.credential_refs,
        );
        needs_approval.push(ApprovalOut {
            target: format!("{}:{}", scope.as_str(), rel),
            sha256: sha,
            reason: "MCP command".to_string(),
        });
    }
    needs_approval.sort_by(|a, b| a.target.cmp(&b.target));

    // Policies → always requires review (original hold reasons preserved).
    for policy in &env.policies {
        let reason = if policy.reason.is_empty() {
            format!("policy {}; never auto-applied", policy.disposition)
        } else {
            format!(
                "policy {}; never auto-applied: {}",
                policy.disposition, policy.reason
            )
        };
        requires_review.push(ReviewItem {
            item: format!("policy:{}", policy.key),
            reason,
        });
    }

    // Secrets required: sorted unique union (canonical already guarantees it).
    let mut secrets_required = env.environment.needs_env.clone();
    secrets_required.sort();
    secrets_required.dedup();

    // Held: secret locations for both harnesses + per-skill content deferrals.
    let mut held: Vec<HeldOut> = Vec::new();
    held.extend(secret_held_for(from_norm));
    if to_norm != from_norm {
        held.extend(secret_held_for(to_norm));
    }
    held.extend(skill_held_for(&env.skills));

    // Binding hash: the SAME full computation `apply_canonical` requires, so
    // `root agent plan` can never emit a hash `root agent apply` rejects.
    // When the plan is not applyable (unsupported source version, or Claude
    // target with MCP servers held), apply refuses regardless; keep the
    // Sprint 011 read-only verdict (report, never gate) and emit a stable
    // informational digest from the shared hashing primitive.
    let informational = !source_supported
        || (to_norm == crate::manifest::CLAUDE_ADAPTER_ID && !env.mcp_servers.is_empty());
    let plan_hash = if informational {
        let preconditions = compute_render_preconditions(to_norm)?;
        let env_hash = env.env_hash()?;
        crate::canonical_apply::canonical_plan_hash(
            &env_hash,
            to_norm,
            &preconditions,
            &BTreeMap::new(),
        )
    } else {
        crate::canonical_apply::binding_plan_hash(env, to_norm)?
    };

    portable.sort();
    // Keep requires_review / unsupported / needs_approval deterministic.
    requires_review.sort_by(|a, b| a.item.cmp(&b.item));
    unsupported.sort_by(|a, b| a.item.cmp(&b.item));

    Ok(TranslationPlan {
        from: from_norm.to_string(),
        to: to_norm.to_string(),
        source_version: env.source_agent_version.clone(),
        source_supported,
        target_version,
        target_supported,
        warnings,
        plan_hash,
        portable,
        requires_review,
        unsupported,
        secrets_required,
        needs_approval,
        held,
        mutated: false,
    })
}

// ---------------------------------------------------------------------------
// diff_canonical
// ---------------------------------------------------------------------------

/// Symmetric read-only diff between two canonical envs (no `plan_hash`).
pub fn diff_canonical(
    a_id: &str,
    a: &CanonicalEnv,
    b_id: &str,
    b: &CanonicalEnv,
) -> Result<TranslationDiff> {
    let a_norm = canonical_adapter_id(a_id)?;
    let b_norm = canonical_adapter_id(b_id)?;
    a.validate()?;
    b.validate()?;

    // Version/support visibility (data only; no categorization change).
    let a_supported = version_supported(a_norm, &a.source_agent_version);
    let b_supported = version_supported(b_norm, &b.source_agent_version);
    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(version_warnings_for(
        "a",
        a_norm,
        Some(&a.source_agent_version),
    ));
    warnings.extend(version_warnings_for(
        "b",
        b_norm,
        Some(&b.source_agent_version),
    ));

    let mut portable: Vec<String> = Vec::new();
    let mut requires_review: Vec<ReviewItem> = Vec::new();
    let mut unsupported: Vec<ReviewItem> = Vec::new();

    // Instructions: portable both ways with direction labels.
    portable.push(format!(
        "{}:{} <-> {}:{}",
        harness_scope_str(a_norm),
        harness_main(a_norm),
        harness_scope_str(b_norm),
        harness_main(b_norm)
    ));

    // Skills: union, portable both ways (names only).
    {
        let mut names = BTreeSet::new();
        for s in &a.skills {
            names.insert(s.name.clone());
        }
        for s in &b.skills {
            names.insert(s.name.clone());
        }
        for name in names {
            portable.push(format!("skill:{name}"));
        }
    }

    // Tools: portable iff allowlisted on BOTH harnesses.
    {
        let allowed_a = allowed_settings_for(a_norm)?;
        let allowed_b = allowed_settings_for(b_norm)?;
        let mut keys = BTreeSet::new();
        for t in &a.tools {
            keys.insert(t.name.clone());
        }
        for t in &b.tools {
            keys.insert(t.name.clone());
        }
        for key in keys {
            if allowed_a.contains(&key.as_str()) && allowed_b.contains(&key.as_str()) {
                portable.push(format!("tools:{key}"));
            } else {
                requires_review.push(ReviewItem {
                    item: format!("tools:{key}"),
                    reason: format!(
                        "allowlisted on {a_norm} [{a_list}] vs {b_norm} [{b_list}]; held, not dropped",
                        a_list = allowed_a.join(", "),
                        b_list = allowed_b.join(", ")
                    ),
                });
            }
        }
    }

    // MCP: union; claude on either side → unsupported, else requires review.
    {
        let mut ids = BTreeSet::new();
        for id in a.mcp_servers.keys() {
            ids.insert(id.clone());
        }
        for id in b.mcp_servers.keys() {
            ids.insert(id.clone());
        }
        // Also surface claude-side held MCP policies? Canonical claude envs
        // carry MCP names as `mcp:<id>` held policies (values never sanitized).
        // For diff symmetry those still count as MCP presence on the claude side.
        for policy in a.policies.iter().chain(b.policies.iter()) {
            if let Some(id) = policy.key.strip_prefix("mcp:") {
                if !id.is_empty() {
                    ids.insert(id.to_string());
                }
            }
        }
        let claude_involved = a_norm == crate::manifest::CLAUDE_ADAPTER_ID
            || b_norm == crate::manifest::CLAUDE_ADAPTER_ID;
        for id in ids {
            if claude_involved {
                unsupported.push(ReviewItem {
                    item: format!("mcp:{id}"),
                    reason: crate::claude::CLAUDE_MCP_HELD_ERROR.to_string(),
                });
            } else {
                requires_review.push(ReviewItem {
                    item: format!("mcp:{id}"),
                    reason: "MCP command descriptor requires hash-bound approval at apply"
                        .to_string(),
                });
            }
        }
    }

    // Policies (non-MCP): union by key → requires review (original hold
    // reasons preserved).
    {
        let mut seen = BTreeSet::new();
        for policy in a.policies.iter().chain(b.policies.iter()) {
            if policy.key.starts_with("mcp:") {
                continue; // already accounted above
            }
            if seen.insert(policy.key.clone()) {
                let reason = if policy.reason.is_empty() {
                    format!("policy {}; never auto-applied", policy.disposition)
                } else {
                    format!(
                        "policy {}; never auto-applied: {}",
                        policy.disposition, policy.reason
                    )
                };
                requires_review.push(ReviewItem {
                    item: format!("policy:{}", policy.key),
                    reason,
                });
            }
        }
    }

    // Secrets: union of both needs_env, sorted unique.
    let mut secrets_set = BTreeSet::new();
    for n in &a.environment.needs_env {
        secrets_set.insert(n.clone());
    }
    for n in &b.environment.needs_env {
        secrets_set.insert(n.clone());
    }
    let secrets_required: Vec<String> = secrets_set.into_iter().collect();

    // Held: secret locations for both sides + per-skill deferrals (union).
    let mut held: Vec<HeldOut> = Vec::new();
    held.extend(secret_held_for(a_norm));
    if b_norm != a_norm {
        held.extend(secret_held_for(b_norm));
    }
    {
        let mut names = BTreeSet::new();
        for s in &a.skills {
            names.insert(s.name.clone());
        }
        for s in &b.skills {
            names.insert(s.name.clone());
        }
        for name in names {
            held.push(HeldOut {
                source: format!("skill:{name}"),
                reason:
                    "skill content requires hash-bound approval at apply; content-approval deferred"
                        .to_string(),
            });
        }
    }

    portable.sort();
    portable.dedup();
    requires_review.sort_by(|a, b| a.item.cmp(&b.item));
    unsupported.sort_by(|a, b| a.item.cmp(&b.item));

    Ok(TranslationDiff {
        a: a_norm.to_string(),
        b: b_norm.to_string(),
        a_version: Some(a.source_agent_version.clone()),
        a_supported,
        b_version: Some(b.source_agent_version.clone()),
        b_supported,
        warnings,
        portable,
        requires_review,
        unsupported,
        secrets_required,
        held,
        mutated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{
        CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalMcp, CanonicalPolicy,
        CanonicalProvenance, CanonicalSkill, CanonicalTool,
    };
    use crate::manifest::SECRET_DISCLOSURE;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn save(keys: &[&'static str]) -> Self {
            let saved = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
            Self { saved }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn create(label: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "root-translate-011-{}-{}-{}-{}",
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

    const ISOLATED_KEYS: &[&str] = &[
        "HOME",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "CLAUDE_CONFIG_DIR",
        "ROOT_DIR",
        "TMPDIR",
        "PATH",
    ];

    fn isolate_env(tmp: &TempDir, extra_path: Option<&Path>) {
        let home = tmp.path().join("home");
        let codex = tmp.path().join("codex");
        let xdg = tmp.path().join("xdg");
        let oc = tmp.path().join("oc");
        let cl = tmp.path().join("cl");
        let root = tmp.path().join("root");
        let tdir = tmp.path().join("tmp");
        for d in [&home, &codex, &xdg, &oc, &cl, &root, &tdir] {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(d)
                    .unwrap();
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(d).unwrap();
            }
        }
        std::env::set_var("HOME", &home);
        std::env::set_var("CODEX_HOME", &codex);
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::set_var("XDG_DATA_HOME", &xdg);
        std::env::set_var("OPENCODE_CONFIG_DIR", &oc);
        std::env::set_var("OPENCODE_DISABLE_AUTOUPDATE", "1");
        std::env::set_var("CLAUDE_CONFIG_DIR", &cl);
        std::env::set_var("ROOT_DIR", &root);
        std::env::set_var("TMPDIR", &tdir);
        if let Some(bin) = extra_path {
            // Prepend the isolated bin dir, keeping existing system entries so
            // `sh` still resolves. `PATH` is a separated list, not one entry.
            let mut dirs: Vec<PathBuf> = vec![bin.to_path_buf()];
            if let Some(old) = std::env::var_os("PATH") {
                if !old.is_empty() {
                    dirs.extend(std::env::split_paths(&old));
                }
            }
            if let Ok(joined) = std::env::join_paths(&dirs) {
                std::env::set_var("PATH", joined);
            } else {
                std::env::set_var("PATH", bin);
            }
        } else {
            std::env::set_var("PATH", "/usr/bin:/bin");
        }
    }

    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn make_codex_shim(dir: &Path, version: &str) {
        // Emits `codex-cli <version>` and never touches the user home.
        let script = dir.join("codex");
        write_executable(
            &script,
            &format!("#!/bin/sh\nprintf 'codex-cli {version}\\n'\n"),
        );
    }

    fn make_opencode_shim(dir: &Path, version: &str) {
        let script = dir.join("opencode");
        write_executable(&script, &format!("#!/bin/sh\nprintf '{version}\\n'\n"));
    }

    fn synthetic_env(
        source_agent: &str,
        source_version: &str,
        tools: Vec<(&str, &str)>,
        mcp_ids: Vec<&str>,
        skills: Vec<&str>,
        policies: Vec<(&str, &str)>,
    ) -> CanonicalEnv {
        let mut mcp_servers = BTreeMap::new();
        let mut needs = BTreeSet::new();
        for id in mcp_ids {
            let refs = vec!["GITHUB_TOKEN".to_string()];
            for r in &refs {
                needs.insert(r.clone());
            }
            mcp_servers.insert(
                id.to_string(),
                CanonicalMcp {
                    id: id.to_string(),
                    transport: "stdio".to_string(),
                    command: vec!["npx".to_string()],
                    args: vec!["-y".to_string(), "pkg".to_string()],
                    cwd: None,
                    credential_refs: refs.clone(),
                    environment: refs,
                    enabled: false,
                },
            );
        }
        CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
            source_agent: source_agent.to_string(),
            source_agent_version: source_version.to_string(),
            instructions: CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: skills
                .into_iter()
                .map(|n| CanonicalSkill {
                    name: n.to_string(),
                    files: vec!["SKILL.md".to_string()],
                })
                .collect(),
            tools: tools
                .into_iter()
                .map(|(k, v)| CanonicalTool {
                    name: k.to_string(),
                    value: v.to_string(),
                })
                .collect(),
            mcp_servers,
            policies: policies
                .into_iter()
                .map(|(k, d)| CanonicalPolicy {
                    key: k.to_string(),
                    disposition: d.to_string(),
                    reason: String::new(),
                })
                .collect(),
            environment: CanonicalEnvironment {
                needs_env: needs.into_iter().collect(),
            },
            provenance: CanonicalProvenance {
                captured_at: "1234567890".to_string(),
                bundle_hash: String::new(),
                disclosure: SECRET_DISCLOSURE.to_string(),
            },
        }
    }

    #[test]
    fn unsupported_adapter_errors() {
        let env = synthetic_env("codex", "0.150.1", vec![], vec![], vec![], vec![]);
        assert!(build_canonical_from_inspect("gemini").is_err());
        assert!(plan_translation("gemini", &env, "codex").is_err());
        assert!(plan_translation("codex", &env, "gemini").is_err());
        assert!(diff_canonical("gemini", &env, "codex", &env).is_err());
        let err = build_canonical_from_inspect("gemini")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported bundle adapter"), "got: {err}");
    }

    #[test]
    fn codex_to_opencode_all_portable_except_mcp_review() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("portable");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_codex_shim(&bindir, "0.150.1");
        make_opencode_shim(&bindir, "1.18.27");
        isolate_env(&tmp, Some(&bindir));

        // Live codex state: model + stdio MCP + one skill.
        let codex_home = std::env::var_os("CODEX_HOME").unwrap();
        let codex_home = Path::new(&codex_home);
        std::fs::write(
            codex_home.join("config.toml"),
            "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\n",
        )
        .unwrap();
        std::fs::write(codex_home.join("AGENTS.md"), "# agents\n").unwrap();
        let shared =
            Path::new(&std::env::var_os("HOME").unwrap()).join(".agents/skills/docs-writer");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("SKILL.md"), "# skill\n").unwrap();

        let env = build_canonical_from_inspect("codex").unwrap();
        assert_eq!(env.source_agent, "codex");
        assert!(env.tools.iter().any(|t| t.name == "model"));
        assert!(env.mcp_servers.contains_key("github"));

        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert!(plan.source_supported);
        assert_eq!(plan.target_version.as_deref(), Some("1.18.27"));
        assert!(plan.target_supported);
        // Instructions portable.
        assert!(
            plan.portable.iter().any(|s| s.contains("AGENTS.md")),
            "portable: {:?}",
            plan.portable
        );
        // Skill portable.
        assert!(plan.portable.iter().any(|s| s == "skill:docs-writer"));
        // Model portable.
        assert!(plan.portable.iter().any(|s| s == "tools:model"));
        // MCP requires review (never portable, never unsupported for opencode).
        assert!(plan.unsupported.is_empty());
        let mcp_review = plan
            .requires_review
            .iter()
            .find(|r| r.item == "mcp:github")
            .expect("mcp:github should require review");
        assert!(mcp_review.reason.contains("hash-bound approval"));
        // Secrets union exact.
        assert_eq!(plan.secrets_required, vec!["GITHUB_TOKEN".to_string()]);
        // Needs approval recomputes.
        assert_eq!(plan.needs_approval.len(), 1);
        let approval = &plan.needs_approval[0];
        assert!(
            approval.target.contains("opencode_home"),
            "got: {}",
            approval.target
        );
        assert!(approval.target.contains("mcp.github"));
        let entry = &env.mcp_servers["github"];
        let recomputed = mcp_command_hash(
            &entry.command,
            &entry.args,
            &entry.cwd,
            &entry.credential_refs,
        );
        assert_eq!(approval.sha256, recomputed);
        assert_eq!(approval.reason, "MCP command");
        assert!(!plan.mutated);
        assert!(!plan.held.is_empty());
        // Plan hash stable.
        let plan2 = plan_translation("codex", &env, "opencode").unwrap();
        assert_eq!(plan.plan_hash, plan2.plan_hash);
    }

    #[test]
    fn reasoning_effort_to_opencode_requires_review_and_canonical_preserved() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model_reasoning_effort", "high")],
            vec![],
            vec![],
            vec![],
        );
        env.validate().unwrap();
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        let item = plan
            .requires_review
            .iter()
            .find(|r| r.item == "tools:model_reasoning_effort")
            .expect("reasoning_effort should require review");
        assert!(item.reason.contains("opencode"), "got: {}", item.reason);
        assert!(
            item.reason.contains("held, not dropped"),
            "got: {}",
            item.reason
        );
        assert!(!plan.portable.iter().any(|s| s.contains("reasoning_effort")));
        // Canonical preserved (not narrowed).
        assert!(env.tools.iter().any(|t| t.name == "model_reasoning_effort"));
        assert!(!plan.mutated);
    }

    #[test]
    fn any_mcp_to_claude_is_unsupported_with_exact_sentinel() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "gpt-5")],
            vec!["github"],
            vec!["docs-writer"],
            vec![],
        );
        let plan = plan_translation("codex", &env, "claude").unwrap();
        assert_eq!(plan.unsupported.len(), 1);
        assert_eq!(plan.unsupported[0].item, "mcp:github");
        assert_eq!(
            plan.unsupported[0].reason,
            crate::claude::CLAUDE_MCP_HELD_ERROR
        );
        // No approval for claude targets.
        assert!(plan.needs_approval.is_empty());
        // No `.claude.json` target string anywhere in JSON output.
        let json = serde_json::to_string(&plan).unwrap();
        assert!(
            !json.contains(".claude.json"),
            "output must not contain .claude.json target: {json}"
        );
        assert!(!plan.mutated);
    }

    #[test]
    fn same_agent_round_trip_empty_review() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "gpt-5")],
            vec![],
            vec!["docs-writer"],
            vec![],
        );
        let plan = plan_translation("codex", &env, "codex").unwrap();
        assert!(
            plan.requires_review.is_empty(),
            "got: {:?}",
            plan.requires_review
        );
        assert!(plan.unsupported.is_empty());
        // Skills still portable since names-only.
        assert!(plan.portable.iter().any(|s| s == "skill:docs-writer"));
        assert!(plan.portable.iter().any(|s| s.contains("AGENTS.md")));
        assert!(!plan.mutated);
    }

    #[test]
    fn unsupported_source_version_demotes_to_review() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.999.0",
            vec![("model", "gpt-5")],
            vec!["github"],
            vec!["docs-writer"],
            vec![],
        );
        env.validate().unwrap(); // validation passes; support is a plan-time verdict
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert!(!plan.source_supported);
        // No skill/MCP item may be portable.
        assert!(
            !plan.portable.iter().any(|s| s.starts_with("skill:")),
            "portable: {:?}",
            plan.portable
        );
        assert!(
            !plan.portable.iter().any(|s| s.starts_with("mcp:")),
            "portable: {:?}",
            plan.portable
        );
        let skill = plan
            .requires_review
            .iter()
            .find(|r| r.item == "skill:docs-writer")
            .expect("skill should be demoted");
        assert!(
            skill.reason.contains("unsupported source version"),
            "got: {}",
            skill.reason
        );
        let mcp = plan
            .requires_review
            .iter()
            .find(|r| r.item == "mcp:github")
            .expect("mcp should require review");
        assert!(
            mcp.reason.contains("unsupported source version"),
            "got: {}",
            mcp.reason
        );
    }

    #[test]
    fn secrets_union_exact_and_approval_recompute() {
        let _env = crate::lock_env();
        // Two MCP servers with overlapping env refs → union sorted unique.
        let mut env = synthetic_env("codex", "0.150.1", vec![], vec!["b-server"], vec![], vec![]);
        env.mcp_servers.insert(
            "a-server".to_string(),
            CanonicalMcp {
                id: "a-server".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec!["A_TOKEN".to_string(), "Z_TOKEN".to_string()],
                environment: vec!["A_TOKEN".to_string(), "Z_TOKEN".to_string()],
                enabled: false,
            },
        );
        {
            let e = env.mcp_servers.get_mut("b-server").unwrap();
            e.credential_refs = vec!["M_TOKEN".to_string(), "Z_TOKEN".to_string()];
            e.environment = vec!["M_TOKEN".to_string(), "Z_TOKEN".to_string()];
        }
        env.environment.needs_env = vec![
            "A_TOKEN".to_string(),
            "M_TOKEN".to_string(),
            "Z_TOKEN".to_string(),
        ];
        env.validate().unwrap();

        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert_eq!(
            plan.secrets_required,
            vec![
                "A_TOKEN".to_string(),
                "M_TOKEN".to_string(),
                "Z_TOKEN".to_string()
            ]
        );
        for approval in &plan.needs_approval {
            let id = approval.target.rsplit('.').next().unwrap().to_string();
            let entry = &env.mcp_servers[&id];
            let recomputed = mcp_command_hash(
                &entry.command,
                &entry.args,
                &entry.cwd,
                &entry.credential_refs,
            );
            assert_eq!(approval.sha256, recomputed);
        }
    }

    #[test]
    fn plan_hash_stable_and_diff_symmetric() {
        let _env = crate::lock_env();
        let a = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "gpt-5")],
            vec!["github"],
            vec!["docs-writer"],
            vec![],
        );
        let b = synthetic_env(
            "opencode",
            "1.18.27",
            vec![("model", "gpt-5")],
            vec![],
            vec!["docs-writer"],
            vec![],
        );
        let p1 = plan_translation("codex", &a, "opencode").unwrap();
        let p2 = plan_translation("codex", &a, "opencode").unwrap();
        assert_eq!(p1.plan_hash, p2.plan_hash);

        let d_ab = diff_canonical("codex", &a, "opencode", &b).unwrap();
        let d_ba = diff_canonical("opencode", &b, "codex", &a).unwrap();
        // Secrets union is direction-independent.
        assert_eq!(d_ab.secrets_required, d_ba.secrets_required);
        assert!(!d_ab.mutated && !d_ba.mutated);
        // Category sizes equal; portable entries equal modulo direction labels
        // (instructions carry `a -> b` vs `b -> a`; skills/tools are bare).
        assert_eq!(d_ab.portable.len(), d_ba.portable.len());
        assert_eq!(d_ab.requires_review.len(), d_ba.requires_review.len());
        assert_eq!(d_ab.unsupported.len(), d_ba.unsupported.len());
        // Every portable entry in ab has a counterpart in ba (exact or reversed).
        for entry in &d_ab.portable {
            let reversed = reverse_arrow(entry);
            assert!(
                d_ba.portable.contains(entry) || d_ba.portable.contains(&reversed),
                "missing counterpart for {entry} in {d_ba:?}"
            );
        }
        // Diff has no plan_hash field by construction.
        let json = serde_json::to_string(&d_ab).unwrap();
        assert!(!json.contains("plan_hash"));
    }

    // -- Sprint 011 Gap 2: symlink/dir render targets abort planning --------

    #[cfg(unix)]
    #[test]
    fn symlink_target_aborts_plan() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("symlink-abort");
        isolate_env(&tmp, None);
        // Render targets for `to=codex` are scope-flat, so the symlink sits
        // at the target path itself; `resolve_target`'s ancestor-component
        // check (cf. scope `descendant_symlink` test) must refuse it.
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, codex_home.join("config.toml")).unwrap();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "gpt-5")],
            vec![],
            vec![],
            vec![],
        );
        let err = plan_translation("codex", &env, "codex").unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "symlink target must abort planning, got: {err:#}"
        );
    }

    #[test]
    fn dir_target_aborts_plan() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("dir-abort");
        isolate_env(&tmp, None);
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::create_dir(codex_home.join("config.toml")).unwrap();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "gpt-5")],
            vec![],
            vec![],
            vec![],
        );
        let err = plan_translation("codex", &env, "codex").unwrap_err();
        assert!(
            err.to_string().contains("directory"),
            "directory target must abort planning, got: {err:#}"
        );
    }

    // -- Sprint 011 Gap 3: source config failures ---------------------------

    #[test]
    fn corrupt_codex_config_propagates_error() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("corrupt-codex");
        isolate_env(&tmp, None);
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("config.toml"), "{ not valid toml !!!").unwrap();
        let err = build_canonical_from_inspect("codex").unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "corrupt codex config must propagate, got ok"
        );
    }

    #[test]
    fn corrupt_opencode_config_propagates_error() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("corrupt-opencode");
        isolate_env(&tmp, None);
        let oc = PathBuf::from(std::env::var_os("OPENCODE_CONFIG_DIR").unwrap());
        std::fs::write(oc.join("opencode.json"), "{ not valid json !!!").unwrap();
        let err = build_canonical_from_inspect("opencode").unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "corrupt opencode config must propagate, got ok"
        );
    }

    #[test]
    fn corrupt_claude_config_propagates_error() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("corrupt-claude");
        isolate_env(&tmp, None);
        let cl = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
        std::fs::write(cl.join("settings.json"), "{ not valid json !!!").unwrap();
        let err = build_canonical_from_inspect("claude").unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "corrupt claude config must propagate, got ok"
        );
    }

    #[test]
    fn absent_config_builds_empty() {
        // Absent source config is ok (empty success), not an error.
        for adapter in ["codex", "opencode", "claude"] {
            let _env = crate::lock_env();
            let _restore = EnvRestore::save(ISOLATED_KEYS);
            let tmp = TempDir::create("absent-config");
            isolate_env(&tmp, None);
            let env = build_canonical_from_inspect(adapter)
                .unwrap_or_else(|e| panic!("absent {adapter} config must build empty, got: {e:#}"));
            assert!(env.tools.is_empty(), "adapter={adapter}");
            assert!(env.mcp_servers.is_empty(), "adapter={adapter}");
            assert!(env.environment.needs_env.is_empty(), "adapter={adapter}");
        }
    }

    #[test]
    fn sanitizer_hostile_mcp_held_with_reason() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("hostile-mcp");
        isolate_env(&tmp, None);
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        // Valid TOML, but the MCP entry uses an unsupported field so the
        // per-server sanitizer refuses it.
        std::fs::write(
            codex_home.join("config.toml"),
            "model = \"gpt-5\"\n[mcp_servers.evil]\ncommand = \"npx\"\nurl = \"https://example.com/mcp\"\n",
        )
        .unwrap();
        let env = build_canonical_from_inspect("codex").unwrap();
        assert!(
            !env.mcp_servers.contains_key("evil"),
            "hostile server must not be exported"
        );
        let held = env
            .policies
            .iter()
            .find(|p| p.key == "mcp:evil")
            .expect("hostile server must leave a held policy");
        assert_eq!(held.disposition, "held");
        assert!(
            held.reason.contains("unsupported field"),
            "held reason must name the problem, got: {}",
            held.reason
        );
        // The reason surfaces in plan-level requires-review items.
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        let item = plan
            .requires_review
            .iter()
            .find(|r| r.item == "policy:mcp:evil")
            .expect("held policy must require review");
        assert!(
            item.reason.contains("unsupported field"),
            "review reason must name the problem, got: {}",
            item.reason
        );
    }

    // -- Sprint 011 Gap 5: unsupported-version warnings ---------------------

    #[test]
    fn unsupported_source_version_warns() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.999.0",
            vec![("model", "gpt-5")],
            vec![],
            vec![],
            vec![],
        );
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert!(!plan.source_supported);
        assert!(!plan.warnings.is_empty(), "unsupported source must warn");
        let warn = plan
            .warnings
            .iter()
            .find(|w| w.contains("source"))
            .expect("source warning must be present");
        assert!(warn.contains("0.999.0"), "got: {warn}");
        assert!(warn.contains("0.150.1"), "got: {warn}");
        assert!(warn.contains("supported exact versions"), "got: {warn}");
    }

    #[test]
    fn unsupported_target_version_warns() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("target-warn");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_opencode_shim(&bindir, "0.0.0");
        isolate_env(&tmp, Some(&bindir));
        let env = synthetic_env("codex", "0.150.1", vec![], vec![], vec![], vec![]);
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert_eq!(plan.target_version.as_deref(), Some("0.0.0"));
        assert!(!plan.target_supported);
        let warn = plan
            .warnings
            .iter()
            .find(|w| w.contains("target"))
            .expect("target warning must be present");
        assert!(warn.contains("0.0.0"), "got: {warn}");
        assert!(warn.contains("1.18.27"), "got: {warn}");
        assert!(warn.contains("supported exact versions"), "got: {warn}");
    }

    #[test]
    fn absent_target_binary_warns() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("absent-warn");
        isolate_env(&tmp, None);
        // Force PATH to an empty dir so no target binary can resolve.
        let empty = tmp.path().join("emptybin");
        std::fs::create_dir_all(&empty).unwrap();
        std::env::set_var("PATH", &empty);
        let env = synthetic_env("codex", "0.150.1", vec![], vec![], vec![], vec![]);
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        assert!(plan.target_version.is_none());
        assert!(!plan.target_supported);
        let warn = plan
            .warnings
            .iter()
            .find(|w| w.contains("target"))
            .expect("absent-target warning must be present");
        assert!(warn.contains("absent"), "got: {warn}");
        assert!(warn.contains("1.18.27"), "got: {warn}");
        assert!(warn.contains("supported exact versions"), "got: {warn}");
    }

    #[test]
    fn diff_reports_versions_and_warnings() {
        let _env = crate::lock_env();
        let a = synthetic_env("codex", "0.150.1", vec![], vec![], vec![], vec![]);
        let b = synthetic_env("opencode", "0.0.0", vec![], vec![], vec![], vec![]);
        let d = diff_canonical("codex", &a, "opencode", &b).unwrap();
        assert_eq!(d.a_version.as_deref(), Some("0.150.1"));
        assert!(d.a_supported);
        assert_eq!(d.b_version.as_deref(), Some("0.0.0"));
        assert!(!d.b_supported);
        let warn = d
            .warnings
            .iter()
            .find(|w| w.contains('b'))
            .expect("diff warning for b must be present");
        assert!(warn.contains("0.0.0"), "got: {warn}");
        assert!(warn.contains("1.18.27"), "got: {warn}");
        assert!(warn.contains("supported exact versions"), "got: {warn}");
        // Supported pairs warn about nothing.
        let ok = diff_canonical("codex", &a, "codex", &a).unwrap();
        assert!(ok.warnings.is_empty(), "got: {:?}", ok.warnings);
    }

    fn reverse_arrow(s: &str) -> String {
        if let Some((l, r)) = s.split_once(" <-> ") {
            format!("{r} <-> {l}")
        } else if let Some((l, r)) = s.split_once(" -> ") {
            format!("{r} -> {l}")
        } else {
            s.to_string()
        }
    }

    #[test]
    fn plan_and_diff_write_nothing() {
        let _env = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("nowrite");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_codex_shim(&bindir, "0.150.1");
        make_opencode_shim(&bindir, "1.18.27");
        isolate_env(&tmp, Some(&bindir));

        let root = Path::new(&std::env::var_os("ROOT_DIR").unwrap()).to_path_buf();
        let before = dir_snapshot(&root);

        let a = synthetic_env(
            "codex",
            "0.150.1",
            vec![("model", "x")],
            vec![],
            vec![],
            vec![],
        );
        let b = synthetic_env(
            "opencode",
            "1.18.27",
            vec![("model", "x")],
            vec![],
            vec![],
            vec![],
        );
        let plan = plan_translation("codex", &a, "opencode").unwrap();
        assert!(!plan.mutated);
        let diff = diff_canonical("codex", &a, "opencode", &b).unwrap();
        assert!(!diff.mutated);
        // Live inspect is also read-only (no bundle/snapshot/journal under ROOT_DIR).
        let _ = build_canonical_from_inspect("codex").unwrap();

        let after = dir_snapshot(&root);
        assert_eq!(before, after, "translate must not write under ROOT_DIR");
    }

    fn dir_snapshot(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if !dir.exists() {
            return out;
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(cur) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&cur) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                out.push(p.display().to_string());
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn diff_mcp_to_claude_unsupported() {
        let a = synthetic_env("codex", "0.150.1", vec![], vec!["github"], vec![], vec![]);
        let b = synthetic_env("claude", "2.1.260", vec![], vec![], vec![], vec![]);
        let d = diff_canonical("codex", &a, "claude", &b).unwrap();
        assert_eq!(d.unsupported.len(), 1);
        assert_eq!(
            d.unsupported[0].reason,
            crate::claude::CLAUDE_MCP_HELD_ERROR
        );
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains(".claude.json"),
            "diff must not emit target: {json}"
        );
    }

    #[test]
    fn policy_items_require_review() {
        let _env = crate::lock_env();
        let env = synthetic_env(
            "codex",
            "0.150.1",
            vec![],
            vec![],
            vec![],
            vec![("shell_environment_policy", "held")],
        );
        let plan = plan_translation("codex", &env, "opencode").unwrap();
        let item = plan
            .requires_review
            .iter()
            .find(|r| r.item == "policy:shell_environment_policy")
            .expect("policy should require review");
        assert!(item.reason.contains("held"), "got: {}", item.reason);
        assert!(
            item.reason.contains("never auto-applied"),
            "got: {}",
            item.reason
        );
    }
}
