//! Same-machine cross-harness canonical apply (Sprint 012).
//!
//! Plan AND apply read LIVE source-harness files at execution time (no
//! transfer document yet — Sprint 013). Plan records content hashes; apply
//! re-reads under the lock and compares → stale plans fail with drift.
//! All mutations reuse the bundle machinery (journal, lock, snapshot,
//! rollback, verify, enable) — this module adds a canonical source and
//! cross-harness rendering, not a second mutation engine.

use crate::apply::{auto_rollback, existing_file_mode, ApplyReport};
use crate::canonical::{canonical_adapter_id, CanonicalEnv};
use crate::journal::{
    completed_mcp_provenance, mcp_provenance_key, new_op_id, require_no_incomplete_op,
    write_journal, ApplyJournal, Phase,
};
use crate::lock::GlobalMutationLock;
use crate::manifest::{
    allowed_settings_for, mcp_approval_target, mcp_command_hash, supported_versions_for,
    ADAPTER_ID, CLAUDE_ADAPTER_ID, MAX_FILE_BYTES, MAX_TOTAL_BYTES, OPENCODE_ADAPTER_ID,
};
use crate::plan::{target_precondition, ApprovalOut, HeldOut};
use crate::scope::{resolve_target, Scope};
use crate::snapshot::{
    record_expected_applied, snapshot_manifest_hash, take_snapshot, write_beside,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use crate::verify::VerifyReport;

// ---------------------------------------------------------------------------
// Public shapes.
// ---------------------------------------------------------------------------

/// One harness-native file to write (scope + rel + exact bytes).
#[derive(Debug, Clone)]
pub struct RenderedTarget {
    pub scope: Scope,
    pub rel: String,
    pub bytes: Vec<u8>,
}

/// Live source bytes keyed by `"scope:rel"`, plus held (missing) entries.
#[derive(Debug, Clone)]
pub struct SourceContent {
    pub files: BTreeMap<String, Vec<u8>>,
    pub held: Vec<HeldOut>,
}

/// Canonical apply plan (read-only, no lock). Serialize for `--json`.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalApplyPlan {
    pub to: String,
    pub env_hash: String,
    pub plan_hash: String,
    pub will_create: Vec<String>,
    pub will_update: Vec<String>,
    pub will_keep: Vec<String>,
    pub rendered_targets: Vec<(Scope, String, String)>,
    pub content_hashes: BTreeMap<String, String>,
    pub needs_approval: Vec<ApprovalOut>,
    pub needs_env: Vec<String>,
    pub held: Vec<HeldOut>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Source reading.
// ---------------------------------------------------------------------------

fn source_instruction_target(source: &str) -> Result<(Scope, String)> {
    match source {
        ADAPTER_ID => Ok((Scope::CodexHome, "AGENTS.md".to_string())),
        OPENCODE_ADAPTER_ID => Ok((Scope::OpenCodeHome, "AGENTS.md".to_string())),
        CLAUDE_ADAPTER_ID => Ok((Scope::ClaudeHome, "CLAUDE.md".to_string())),
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

fn target_instruction_target(to: &str) -> Result<(Scope, String)> {
    source_instruction_target(to)
}

fn source_skill_candidates(source: &str, skill: &str, file: &str) -> Result<Vec<(Scope, String)>> {
    crate::scope::validate_rel(file)?;
    match source {
        ADAPTER_ID => Ok(vec![(Scope::SharedSkills, format!("{}/{}", skill, file))]),
        OPENCODE_ADAPTER_ID => Ok(vec![
            (Scope::OpenCodeHome, format!("skills/{}/{}", skill, file)),
            (Scope::SharedSkills, format!("{}/{}", skill, file)),
        ]),
        CLAUDE_ADAPTER_ID => Ok(vec![
            (Scope::ClaudeHome, format!("skills/{}/{}", skill, file)),
            (Scope::SharedSkills, format!("{}/{}", skill, file)),
        ]),
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

/// Read one source file: missing → Ok(None) (held, never Err for absence);
/// symlink/dir/unreadable/oversize/malformed → Err (gap-3 rule).
fn read_source_file(scope: Scope, rel: &str) -> Result<Option<Vec<u8>>> {
    let target = match resolve_target(scope, rel) {
        Ok(p) => p,
        Err(e) => {
            // resolve_target fails for symlinks/escapes/traversal — fail closed.
            // Missing files surface as NotFound inside validate_descendants?
            // Actually resolve_target Ok for missing tails; Err only for
            // symlink/escape/invalid. Propagate as Err.
            return Err(e);
        }
    };
    match std::fs::symlink_metadata(&target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to stat source {}", target.display()))?,
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!(
                    "Refusing to read symlink source {} (bundle v1 rejects all symlinks)",
                    target.display()
                );
            }
            if m.is_dir() {
                anyhow::bail!(
                    "Refusing to read directory source {} as a file",
                    target.display()
                );
            }
            if !m.is_file() {
                anyhow::bail!(
                    "Refusing to read non-regular source file {}",
                    target.display()
                );
            }
            if m.len() > MAX_FILE_BYTES {
                anyhow::bail!("Source file '{}' exceeds per-file limit", rel);
            }
            let bytes = std::fs::read(&target)
                .with_context(|| format!("Failed to read source {}", target.display()))?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                anyhow::bail!("Source file '{}' exceeds per-file limit", rel);
            }
            Ok(Some(bytes))
        }
    }
}

/// Error for a declared source artifact that does not exist on this machine.
/// Sprint 012 is same-machine cross-harness only: apply must refuse rather
/// than silently transfer nothing.
fn absent_source_content_error(key: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "source content '{key}' is not available on this machine; Sprint 012 applies on the machine where the source environment lives (same-machine cross-harness). Cross-machine content transfer is Sprint 013."
    )
}

/// Reads live source files: instruction main file in source scope, skill
/// files in native+shared scopes. Export-grade rules: missing → held (never
/// silent, never Err); unreadable/malformed → Err; symlinks rejected;
/// size-capped per file and total. Secret bytes are NOT scanned (verbatim
/// copy under disclosure); literal secret-shaped tool values are refused via
/// `env.validate()` (looks_like_secret_value).
pub fn collect_source_content(env: &CanonicalEnv) -> Result<SourceContent> {
    collect_source_content_impl(env, false)
}

/// Apply-grade sibling of [`collect_source_content`]: a declared instruction
/// or skill file that is absent on this machine is a hard error naming the
/// missing artifact (Sprint 012 is same-machine cross-harness only).
pub fn collect_source_content_strict(env: &CanonicalEnv) -> Result<SourceContent> {
    collect_source_content_impl(env, true)
}

fn collect_source_content_impl(env: &CanonicalEnv, strict: bool) -> Result<SourceContent> {
    env.validate()?;
    let source = canonical_adapter_id(&env.source_agent)?.to_string();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut held: Vec<HeldOut> = Vec::new();
    let mut total: u64 = 0;

    let (instr_scope, instr_rel) = source_instruction_target(&source)?;
    match read_source_file(instr_scope, &instr_rel)? {
        Some(bytes) => {
            total = total
                .checked_add(bytes.len() as u64)
                .context("source total size overflow")?;
            if total > MAX_TOTAL_BYTES {
                anyhow::bail!("Source content exceeds total size limit");
            }
            files.insert(format!("{}:{}", instr_scope.as_str(), instr_rel), bytes);
        }
        None => {
            let key = format!("{}:{}", instr_scope.as_str(), instr_rel);
            if strict {
                return Err(absent_source_content_error(&key));
            }
            held.push(HeldOut {
                source: format!("{} (absent)", key),
                reason: "absent on source".to_string(),
            });
        }
    }

    for skill in &env.skills {
        for f in &skill.files {
            let candidates = source_skill_candidates(&source, &skill.name, f)?;
            let mut found: Option<(Scope, String, Vec<u8>)> = None;
            for (scope, rel) in &candidates {
                match read_source_file(*scope, rel)? {
                    Some(bytes) => {
                        found = Some((*scope, rel.clone(), bytes));
                        break;
                    }
                    None => continue,
                }
            }
            match found {
                Some((scope, rel, bytes)) => {
                    total = total
                        .checked_add(bytes.len() as u64)
                        .context("source total size overflow")?;
                    if total > MAX_TOTAL_BYTES {
                        anyhow::bail!("Source content exceeds total size limit");
                    }
                    files.insert(format!("{}:{}", scope.as_str(), rel), bytes);
                }
                None => {
                    if strict {
                        let (scope, rel) = &candidates[0];
                        return Err(absent_source_content_error(&format!(
                            "{}:{}",
                            scope.as_str(),
                            rel
                        )));
                    }
                    held.push(HeldOut {
                        source: format!("skill:{}/{}", skill.name, f),
                        reason: "absent on source".to_string(),
                    });
                }
            }
        }
    }
    Ok(SourceContent { files, held })
}

// ---------------------------------------------------------------------------
// Rendering (no writes).
// ---------------------------------------------------------------------------

fn is_markdown_rel(rel: &str) -> bool {
    Path::new(rel).extension().and_then(|s| s.to_str()) == Some("md")
}

fn probe_live_target_version(adapter: &str) -> Option<String> {
    match adapter {
        ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::codex::CODEX_BINARY)?;
            crate::codex::probe_codex_version(&bin).ok()
        }
        OPENCODE_ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::opencode::OPENCODE_BINARY)?;
            crate::opencode::probe_opencode_version(&bin).ok()
        }
        CLAUDE_ADAPTER_ID => {
            let bin = crate::codex::find_on_path(crate::claude::CLAUDE_BINARY)?;
            crate::claude::probe_claude_version(&bin).ok()
        }
        _ => None,
    }
}

fn target_version_warnings(to: &str, live: Option<&str>) -> Vec<String> {
    let Ok(supported) = supported_versions_for(to) else {
        return Vec::new();
    };
    match live {
        Some(v) if supported.contains(&v) => Vec::new(),
        Some(v) => vec![format!(
            "target {to} version '{v}' is not in supported exact versions {supported:?}; portable items demoted to Requires review"
        )],
        None => vec![format!(
            "target {to} binary absent; live version unknown; supported exact versions {supported:?}; version-gated items held for review"
        )],
    }
}

fn check_source_gate(env: &CanonicalEnv) -> Result<String> {
    let source = canonical_adapter_id(&env.source_agent)?.to_string();
    let supported = supported_versions_for(&source)?;
    if !supported.contains(&env.source_agent_version.as_str()) {
        anyhow::bail!(
            "unsupported source agent version '{}' for adapter '{}'. Supported exact versions: {:?}",
            env.source_agent_version,
            source,
            supported
        );
    }
    Ok(source)
}

fn check_target_gate(to_norm: &str) -> Result<String> {
    let supported = supported_versions_for(to_norm)?;
    match probe_live_target_version(to_norm) {
        Some(v) if supported.contains(&v.as_str()) => Ok(v),
        Some(v) => anyhow::bail!(
            "unsupported target agent version '{}' for adapter '{}'. Supported exact versions: {:?}",
            v,
            to_norm,
            supported
        ),
        None => anyhow::bail!(
            "unsupported target agent version 'unknown' for adapter '{}' (binary absent or unparseable). Supported exact versions: {:?}",
            to_norm,
            supported
        ),
    }
}

/// Renders harness-native bytes WITHOUT writing. Unknown target keys are
/// preserved (toml_edit / JSONC-aware patch / settings patch). Non-allowlisted
/// tools and policies are held (loudly skipped, never rendered).
pub fn render_canonical(
    env: &CanonicalEnv,
    to: &str,
    content: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<RenderedTarget>, Vec<HeldOut>)> {
    env.validate()?;
    let to_norm = canonical_adapter_id(to)?.to_string();
    // Claude-MCP-held gate BEFORE anything else.
    if to_norm == CLAUDE_ADAPTER_ID && !env.mcp_servers.is_empty() {
        return Err(crate::claude::mcp_apply_gated_error());
    }
    let allowed = allowed_settings_for(&to_norm)?.to_vec();
    let mut held: Vec<HeldOut> = Vec::new();
    let mut out: Vec<RenderedTarget> = Vec::new();

    // Policies are never rendered (held, not dropped).
    for p in &env.policies {
        let reason = if p.reason.is_empty() {
            format!("policy {}; never auto-applied", p.disposition)
        } else {
            format!("policy {}; never auto-applied: {}", p.disposition, p.reason)
        };
        held.push(HeldOut {
            source: format!("policy:{}", p.key),
            reason,
        });
    }

    // Instructions: source bytes → target main file.
    {
        let source_norm = canonical_adapter_id(&env.source_agent)?.to_string();
        let (src_scope, src_rel) = source_instruction_target(&source_norm)?;
        let src_key = format!("{}:{}", src_scope.as_str(), src_rel);
        // Fallback: search any content key ending with the role filename
        // (covers native-vs-shared skew).
        let bytes = content.get(&src_key).cloned().or_else(|| {
            let want = src_rel.clone();
            content
                .iter()
                .find(|(k, _)| k.ends_with(&format!(":{}", want)) || k.ends_with(&want))
                .map(|(_, v)| v.clone())
        });
        let (tgt_scope, tgt_rel) = target_instruction_target(&to_norm)?;
        match bytes {
            Some(b) => out.push(RenderedTarget {
                scope: tgt_scope,
                rel: tgt_rel,
                bytes: b,
            }),
            None => held.push(HeldOut {
                source: format!("instructions:{} (absent)", src_rel),
                reason: "absent on source; not rendered".to_string(),
            }),
        }
    }

    // Skills → SharedSkills (portable library) for all targets.
    {
        let source_norm = canonical_adapter_id(&env.source_agent)?.to_string();
        for skill in &env.skills {
            for f in &skill.files {
                let candidates = source_skill_candidates(&source_norm, &skill.name, f)?;
                let mut bytes: Option<Vec<u8>> = None;
                for (scope, rel) in &candidates {
                    let key = format!("{}:{}", scope.as_str(), rel);
                    if let Some(b) = content.get(&key) {
                        bytes = Some(b.clone());
                        break;
                    }
                }
                match bytes {
                    Some(b) => {
                        let rel = format!("{}/{}", skill.name, f);
                        // validate target rel (fail closed on weird names).
                        crate::scope::validate_rel(&rel)?;
                        out.push(RenderedTarget {
                            scope: Scope::SharedSkills,
                            rel,
                            bytes: b,
                        });
                    }
                    None => held.push(HeldOut {
                        source: format!("skill:{}/{} (absent)", skill.name, f),
                        reason: "absent on source; not rendered".to_string(),
                    }),
                }
            }
        }
    }

    // Config per target (settings allowlisted; MCP disabled).
    match to_norm.as_str() {
        ADAPTER_ID => {
            let (scope, rel, bytes, extra_held) = render_codex_config(env, &allowed)?;
            out.push(RenderedTarget { scope, rel, bytes });
            held.extend(extra_held);
        }
        OPENCODE_ADAPTER_ID => {
            let (scope, rel, bytes, extra_held) = render_opencode_config(env, &allowed)?;
            out.push(RenderedTarget { scope, rel, bytes });
            held.extend(extra_held);
        }
        CLAUDE_ADAPTER_ID => {
            let (scope, rel, bytes, extra_held) = render_claude_config(env, &allowed)?;
            out.push(RenderedTarget { scope, rel, bytes });
            held.extend(extra_held);
        }
        other => return Err(crate::manifest::unsupported_adapter_error(other)),
    }

    Ok((out, held))
}

fn render_codex_config(
    env: &CanonicalEnv,
    allowed: &[&str],
) -> Result<(Scope, String, Vec<u8>, Vec<HeldOut>)> {
    let mut held = Vec::new();
    let home = crate::codex::codex_home()?;
    let path = home.join("config.toml");
    let original = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("Failed to stat target config.toml")?,
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!(
                    "Refusing to render config patch through symlink target 'codex_home:config.toml'; bundle v1 rejects symlinks"
                );
            }
            if m.is_dir() {
                anyhow::bail!("Refusing to replace directory target 'codex_home:config.toml'");
            }
            if !m.is_file() {
                anyhow::bail!("target config.toml exists but is not a regular file");
            }
            let bytes = std::fs::read(&path).context("Failed to read target config.toml")?;
            if bytes.len() as u64 > 1024 * 1024 {
                anyhow::bail!("target config.toml exceeds size limit");
            }
            String::from_utf8(bytes).context("target config.toml is not valid UTF-8")?
        }
    };
    let mut doc: toml_edit::DocumentMut = if original.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        original
            .parse()
            .context("Failed to parse target config.toml")?
    };
    for t in &env.tools {
        if allowed.contains(&t.name.as_str()) {
            doc[&t.name] = toml_edit::value(t.value.clone());
        } else {
            held.push(HeldOut {
                source: format!("tools:{}", t.name),
                reason: format!(
                    "codex allowlist is [{}]; held, not dropped",
                    allowed.join(", ")
                ),
            });
        }
    }
    if doc.get("mcp_servers").is_none() {
        let mut servers = toml_edit::Table::new();
        servers.set_implicit(true);
        doc["mcp_servers"] = toml_edit::Item::Table(servers);
    }
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(|item| item.as_table_mut())
        .context("target mcp_servers is not a table; refusing to patch")?;
    // Deterministic order.
    let mut ids: Vec<&String> = env.mcp_servers.keys().collect();
    ids.sort();
    for id in ids {
        let entry = &env.mcp_servers[id];
        if id.is_empty() || id.len() > 128 {
            anyhow::bail!("invalid MCP server id '{}'", id);
        }
        // Mutate an existing per-server table IN PLACE so unknown fields
        // (e.g. `timeout`) survive; only rendered known keys are overwritten.
        // Create the table when the id does not exist yet.
        if servers.get(id).and_then(|item| item.as_table()).is_none() {
            servers.insert(id, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let table = servers
            .get_mut(id)
            .and_then(|item| item.as_table_mut())
            .context("target mcp_servers entry is not a table; refusing to patch")?;
        table["transport"] = toml_edit::value("stdio");
        if !entry.command.is_empty() {
            table["command"] = toml_edit::value(entry.command[0].clone());
        }
        let mut args = toml_edit::Array::new();
        for a in &entry.args {
            args.push(a.clone());
        }
        table["args"] = toml_edit::Item::Value(toml_edit::Value::Array(args));
        // `cwd` is a rendered known key: set when Some, removed when None.
        match &entry.cwd {
            Some(cwd) => {
                table["cwd"] = toml_edit::value(cwd.clone());
            }
            None => {
                table.remove("cwd");
            }
        }
        let mut env_vars = toml_edit::Array::new();
        for k in &entry.credential_refs {
            env_vars.push(k.clone());
        }
        table["env_vars"] = toml_edit::Item::Value(toml_edit::Value::Array(env_vars));
        table["enabled"] = toml_edit::value(false);
    }
    let rendered = doc.to_string();
    if rendered.len() > 1024 * 1024 {
        anyhow::bail!("Patched config.toml exceeds size limit");
    }
    Ok((
        Scope::CodexHome,
        "config.toml".to_string(),
        rendered.into_bytes(),
        held,
    ))
}

fn manifest_for_patch(
    env: &CanonicalEnv,
    to_norm: &str,
    allowed: &[&str],
) -> Result<(crate::manifest::Manifest, Vec<HeldOut>)> {
    use crate::manifest::{Manifest, McpEntry, SECRET_DISCLOSURE};
    let mut held = Vec::new();
    let mut settings: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for t in &env.tools {
        if allowed.contains(&t.name.as_str()) {
            settings.insert(t.name.clone(), serde_json::Value::String(t.value.clone()));
        } else {
            held.push(HeldOut {
                source: format!("tools:{}", t.name),
                reason: format!(
                    "{to_norm} allowlist is [{}]; held, not dropped",
                    allowed.join(", ")
                ),
            });
        }
    }
    // Source version for the throwaway manifest: use the env's own version when
    // it is gated for the TARGET (same-agent round-trip), else fall back to the
    // target's gated version so Manifest construction never fails validation
    // when patching (patch itself does not validate).
    let gated_version = match supported_versions_for(to_norm) {
        Ok(list) if list.contains(&env.source_agent_version.as_str()) => {
            env.source_agent_version.clone()
        }
        Ok(list) => list.first().map(|s| s.to_string()).unwrap_or_default(),
        Err(_) => env.source_agent_version.clone(),
    };
    let mut m = Manifest::new_for(to_norm, gated_version, None);
    m.settings = settings;
    m.disclosure = SECRET_DISCLOSURE.to_string();
    let mut ids: Vec<&String> = env.mcp_servers.keys().collect();
    ids.sort();
    for id in ids {
        let e = &env.mcp_servers[id];
        let hash = mcp_command_hash(&e.command, &e.args, &e.cwd, &e.credential_refs);
        m.mcp.insert(
            id.clone(),
            McpEntry {
                transport: "stdio".to_string(),
                enabled: false,
                needs_env: e.credential_refs.clone(),
                command_sha256: Some(hash),
                command: e.command.clone(),
                args: e.args.clone(),
                cwd: e.cwd.clone(),
                env_keys: e.credential_refs.clone(),
            },
        );
    }
    Ok((m, held))
}

fn render_opencode_config(
    env: &CanonicalEnv,
    allowed: &[&str],
) -> Result<(Scope, String, Vec<u8>, Vec<HeldOut>)> {
    let path = crate::opencode::live_config_path()?;
    let mut value = if path.exists() {
        crate::opencode::load_config_value(&path)?
    } else {
        serde_json::json!({})
    };
    let to_norm = OPENCODE_ADAPTER_ID;
    let (m, held) = manifest_for_patch(env, to_norm, allowed)?;
    crate::opencode::patch_config_value(&mut value, &m)?;
    let bytes = crate::opencode::render_pretty_json(&value)?;
    let rel = crate::opencode::config_rel()?;
    Ok((Scope::OpenCodeHome, rel, bytes, held))
}

fn render_claude_config(
    env: &CanonicalEnv,
    allowed: &[&str],
) -> Result<(Scope, String, Vec<u8>, Vec<HeldOut>)> {
    let to_norm = CLAUDE_ADAPTER_ID;
    let (m, held) = manifest_for_patch(env, to_norm, allowed)?;
    let (path, bytes) = crate::claude::prepare_settings_patch(&m)?;
    let rel = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("settings.json")
        .to_string();
    Ok((Scope::ClaudeHome, rel, bytes, held))
}

// ---------------------------------------------------------------------------
// Plan hash + plan.
// ---------------------------------------------------------------------------

/// Stable sha256 over canonical JSON of the tuple (same family as
/// `plan::plan_hash_for`). Deterministic for identical inputs.
pub fn canonical_plan_hash(
    env_hash: &str,
    to: &str,
    preconditions: &BTreeMap<String, String>,
    content_hashes: &BTreeMap<String, String>,
) -> String {
    let canonical = serde_json::json!({
        "env_hash": env_hash,
        "to": to,
        "target_preconditions": preconditions,
        "content_hashes": content_hashes,
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    root_lockfile::compute_sha256(&bytes)
}

struct PlanBuild {
    plan: CanonicalApplyPlan,
    rendered: Vec<RenderedTarget>,
    preconditions: BTreeMap<String, String>,
}

fn build_plan_and_render(env: &CanonicalEnv, to: &str) -> Result<PlanBuild> {
    build_plan_and_render_impl(env, to, false)
}

/// Apply-grade plan build: identical to [`build_plan_and_render`] except that
/// source content must be present on this machine (same-machine limitation).
fn build_plan_and_render_strict(env: &CanonicalEnv, to: &str) -> Result<PlanBuild> {
    build_plan_and_render_impl(env, to, true)
}

fn build_plan_and_render_impl(
    env: &CanonicalEnv,
    to: &str,
    strict_content: bool,
) -> Result<PlanBuild> {
    env.validate()?;
    let to_norm = canonical_adapter_id(to)?.to_string();
    // Exact SOURCE gate (fail closed). Target only warns (read-only discovery).
    check_source_gate(env)?;
    // Claude gate before any rendering.
    if to_norm == CLAUDE_ADAPTER_ID && !env.mcp_servers.is_empty() {
        return Err(crate::claude::mcp_apply_gated_error());
    }
    let content = if strict_content {
        collect_source_content_strict(env)?
    } else {
        collect_source_content(env)?
    };
    let (rendered, render_held) = render_canonical(env, &to_norm, &content.files)?;

    let mut content_hashes: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &content.files {
        content_hashes.insert(k.clone(), root_lockfile::compute_sha256(v));
    }

    let mut preconditions: BTreeMap<String, String> = BTreeMap::new();
    for rt in &rendered {
        let (key, state) = target_precondition(rt.scope, &rt.rel)?;
        if state == "symlink:held" {
            anyhow::bail!(
                "Refusing to plan write through symlink target '{}'; bundle v1 rejects symlinks",
                key
            );
        }
        if state == "dir:present" {
            anyhow::bail!(
                "Refusing to replace directory target '{}' with a rendered translation file",
                key
            );
        }
        preconditions.insert(key, state);
    }

    let env_hash = env.env_hash()?;
    let plan_hash = canonical_plan_hash(&env_hash, &to_norm, &preconditions, &content_hashes);

    // will_create / will_update / will_keep (same semantics as PlanReport).
    let mut will_create = Vec::new();
    let mut will_update = Vec::new();
    let mut will_keep = Vec::new();
    let mut rendered_targets: Vec<(Scope, String, String)> = Vec::new();
    for rt in &rendered {
        let digest = root_lockfile::compute_sha256(&rt.bytes);
        rendered_targets.push((rt.scope, rt.rel.clone(), digest.clone()));
        let label = format!("{}:{}", rt.scope.as_str(), rt.rel);
        let key = label.clone();
        match preconditions.get(&key).map(|s| s.as_str()) {
            Some("missing") | None => will_create.push(label),
            Some(state) => {
                let live_hex = state.strip_prefix("sha256:").unwrap_or("");
                if live_hex.eq_ignore_ascii_case(&digest) {
                    will_keep.push(label);
                } else {
                    will_update.push(label);
                }
            }
        }
    }

    // needs_approval: MCP descriptors + executable skill files.
    let mut needs_approval: Vec<ApprovalOut> = Vec::new();
    {
        let mut ids: Vec<&String> = env.mcp_servers.keys().collect();
        ids.sort();
        for id in ids {
            let e = &env.mcp_servers[id];
            let sha = mcp_command_hash(&e.command, &e.args, &e.cwd, &e.credential_refs);
            let (scope, rel) = mcp_approval_target(&to_norm, id)?;
            needs_approval.push(ApprovalOut {
                target: format!("{}:{}", scope.as_str(), rel),
                sha256: sha,
                reason: "MCP command".to_string(),
            });
        }
        for rt in &rendered {
            if rt.scope == Scope::SharedSkills && !is_markdown_rel(&rt.rel) {
                let digest = root_lockfile::compute_sha256(&rt.bytes);
                needs_approval.push(ApprovalOut {
                    target: format!("{}:{}", rt.scope.as_str(), rt.rel),
                    sha256: digest,
                    reason: format!("executable skill file {}", rt.rel),
                });
            }
        }
        needs_approval.sort_by(|a, b| a.target.cmp(&b.target));
    }

    let mut held = content.held.clone();
    held.extend(render_held);

    let live_version = probe_live_target_version(&to_norm);
    let mut warnings = target_version_warnings(&to_norm, live_version.as_deref());
    // Plan stays lenient: absent source content is reported (held + warning);
    // apply refuses via `collect_source_content_strict`.
    for h in &content.held {
        if h.reason.contains("absent on source") {
            warnings.push(format!(
                "source content absent on this machine; not transferred: {}",
                h.source
            ));
        }
    }

    let plan = CanonicalApplyPlan {
        to: to_norm.clone(),
        env_hash,
        plan_hash,
        will_create,
        will_update,
        will_keep,
        rendered_targets,
        content_hashes,
        needs_approval,
        needs_env: env.environment.needs_env.clone(),
        held,
        warnings,
    };
    Ok(PlanBuild {
        plan,
        rendered,
        preconditions,
    })
}

/// Read-only (no lock): validates env + exact SOURCE gate, collects source
/// content, renders, computes preconditions, assembles the plan.
pub fn plan_canonical_apply(env: &CanonicalEnv, to: &str) -> Result<CanonicalApplyPlan> {
    Ok(build_plan_and_render(env, to)?.plan)
}

/// The single plan/apply binding hash implementation. Performs the FULL plan
/// computation (source gate, Claude-MCP gate, source collection, rendering,
/// target preconditions incl. symlink/dir aborts, `env_hash`, then
/// [`canonical_plan_hash`]) and returns exactly the hash `apply_canonical`
/// requires. `translate::plan_translation` binds this same hash so
/// `root agent plan` can never emit a hash apply rejects.
pub fn binding_plan_hash(env: &CanonicalEnv, to: &str) -> Result<String> {
    Ok(build_plan_and_render(env, to)?.plan.plan_hash)
}

// ---------------------------------------------------------------------------
// Apply.
// ---------------------------------------------------------------------------

fn verify_canonical_applied(
    env: &CanonicalEnv,
    to_norm: &str,
    rendered: &[RenderedTarget],
) -> Result<()> {
    let report = verify_agent(to_norm)?;
    if !report.success {
        anyhow::bail!(
            "verification failed: post-apply checks did not pass ({:?})",
            report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
    }
    for rt in rendered {
        let target = resolve_target(rt.scope, &rt.rel)?;
        let live = std::fs::read(&target)
            .with_context(|| format!("verification failed: {} missing after apply", rt.rel))?;
        let digest = root_lockfile::compute_sha256(&live);
        let expected = root_lockfile::compute_sha256(&rt.bytes);
        if !digest.eq_ignore_ascii_case(&expected) {
            anyhow::bail!(
                "verification failed: hash mismatch for '{}' after apply",
                rt.rel
            );
        }
    }
    if !env.mcp_servers.is_empty() {
        match to_norm {
            ADAPTER_ID => {
                let home = crate::codex::codex_home()?;
                let text = std::fs::read_to_string(home.join("config.toml"))
                    .context("verification failed: config.toml missing")?;
                let doc: toml_edit::DocumentMut = text
                    .parse()
                    .context("verification failed: config.toml unparsable")?;
                for id in env.mcp_servers.keys() {
                    let server = doc.get("mcp_servers").and_then(|v| v.get(id));
                    let Some(server) = server else {
                        anyhow::bail!(
                            "verification failed: MCP server '{}' missing after apply",
                            id
                        );
                    };
                    let enabled = server
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if enabled {
                        anyhow::bail!(
                            "verification failed: MCP server '{}' is enabled; canonical apply must stay disabled",
                            id
                        );
                    }
                }
            }
            OPENCODE_ADAPTER_ID => {
                let config = crate::opencode::live_config_path()?;
                let value = crate::opencode::load_config_value(&config)
                    .context("verification failed: opencode config missing")?;
                for id in env.mcp_servers.keys() {
                    let Some(server) = value.get("mcp").and_then(|v| v.get(id)) else {
                        anyhow::bail!(
                            "verification failed: MCP server '{}' missing after apply",
                            id
                        );
                    };
                    let enabled = server
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if enabled {
                        anyhow::bail!(
                            "verification failed: MCP server '{}' is enabled; canonical apply must stay disabled",
                            id
                        );
                    }
                }
            }
            CLAUDE_ADAPTER_ID => {
                return Err(crate::claude::mcp_apply_gated_error());
            }
            other => return Err(crate::manifest::unsupported_adapter_error(other)),
        }
    }
    Ok(())
}

/// Canonical apply. `apply_flag=false` → plan-only Err (no writes).
/// Otherwise mirrors `apply_bundle` phases: source gate, Claude gate before
/// lock, live target gate, lock, drift recheck under lock, exact approvals,
/// journal + snapshot + write_beside + post-verify + Done, auto-rollback.
pub fn apply_canonical(
    env: &CanonicalEnv,
    to: &str,
    plan_hash: &str,
    approvals: &[String],
    apply_flag: bool,
) -> Result<ApplyReport> {
    if !apply_flag {
        anyhow::bail!(
            "Plan only: no writes performed. Re-run with --apply --plan-hash {} to mutate.",
            plan_hash
        );
    }
    env.validate()?;
    let to_norm = canonical_adapter_id(to)?.to_string();
    check_source_gate(env)?;
    // Claude-MCP-held gate BEFORE the lock.
    if to_norm == CLAUDE_ADAPTER_ID && !env.mcp_servers.is_empty() {
        return Err(crate::claude::mcp_apply_gated_error());
    }
    // Exact live TARGET gate (fail closed, names exact versions).
    check_target_gate(&to_norm)?;

    let _guard = GlobalMutationLock::acquire()?;
    require_no_incomplete_op()?;
    // Re-gate target under lock (binary could have changed).
    check_target_gate(&to_norm)?;

    // Recompute under lock; drift → exit-5 family, zero writes.
    // Strict: absent source content must refuse, never silently transfer nothing.
    let built = build_plan_and_render_strict(env, &to_norm)?;
    if built.plan.plan_hash != plan_hash {
        anyhow::bail!(
            "Drift detected: target state changed since plan (expected plan {}, computed {}). Re-run plan.",
            plan_hash,
            built.plan.plan_hash
        );
    }

    // Approvals must match needs_approval EXACTLY as sets.
    let approved: BTreeSet<String> = approvals.iter().map(|s| s.to_lowercase()).collect();
    for a in built.plan.needs_approval.iter() {
        if !approved.contains(&a.sha256.to_lowercase()) {
            anyhow::bail!(
                "Missing hash-bound approval for '{}' (sha256:{}). Pass --approve <sha256> per item; global approval is forbidden.",
                a.target,
                a.sha256
            );
        }
    }
    let expected: BTreeSet<String> = built
        .plan
        .needs_approval
        .iter()
        .map(|a| a.sha256.to_lowercase())
        .collect();
    for ap in &approved {
        if !expected.contains(ap) {
            anyhow::bail!(
                "Unknown approval hash '{}' (matches nothing in canonical plan)",
                ap
            );
        }
    }

    let prior_mcp_provenance = completed_mcp_provenance()?;
    let mut mcp_provenance = prior_mcp_provenance.clone();
    for (id, entry) in &env.mcp_servers {
        let descriptor_hash = mcp_command_hash(
            &entry.command,
            &entry.args,
            &entry.cwd,
            &entry.credential_refs,
        );
        mcp_provenance.insert(
            mcp_provenance_key(&to_norm, id),
            descriptor_hash.to_lowercase(),
        );
    }

    let op_id = new_op_id();
    let mut journal = ApplyJournal {
        op_id: op_id.clone(),
        agent: to_norm.clone(),
        plan_hash: plan_hash.to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: Phase::Planned,
        completed_paths: Vec::new(),
        target_preconditions: built.preconditions.clone(),
        mcp_provenance,
        prior_mcp_provenance,
    };
    write_journal(&journal)?;

    let snap_targets: Vec<(Scope, String)> = built
        .rendered
        .iter()
        .map(|r| (r.scope, r.rel.clone()))
        .collect();
    let snap = take_snapshot(&to_norm, &op_id, &snap_targets, None)?;
    journal.snapshot_id = Some(snap.id.clone());
    journal.snapshot_manifest_hash = Some(snapshot_manifest_hash(&snap)?);
    journal.phase = Phase::Snapshotted;
    write_journal(&journal)?;
    journal.phase = Phase::Applying;
    write_journal(&journal)?;

    let outcome: Result<ApplyReport> = (|| {
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        for rt in &built.rendered {
            let target = resolve_target(rt.scope, &rt.rel)?;
            let expected_hash = root_lockfile::compute_sha256(&rt.bytes);
            if let Ok(live) = std::fs::read(&target) {
                if root_lockfile::compute_sha256(&live).eq_ignore_ascii_case(&expected_hash) {
                    let hash =
                        record_expected_applied(&snap.id, rt.scope, &rt.rel, &expected_hash)?;
                    journal.snapshot_manifest_hash = Some(hash);
                    skipped.push(format!("{}:{}", rt.scope.as_str(), rt.rel));
                    write_journal(&journal)?;
                    continue;
                }
            }
            // Modes: config preserves, content enforces 0644/0755.
            let is_config = matches!(
                (rt.scope, rt.rel.as_str()),
                (Scope::CodexHome, "config.toml") | (Scope::ClaudeHome, "settings.json")
            ) || (rt.scope == Scope::OpenCodeHome
                && (rt.rel == "opencode.json" || rt.rel == "opencode.jsonc"));
            let mode = if is_config {
                existing_file_mode(&target).unwrap_or(0o600)
            } else if rt.scope == Scope::SharedSkills && !is_markdown_rel(&rt.rel) {
                0o755
            } else {
                0o644
            };
            let hash = record_expected_applied(&snap.id, rt.scope, &rt.rel, &expected_hash)?;
            journal.snapshot_manifest_hash = Some(hash);
            write_journal(&journal)?;
            write_beside(&target, &rt.bytes, mode)?;
            let label = format!("{}:{}", rt.scope.as_str(), rt.rel);
            journal.completed_paths.push(label.clone());
            write_journal(&journal)?;
            applied.push(label);
        }
        let mut mcp_imported = Vec::new();
        if !env.mcp_servers.is_empty() {
            let mut ids: Vec<String> = env.mcp_servers.keys().cloned().collect();
            ids.sort();
            mcp_imported = ids;
        }
        journal.phase = Phase::Verifying;
        write_journal(&journal)?;
        verify_canonical_applied(env, &to_norm, &built.rendered)?;
        Ok(ApplyReport {
            op_id: op_id.clone(),
            snapshot_id: snap.id.clone(),
            plan_hash: plan_hash.to_string(),
            applied,
            skipped_identical: skipped,
            mcp_imported,
        })
    })();

    match outcome {
        Ok(report) => {
            journal.phase = Phase::Done;
            write_journal(&journal)?;
            Ok(report)
        }
        Err(e) => Err(auto_rollback(&snap, &mut journal, "Apply", e)),
    }
}

/// Standalone verify (no env needed): binary + version + config parse.
pub fn verify_agent(adapter: &str) -> Result<VerifyReport> {
    let norm = canonical_adapter_id(adapter)?;
    match norm {
        ADAPTER_ID => crate::verify::verify_codex(),
        OPENCODE_ADAPTER_ID => crate::verify::verify_opencode(),
        CLAUDE_ADAPTER_ID => crate::verify::verify_claude(),
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{
        CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalMcp,
        CanonicalProvenance, CanonicalTool, CANONICAL_SCHEMA_VERSION,
    };
    use crate::manifest::SECRET_DISCLOSURE;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn save(keys: &[&'static str]) -> Self {
            Self {
                saved: keys.iter().map(|k| (*k, std::env::var_os(k))).collect(),
            }
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
                "root-canapply-012-{}-{}-{}-{}",
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

    fn make_shim(dir: &Path, name: &str, version_line: &str) {
        let script = dir.join(name);
        write_executable(&script, &format!("#!/bin/sh\nprintf '{version_line}\\n'\n"));
    }

    fn minimal_env_for(source: &str, version: &str) -> CanonicalEnv {
        CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
            source_agent: source.to_string(),
            source_agent_version: version.to_string(),
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

    fn codex_env_with_mcp() -> CanonicalEnv {
        let mut env = minimal_env_for("codex", "0.150.1");
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
        env
    }

    #[test]
    fn canonical_plan_hash_stable() {
        let mut a = BTreeMap::new();
        a.insert("k".to_string(), "missing".to_string());
        let mut b = a.clone();
        b.insert("k".to_string(), "sha256:abc".to_string());
        let h1 = canonical_plan_hash("eh", "codex", &a, &a);
        let h2 = canonical_plan_hash("eh", "codex", &a, &a);
        let h3 = canonical_plan_hash("eh", "codex", &b, &a);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn apply_requires_plan_hash_and_approvals() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("planflag");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();

        let env = codex_env_with_mcp();
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        // Missing --apply flag → plan-only Err with exact family.
        let err = apply_canonical(&env, "codex", &plan.plan_hash, &[], false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Plan only: no writes performed. Re-run with --apply --plan-hash"),
            "got: {err}"
        );
        // Missing approval → Err naming sha.
        let err = apply_canonical(&env, "codex", &plan.plan_hash, &[], true)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&plan.needs_approval[0].sha256),
            "missing approval must name sha, got: {err}"
        );
        // Extra unknown approval → Err.
        let mut approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        approvals.push("d".repeat(64));
        let err = apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&"d".repeat(64)),
            "extra approval must name sha, got: {err}"
        );
    }

    #[test]
    fn apply_drift_fails() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("drift");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        std::fs::write(codex_home.join("config.toml"), "model = \"old\"\n").unwrap();

        let env = minimal_env_for("codex", "0.150.1");
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        // Externally modify target after plan.
        std::fs::write(codex_home.join("config.toml"), "model = \"evil\"\n").unwrap();
        let before = std::fs::read(codex_home.join("config.toml")).unwrap();
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        let err = apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Drift detected"), "got: {err}");
        let after = std::fs::read(codex_home.join("config.toml")).unwrap();
        assert_eq!(before, after, "drift must leave target bytes unchanged");
        // No Done journal.
        match crate::journal::read_journal().unwrap() {
            None => {}
            Some(j) => assert_ne!(j.phase, crate::journal::Phase::Done, "no Done on drift"),
        }
    }

    #[test]
    fn apply_skips_identical() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("identical");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# hello\n").unwrap();

        // Plan first to learn expected bytes, then pre-seed identical targets.
        let mut env = minimal_env_for("codex", "0.150.1");
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "gpt-5".to_string(),
        });
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        // Apply once to seed.
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        let first = apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true).unwrap();
        assert!(!first.applied.is_empty() || !first.skipped_identical.is_empty());
        // Re-plan (fresh preconditions) then apply → everything identical.
        let plan2 = plan_canonical_apply(&env, "codex").unwrap();
        let approvals2: Vec<String> = plan2
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        let cfg = codex_home.join("config.toml");
        let agents = codex_home.join("AGENTS.md");
        let mtime_cfg = std::fs::metadata(&cfg).unwrap().modified().unwrap();
        let mtime_agents = std::fs::metadata(&agents).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = apply_canonical(&env, "codex", &plan2.plan_hash, &approvals2, true).unwrap();
        assert!(
            !second.skipped_identical.is_empty(),
            "identical targets must be skipped, got: {second:?}"
        );
        assert_eq!(
            std::fs::metadata(&cfg).unwrap().modified().unwrap(),
            mtime_cfg,
            "mtime must not churn on identical"
        );
        assert_eq!(
            std::fs::metadata(&agents).unwrap().modified().unwrap(),
            mtime_agents,
            "mtime must not churn on identical"
        );
    }

    #[test]
    fn rollback_restores_bytes() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("rollback");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        std::fs::write(codex_home.join("config.toml"), "model = \"orig\"\n").unwrap();
        let orig_agents = std::fs::read(codex_home.join("AGENTS.md")).unwrap();
        let orig_cfg = std::fs::read(codex_home.join("config.toml")).unwrap();

        let mut env = minimal_env_for("codex", "0.150.1");
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "gpt-5".to_string(),
        });
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true).unwrap();
        // Mutate after apply, then rollback_last must restore ORIGINAL bytes.
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        let report = crate::apply::rollback_last().unwrap();
        assert!(!report.snapshot_id.is_empty());
        assert_eq!(
            std::fs::read(codex_home.join("AGENTS.md")).unwrap(),
            orig_agents
        );
        assert_eq!(
            std::fs::read(codex_home.join("config.toml")).unwrap(),
            orig_cfg
        );
        let j = crate::journal::read_journal().unwrap().unwrap();
        assert_eq!(j.phase, crate::journal::Phase::RolledBack);
    }

    #[test]
    fn claude_mcp_apply_gated() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("claudemcp");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "claude", "2.1.260");
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        // Source codex file for content.
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();

        let mut env = minimal_env_for("codex", "0.150.1");
        env.mcp_servers.insert(
            "github".to_string(),
            CanonicalMcp {
                id: "github".to_string(),
                transport: "stdio".to_string(),
                command: vec!["npx".to_string()],
                args: vec![],
                cwd: None,
                credential_refs: vec![],
                environment: vec![],
                enabled: false,
            },
        );
        let root = PathBuf::from(std::env::var_os("ROOT_DIR").unwrap());
        let lock_path = root.join("root.lockfile");
        if lock_path.exists() {
            std::fs::remove_file(&lock_path).unwrap();
        }
        let err = apply_canonical(&env, "claude", "deadbeef", &[], true)
            .unwrap_err()
            .to_string();
        assert_eq!(err, crate::claude::CLAUDE_MCP_HELD_ERROR);
        assert!(
            !lock_path.exists(),
            "Claude MCP gate must fire before lock acquisition"
        );
    }

    #[test]
    fn version_gates_fail_closed() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("vergates");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();

        // 0.150.1 ok (plan succeeds).
        let env_ok = minimal_env_for("codex", "0.150.1");
        assert!(plan_canonical_apply(&env_ok, "codex").is_ok());

        // Unsupported SOURCE fails closed at plan.
        let env_bad_src = minimal_env_for("codex", "9.9.9");
        let err = plan_canonical_apply(&env_bad_src, "codex")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported source agent version"),
            "got: {err}"
        );
        assert!(err.contains("9.9.9"), "got: {err}");
        assert!(err.contains("0.150.1"), "got: {err}");

        // Unsupported TARGET fails closed at apply (names exact list).
        let bindir2 = tmp.path().join("bin2");
        std::fs::create_dir_all(&bindir2).unwrap();
        make_shim(&bindir2, "codex", "codex-cli 9.9.9");
        isolate_env(&tmp, Some(&bindir2));
        // Re-create source file under new isolation (isolate_env wiped CODEX_HOME).
        let codex_home2 = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home2.join("AGENTS.md"), "# src\n").unwrap();
        let plan = plan_canonical_apply(&env_ok, "codex").unwrap();
        assert!(
            !plan.warnings.is_empty(),
            "ungated target must warn at plan"
        );
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        let err = apply_canonical(&env_ok, "codex", &plan.plan_hash, &approvals, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("9.9.9"), "got: {err}");
        assert!(err.contains("0.150.1"), "got: {err}");
    }

    #[test]
    fn unknown_target_keys_preserved() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("unknownkeys");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        make_shim(&bindir, "opencode", "1.18.27");
        isolate_env(&tmp, Some(&bindir));

        // Codex: extra table + extra MCP server survive.
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        std::fs::write(
            codex_home.join("config.toml"),
            "model = \"old\"\ntarget_only_experimental = \"preserve-this-value\"\n\n[mcp_servers.other]\ncommand = \"echo\"\nenabled = true\n",
        )
        .unwrap();
        let mut env = minimal_env_for("codex", "0.150.1");
        env.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "gpt-5".to_string(),
        });
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true).unwrap();
        let after = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
        assert!(
            after.contains("preserve-this-value"),
            "extra key lost:\n{after}"
        );
        assert!(
            after.contains("[mcp_servers.other]"),
            "extra server lost:\n{after}"
        );

        // OpenCode: $schema/experimental/remote/timeout survive.
        let oc = PathBuf::from(std::env::var_os("OPENCODE_CONFIG_DIR").unwrap());
        std::fs::write(
            oc.join("opencode.json"),
            r#"{
  "$schema": "https://example.invalid/schema.json",
  "model": "old",
  "experimental": { "keep": true },
  "mcp": {
    "other": { "type": "remote", "url": "https://example.invalid", "enabled": true },
    "github": { "type": "local", "command": ["npx", "-y", "pkg"], "enabled": true, "timeout": 30 }
  }
}"#,
        )
        .unwrap();
        // Source for opencode apply: opencode home AGENTS.md.
        std::fs::write(oc.join("AGENTS.md"), "# src oc\n").unwrap();
        let mut env_oc = minimal_env_for("opencode", "1.18.27");
        env_oc.tools.push(CanonicalTool {
            name: "model".to_string(),
            value: "gpt-y".to_string(),
        });
        let plan_oc = plan_canonical_apply(&env_oc, "opencode").unwrap();
        let approvals_oc: Vec<String> = plan_oc
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        apply_canonical(&env_oc, "opencode", &plan_oc.plan_hash, &approvals_oc, true).unwrap();
        let cfg_path = crate::opencode::live_config_path().unwrap();
        let v = crate::opencode::load_config_value(&cfg_path).unwrap();
        assert_eq!(v["$schema"], "https://example.invalid/schema.json");
        assert_eq!(v["experimental"]["keep"], true);
        assert_eq!(v["mcp"]["other"]["url"], "https://example.invalid");
    }

    #[test]
    fn dummy_token_never_persisted_and_enable_needs_real_env() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("dummy");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        std::env::remove_var("GITHUB_TOKEN");
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();

        let env = codex_env_with_mcp();
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true).unwrap();
        let cfg = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
        assert!(
            cfg.contains("GITHUB_TOKEN"),
            "config must reference env name, got:\n{cfg}"
        );
        assert!(
            !cfg.contains("dummy") && !cfg.contains("sk-") && !cfg.contains("ghp_"),
            "config must never contain values, got:\n{cfg}"
        );
        // Enable with unset var fails with missing-secret family (provenance ok).
        let ep = crate::codex::enable_plan("github").unwrap();
        let err = crate::apply::enable_server(
            "github",
            &ep.plan_hash,
            std::slice::from_ref(&ep.descriptor_hash),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("secret references missing"), "got: {err}");
    }

    #[test]
    fn verify_agent_delegates_and_rejects_unknown() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("verify");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let report = verify_agent("codex").unwrap();
        assert_eq!(report.agent, "codex");
        assert!(verify_agent("gemini").is_err());
    }

    #[test]
    fn held_not_dropped_lists_skipped_tools() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("held");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "opencode", "1.18.27");
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        // service_tier is allowlisted for codex but NOT opencode → held.
        let mut env = minimal_env_for("codex", "0.150.1");
        env.tools.push(CanonicalTool {
            name: "service_tier".to_string(),
            value: "flex".to_string(),
        });
        let plan = plan_canonical_apply(&env, "opencode").unwrap();
        assert!(
            plan.held.iter().any(|h| h.source.contains("service_tier")),
            "non-allowlisted tool must be held, got: {:?}",
            plan.held
        );
        // Rendered opencode config must NOT contain service_tier.
        let (_, render_held) =
            render_canonical(&env, "opencode", &BTreeMap::new()).unwrap_or_else(|_| {
                // render needs content; use empty content with held instruction (still renders config).
                // Fall back: collect then render.
                let c = collect_source_content(&env).unwrap();
                render_canonical(&env, "opencode", &c.files).unwrap()
            });
        assert!(
            render_held
                .iter()
                .any(|h| h.source.contains("service_tier")),
            "render held must list skipped tool"
        );
    }

    #[test]
    fn plan_hash_equals_apply_binding_hash() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("bindhash");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "opencode", "1.18.27");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();

        let env = minimal_env_for("codex", "0.150.1");
        let binding = binding_plan_hash(&env, "opencode").unwrap();
        let plan = plan_canonical_apply(&env, "opencode").unwrap();
        assert_eq!(
            plan.plan_hash, binding,
            "plan_canonical_apply must equal binding_plan_hash"
        );
        let translation = crate::translate::plan_translation("codex", &env, "opencode").unwrap();
        assert_eq!(
            translation.plan_hash, binding,
            "plan_translation must emit the same hash apply accepts"
        );
        // The binding hash is directly applyable.
        apply_canonical(&env, "opencode", &binding, &[], true).unwrap();
    }

    #[test]
    fn apply_refuses_when_source_content_absent() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("absentapply");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        // Empty source home: no AGENTS.md.
        let env = minimal_env_for("codex", "0.150.1");
        let err = apply_canonical(&env, "codex", "deadbeef", &[], true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not available"), "got: {err}");
        assert!(err.contains("Sprint 013"), "got: {err}");
    }

    #[test]
    fn plan_reports_absent_source_content() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("absentplan");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let env = minimal_env_for("codex", "0.150.1");
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        assert!(
            plan.held.iter().any(|h| h.source.contains("AGENTS.md")),
            "held must name the absent artifact, got: {:?}",
            plan.held
        );
        assert!(
            plan.warnings.iter().any(|w| w.contains("absent")),
            "absent source content must warn, got: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn codex_mcp_unknown_server_fields_preserved() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("mcpunknown");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        make_shim(&bindir, "codex", "codex-cli 0.150.1");
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# src\n").unwrap();
        std::fs::write(
            codex_home.join("config.toml"),
            "[mcp_servers.github]\ntimeout = 30\nenabled = true\nfuture_key = \"keep-me\"\n",
        )
        .unwrap();

        let env = codex_env_with_mcp();
        let plan = plan_canonical_apply(&env, "codex").unwrap();
        let approvals: Vec<String> = plan
            .needs_approval
            .iter()
            .map(|a| a.sha256.clone())
            .collect();
        apply_canonical(&env, "codex", &plan.plan_hash, &approvals, true).unwrap();

        let text = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
        let after: toml_edit::DocumentMut = text.parse().unwrap();
        let server = after
            .get("mcp_servers")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .expect("github server must exist");
        assert_eq!(
            server.get("timeout").and_then(|v| v.as_integer()),
            Some(30),
            "unknown timeout must survive:\n{text}"
        );
        assert_eq!(
            server.get("future_key").and_then(|v| v.as_str()),
            Some("keep-me"),
            "unknown future key must survive:\n{text}"
        );
        assert_eq!(
            server.get("enabled").and_then(|v| v.as_bool()),
            Some(false),
            "enabled must be forced false:\n{text}"
        );
    }
}
