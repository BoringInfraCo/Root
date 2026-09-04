//! Agent snapshots: manifest + blobs (never one content-bearing JSON).
//!
//! Records for each touched target: tombstone when the path was missing
//! (so rollback deletes created files), original bytes hash, mode, and
//! parent directories created (so rollback can remove them when empty).
//! Snapshot dirs `0700`, files `0600` — snapshots may duplicate target
//! secrets already present on disk.

use crate::manifest::hash_file_capped;
use crate::scope::{check_duplicates, resolve_target, revalidate_target, validate_rel, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_SNAPSHOT_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_SNAPSHOT_BLOB_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SNAPSHOT_ENTRIES: usize = 512;
const MAX_SNAPSHOTS: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEntry {
    pub scope: Scope,
    pub rel: String,
    /// False when the path did not exist before apply (tombstone).
    pub existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default)]
    pub is_dir: bool,
    /// Scope-relative parent directories that did not exist before apply.
    /// Absolute paths are never persisted or trusted for cleanup.
    #[serde(default)]
    pub created_dirs: Vec<String>,
    /// Hash of the bytes apply wrote (recorded post-write). Rollback of a
    /// tombstoned path proceeds ONLY when the live file still matches this
    /// hash; any drift refuses rather than deleting user data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_sha256: Option<String>,
}

fn valid_snapshot_agent(agent: &str) -> bool {
    agent == crate::manifest::ADAPTER_ID || agent == crate::manifest::OPENCODE_ADAPTER_ID
}

/// Snapshot id rules: `asnap_` prefix, bounded, no separators/traversal.
pub fn valid_snapshot_id(id: &str) -> bool {
    if !id.starts_with("asnap_") || id.len() > 200 || id.len() < 8 {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSnapshot {
    pub schema_version: u32,
    pub id: String,
    pub agent: String,
    pub op_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    pub entries: Vec<SnapshotEntry>,
}

pub fn snapshots_dir() -> Result<PathBuf> {
    let root = root_lockfile::get_root_dir()?;
    Ok(root.join("agent-snapshots"))
}

pub fn snapshot_path(id: &str) -> Result<PathBuf> {
    if !valid_snapshot_id(id) {
        anyhow::bail!("invalid snapshot id");
    }
    let base = snapshots_dir()?;
    match std::fs::symlink_metadata(&base) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            anyhow::bail!("snapshot base '{}' is not a real directory", base.display())
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Cannot stat snapshot base"),
    }
    Ok(base.join(id))
}

/// Canonical hash of a snapshot manifest (tamper-evidence binding recorded in
/// the journal; blob digests inside the manifest authenticate contents).
pub fn snapshot_manifest_hash(snap: &AgentSnapshot) -> Result<String> {
    let bytes = serde_json::to_vec_pretty(snap).context("Failed to serialize snapshot")?;
    Ok(root_lockfile::compute_sha256(&bytes))
}

/// Load and fully authenticate a snapshot: id rules, bounded manifest read,
/// strict schema, and per-blob digest verification. Refuses on any mismatch.
pub fn load_snapshot(id: &str) -> Result<AgentSnapshot> {
    let dir = snapshot_path(id)?;
    require_real_dir(&dir, &format!("Snapshot '{}' directory", id))?;
    let manifest_path = dir.join("manifest.json");
    let meta = require_regular_file(&manifest_path, "snapshot manifest")
        .with_context(|| format!("Snapshot '{}' not found", id))?;
    if meta.len() > MAX_SNAPSHOT_MANIFEST_BYTES {
        anyhow::bail!("Snapshot '{}' manifest exceeds size limit", id);
    }
    let bytes = std::fs::read(&manifest_path).context("Failed to read snapshot manifest")?;
    let integrity_path = dir.join("manifest.sha256");
    let integrity_meta = require_regular_file(&integrity_path, "snapshot manifest digest")?;
    if integrity_meta.len() > 128 {
        anyhow::bail!("Snapshot '{}' manifest digest exceeds size limit", id);
    }
    let expected = std::fs::read_to_string(&integrity_path)
        .context("Failed to read snapshot manifest digest")?;
    let expected = expected.trim();
    if !crate::manifest::is_hex_sha256(expected) {
        anyhow::bail!("Snapshot '{}' has malformed manifest digest", id);
    }
    let actual_manifest_hash = root_lockfile::compute_sha256(&bytes);
    if !actual_manifest_hash.eq_ignore_ascii_case(expected) {
        anyhow::bail!(
            "Snapshot '{}' manifest digest mismatch: refusing rollback (corrupt or tampered)",
            id
        );
    }
    let snap: AgentSnapshot =
        serde_json::from_slice(&bytes).context("Snapshot manifest is corrupt")?;
    validate_snapshot(&snap, id)?;
    let blobs_dir = dir.join("blobs");
    require_real_dir(&blobs_dir, "snapshot blobs directory")?;
    // Authenticate every stored blob against its recorded digest.
    for entry in &snap.entries {
        if let Some(digest) = &entry.sha256 {
            let blob = blobs_dir.join(digest.to_lowercase());
            let bmeta = require_regular_file(&blob, "snapshot blob")
                .with_context(|| format!("Snapshot '{}' is missing a blob", id))?;
            if bmeta.len() > MAX_SNAPSHOT_BLOB_BYTES {
                anyhow::bail!("Snapshot '{}' blob exceeds size limit", id);
            }
            let actual = hash_file_capped(&blob, MAX_SNAPSHOT_BLOB_BYTES)?;
            if !actual.eq_ignore_ascii_case(digest) {
                anyhow::bail!(
                    "Snapshot '{}' blob digest mismatch: refusing rollback (corrupt or tampered)",
                    id
                );
            }
        }
    }
    Ok(snap)
}

fn validate_snapshot(snap: &AgentSnapshot, directory_id: &str) -> Result<()> {
    if snap.schema_version != SNAPSHOT_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported snapshot schema version {} (expected {})",
            snap.schema_version,
            SNAPSHOT_SCHEMA_VERSION
        );
    }
    if snap.id != directory_id || !valid_snapshot_id(&snap.id) {
        anyhow::bail!("Snapshot id mismatch (directory vs manifest)");
    }
    if !valid_snapshot_agent(&snap.agent) {
        anyhow::bail!(
            "unsupported snapshot agent '{}': expected codex or opencode",
            snap.agent
        );
    }
    if snap.op_id.is_empty()
        || snap.op_id.len() > 200
        || !snap
            .op_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("Snapshot '{}' has malformed operation id", snap.id);
    }
    if snap.created.as_ref().is_some_and(|s| s.len() > 128) {
        anyhow::bail!("Snapshot '{}' has oversized created metadata", snap.id);
    }
    if snap.entries.len() > MAX_SNAPSHOT_ENTRIES {
        anyhow::bail!("Snapshot '{}' has too many entries", snap.id);
    }
    let targets: Vec<_> = snap
        .entries
        .iter()
        .map(|e| (e.scope, e.rel.clone()))
        .collect();
    check_duplicates(&targets)?;
    for entry in &snap.entries {
        validate_rel(&entry.rel)?;
        if entry.is_dir {
            anyhow::bail!(
                "Snapshot '{}' contains a directory target; bundle v1 snapshots regular files only",
                snap.id
            );
        }
        if let Some(mode) = entry.mode {
            if mode > 0o777 {
                anyhow::bail!("Snapshot '{}' has unsupported file mode", snap.id);
            }
        }
        match (entry.existed, entry.sha256.as_deref(), entry.mode) {
            (true, Some(digest), Some(_)) if crate::manifest::is_hex_sha256(digest) => {}
            (false, None, None) => {}
            _ => anyhow::bail!("Snapshot '{}' has inconsistent entry metadata", snap.id),
        }
        if let Some(applied) = &entry.applied_sha256 {
            if !crate::manifest::is_hex_sha256(applied) {
                anyhow::bail!("Snapshot '{}' has malformed applied digest", snap.id);
            }
        }
        let entry_parent = Path::new(&entry.rel)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let mut seen = HashSet::new();
        for created_dir in &entry.created_dirs {
            validate_rel(created_dir)?;
            let created_path = Path::new(created_dir);
            if !entry_parent.starts_with(created_path) {
                anyhow::bail!(
                    "Snapshot '{}' has a created directory outside the target's proper parents",
                    snap.id
                );
            }
            if !seen.insert(created_dir.to_lowercase()) {
                anyhow::bail!("Snapshot '{}' has duplicate created directories", snap.id);
            }
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<std::fs::Metadata> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot stat {} {}", label, path.display()))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        anyhow::bail!(
            "{} '{}' is not a regular non-symlink file",
            label,
            path.display()
        );
    }
    Ok(meta)
}

fn require_real_dir(path: &Path, label: &str) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot stat {} {}", label, path.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        anyhow::bail!("{} '{}' is not a real directory", label, path.display());
    }
    Ok(())
}

struct IncompleteSnapshotDir {
    path: PathBuf,
    armed: bool,
}

impl Drop for IncompleteSnapshotDir {
    fn drop(&mut self) {
        if self.armed {
            // This path was freshly generated, validated, and created by this
            // process. Cleanup prevents a failed snapshot attempt from
            // leaving an unreadable directory that blocks future recovery.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Record the exact hash apply intends to write for one entry.
///
/// This MUST be called and durably journal-bound before mutating the target.
/// That ordering closes the crash window between a successful rename and a
/// post-write journal update. Rollback accepts either this expected hash or,
/// for a pre-existing target, the original snapshot hash.
pub fn record_expected_applied(
    snapshot_id: &str,
    scope: Scope,
    rel: &str,
    expected_sha256: &str,
) -> Result<String> {
    if !crate::manifest::is_hex_sha256(expected_sha256) {
        anyhow::bail!("refusing to record malformed expected-applied hash");
    }
    let snap = load_snapshot(snapshot_id)?;
    let mut updated = snap;
    let entry = updated
        .entries
        .iter_mut()
        .find(|e| e.scope == scope && e.rel == rel)
        .context("Snapshot entry not found for applied path")?;
    entry.applied_sha256 = Some(expected_sha256.to_lowercase());
    let dir = snapshot_path(snapshot_id)?;
    write_snapshot_manifest(&dir, &updated)?;
    snapshot_manifest_hash(&updated)
}

/// Backward-compatible alias. New mutation call sites must use
/// `record_expected_applied` before writing, not after writing.
pub fn record_applied(
    snapshot_id: &str,
    scope: Scope,
    rel: &str,
    applied_sha256: &str,
) -> Result<String> {
    record_expected_applied(snapshot_id, scope, rel, applied_sha256)
}

fn write_snapshot_manifest(dir: &Path, snap: &AgentSnapshot) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(snap)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_MANIFEST_BYTES {
        anyhow::bail!("Snapshot manifest exceeds size limit");
    }
    let digest = root_lockfile::compute_sha256(&bytes);
    snapshot_write_atomic(&dir.join("manifest.json"), &bytes)?;
    snapshot_write_atomic(&dir.join("manifest.sha256"), digest.as_bytes())?;
    Ok(())
}

pub(crate) fn snapshot_write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Snapshot path has no parent")?;
    let tmp = parent.join(format!(".manifest.{}.{}.tmp", std::process::id(), nanos()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path).context("Failed to rewrite snapshot manifest")?;
    if let Ok(parent_dir) = std::fs::File::open(parent) {
        let _ = parent_dir.sync_all();
    }
    Ok(())
}

/// Take a snapshot of the given targets. Copies original bytes into
/// `<snap>/blobs/<sha256>` for files that exist.
pub fn take_snapshot(
    agent: &str,
    op_id: &str,
    targets: &[(Scope, String)],
    created: Option<String>,
) -> Result<AgentSnapshot> {
    if !valid_snapshot_agent(agent) {
        anyhow::bail!(
            "unsupported snapshot agent '{}': expected codex or opencode",
            agent
        );
    }
    if op_id.is_empty()
        || op_id.len() > 200
        || !op_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("malformed snapshot operation id");
    }
    if targets.len() > MAX_SNAPSHOT_ENTRIES {
        anyhow::bail!("too many snapshot targets");
    }
    check_duplicates(targets)?;
    let id = format!(
        "asnap_{}_{}_{}",
        chrono_now_compact(),
        std::process::id(),
        op_id
    );
    if !valid_snapshot_id(&id) {
        anyhow::bail!("generated snapshot id is invalid or exceeds its size limit");
    }
    let base = snapshots_dir()?;
    if let Some(root) = base.parent() {
        std::fs::create_dir_all(root).context("Failed to create Root state directory")?;
    }
    match std::fs::create_dir(&base) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).context("Failed to create snapshot base"),
    }
    require_real_dir(&base, "snapshot base")?;
    let dir = snapshot_path(&id)?;
    std::fs::create_dir(&dir).context("Failed to create snapshot dir")?;
    std::fs::create_dir(dir.join("blobs")).context("Failed to create snapshot blobs dir")?;
    let mut incomplete = IncompleteSnapshotDir {
        path: dir.clone(),
        armed: true,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(dir.join("blobs"), std::fs::Permissions::from_mode(0o700))?;
    }
    let mut entries = Vec::new();
    for (scope, rel) in targets {
        let target = resolve_target(*scope, rel)?;
        // Determine which parents are missing (for later rmdir).
        let mut created_dirs = Vec::new();
        let root = scope_root_for(*scope)?;
        if let Some(parent_rel) = Path::new(rel).parent() {
            let mut relative_prefix = PathBuf::new();
            for comp in parent_rel.components() {
                relative_prefix.push(comp.as_os_str());
                let relative = relative_prefix
                    .to_str()
                    .context("Snapshot target path is not valid UTF-8")?;
                let parent_target = resolve_target(*scope, relative)?;
                revalidate_target(*scope, relative, &parent_target)?;
                if !root.join(&relative_prefix).exists() {
                    created_dirs.push(relative.to_string());
                }
            }
        }
        let meta = std::fs::symlink_metadata(&target);
        match meta {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                entries.push(SnapshotEntry {
                    scope: *scope,
                    rel: rel.clone(),
                    existed: false,
                    sha256: None,
                    mode: None,
                    is_dir: false,
                    created_dirs,
                    applied_sha256: None,
                });
            }
            Err(e) => return Err(e).context("Failed to stat target"),
            Ok(m) => {
                if m.file_type().is_symlink() {
                    anyhow::bail!("Refusing to snapshot symlink {}", target.display());
                }
                if !m.is_file() {
                    anyhow::bail!("Refusing to snapshot non-regular file {}", target.display());
                }
                revalidate_target(*scope, rel, &target)?;
                let digest = hash_file_capped(&target, MAX_SNAPSHOT_BLOB_BYTES)?;
                let blob_dest = dir.join("blobs").join(&digest);
                if !blob_dest.exists() {
                    std::fs::copy(&target, &blob_dest)?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&blob_dest, std::fs::Permissions::from_mode(0o600))?;
                }
                entries.push(SnapshotEntry {
                    scope: *scope,
                    rel: rel.clone(),
                    existed: true,
                    sha256: Some(digest),
                    mode: mode_of(&m),
                    is_dir: false,
                    created_dirs,
                    applied_sha256: None,
                });
            }
        }
    }
    let snap = AgentSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        id: id.clone(),
        agent: agent.to_string(),
        op_id: op_id.to_string(),
        created,
        entries,
    };
    validate_snapshot(&snap, &id)?;
    write_snapshot_manifest(&dir, &snap)?;
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    incomplete.armed = false;
    Ok(snap)
}

fn scope_root_for(scope: Scope) -> Result<PathBuf> {
    crate::scope::scope_root(scope)
}

#[cfg(unix)]
fn mode_of(m: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_m: &std::fs::Metadata) -> Option<u32> {
    None
}

fn chrono_now_compact() -> String {
    // No chrono dep in this crate: use SystemTime secs + nanos.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{:09}", now.as_secs(), now.subsec_nanos())
}

/// Restore a snapshot (used by rollback).
///
/// Safety rules:
/// - Tombstoned paths (missing before apply): the live path must be a regular
///   non-symlink file whose hash still equals `applied_sha256`. Directories
///   are NEVER recursively deleted; drift refuses instead of destroying user
///   data. A path that is already absent counts as restored.
/// - Pre-existing files: original bytes are rewritten, unless the live file
///   matches NEITHER the applied hash NOR the original hash (post-apply user
///   edits) — then rollback refuses with drift rather than overwriting.
/// - Created parent dirs: non-recursive `remove_dir` only (silently skips
///   non-empty or non-directory paths).
pub fn restore_snapshot(snap: &AgentSnapshot) -> Result<Vec<String>> {
    // Re-authenticate at restore time (manifest may have changed on disk).
    let authenticated = load_snapshot(&snap.id)?;
    let snap = &authenticated;
    let dir = snapshot_path(&snap.id)?;
    let mut restored = Vec::new();
    for entry in &snap.entries {
        let target = resolve_target(entry.scope, &entry.rel)?;
        if !entry.existed {
            let applied = entry.applied_sha256.as_ref().with_context(|| {
                format!(
                    "refusing rollback of '{}': no applied hash recorded",
                    target.display()
                )
            })?;
            match std::fs::symlink_metadata(&target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    restored.push(format!("already-absent {}", target.display()));
                }
                Err(e) => return Err(e).context("Failed to stat rollback target"),
                Ok(m) => {
                    if m.file_type().is_symlink() || !m.is_file() {
                        anyhow::bail!(
                            "refusing rollback of '{}': target is not a regular non-symlink file (drift)",
                            target.display()
                        );
                    }
                    revalidate_target(entry.scope, &entry.rel, &target)?;
                    let live = hash_file_capped(&target, MAX_SNAPSHOT_BLOB_BYTES)?;
                    if !live.eq_ignore_ascii_case(applied) {
                        anyhow::bail!(
                            "refusing rollback of '{}': live file changed since apply (drift — back it up and remove it manually)",
                            target.display()
                        );
                    }
                    revalidate_target(entry.scope, &entry.rel, &target)?;
                    std::fs::remove_file(&target)?;
                    restored.push(format!("removed {}", target.display()));
                }
            }
            cleanup_created_dirs(entry, &mut restored)?;
            continue;
        }
        let digest = entry
            .sha256
            .as_ref()
            .context("Snapshot entry missing hash")?;
        let blob = dir.join("blobs").join(digest.to_lowercase());
        let original = std::fs::read(&blob).context("Missing snapshot blob")?;
        let live_meta = std::fs::symlink_metadata(&target).with_context(|| {
            format!(
                "refusing rollback of '{}': pre-existing file is now absent (drift)",
                target.display()
            )
        })?;
        if live_meta.file_type().is_symlink() || !live_meta.is_file() {
            anyhow::bail!(
                "refusing rollback of '{}': target is not a regular non-symlink file (drift)",
                target.display()
            );
        }
        revalidate_target(entry.scope, &entry.rel, &target)?;
        let live = hash_file_capped(&target, MAX_SNAPSHOT_BLOB_BYTES)?;
        let original_hash = root_lockfile::compute_sha256(&original);
        let is_original = live.eq_ignore_ascii_case(&original_hash);
        let is_expected = entry
            .applied_sha256
            .as_ref()
            .is_some_and(|expected| live.eq_ignore_ascii_case(expected));
        if !is_original && !is_expected {
            anyhow::bail!(
                "refusing rollback of '{}': live file changed since apply (drift)",
                target.display()
            );
        }
        revalidate_target(entry.scope, &entry.rel, &target)?;
        write_beside(&target, &original, entry.mode.unwrap_or(0o600))?;
        restored.push(format!("restored {}", target.display()));
        cleanup_created_dirs(entry, &mut restored)?;
    }
    Ok(restored)
}

fn cleanup_created_dirs(entry: &SnapshotEntry, restored: &mut Vec<String>) -> Result<()> {
    let mut dirs = entry.created_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(Path::new(d).components().count()));
    for rel in dirs {
        // `load_snapshot` already validates that this is a constrained proper
        // parent of entry.rel. Resolve again immediately before deletion so a
        // replaced ancestor symlink cannot redirect cleanup.
        let path = resolve_target(entry.scope, &rel)?;
        revalidate_target(entry.scope, &rel, &path)?;
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("Failed to stat created directory"),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                anyhow::bail!(
                    "refusing rollback cleanup of '{}': expected an unchanged directory (drift)",
                    path.display()
                );
            }
            Ok(_) => match std::fs::remove_dir(&path) {
                Ok(()) => restored.push(format!("removed-dir {}", path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(e) => return Err(e).context("Failed to remove created directory"),
            },
        }
    }
    Ok(())
}

/// Write bytes beside the target then rename (crash-safe, same-dir atomic).
pub fn write_beside(target: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = target.parent().context("Target has no parent")?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        nanos()
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        use std::io::Write;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, target).context("Failed to rename into place")?;
    if let Ok(d) = std::fs::File::open(parent) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// List snapshots (newest first by name).
pub fn list_snapshots() -> Result<Vec<AgentSnapshot>> {
    let dir = snapshots_dir()?;
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("Cannot stat snapshot base"),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            anyhow::bail!("snapshot base '{}' is not a real directory", dir.display())
        }
        Ok(_) => {}
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            if file_type.is_symlink() {
                anyhow::bail!(
                    "snapshot base contains symlink '{}'; refusing",
                    entry.path().display()
                );
            }
            continue;
        }
        if out.len() >= MAX_SNAPSHOTS {
            anyhow::bail!("snapshot count exceeds limit of {}", MAX_SNAPSHOTS);
        }
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("snapshot directory name is not valid UTF-8"))?;
        if !valid_snapshot_id(&id) {
            anyhow::bail!(
                "snapshot base contains invalid snapshot directory '{}': refusing",
                id
            );
        }
        out.push(load_snapshot(&id)?);
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.id.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        root_dir: Option<std::ffi::OsString>,
        codex_home: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn isolate(tmp: &Path) -> Self {
            let guard = Self {
                root_dir: std::env::var_os("ROOT_DIR"),
                codex_home: std::env::var_os("CODEX_HOME"),
            };
            let root = tmp.join("root");
            let codex = tmp.join("codex");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&codex).unwrap();
            std::env::set_var("ROOT_DIR", root);
            std::env::set_var("CODEX_HOME", codex);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.root_dir.take() {
                Some(v) => std::env::set_var("ROOT_DIR", v),
                None => std::env::remove_var("ROOT_DIR"),
            }
            match self.codex_home.take() {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }

    fn unique_tmp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "root_snapshot_{}_{}_{}",
            label,
            std::process::id(),
            nanos()
        ))
    }

    #[test]
    fn snapshot_ids_are_strictly_constrained() {
        assert!(valid_snapshot_id("asnap_123_op-test"));
        for invalid in [
            "../asnap_x",
            "asnap_../x",
            "asnap_x/y",
            "asnap_x.y",
            "wrong_123",
        ] {
            assert!(!valid_snapshot_id(invalid), "accepted: {invalid}");
        }
    }

    #[test]
    fn load_rejects_manifest_digest_mismatch() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp = unique_tmp("manifest_tamper");
        let _env = EnvGuard::isolate(&tmp);
        let snap = take_snapshot(
            "codex",
            "op_test",
            &[(Scope::CodexHome, "new.txt".to_string())],
            None,
        )
        .unwrap();
        let manifest = snapshot_path(&snap.id).unwrap().join("manifest.json");
        let mut bytes = std::fs::read(&manifest).unwrap();
        bytes.push(b' ');
        std::fs::write(manifest, bytes).unwrap();
        let err = load_snapshot(&snap.id).unwrap_err();
        assert!(err.to_string().contains("digest mismatch"), "got: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_rejects_tampered_blob() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp = unique_tmp("blob_tamper");
        let _env = EnvGuard::isolate(&tmp);
        std::fs::write(tmp.join("codex").join("existing.txt"), b"original").unwrap();
        let snap = take_snapshot(
            "codex",
            "op_test",
            &[(Scope::CodexHome, "existing.txt".to_string())],
            None,
        )
        .unwrap();
        let digest = snap.entries[0].sha256.as_ref().unwrap();
        let blob = snapshot_path(&snap.id).unwrap().join("blobs").join(digest);
        std::fs::write(blob, b"tampered").unwrap();
        let err = load_snapshot(&snap.id).unwrap_err();
        assert!(
            err.to_string().contains("blob digest mismatch"),
            "got: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn created_directories_are_scope_relative_and_cannot_be_redirected() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp = unique_tmp("created_dirs");
        let _env = EnvGuard::isolate(&tmp);
        let snap = take_snapshot(
            "codex",
            "op_test",
            &[(Scope::CodexHome, "nested/leaf/new.txt".to_string())],
            None,
        )
        .unwrap();
        assert_eq!(snap.entries[0].created_dirs, ["nested", "nested/leaf"]);

        let mut malicious = snap.clone();
        malicious.entries[0].created_dirs = vec!["../outside".to_string()];
        let dir = snapshot_path(&snap.id).unwrap();
        write_snapshot_manifest(&dir, &malicious).unwrap();
        let err = load_snapshot(&snap.id).unwrap_err();
        assert!(
            err.to_string().contains("invalid bundle path"),
            "got: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn snapshot_rejects_non_regular_targets() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let tmp = unique_tmp("non_regular");
        let _env = EnvGuard::isolate(&tmp);
        std::fs::create_dir(tmp.join("codex").join("directory-target")).unwrap();
        let err = take_snapshot(
            "codex",
            "op_test",
            &[(Scope::CodexHome, "directory-target".to_string())],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("non-regular"), "got: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
