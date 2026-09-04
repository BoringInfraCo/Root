//! Crash-recovery journal: `~/.root/agent-apply.json`.
//!
//! Multi-file apply is NOT globally atomic. The journal records op id,
//! target preconditions, snapshot id, phase, and completed paths under the
//! global mutation lock so a crash can be recovered via rollback.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planned,
    Snapshotted,
    Applying,
    Verifying,
    Done,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyJournal {
    pub op_id: String,
    pub agent: String,
    pub plan_hash: String,
    pub snapshot_id: Option<String>,
    /// Tamper-evidence binding: canonical hash of the snapshot manifest at
    /// the time it was taken (and re-recorded after each applied-hash
    /// update). Rollback verifies this before restoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_manifest_hash: Option<String>,
    pub phase: Phase,
    pub completed_paths: Vec<String>,
    pub target_preconditions: BTreeMap<String, String>,
    /// MCP command descriptors imported by a completed bundle apply. This is
    /// the provenance allowlist used by the separately protected `enable`
    /// mutation. Keys are server ids; values are canonical descriptor hashes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_provenance: BTreeMap<String, String>,
    /// Provenance state to restore if this operation is rolled back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prior_mcp_provenance: BTreeMap<String, String>,
}

pub fn journal_path() -> Result<PathBuf> {
    Ok(root_lockfile::get_root_dir()?.join("agent-apply.json"))
}

pub fn read_journal() -> Result<Option<ApplyJournal>> {
    let path = journal_path()?;
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("Failed to read apply journal"),
        Ok(bytes) => {
            let j: ApplyJournal =
                serde_json::from_slice(&bytes).context("Failed to parse apply journal")?;
            Ok(Some(j))
        }
    }
}

/// Write journal (0600, tmp-beside + rename + fsync).
pub fn write_journal(journal: &ApplyJournal) -> Result<()> {
    let path = journal_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(journal)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), nonce));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .context("Failed to create temporary apply journal")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn new_op_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "op_{}_{}_{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

/// Refuse new mutations when a previous op is incomplete.
pub fn require_no_incomplete_op() -> Result<()> {
    if let Some(j) = read_journal()? {
        if phase_requires_recovery(j.phase) {
            anyhow::bail!(
                "Incomplete agent-bundle operation {} (phase {:?}). Run `root agent-bundle rollback --last` to recover before starting a new mutation.",
                j.op_id,
                j.phase
            );
        }
    }
    Ok(())
}

/// A failed operation remains incomplete until its retained snapshot is
/// successfully restored. Treating `Failed` as terminal would let a later
/// mutation overwrite the recovery journal and strand the snapshot.
pub fn phase_requires_recovery(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Snapshotted | Phase::Applying | Phase::Verifying | Phase::Failed
    )
}

/// Namespaced provenance key so Codex and OpenCode cannot authorize each other.
pub fn mcp_provenance_key(adapter: &str, server: &str) -> String {
    format!("{}:{}", adapter, server)
}

/// Return provenance only from a completed state transition. In-progress or
/// failed journals must never authorize an executable MCP command.
pub fn completed_mcp_provenance() -> Result<BTreeMap<String, String>> {
    match read_journal()? {
        Some(j) if matches!(j.phase, Phase::Done | Phase::RolledBack) => Ok(j.mcp_provenance),
        _ => Ok(BTreeMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{phase_requires_recovery, Phase};

    #[test]
    fn failed_phase_still_requires_recovery() {
        assert!(phase_requires_recovery(Phase::Snapshotted));
        assert!(phase_requires_recovery(Phase::Applying));
        assert!(phase_requires_recovery(Phase::Verifying));
        assert!(phase_requires_recovery(Phase::Failed));
        assert!(!phase_requires_recovery(Phase::Planned));
        assert!(!phase_requires_recovery(Phase::Done));
        assert!(!phase_requires_recovery(Phase::RolledBack));
    }
}
