//! Transactional (crash-recoverable) apply, rollback, purge, enable.
//!
//! Multi-file apply is NOT globally atomic. Every mutation holds the global
//! mutation lock (`root.lockfile`), records an `agent-apply.json` journal
//! (op id, plan hash, snapshot id, phase, completed paths), writes temp files
//! beside each target + fsync + rename, and auto-rollbacks from the snapshot
//! when post-verify fails.

use crate::journal::{
    completed_mcp_provenance, mcp_provenance_key, new_op_id, phase_requires_recovery,
    require_no_incomplete_op, write_journal, ApplyJournal, Phase,
};
use crate::lock::GlobalMutationLock;
use crate::manifest::{
    load_bundle, manifest_hash, unsupported_adapter_error, Manifest, ADAPTER_ID,
    OPENCODE_ADAPTER_ID,
};
use crate::plan::{compute_plan, plan_hash_for};
use crate::scope::{resolve_target, Scope};
use crate::snapshot::{
    list_snapshots, restore_snapshot, snapshot_path, take_snapshot, write_beside, AgentSnapshot,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    pub op_id: String,
    pub snapshot_id: String,
    pub plan_hash: String,
    pub applied: Vec<String>,
    pub skipped_identical: Vec<String>,
    pub mcp_imported: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollbackReport {
    pub snapshot_id: String,
    pub restored: Vec<String>,
}

/// Apply a bundle. `plan_hash` MUST be a `plan` output for the current target
/// state; it is revalidated under the lock (drift → error). `approvals` are
/// exact sha256 hex digests (files + MCP command hashes); no global boolean.
pub fn apply_bundle(
    bundle_dir: &Path,
    plan_hash: &str,
    approvals: &[String],
) -> Result<ApplyReport> {
    let manifest = load_bundle(bundle_dir)?;
    let _guard = GlobalMutationLock::acquire()?;
    require_no_incomplete_op()?;

    let prior_mcp_provenance = completed_mcp_provenance()?;
    let mut mcp_provenance = prior_mcp_provenance.clone();
    for (id, entry) in &manifest.mcp {
        let descriptor_hash = entry.command_sha256.clone().with_context(|| {
            format!(
                "Validated MCP server '{}' is missing its command descriptor hash",
                id
            )
        })?;
        mcp_provenance.insert(
            mcp_provenance_key(&manifest.adapter, id),
            descriptor_hash.to_lowercase(),
        );
    }

    // Recompute preconditions under lock; verify plan hash (drift detection).
    let plan = compute_plan(bundle_dir, &manifest)?;
    if plan.plan_hash != plan_hash {
        anyhow::bail!(
            "Drift detected: target state changed since plan (expected plan {}, computed {}). Re-run plan.",
            plan_hash,
            plan.plan_hash
        );
    }

    let approved: BTreeSet<String> = approvals.iter().map(|s| s.to_lowercase()).collect();
    for a in manifest.needs_approval.iter() {
        if !approved.contains(&a.sha256.to_lowercase()) {
            anyhow::bail!(
                "Missing hash-bound approval for '{}' (sha256:{}). Pass --approve <sha256> per item; global approval is forbidden.",
                a.rel,
                a.sha256
            );
        }
    }
    // Defensive: approvals must all correspond to something in the bundle.
    for ap in &approved {
        let known = manifest
            .needs_approval
            .iter()
            .any(|a| a.sha256.to_lowercase() == *ap)
            || manifest
                .files
                .iter()
                .any(|f| f.sha256.to_lowercase() == *ap);
        if !known {
            anyhow::bail!("Unknown approval hash '{}' (matches nothing in bundle)", ap);
        }
    }

    let op_id = new_op_id();
    let mut journal = ApplyJournal {
        op_id: op_id.clone(),
        agent: manifest.adapter.clone(),
        plan_hash: plan_hash.to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: Phase::Planned,
        completed_paths: Vec::new(),
        target_preconditions: plan.target_preconditions.clone(),
        mcp_provenance,
        prior_mcp_provenance,
    };
    write_journal(&journal)?;

    // Snapshot every target we may touch (files + adapter config when patched).
    let mut snap_targets: Vec<(Scope, String)> = manifest
        .files
        .iter()
        .map(|f| (f.scope, f.rel.clone()))
        .collect();
    if !manifest.settings.is_empty() || !manifest.mcp.is_empty() {
        let cfg = config_snap_target(&manifest)?;
        if !snap_targets.contains(&cfg) {
            snap_targets.push(cfg);
        }
    }
    let snap = take_snapshot(&manifest.adapter, &op_id, &snap_targets, None)?;
    journal.snapshot_id = Some(snap.id.clone());
    journal.snapshot_manifest_hash = Some(crate::snapshot::snapshot_manifest_hash(&snap)?);
    journal.phase = Phase::Snapshotted;
    write_journal(&journal)?;
    journal.phase = Phase::Applying;
    write_journal(&journal)?;

    let outcome: Result<ApplyReport> = (|| {
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        // 1. File blobs.
        for f in &manifest.files {
            let target = resolve_target(f.scope, &f.rel)?;
            let blob = crate::blob::read_blob(bundle_dir, &f.sha256)?;
            // Skip identical (avoid churning mtimes). Skipped paths keep
            // their tombstone semantics; record the (matching) hash anyway so
            // rollback can prove the file is unchanged.
            if let Ok(live) = std::fs::read(&target) {
                if root_lockfile::compute_sha256(&live) == f.sha256.to_lowercase()
                    || root_lockfile::compute_sha256(&live) == f.sha256
                {
                    let hash = crate::snapshot::record_expected_applied(
                        &snap.id, f.scope, &f.rel, &f.sha256,
                    )?;
                    journal.snapshot_manifest_hash = Some(hash);
                    skipped.push(format!("{}:{}", f.scope.as_str(), f.rel));
                    write_journal(&journal)?;
                    continue;
                }
            }
            let mode = parse_mode(&f.mode)?;
            let hash =
                crate::snapshot::record_expected_applied(&snap.id, f.scope, &f.rel, &f.sha256)?;
            journal.snapshot_manifest_hash = Some(hash);
            // Durably bind the expected post-write hash before the rename so
            // crash recovery can distinguish Root's bytes from user drift.
            write_journal(&journal)?;
            write_beside(&target, &blob, mode)?;
            let label = format!("{}:{}", f.scope.as_str(), f.rel);
            journal.completed_paths.push(label.clone());
            write_journal(&journal)?;
            applied.push(label);
        }
        // 2. Config patch (settings + MCP disabled).
        let mut mcp_imported = Vec::new();
        if !manifest.settings.is_empty() || !manifest.mcp.is_empty() {
            let (cfg_scope, cfg_rel) = config_snap_target(&manifest)?;
            let prepared = prepare_adapter_config(&manifest)?;
            let hash = crate::snapshot::record_expected_applied(
                &snap.id,
                cfg_scope,
                &cfg_rel,
                &prepared.sha256,
            )?;
            journal.snapshot_manifest_hash = Some(hash);
            write_journal(&journal)?;
            write_beside(&prepared.path, &prepared.bytes, prepared.mode)?;
            mcp_imported = manifest.mcp.keys().cloned().collect();
            let label = format!("{}:{}", cfg_scope.as_str(), cfg_rel);
            journal.completed_paths.push(label.clone());
            write_journal(&journal)?;
            applied.push(label);
        }
        // 3. Post-verify (before Done).
        journal.phase = Phase::Verifying;
        write_journal(&journal)?;
        verify_applied(bundle_dir, &manifest)?;
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

/// Restore after a failed mutation. When the restore itself fails, the
/// journal is marked `Failed` (NOT `RolledBack`) and the error preserves
/// rollback-failed semantics so recovery remains required. Always returns an
/// `Err` carrying the combined message.
fn auto_rollback(
    snap: &AgentSnapshot,
    journal: &mut ApplyJournal,
    op_name: &str,
    op_error: anyhow::Error,
) -> anyhow::Error {
    match restore_snapshot(snap) {
        Ok(rb) => {
            journal.phase = Phase::RolledBack;
            journal.mcp_provenance = journal.prior_mcp_provenance.clone();
            journal
                .completed_paths
                .push(format!("auto-rollback: {} entries", rb.len()));
            match write_journal(journal) {
                Ok(()) => match snapshot_path(&snap.id)
                    .and_then(|path| std::fs::remove_dir_all(path).map_err(Into::into))
                {
                    Ok(()) => anyhow::anyhow!(
                        "{} failed and was rolled back: {}. Rolled back {} paths.",
                        op_name,
                        op_error,
                        rb.len()
                    ),
                    Err(cleanup_error) => anyhow::anyhow!(
                        "{} failed: {}. Target data was rolled back ({} paths), but the restored snapshot could not be removed: {}. The journal records a completed rollback; inspect and purge the retained snapshot before another rollback.",
                        op_name,
                        op_error,
                        rb.len(),
                        cleanup_error
                    ),
                },
                Err(journal_error) => anyhow::anyhow!(
                    "{} failed: {}. Target data was rolled back ({} paths), but the recovery journal could not be updated: {}. The on-disk journal may still report an incomplete operation; inspect it before any further mutation.",
                    op_name,
                    op_error,
                    rb.len(),
                    journal_error
                ),
            }
        }
        Err(rb_error) => {
            journal.phase = Phase::Failed;
            match write_journal(journal) {
                Ok(()) => anyhow::anyhow!(
                    "{} failed: {}. Rollback failed: {}. Recovery still required — snapshot '{}' retained; resolve drift, then run rollback.",
                    op_name,
                    op_error,
                    rb_error,
                    snap.id
                ),
                Err(journal_error) => anyhow::anyhow!(
                    "{} failed: {}. Rollback failed: {}. The failed recovery state also could not be written to the journal: {}. Recovery still required — snapshot '{}' retained; do not start another mutation.",
                    op_name,
                    op_error,
                    rb_error,
                    journal_error,
                    snap.id
                ),
            }
        }
    }
}

fn config_snap_target(manifest: &Manifest) -> Result<(Scope, String)> {
    match manifest.adapter.as_str() {
        ADAPTER_ID => Ok((Scope::CodexHome, "config.toml".to_string())),
        OPENCODE_ADAPTER_ID => Ok((Scope::OpenCodeHome, crate::opencode::config_rel()?)),
        other => Err(unsupported_adapter_error(other)),
    }
}

fn prepare_adapter_config(manifest: &Manifest) -> Result<PreparedConfig> {
    match manifest.adapter.as_str() {
        ADAPTER_ID => prepare_config_toml(manifest),
        OPENCODE_ADAPTER_ID => prepare_config_opencode(manifest),
        other => Err(unsupported_adapter_error(other)),
    }
}

fn verify_applied(bundle_dir: &Path, manifest: &Manifest) -> Result<()> {
    match manifest.adapter.as_str() {
        ADAPTER_ID => crate::verify::verify_codex_applied(bundle_dir, manifest),
        OPENCODE_ADAPTER_ID => crate::verify::verify_opencode_applied(bundle_dir, manifest),
        other => Err(unsupported_adapter_error(other)),
    }
}

fn parse_mode(mode: &str) -> Result<u32> {
    match mode {
        "0644" => Ok(0o644),
        "0600" => Ok(0o600),
        "0755" => Ok(0o755),
        other => anyhow::bail!("unsupported mode '{}'", other),
    }
}

struct PreparedConfig {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
    mode: u32,
    sha256: String,
}

/// Render `$CODEX_HOME/config.toml` with allowlisted settings and disabled MCP
/// entries while preserving unknown target fields via `toml_edit`. This is
/// deliberately non-mutating: apply records and journals the expected digest
/// before handing these bytes to `write_beside`.
fn prepare_config_toml(manifest: &Manifest) -> Result<PreparedConfig> {
    use crate::codex::ALLOWED_SETTINGS;
    let home = crate::codex::codex_home()?;
    let path = home.join("config.toml");
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = if original.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        original
            .parse()
            .context("Failed to parse target config.toml")?
    };
    // Settings (string scalars only).
    for key in ALLOWED_SETTINGS {
        if let Some(val) = manifest.settings.get(*key) {
            if let Some(s) = val.as_str() {
                doc[*key] = toml_edit::value(s);
            }
        }
    }
    // Nested `doc["mcp_servers"][id] = Item::Table(...)` makes toml_edit
    // emit `mcp_servers = {}` and drop the server table. Insert into a
    // standard table so `[mcp_servers.<id>]` actually persists.
    if doc.get("mcp_servers").is_none() {
        let mut servers = toml_edit::Table::new();
        servers.set_implicit(true);
        doc["mcp_servers"] = toml_edit::Item::Table(servers);
    }
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(|item| item.as_table_mut())
        .context("target mcp_servers is not a table; refusing to patch")?;
    for (id, entry) in &manifest.mcp {
        if entry.enabled {
            anyhow::bail!("Refusing to apply enabled MCP entry '{}'", id);
        }
        if id.is_empty() || id.len() > 128 {
            anyhow::bail!("invalid MCP server id '{}'", id);
        }
        let mut table = toml_edit::Table::new();
        table["transport"] = toml_edit::value("stdio");
        if !entry.command.is_empty() {
            table["command"] = toml_edit::value(entry.command[0].clone());
        }
        let mut args = toml_edit::Array::new();
        for a in &entry.args {
            args.push(a.clone());
        }
        table["args"] = toml_edit::Item::Value(toml_edit::Value::Array(args));
        if let Some(cwd) = &entry.cwd {
            table["cwd"] = toml_edit::value(cwd.clone());
        }
        // env_vars whitelist (names only — never secret values).
        let mut env_vars = toml_edit::Array::new();
        for k in &entry.env_keys {
            env_vars.push(k.clone());
        }
        table["env_vars"] = toml_edit::Item::Value(toml_edit::Value::Array(env_vars));
        table["enabled"] = toml_edit::value(false);
        servers.insert(id, toml_edit::Item::Table(table));
    }
    let rendered = doc.to_string();
    if rendered.len() > 1024 * 1024 {
        anyhow::bail!("Patched config.toml exceeds size limit");
    }
    // Preserve existing mode (0600 expected) or use 0600 for new files.
    let mode = existing_file_mode(&path).unwrap_or(0o600);
    let bytes = rendered.into_bytes();
    let sha256 = root_lockfile::compute_sha256(&bytes);
    Ok(PreparedConfig {
        path,
        bytes,
        mode,
        sha256,
    })
}

fn prepare_config_opencode(manifest: &Manifest) -> Result<PreparedConfig> {
    let path = crate::opencode::live_config_path()?;
    let mut value = if path.exists() {
        crate::opencode::load_config_value(&path)?
    } else {
        serde_json::json!({})
    };
    crate::opencode::patch_config_value(&mut value, manifest)?;
    let bytes = crate::opencode::render_pretty_json(&value)?;
    let mode = existing_file_mode(&path).unwrap_or(0o600);
    let sha256 = root_lockfile::compute_sha256(&bytes);
    Ok(PreparedConfig {
        path,
        bytes,
        mode,
        sha256,
    })
}

/// Existing file mode masked to permission bits, or `None` when unknown.
#[cfg(unix)]
fn existing_file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn existing_file_mode(_path: &Path) -> Option<u32> {
    None
}

/// Roll back the most recent snapshot.
///
/// The snapshot is re-authenticated (id rules, schema, blob digests) and, when
/// the current journal references the same snapshot, bound against the
/// journal's manifest hash. A failed restore marks the journal `Failed` and
/// returns rollback-failed semantics so recovery remains required.
pub fn rollback_last() -> Result<RollbackReport> {
    let _guard = GlobalMutationLock::acquire()?;
    let prior_journal = crate::journal::read_journal()?;
    let snaps = list_snapshots()?;
    let recovery_id = prior_journal.as_ref().and_then(|journal| {
        phase_requires_recovery(journal.phase)
            .then(|| journal.snapshot_id.clone())
            .flatten()
    });
    let newest = if let Some(id) = recovery_id {
        snaps
            .iter()
            .find(|snapshot| snapshot.id == id)
            .cloned()
            .with_context(|| {
                format!(
                    "Recovery journal references missing snapshot '{}'; refusing to restore a different snapshot",
                    id
                )
            })?
    } else if prior_journal
        .as_ref()
        .is_some_and(|journal| phase_requires_recovery(journal.phase))
    {
        anyhow::bail!(
            "Recovery journal has no snapshot id; refusing to restore an unrelated snapshot"
        );
    } else {
        snaps.into_iter().next().context(
            "No agent snapshots available for rollback. Snapshots are created automatically before every apply or enable.",
        )?
    };
    let snap = crate::snapshot::load_snapshot(&newest.id)?;
    // Journal binding when the journal references this snapshot.
    if let Some(journal) = prior_journal.as_ref() {
        if journal.snapshot_id.as_deref() == Some(snap.id.as_str()) {
            if let Some(bound) = &journal.snapshot_manifest_hash {
                let current = crate::snapshot::snapshot_manifest_hash(&snap)?;
                if current != *bound {
                    // Incomplete ops record applied hashes on the snapshot
                    // before the journal hash is rewritten. load_snapshot
                    // already authenticated the snapshot; refusing here
                    // deadlocks recovery.
                    if !phase_requires_recovery(journal.phase) {
                        let detail = format!(
                            "snapshot manifest hash mismatch (journal {}, disk {})",
                            bound, current
                        );
                        if let Err(journal_error) =
                            mark_journal_failed(&snap.agent, Some(snap.id.clone()), detail)
                        {
                            anyhow::bail!(
                                "Rollback failed: snapshot '{}' manifest does not match the journal binding, and the failed state could not be recorded: {}. Recovery still required.",
                                snap.id,
                                journal_error
                            );
                        }
                        anyhow::bail!(
                            "Rollback failed: snapshot '{}' manifest does not match the journal binding (corrupt or tampered). Recovery still required.",
                            snap.id
                        );
                    }
                }
            }
        }
    }
    match restore_snapshot(&snap) {
        Ok(restored) => {
            let restored_provenance = prior_journal
                .as_ref()
                .filter(|journal| journal.snapshot_id.as_deref() == Some(snap.id.as_str()))
                .map(|journal| journal.prior_mcp_provenance.clone())
                .unwrap_or_default();
            let journal = ApplyJournal {
                op_id: crate::journal::new_op_id(),
                agent: snap.agent.clone(),
                plan_hash: String::new(),
                snapshot_id: Some(snap.id.clone()),
                snapshot_manifest_hash: None,
                phase: Phase::RolledBack,
                completed_paths: restored.clone(),
                target_preconditions: BTreeMap::new(),
                mcp_provenance: restored_provenance.clone(),
                prior_mcp_provenance: restored_provenance,
            };
            write_journal(&journal)?;
            // Remove only after persisting the successful recovery state. If
            // cleanup fails, the restored data and terminal journal remain
            // truthful; the retained snapshot can be purged explicitly.
            std::fs::remove_dir_all(snapshot_path(&snap.id)?).with_context(|| {
                format!(
                    "Rollback restored snapshot '{}', but its snapshot directory could not be removed; run purge after inspecting it",
                    snap.id
                )
            })?;
            Ok(RollbackReport {
                snapshot_id: snap.id,
                restored,
            })
        }
        Err(e) => {
            if let Err(journal_error) =
                mark_journal_failed(&snap.agent, Some(snap.id.clone()), format!("{}", e))
            {
                anyhow::bail!(
                    "Rollback failed for snapshot '{}': {}. The failed state could not be recorded: {}. Recovery still required.",
                    snap.id,
                    e,
                    journal_error
                );
            }
            anyhow::bail!(
                "Rollback failed for snapshot '{}': {}. Recovery still required.",
                snap.id,
                e
            );
        }
    }
}

fn mark_journal_failed(agent: &str, snapshot_id: Option<String>, detail: String) -> Result<()> {
    let mut journal = match crate::journal::read_journal()? {
        Some(existing) if existing.snapshot_id == snapshot_id => existing,
        _ => ApplyJournal {
            op_id: crate::journal::new_op_id(),
            agent: agent.to_string(),
            plan_hash: String::new(),
            snapshot_id,
            snapshot_manifest_hash: None,
            phase: Phase::Failed,
            completed_paths: Vec::new(),
            target_preconditions: BTreeMap::new(),
            mcp_provenance: BTreeMap::new(),
            prior_mcp_provenance: BTreeMap::new(),
        },
    };
    journal.phase = Phase::Failed;
    journal.completed_paths.push(format!("failure: {}", detail));
    write_journal(&journal)
}

/// Delete snapshots. Requires explicit `confirm == true` (purge confirmation).
pub fn purge_snapshots(id: Option<&str>, confirm: bool) -> Result<Vec<String>> {
    if !confirm {
        anyhow::bail!(
            "Snapshot purge requires explicit confirmation (--yes). No snapshots were deleted."
        );
    }
    let _guard = GlobalMutationLock::acquire()?;
    require_no_incomplete_op()?;
    if let Some(want) = id {
        if !crate::snapshot::valid_snapshot_id(want) {
            anyhow::bail!("invalid snapshot id");
        }
    }
    let snaps = list_snapshots()?;
    let mut deleted = Vec::new();
    for snap in snaps {
        if let Some(want) = id {
            if snap.id != want {
                continue;
            }
        }
        std::fs::remove_dir_all(snapshot_path(&snap.id)?)?;
        deleted.push(snap.id);
    }
    if id.is_some() && deleted.is_empty() {
        anyhow::bail!("Snapshot '{}' not found", id.unwrap_or(""));
    }
    Ok(deleted)
}

/// Enable a previously applied (disabled) MCP server. Full protected mutation:
/// fresh `enable-plan` hash + hash-bound approval of the LIVE canonical
/// command descriptor + required env presence + lock + journal + snapshot +
/// verify + rollback on failure.
///
/// `plan_hash` must come from `enable-plan` for the current config state;
/// `approvals` must contain the plan's `descriptor_hash` (exact match, no
/// extras). Approval is therefore bound to precisely the command being
/// enabled — a changed command invalidates both the plan and the approval.
pub fn enable_server(id: &str, plan_hash: &str, approvals: &[String]) -> Result<ApplyReport> {
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("invalid MCP server id");
    }
    // Read-only preflight before the lock, then revalidation under the lock.
    let preflight = crate::codex::enable_plan(id)?;
    check_enable_approval(&preflight, plan_hash, approvals)?;
    let _guard = GlobalMutationLock::acquire()?;
    require_no_incomplete_op()?;
    let live = crate::codex::enable_plan(id)?;
    if live.plan_hash != plan_hash || live.descriptor_hash != preflight.descriptor_hash {
        anyhow::bail!(
            "Drift detected: config changed since enable plan for '{}'. Re-run enable plan.",
            id
        );
    }
    check_enable_approval(&live, plan_hash, approvals)?;
    let mcp_provenance = completed_mcp_provenance()?;
    check_enable_provenance("codex", id, &live.descriptor_hash, &mcp_provenance)?;
    // Required secret references must exist in the environment (names only —
    // values are never read or stored).
    let missing: Vec<String> = live
        .needs_env
        .iter()
        .filter(|k| std::env::var_os(k).is_none())
        .cloned()
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "Cannot enable MCP server '{}': required secret references missing from environment: {}. Set them, then retry.",
            id,
            missing.join(", ")
        );
    }

    let op_id = new_op_id();
    let home = crate::codex::codex_home()?;
    let config_path = home.join("config.toml");
    let snap = take_snapshot(
        "codex",
        &op_id,
        &[(Scope::CodexHome, "config.toml".to_string())],
        None,
    )?;
    let mut journal = ApplyJournal {
        op_id: op_id.clone(),
        agent: "codex".to_string(),
        plan_hash: plan_hash.to_string(),
        snapshot_id: Some(snap.id.clone()),
        snapshot_manifest_hash: Some(crate::snapshot::snapshot_manifest_hash(&snap)?),
        phase: Phase::Applying,
        completed_paths: Vec::new(),
        target_preconditions: BTreeMap::from([(
            "codex_home:config.toml".to_string(),
            live.config_precondition.clone(),
        )]),
        mcp_provenance: mcp_provenance.clone(),
        prior_mcp_provenance: mcp_provenance,
    };
    write_journal(&journal)?;

    let outcome: Result<ApplyReport> = (|| {
        let text = std::fs::read_to_string(&config_path).context("config.toml not found")?;
        let mut doc: toml_edit::DocumentMut =
            text.parse().context("Failed to parse config.toml")?;
        if doc.get("mcp_servers").and_then(|v| v.get(id)).is_none() {
            anyhow::bail!("MCP server '{}' not found in config.toml", id);
        }
        doc["mcp_servers"][id]["enabled"] = toml_edit::value(true);
        let rendered = doc.to_string();
        if rendered.len() > 1024 * 1024 {
            anyhow::bail!("Patched config.toml exceeds size limit");
        }
        let rendered_hash = root_lockfile::compute_sha256(rendered.as_bytes());
        let manifest_hash = crate::snapshot::record_expected_applied(
            &snap.id,
            Scope::CodexHome,
            "config.toml",
            &rendered_hash,
        )?;
        journal.snapshot_manifest_hash = Some(manifest_hash);
        write_journal(&journal)?;
        let mode = existing_file_mode(&config_path).unwrap_or(0o600);
        write_beside(&config_path, rendered.as_bytes(), mode)?;
        journal.phase = Phase::Verifying;
        write_journal(&journal)?;
        crate::verify::verify_codex_enabled(id)?;
        Ok(ApplyReport {
            op_id: op_id.clone(),
            snapshot_id: snap.id.clone(),
            plan_hash: plan_hash.to_string(),
            applied: vec![format!("codex_home:config.toml (enable {})", id)],
            skipped_identical: vec![],
            mcp_imported: vec![id.to_string()],
        })
    })();

    match outcome {
        Ok(report) => {
            journal.phase = Phase::Done;
            write_journal(&journal)?;
            Ok(report)
        }
        Err(e) => Err(auto_rollback(&snap, &mut journal, "Enable", e)),
    }
}

/// OpenCode MCP enable: snapshot config, set `mcp.<id>.enabled = true`,
/// verify, auto-rollback on failure. Provenance from completed apply journal.
pub fn enable_opencode_server(
    id: &str,
    plan_hash: &str,
    approvals: &[String],
) -> Result<ApplyReport> {
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("invalid MCP server id");
    }
    let preflight = crate::opencode::enable_plan(id)?;
    check_enable_approval(&preflight, plan_hash, approvals)?;
    let _guard = GlobalMutationLock::acquire()?;
    require_no_incomplete_op()?;
    let live = crate::opencode::enable_plan(id)?;
    if live.plan_hash != plan_hash || live.descriptor_hash != preflight.descriptor_hash {
        anyhow::bail!(
            "Drift detected: config changed since enable plan for '{}'. Re-run enable plan.",
            id
        );
    }
    check_enable_approval(&live, plan_hash, approvals)?;
    let mcp_provenance = completed_mcp_provenance()?;
    check_enable_provenance(
        OPENCODE_ADAPTER_ID,
        id,
        &live.descriptor_hash,
        &mcp_provenance,
    )?;
    let missing: Vec<String> = live
        .needs_env
        .iter()
        .filter(|k| std::env::var_os(k).is_none())
        .cloned()
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "Cannot enable MCP server '{}': required secret references missing from environment: {}. Set them, then retry.",
            id,
            missing.join(", ")
        );
    }

    let op_id = new_op_id();
    let config_path = crate::opencode::live_config_path()?;
    let cfg_rel = crate::opencode::config_rel()?;
    let snap = take_snapshot(
        OPENCODE_ADAPTER_ID,
        &op_id,
        &[(Scope::OpenCodeHome, cfg_rel.clone())],
        None,
    )?;
    let mut journal = ApplyJournal {
        op_id: op_id.clone(),
        agent: OPENCODE_ADAPTER_ID.to_string(),
        plan_hash: plan_hash.to_string(),
        snapshot_id: Some(snap.id.clone()),
        snapshot_manifest_hash: Some(crate::snapshot::snapshot_manifest_hash(&snap)?),
        phase: Phase::Applying,
        completed_paths: Vec::new(),
        target_preconditions: BTreeMap::from([(
            format!("opencode_home:{}", cfg_rel),
            live.config_precondition.clone(),
        )]),
        mcp_provenance: mcp_provenance.clone(),
        prior_mcp_provenance: mcp_provenance,
    };
    write_journal(&journal)?;

    let outcome: Result<ApplyReport> = (|| {
        let mut value = crate::opencode::load_config_value(&config_path)?;
        crate::opencode::set_mcp_enabled(&mut value, id, true)?;
        let rendered = crate::opencode::render_pretty_json(&value)?;
        let rendered_hash = root_lockfile::compute_sha256(&rendered);
        let manifest_hash = crate::snapshot::record_expected_applied(
            &snap.id,
            Scope::OpenCodeHome,
            &cfg_rel,
            &rendered_hash,
        )?;
        journal.snapshot_manifest_hash = Some(manifest_hash);
        write_journal(&journal)?;
        let mode = existing_file_mode(&config_path).unwrap_or(0o600);
        write_beside(&config_path, &rendered, mode)?;
        journal.phase = Phase::Verifying;
        write_journal(&journal)?;
        crate::verify::verify_opencode_enabled(id)?;
        Ok(ApplyReport {
            op_id: op_id.clone(),
            snapshot_id: snap.id.clone(),
            plan_hash: plan_hash.to_string(),
            applied: vec![format!("opencode_home:{} (enable {})", cfg_rel, id)],
            skipped_identical: vec![],
            mcp_imported: vec![id.to_string()],
        })
    })();

    match outcome {
        Ok(report) => {
            journal.phase = Phase::Done;
            write_journal(&journal)?;
            Ok(report)
        }
        Err(e) => Err(auto_rollback(&snap, &mut journal, "Enable", e)),
    }
}

fn check_enable_approval(
    plan: &crate::codex::EnablePlan,
    plan_hash: &str,
    approvals: &[String],
) -> Result<()> {
    if plan.plan_hash != plan_hash {
        anyhow::bail!(
            "Enable plan hash mismatch for '{}'. Re-run enable plan.",
            plan.server
        );
    }
    let approved: BTreeSet<String> = approvals.iter().map(|s| s.to_lowercase()).collect();
    if !approved.contains(&plan.descriptor_hash.to_lowercase()) {
        anyhow::bail!(
            "Missing hash-bound approval for MCP server '{}' (descriptor sha256:{}). Pass --approve <sha256>; global approval is forbidden.",
            plan.server,
            plan.descriptor_hash
        );
    }
    for ap in &approved {
        if *ap != plan.descriptor_hash.to_lowercase() {
            anyhow::bail!(
                "Unknown approval hash '{}' (matches nothing in enable plan)",
                ap
            );
        }
    }
    Ok(())
}

fn check_enable_provenance(
    adapter: &str,
    server: &str,
    descriptor_hash: &str,
    provenance: &BTreeMap<String, String>,
) -> Result<()> {
    let key = mcp_provenance_key(adapter, server);
    match provenance.get(&key) {
        Some(imported_hash) if imported_hash.eq_ignore_ascii_case(descriptor_hash) => Ok(()),
        Some(_) => anyhow::bail!(
            "MCP server '{}' no longer matches its imported {} bundle provenance. Re-apply a reviewed bundle and generate a new enable plan.",
            server,
            adapter
        ),
        None => anyhow::bail!(
            "MCP server '{}' has no completed agent-bundle provenance for adapter '{}' and cannot be enabled by Root. Import it from a reviewed bundle first.",
            server,
            adapter
        ),
    }
}

/// Verify a plan hash is current (used by CLI preflight without mutation).
pub fn check_plan_current(bundle_dir: &Path, manifest: &Manifest, plan_hash: &str) -> Result<()> {
    let plan = compute_plan(bundle_dir, manifest)?;
    let _ = manifest_hash(manifest)?;
    let expect = plan_hash_for(
        &crate::manifest::manifest_hash(manifest)?,
        &plan.target_preconditions,
    )?;
    if expect != plan_hash {
        anyhow::bail!("Drift detected: plan hash is stale. Re-run plan.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_enable_approval, check_enable_provenance};
    use crate::codex::EnablePlan;
    use std::collections::BTreeMap;

    fn enable_plan() -> EnablePlan {
        EnablePlan {
            server: "reviewed".to_string(),
            descriptor_hash: "a".repeat(64),
            config_precondition: format!("sha256:{}", "b".repeat(64)),
            needs_env: Vec::new(),
            plan_hash: "c".repeat(64),
        }
    }

    #[test]
    fn enable_approval_is_exact_and_hash_bound() {
        let plan = enable_plan();
        assert!(check_enable_approval(
            &plan,
            &plan.plan_hash,
            std::slice::from_ref(&plan.descriptor_hash)
        )
        .is_ok());
        assert!(
            check_enable_approval(&plan, "stale", std::slice::from_ref(&plan.descriptor_hash))
                .is_err()
        );
        assert!(check_enable_approval(&plan, &plan.plan_hash, &[]).is_err());
        assert!(check_enable_approval(
            &plan,
            &plan.plan_hash,
            &[plan.descriptor_hash.clone(), "d".repeat(64)]
        )
        .is_err());
    }

    #[test]
    fn enable_requires_matching_completed_import_provenance() {
        let plan = enable_plan();
        let mut provenance = BTreeMap::new();
        assert!(
            check_enable_provenance("codex", &plan.server, &plan.descriptor_hash, &provenance)
                .is_err()
        );
        provenance.insert(
            crate::journal::mcp_provenance_key("codex", &plan.server),
            "d".repeat(64),
        );
        assert!(
            check_enable_provenance("codex", &plan.server, &plan.descriptor_hash, &provenance)
                .is_err()
        );
        provenance.insert(
            crate::journal::mcp_provenance_key("codex", &plan.server),
            plan.descriptor_hash.clone(),
        );
        assert!(
            check_enable_provenance("codex", &plan.server, &plan.descriptor_hash, &provenance)
                .is_ok()
        );
        assert!(check_enable_provenance(
            "opencode",
            &plan.server,
            &plan.descriptor_hash,
            &provenance
        )
        .is_err());
    }
}
