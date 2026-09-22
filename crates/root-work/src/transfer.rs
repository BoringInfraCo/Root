//! Versioned, integrity-checked workspace transfer documents.
//!
//! Export/import move *recorded engineering work state* between Root
//! directories. Documents are versioned and hash-protected, carry credential
//! *names* at most, and never include session transcripts or conversation
//! history. Export opens the source read-only and never migrates it; import is
//! all-or-nothing: rows are inserted into a staged database next to its final
//! location, and the staged database, workspace index, and optional project
//! pointer are published only after every insert succeeds — any failure rolls
//! every artifact back. Import only ever targets a fresh workspace.

use crate::db;
use crate::id;
use crate::model::*;
use crate::paths;
use crate::pointer;
use crate::registry::{WorkIndex, WorkspaceEntry};
use crate::repository::Repository;
use crate::secrets;
use crate::store::{
    lookup_workspace, row_to_artifact, row_to_checkpoint, row_to_decision, row_to_event,
    row_to_finding, row_to_goal, row_to_provenance, row_to_session, row_to_workspace,
    WorkspaceLookup,
};
use crate::time::now_rfc3339;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Current workspace-transfer document version.
pub const ROOTWS_VERSION: u32 = 1;

/// Upper bound on a transfer document on disk (64 MiB).
pub const MAX_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;

const DISCLOSURE: &str =
    "Root workspace transfer: recorded work state and credential names only — \
never secret values, session transcripts, or conversation history.";

/// A self-contained, hash-protected snapshot of one workspace's work state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTransfer {
    pub rootws_version: u32,
    pub created_at: String,
    pub source_root_dir_hint: String,
    pub workspace: WorkspaceRecord,
    #[serde(default)]
    pub goals: Vec<GoalRecord>,
    #[serde(default)]
    pub sessions: Vec<SessionRecord>,
    #[serde(default)]
    pub decisions: Vec<DecisionRecord>,
    #[serde(default)]
    pub findings: Vec<FindingRecord>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRecord>,
    #[serde(default)]
    pub provenance: Vec<ProvenanceRecord>,
    #[serde(default)]
    pub checkpoints: Vec<CheckpointRecord>,
    #[serde(default)]
    pub events: Vec<WorkEvent>,
    pub disclosure: String,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub path: String,
    pub workspace_id: String,
    pub goals: usize,
    pub decisions: usize,
    pub findings: usize,
    pub artifacts: usize,
    pub checkpoints: usize,
    pub events: usize,
    pub payload_sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub workspace_id: String,
    pub workspace_name: String,
    pub goals: usize,
    pub decisions: usize,
    pub findings: usize,
    pub artifacts: usize,
    pub checkpoints: usize,
    pub events: usize,
    pub latest_checkpoint_id: Option<String>,
}

/// Export a workspace's work state to a versioned, hash-protected document.
///
/// The source database is opened **read-only** (never migrated, never backed
/// up, never written). When `checkpoint` is set, the document reconstructs the
/// workspace **as of that checkpoint** rather than exporting current state:
///
/// * checkpoints: rows created at or before the target (by `created_at`, then
///   `rowid` as the tie-breaker);
/// * events: the append-only ledger truncated at the target's
///   `checkpoint.created` sequence (falling back to `timestamp <= created_at`
///   when that event is absent);
/// * goals, sessions, decisions, findings, artifacts: rows whose creation
///   event (`goal.set`, `session.started`, `decision.created`,
///   `finding.created`, `artifact.created`) has `sequence <=` that cutoff;
///   rows with no creation event fall back to `created_at/started_at <=` the
///   target checkpoint's `created_at`;
/// * goal status: the active goal is restored from the latest `goal.set` at
///   or before the cutoff, so a goal superseded only *after* the checkpoint
///   still exports as `active`;
/// * provenance: only rows referenced by the filtered collections;
/// * agent environment: carried on checkpoint rows (`agent_env_ref` and the
///   embedded snapshot), so it is filtered with the checkpoints.
///
/// Rows have no post-creation update history in the ledger, so the status of
/// included decisions/findings/artifacts/sessions is exported as currently
/// stored (Root never mutates them after creation). Without `checkpoint`, the
/// full current state is exported unchanged.
pub fn export_workspace(
    root_dir: &Path,
    repository: &Repository,
    checkpoint: Option<&str>,
    out: &Path,
    force: bool,
) -> Result<ExportReport> {
    let workspace_id = match lookup_workspace(root_dir, repository)? {
        WorkspaceLookup::Bound(id) => id,
        WorkspaceLookup::Absent => bail!(
            "No Root workspace found for this repository.\n\n\
             Initialize one with:  root workspace init"
        ),
        WorkspaceLookup::PointerInvalid(message) => bail!("{message}"),
    };
    let db_path = paths::database_path(root_dir, &workspace_id);
    if !db_path.exists() {
        bail!(
            "Workspace metadata is present but the database is missing.\n\nExpected: {}",
            db_path.display()
        );
    }
    let conn = db::open_read_only(&db_path)?;

    let workspace = conn
        .query_row(
            "SELECT id, name, repo_path, repo_identity, created_at, updated_at
             FROM workspaces WHERE id = ?1",
            params![workspace_id],
            row_to_workspace,
        )
        .optional()?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Workspace metadata points at '{}', but no matching workspace exists in {}.",
                workspace_id,
                db_path.display()
            )
        })?;

    let (goals, sessions, decisions, findings, artifacts, checkpoints, events) = match checkpoint {
        Some(checkpoint_id) => as_of_collections(&conn, &workspace_id, checkpoint_id)?,
        None => (
            query_goals(&conn, &workspace_id)?,
            query_sessions(&conn, &workspace_id)?,
            query_decisions(&conn, &workspace_id)?,
            query_findings(&conn, &workspace_id)?,
            query_artifacts(&conn, &workspace_id)?,
            query_checkpoints(&conn, &workspace_id)?,
            query_events(&conn, &workspace_id)?,
        ),
    };

    let provenance = query_referenced_provenance(
        &conn,
        &goals,
        &decisions,
        &findings,
        &artifacts,
        &checkpoints,
    )?;

    let mut transfer = WorkspaceTransfer {
        rootws_version: ROOTWS_VERSION,
        created_at: now_rfc3339(),
        source_root_dir_hint: root_dir.display().to_string(),
        workspace,
        goals,
        sessions,
        decisions,
        findings,
        artifacts,
        provenance,
        checkpoints,
        events,
        disclosure: DISCLOSURE.to_string(),
        payload_sha256: String::new(),
    };
    let digest = payload_digest(&transfer)?;
    transfer.payload_sha256 = digest.clone();

    let content = serde_json::to_vec_pretty(&transfer)
        .context("Failed to serialize the workspace transfer")?;
    if content.len() as u64 > MAX_TRANSFER_BYTES {
        bail!(
            "Workspace transfer is {} bytes, which exceeds the {} byte limit.",
            content.len(),
            MAX_TRANSFER_BYTES
        );
    }
    write_atomically(out, force, &content)?;

    Ok(ExportReport {
        path: out.display().to_string(),
        workspace_id: transfer.workspace.id.clone(),
        goals: transfer.goals.len(),
        decisions: transfer.decisions.len(),
        findings: transfer.findings.len(),
        artifacts: transfer.artifacts.len(),
        checkpoints: transfer.checkpoints.len(),
        events: transfer.events.len(),
        payload_sha256: digest,
        bytes: content.len() as u64,
    })
}

/// Import a transfer document into a *fresh* workspace under `root_dir`.
///
/// The document is strictly parsed, version-checked, digest-verified, and
/// secret-scanned before anything is created. Rows are inserted into a staged
/// database written beside its final path; the staged database is then
/// published by a single rename, followed by the workspace index and the
/// optional project pointer. Any failure at or after the rename removes the
/// published database and restores the index to its pre-import bytes, so a
/// failed import leaves no database, no index change, and no pointer change
/// and is immediately retryable.
pub fn import_workspace(
    root_dir: &Path,
    project: &Path,
    file: &Path,
    write_pointer: bool,
) -> Result<ImportReport> {
    let meta = std::fs::symlink_metadata(file)
        .with_context(|| format!("Workspace transfer not found at {}", file.display()))?;
    if meta.file_type().is_symlink() {
        bail!("Refusing to import: {} is a symlink.", file.display());
    }
    if !meta.is_file() {
        bail!(
            "Refusing to import: {} is not a regular file.",
            file.display()
        );
    }
    if meta.len() > MAX_TRANSFER_BYTES {
        bail!(
            "Workspace transfer at {} is {} bytes, which exceeds the {} byte limit.",
            file.display(),
            meta.len(),
            MAX_TRANSFER_BYTES
        );
    }
    let bytes = std::fs::read(file)
        .with_context(|| format!("Failed to read workspace transfer {}", file.display()))?;
    if bytes.len() as u64 > MAX_TRANSFER_BYTES {
        bail!(
            "Workspace transfer exceeds the {} byte limit.",
            MAX_TRANSFER_BYTES
        );
    }

    let transfer: WorkspaceTransfer = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Workspace transfer at {} is not a valid Root workspace document.",
            file.display()
        )
    })?;

    if transfer.rootws_version > ROOTWS_VERSION {
        bail!(
            "Workspace transfer version {} is newer than this build supports ({}).\n\n\
             Upgrade Root to import this document.",
            transfer.rootws_version,
            ROOTWS_VERSION
        );
    }
    if transfer.rootws_version < 1 {
        bail!(
            "Workspace transfer version {} is not supported.",
            transfer.rootws_version
        );
    }

    let expected = payload_digest(&transfer)?;
    if expected != transfer.payload_sha256 {
        bail!(
            "Workspace transfer integrity check failed: payload digest mismatch.\n\n\
             The document may be truncated, stale, or tampered with."
        );
    }

    scan_transfer_secrets(&transfer)?;

    let repository = Repository::discover(project)?;

    let index = WorkIndex::load(root_dir)?;
    if index.find_by_identity(&repository.identity()).is_some()
        || index
            .workspaces
            .iter()
            .any(|entry| entry.id == transfer.workspace.id)
    {
        bail!(
            "A workspace already exists for this repository; import targets a fresh workspace.\n\n\
             Import into a fresh Root directory, or inspect the existing workspace with \
             `root workspace status`."
        );
    }

    let workspace_id = transfer.workspace.id.clone();
    if !id::is_filesystem_safe(&workspace_id) {
        bail!("Workspace id '{}' is not filesystem-safe.", workspace_id);
    }
    let db_path = paths::database_path(root_dir, &workspace_id);
    let workspace_dir = paths::workspace_dir(root_dir, &workspace_id);
    let index_path = paths::index_path(root_dir);
    let index_before = std::fs::read(&index_path).ok();

    // Clear anything a previously interrupted import left at this exact
    // location, but refuse databases that are not clearly such leftovers —
    // a genuine pre-existing workspace must keep failing with "already has
    // work state".
    prepare_import_target(&workspace_id, &db_path, &workspace_dir, &repository)?;

    // Stage the complete database beside its final path, then publish it with
    // a single rename. On any staging or publish failure the workspace
    // directory (staging files included) is removed, so no database — final or
    // temporary — survives a failed import.
    let staging_db = match stage_import(root_dir, &workspace_id, &transfer, &repository) {
        Ok(staging_db) => staging_db,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&workspace_dir);
            return Err(error);
        }
    };
    if let Err(error) = publish_staged_database(&staging_db, &db_path, &workspace_dir) {
        let _ = std::fs::remove_dir_all(&workspace_dir);
        return Err(error);
    }

    // The database is at its final path from here on: every later failure
    // removes it again and restores the index to its pre-import bytes.
    let mut index = index;
    index.upsert(WorkspaceEntry {
        id: workspace_id.clone(),
        repo_path: repository.root.display().to_string(),
        repo_identity: repository.identity(),
    });

    if failpoint::tripped(failpoint::INDEX_SAVE) {
        rollback_published_import(&workspace_dir, &index_path, &index_before);
        bail!("test-injected failure: workspace index save");
    }
    if let Err(error) = index.save(root_dir) {
        rollback_published_import(&workspace_dir, &index_path, &index_before);
        return Err(error).context("Failed to save the workspace index");
    }

    if write_pointer {
        if failpoint::tripped(failpoint::POINTER_SAVE) {
            rollback_published_import(&workspace_dir, &index_path, &index_before);
            bail!("test-injected failure: workspace pointer save");
        }
        let pointer = pointer::WorkspacePointer {
            workspace_id: workspace_id.clone(),
            root_dir_hint: Some(root_dir.display().to_string()),
        };
        if let Err(error) = pointer::save(&repository.root, &pointer) {
            rollback_published_import(&workspace_dir, &index_path, &index_before);
            return Err(error).context("Failed to write the workspace pointer");
        }
    }

    Ok(ImportReport {
        workspace_id,
        workspace_name: transfer.workspace.name.clone(),
        goals: transfer.goals.len(),
        decisions: transfer.decisions.len(),
        findings: transfer.findings.len(),
        artifacts: transfer.artifacts.len(),
        checkpoints: transfer.checkpoints.len(),
        events: transfer.events.len(),
        latest_checkpoint_id: transfer.checkpoints.last().map(|cp| cp.id.clone()),
    })
}

/// Test-only failure injection for the import publish steps. The constants and
/// [`failpoint::tripped`] are always compiled so `import_workspace` reads the
/// same in tests and release builds; `set`/`clear` exist only under `cfg(test)`
/// and are thread-local, so parallel in-crate tests never observe each other's
/// failpoints.
mod failpoint {
    /// Trip before the workspace index is saved.
    pub const INDEX_SAVE: u8 = 1;
    /// Trip before the project workspace pointer is saved.
    pub const POINTER_SAVE: u8 = 2;

    #[cfg(test)]
    pub fn set(step: u8) {
        CURRENT.with(|current| current.set(step));
    }

    #[cfg(test)]
    pub fn clear() {
        CURRENT.with(|current| current.set(0));
    }

    #[cfg(test)]
    pub fn tripped(step: u8) -> bool {
        CURRENT.with(|current| current.get() == step)
    }

    #[cfg(not(test))]
    pub fn tripped(_step: u8) -> bool {
        false
    }

    #[cfg(test)]
    thread_local! {
        static CURRENT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    }
}

/// Remove leftovers of a previously interrupted import at this exact target
/// location; refuse anything that cannot be attributed to one.
///
/// Root only ever writes `work/<workspace-id>/state.db` for ids registered in
/// the workspace index (which the caller has already checked are absent), for
/// freshly generated ids, or for the id of a document being imported. A
/// database at the incoming document's id that is *not* in the index is
/// therefore either a crashed import (possibly pre-fix, when the database was
/// created before the index entry) or an empty, undiscoverable init shell —
/// both safe to clear. Anything else (symlink, directory, or a database whose
/// workspace rows do not belong to this id and repository) is refused with the
/// existing "already has work state" error.
fn prepare_import_target(
    workspace_id: &str,
    db_path: &Path,
    workspace_dir: &Path,
    repository: &Repository,
) -> Result<()> {
    match std::fs::symlink_metadata(workspace_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                bail!(
                    "Refusing to import: {} is a symlink.",
                    workspace_dir.display()
                );
            }
            if !meta.is_dir() {
                bail!(
                    "Refusing to import: {} is not a directory.",
                    workspace_dir.display()
                );
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", workspace_dir.display()));
        }
    }

    match std::fs::symlink_metadata(db_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                bail!("Refusing to import: {} is a symlink.", db_path.display());
            }
            if meta.is_dir() {
                bail!(
                    "Refusing to import: {} is not a directory.",
                    db_path.display()
                );
            }
            if !leftover_import_database(db_path, workspace_id, repository) {
                bail!(
                    "'{}' already has work state at {}; import targets a fresh workspace.",
                    workspace_id,
                    db_path.display()
                );
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to inspect {}", db_path.display()));
        }
    }

    std::fs::remove_dir_all(workspace_dir).with_context(|| {
        format!(
            "Failed to clear leftover import state at {}",
            workspace_dir.display()
        )
    })?;
    Ok(())
}

/// True when `db_path` holds only what an interrupted import of this document
/// (or an unindexed, undiscoverable init shell) can leave behind.
fn leftover_import_database(db_path: &Path, workspace_id: &str, repository: &Repository) -> bool {
    let Some(rows) = read_workspace_identities(db_path) else {
        // Unreadable as a workspace database: interrupted creation or
        // migration. Nothing discoverable can live here.
        return true;
    };
    if rows.is_empty() {
        // Schema without a workspace row: a rolled-back or pre-commit import.
        return true;
    }
    let identity = repository.identity();
    rows.iter()
        .any(|(id, row_identity)| id == workspace_id && row_identity == &identity)
}

/// Read `(id, repo_identity)` rows without schema checks, so even an older-
/// schema database can be attributed rather than blindly replaced.
fn read_workspace_identities(db_path: &Path) -> Option<Vec<(String, String)>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut stmt = conn
        .prepare("SELECT id, repo_identity FROM workspaces")
        .ok()?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?;
    rows.collect::<rusqlite::Result<Vec<_>>>().ok()
}

/// Build the complete workspace database at a temp path under the workspace
/// directory and fsync it. Returns the staging path; the caller publishes it
/// or removes the whole workspace directory on failure.
fn stage_import(
    root_dir: &Path,
    workspace_id: &str,
    transfer: &WorkspaceTransfer,
    repository: &Repository,
) -> Result<PathBuf> {
    std::fs::create_dir_all(paths::exports_dir(root_dir, workspace_id))
        .with_context(|| format!("Failed to create work directory for {workspace_id}"))?;
    let staging_db = paths::workspace_dir(root_dir, workspace_id)
        .join(format!("state.db.import-{}.tmp", std::process::id()));
    // Remove any same-pid leftover staging files before opening.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = staging_db.as_os_str().to_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }

    let conn = db::open(&staging_db)?;
    {
        let tx = conn.unchecked_transaction()?;
        insert_transfer(&tx, transfer, repository)?;
        tx.commit()?;
    }
    conn.close().map_err(|(_, error)| {
        anyhow::anyhow!("Failed to close the staged workspace database: {error}")
    })?;
    // A clean close checkpoints the WAL away; anything else means the stage is
    // not a self-contained file and must not be published.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = staging_db.as_os_str().to_os_string();
        name.push(suffix);
        let sidecar = PathBuf::from(name);
        if sidecar.exists() {
            bail!(
                "Staged workspace database left {} behind after close.",
                sidecar.display()
            );
        }
    }
    fsync_file(&staging_db)?;
    Ok(staging_db)
}

/// Atomically move the staged database to its final path (the publish point)
/// and make the rename durable.
fn publish_staged_database(staging_db: &Path, db_path: &Path, workspace_dir: &Path) -> Result<()> {
    std::fs::rename(staging_db, db_path).with_context(|| {
        format!(
            "Failed to publish the imported workspace database at {}",
            db_path.display()
        )
    })?;
    fsync_dir(workspace_dir)?;
    Ok(())
}

/// Undo a published import: remove the workspace directory (final database and
/// staging remnants included) and restore the index to its pre-import bytes.
///
/// The pointer is only written after this runs on the failure path, and
/// [`pointer::save`] is itself atomic, so a failed pointer save leaves the
/// previous pointer untouched.
fn rollback_published_import(
    workspace_dir: &Path,
    index_path: &Path,
    index_before: &Option<Vec<u8>>,
) {
    let _ = std::fs::remove_dir_all(workspace_dir);
    match index_before {
        Some(bytes) => {
            let _ = root_lockfile::atomic_write(index_path, bytes);
        }
        None => {
            let _ = std::fs::remove_file(index_path);
        }
    }
}

fn fsync_file(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {} for fsync", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to fsync {}", path.display()))?;
    Ok(())
}

fn fsync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let dir = std::fs::File::open(path)
            .with_context(|| format!("Failed to open directory {} for fsync", path.display()))?;
        dir.sync_all()
            .with_context(|| format!("Failed to fsync directory {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// SHA-256 over the canonical JSON of the document with `payload_sha256 = ""`.
fn payload_digest(transfer: &WorkspaceTransfer) -> Result<String> {
    let mut unsigned = transfer.clone();
    unsigned.payload_sha256 = String::new();
    let canonical = serde_json::to_vec(&unsigned)
        .context("Failed to serialize the workspace transfer for hashing")?;
    Ok(root_lockfile::compute_sha256(&canonical))
}

fn write_atomically(out: &Path, force: bool, content: &[u8]) -> Result<()> {
    match std::fs::symlink_metadata(out) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                bail!(
                    "Refusing to write workspace transfer: {} is a symlink.",
                    out.display()
                );
            }
            if !meta.is_file() {
                bail!(
                    "Refusing to write workspace transfer: {} is not a regular file.",
                    out.display()
                );
            }
            if !force {
                bail!(
                    "Workspace transfer already exists at {}.\n\nPass --force to overwrite it.",
                    out.display()
                );
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to inspect {}", out.display()));
        }
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
    }
    root_lockfile::atomic_write(out, content)
        .with_context(|| format!("Failed to write workspace transfer at {}", out.display()))
}

fn scan_transfer_secrets(transfer: &WorkspaceTransfer) -> Result<()> {
    let mut texts: Vec<&str> = vec![&transfer.workspace.name];
    for goal in &transfer.goals {
        texts.push(&goal.statement);
    }
    for decision in &transfer.decisions {
        texts.push(&decision.statement);
        if let Some(rationale) = &decision.rationale {
            texts.push(rationale);
        }
    }
    for finding in &transfer.findings {
        texts.push(&finding.statement);
        if let Some(evidence) = &finding.evidence_ref {
            texts.push(evidence);
        }
    }
    for artifact in &transfer.artifacts {
        texts.push(&artifact.uri);
        if let Some(fingerprint) = &artifact.fingerprint {
            texts.push(fingerprint);
        }
    }
    for checkpoint in &transfer.checkpoints {
        if let Some(message) = &checkpoint.message {
            texts.push(message);
        }
        texts.push(&checkpoint.continuation_summary);
        texts.push(&checkpoint.snapshot);
        if let Some(agent_env_ref) = &checkpoint.agent_env_ref {
            texts.push(agent_env_ref);
        }
    }
    for event in &transfer.events {
        if let Some(payload) = &event.payload {
            texts.push(payload);
        }
    }
    for provenance in &transfer.provenance {
        if let Some(agent) = &provenance.agent {
            texts.push(agent);
        }
        if let Some(evidence) = &provenance.evidence_ref {
            texts.push(evidence);
        }
    }
    for session in &transfer.sessions {
        if let Some(identity) = &session.agent_identity {
            texts.push(identity);
        }
    }
    for text in texts {
        if let Some(label) = secrets::detect(text) {
            bail!("{}", secrets::refusal(label));
        }
    }
    Ok(())
}

fn insert_transfer(
    tx: &Connection,
    transfer: &WorkspaceTransfer,
    repository: &Repository,
) -> Result<()> {
    let workspace = &transfer.workspace;
    tx.execute(
        "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            workspace.id,
            workspace.name,
            repository.root.display().to_string(),
            repository.identity(),
            workspace.created_at,
            workspace.updated_at
        ],
    )?;

    for record in &transfer.provenance {
        tx.execute(
            "INSERT INTO provenance (id, source_type, agent, harness, session_id, evidence_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.source_type,
                record.agent,
                record.harness,
                record.session_id,
                record.evidence_ref,
                record.created_at
            ],
        )?;
    }

    for record in &transfer.goals {
        tx.execute(
            "INSERT INTO goals (id, workspace_id, statement, status, created_at, completed_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.workspace_id,
                record.statement,
                record.status,
                record.created_at,
                record.completed_at,
                record.provenance_id
            ],
        )?;
    }

    for record in &transfer.sessions {
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, harness, agent_identity, started_at, ended_at, resumed_from_checkpoint_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.workspace_id,
                record.harness,
                record.agent_identity,
                record.started_at,
                record.ended_at,
                record.resumed_from_checkpoint_id
            ],
        )?;
    }

    for record in &transfer.decisions {
        tx.execute(
            "INSERT INTO decisions (id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id,
                record.workspace_id,
                record.goal_id,
                record.statement,
                record.rationale,
                record.status,
                record.created_at,
                record.provenance_id
            ],
        )?;
    }

    for record in &transfer.findings {
        tx.execute(
            "INSERT INTO findings (id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id,
                record.workspace_id,
                record.goal_id,
                record.statement,
                record.evidence_ref,
                record.status,
                record.created_at,
                record.provenance_id
            ],
        )?;
    }

    for record in &transfer.artifacts {
        tx.execute(
            "INSERT INTO artifacts (id, workspace_id, kind, uri, fingerprint, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.workspace_id,
                record.kind,
                record.uri,
                record.fingerprint,
                record.created_at,
                record.provenance_id
            ],
        )?;
    }

    for record in &transfer.checkpoints {
        tx.execute(
            "INSERT INTO checkpoints (
                id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                profile_reference, environment_status, continuation_summary, snapshot,
                created_at, provenance_id, agent_env_ref
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                record.id,
                record.workspace_id,
                record.goal_id,
                record.message,
                record.work_revision,
                record.git_head,
                record.git_branch,
                record.git_dirty as i64,
                record.git_dirty_fingerprint,
                record.rootfile_digest,
                record.root_lock_digest,
                record.profile_reference,
                record.environment_status,
                record.continuation_summary,
                record.snapshot,
                record.created_at,
                record.provenance_id,
                record.agent_env_ref
            ],
        )?;
    }

    for event in &transfer.events {
        tx.execute(
            "INSERT INTO work_events (sequence, workspace_id, event_type, entity_type, entity_id, payload, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.sequence,
                event.workspace_id,
                event.event_type,
                event.entity_type,
                event.entity_id,
                event.payload,
                event.timestamp
            ],
        )?;
    }

    Ok(())
}

fn query_goals(conn: &Connection, workspace_id: &str) -> Result<Vec<GoalRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, statement, status, created_at, completed_at, provenance_id
         FROM goals WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_goal)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_sessions(conn: &Connection, workspace_id: &str) -> Result<Vec<SessionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, harness, agent_identity, started_at, ended_at, resumed_from_checkpoint_id
         FROM sessions WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_session)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_decisions(conn: &Connection, workspace_id: &str) -> Result<Vec<DecisionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id
         FROM decisions WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_decision)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_findings(conn: &Connection, workspace_id: &str) -> Result<Vec<FindingRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id
         FROM findings WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_finding)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_artifacts(conn: &Connection, workspace_id: &str) -> Result<Vec<ArtifactRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, kind, uri, fingerprint, created_at, provenance_id
         FROM artifacts WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_artifact)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_checkpoints(conn: &Connection, workspace_id: &str) -> Result<Vec<CheckpointRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                profile_reference, environment_status, continuation_summary, snapshot,
                created_at, provenance_id, agent_env_ref
         FROM checkpoints WHERE workspace_id = ?1 ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_checkpoint)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_events(conn: &Connection, workspace_id: &str) -> Result<Vec<WorkEvent>> {
    let mut stmt = conn.prepare(
        "SELECT sequence, workspace_id, event_type, entity_type, entity_id, payload, timestamp
         FROM work_events WHERE workspace_id = ?1 ORDER BY sequence ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], row_to_event)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The as-of boundary of a checkpoint: the last event sequence that belongs to
/// its state, plus the checkpoint's `created_at`/`rowid` for orderings the
/// ledger cannot express (checkpoints themselves, and rows with no creation
/// event).
struct AsOfBoundary {
    sequence: i64,
    created_at: String,
    rowid: i64,
}

/// Resolve the target checkpoint and its event-ledger cutoff.
///
/// The authoritative cutoff is the sequence of the target's `checkpoint.created`
/// event — the checkpoint row and that event commit in the same transaction, so
/// every event at or before it observed-or-created exactly the state the
/// checkpoint captured (equivalently: `sequence <= work_revision` for rows
/// written before the checkpoint, since `work_revision` is the event count at
/// capture time). When that event is absent, fall back to the last sequence
/// whose timestamp is at or before the checkpoint's `created_at`.
fn as_of_boundary(
    conn: &Connection,
    workspace_id: &str,
    checkpoint_id: &str,
) -> Result<AsOfBoundary> {
    let target: Option<(String, i64)> = conn
        .query_row(
            "SELECT created_at, rowid FROM checkpoints WHERE id = ?1 AND workspace_id = ?2",
            params![checkpoint_id, workspace_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (created_at, rowid) = target.ok_or_else(|| {
        anyhow::anyhow!("Unknown checkpoint '{}' in this workspace.", checkpoint_id)
    })?;

    let sequence: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM work_events
         WHERE workspace_id = ?1 AND event_type = 'checkpoint.created' AND entity_id = ?2",
        params![workspace_id, checkpoint_id],
        |row| row.get(0),
    )?;
    let sequence = if sequence > 0 {
        sequence
    } else {
        conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM work_events
             WHERE workspace_id = ?1 AND timestamp <= ?2",
            params![workspace_id, created_at],
            |row| row.get(0),
        )?
    };

    Ok(AsOfBoundary {
        sequence,
        created_at,
        rowid,
    })
}

/// Static description of a row collection and the ledger event that announces
/// its creation.
struct AsOfTable {
    table: &'static str,
    event_type: &'static str,
    timestamp_column: &'static str,
    columns: &'static str,
}

const AS_OF_GOALS: AsOfTable = AsOfTable {
    table: "goals",
    event_type: "goal.set",
    timestamp_column: "created_at",
    columns: "id, workspace_id, statement, status, created_at, completed_at, provenance_id",
};

const AS_OF_SESSIONS: AsOfTable = AsOfTable {
    table: "sessions",
    event_type: "session.started",
    timestamp_column: "started_at",
    columns: "id, workspace_id, harness, agent_identity, started_at, ended_at, \
              resumed_from_checkpoint_id",
};

const AS_OF_DECISIONS: AsOfTable = AsOfTable {
    table: "decisions",
    event_type: "decision.created",
    timestamp_column: "created_at",
    columns: "id, workspace_id, goal_id, statement, rationale, status, created_at, \
              provenance_id",
};

const AS_OF_FINDINGS: AsOfTable = AsOfTable {
    table: "findings",
    event_type: "finding.created",
    timestamp_column: "created_at",
    columns: "id, workspace_id, goal_id, statement, evidence_ref, status, created_at, \
              provenance_id",
};

const AS_OF_ARTIFACTS: AsOfTable = AsOfTable {
    table: "artifacts",
    event_type: "artifact.created",
    timestamp_column: "created_at",
    columns: "id, workspace_id, kind, uri, fingerprint, created_at, provenance_id",
};

/// Load the rows of one collection that existed at the as-of boundary.
///
/// A row qualifies when its creation event's sequence is at or before the
/// cutoff. Rows with no matching creation event (documents imported without a
/// ledger, or directly seeded databases) fall back to a `created_at <=`
/// comparison against the target checkpoint's timestamp.
fn as_of_rows<T>(
    conn: &Connection,
    workspace_id: &str,
    boundary: &AsOfBoundary,
    spec: &AsOfTable,
    mapper: fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let table = spec.table;
    let sql = format!(
        "SELECT {columns} FROM {table}
         WHERE workspace_id = ?1 AND (
             EXISTS (
                 SELECT 1 FROM work_events e
                 WHERE e.workspace_id = {table}.workspace_id
                   AND e.event_type = ?4
                   AND e.entity_id = {table}.id
                   AND e.sequence <= ?2
             )
             OR (
                 NOT EXISTS (
                     SELECT 1 FROM work_events e
                     WHERE e.workspace_id = {table}.workspace_id
                       AND e.event_type = ?4
                       AND e.entity_id = {table}.id
                 )
                 AND {table}.{timestamp_column} <= ?3
             )
         )
         ORDER BY {table}.rowid ASC",
        columns = spec.columns,
        timestamp_column = spec.timestamp_column,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            workspace_id,
            boundary.sequence,
            boundary.created_at,
            spec.event_type
        ],
        mapper,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Reconstruct every exported collection as of `checkpoint_id`.
///
/// Collections reconstructed as of a checkpoint, in export order:
/// `(goals, sessions, decisions, findings, artifacts, checkpoints, events)`,
/// all filtered to the boundary described by [`as_of_boundary`].
type AsOfCollections = (
    Vec<GoalRecord>,
    Vec<SessionRecord>,
    Vec<DecisionRecord>,
    Vec<FindingRecord>,
    Vec<ArtifactRecord>,
    Vec<CheckpointRecord>,
    Vec<WorkEvent>,
);

fn as_of_collections(
    conn: &Connection,
    workspace_id: &str,
    checkpoint_id: &str,
) -> Result<AsOfCollections> {
    let boundary = as_of_boundary(conn, workspace_id, checkpoint_id)?;

    let mut goals = as_of_rows(conn, workspace_id, &boundary, &AS_OF_GOALS, row_to_goal)?;
    let sessions = as_of_rows(
        conn,
        workspace_id,
        &boundary,
        &AS_OF_SESSIONS,
        row_to_session,
    )?;
    let decisions = as_of_rows(
        conn,
        workspace_id,
        &boundary,
        &AS_OF_DECISIONS,
        row_to_decision,
    )?;
    let findings = as_of_rows(
        conn,
        workspace_id,
        &boundary,
        &AS_OF_FINDINGS,
        row_to_finding,
    )?;
    let artifacts = as_of_rows(
        conn,
        workspace_id,
        &boundary,
        &AS_OF_ARTIFACTS,
        row_to_artifact,
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                profile_reference, environment_status, continuation_summary, snapshot,
                created_at, provenance_id, agent_env_ref
         FROM checkpoints
         WHERE workspace_id = ?1 AND (created_at < ?2 OR (created_at = ?2 AND rowid <= ?3))
         ORDER BY rowid ASC",
    )?;
    let checkpoints = stmt
        .query_map(
            params![workspace_id, boundary.created_at, boundary.rowid],
            row_to_checkpoint,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut event_stmt = conn.prepare(
        "SELECT sequence, workspace_id, event_type, entity_type, entity_id, payload, timestamp
         FROM work_events WHERE workspace_id = ?1 AND sequence <= ?2 ORDER BY sequence ASC",
    )?;
    let events = event_stmt
        .query_map(params![workspace_id, boundary.sequence], row_to_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Restore the goal that was active at the boundary: the latest `goal.set`
    // at or before the cutoff. A goal superseded only after the checkpoint
    // still exports as `active`, and an included goal that is `active` only in
    // the current state is demoted, so the document agrees with its own
    // truncated ledger.
    let active_goal_id: Option<String> = conn
        .query_row(
            "SELECT entity_id FROM work_events
             WHERE workspace_id = ?1 AND event_type = 'goal.set' AND sequence <= ?2
               AND entity_id IS NOT NULL
             ORDER BY sequence DESC LIMIT 1",
            params![workspace_id, boundary.sequence],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(active_id) = active_goal_id {
        for goal in &mut goals {
            if goal.id == active_id {
                goal.status = GOAL_ACTIVE.to_string();
            } else if goal.status == GOAL_ACTIVE {
                goal.status = GOAL_SUPERSEDED.to_string();
            }
        }
    }

    Ok((
        goals,
        sessions,
        decisions,
        findings,
        artifacts,
        checkpoints,
        events,
    ))
}

fn query_referenced_provenance(
    conn: &Connection,
    goals: &[GoalRecord],
    decisions: &[DecisionRecord],
    findings: &[FindingRecord],
    artifacts: &[ArtifactRecord],
    checkpoints: &[CheckpointRecord],
) -> Result<Vec<ProvenanceRecord>> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for goal in goals {
        if let Some(id) = &goal.provenance_id {
            ids.insert(id.clone());
        }
    }
    for decision in decisions {
        if let Some(id) = &decision.provenance_id {
            ids.insert(id.clone());
        }
    }
    for finding in findings {
        if let Some(id) = &finding.provenance_id {
            ids.insert(id.clone());
        }
    }
    for artifact in artifacts {
        if let Some(id) = &artifact.provenance_id {
            ids.insert(id.clone());
        }
    }
    for checkpoint in checkpoints {
        if let Some(id) = &checkpoint.provenance_id {
            ids.insert(id.clone());
        }
    }

    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        let record = conn
            .query_row(
                "SELECT id, source_type, agent, harness, session_id, evidence_ref, created_at
                 FROM provenance WHERE id = ?1",
                params![id],
                row_to_provenance,
            )
            .optional()?;
        if let Some(record) = record {
            records.push(record);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use crate::store::{NewArtifact, NewCheckpoint, WorkStore};
    use std::path::PathBuf;

    struct Fixture {
        base: PathBuf,
        root_a: PathBuf,
        repo_a: PathBuf,
        root_b: PathBuf,
        repo_b: PathBuf,
        out: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_work_transfer_{}_{}",
                tag,
                std::process::id()
            ));
            let root_a = base.join("root_a");
            let repo_a = base.join("repo_a");
            let root_b = base.join("root_b");
            let repo_b = base.join("repo_b");
            std::fs::create_dir_all(repo_a.join(".git")).unwrap();
            std::fs::create_dir_all(repo_b.join(".git")).unwrap();
            Self {
                base: base.clone(),
                root_a,
                repo_a,
                root_b,
                repo_b,
                out: base.join("transfer.rootws.json"),
            }
        }

        fn repository_a(&self) -> Repository {
            Repository::discover(&self.repo_a).unwrap()
        }

        fn repository_b(&self) -> Repository {
            Repository::discover(&self.repo_b).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn prepare_source(fixture: &Fixture) {
        WorkStore::init_at(&fixture.root_a, fixture.repository_a()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_a, fixture.repository_a()).unwrap();
        store.set_goal("Ship the transfer feature").unwrap();
        store
            .add_decision("Version the document", Some("forward compatibility"))
            .unwrap();
        let revision = store.work_revision().unwrap();
        store
            .create_checkpoint(NewCheckpoint {
                message: Some("pre-transfer"),
                work_revision: revision,
                git_head: Some("deadbeef"),
                git_branch: Some("main"),
                git_dirty: false,
                git_dirty_fingerprint: None,
                rootfile_digest: None,
                root_lock_digest: None,
                profile_reference: None,
                environment_status: model::ENV_OBSERVED,
                continuation_summary: "ready to transfer",
                snapshot: "{}",
                agent_env_ref: Some(r#"{"adapter":"codex","skills":["docs-writer"]}"#),
            })
            .unwrap();
    }

    fn write_transfer(path: &Path, mut transfer: WorkspaceTransfer) {
        transfer.payload_sha256 = String::new();
        let canonical = serde_json::to_vec(&transfer).unwrap();
        transfer.payload_sha256 = root_lockfile::compute_sha256(&canonical);
        std::fs::write(path, serde_json::to_vec_pretty(&transfer).unwrap()).unwrap();
    }

    fn read_transfer(path: &Path) -> WorkspaceTransfer {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn seed_v2_workspace(root_dir: &Path, repository: &Repository) -> String {
        let workspace_id = "root_ws_V2TV".to_string();
        let db_path = paths::database_path(root_dir, &workspace_id);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(db::MIGRATION_V1).unwrap();
        conn.execute_batch(db::MIGRATION_V2).unwrap();
        conn.execute_batch(
            "CREATE TABLE work_schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
             INSERT INTO work_schema_migrations (version, applied_at) VALUES (2, 'now');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
             VALUES (?1, 'seed', ?2, ?3, 'now', 'now')",
            params![
                workspace_id,
                repository.root.display().to_string(),
                repository.identity()
            ],
        )
        .unwrap();
        drop(conn);

        let mut index = WorkIndex::load(root_dir).unwrap();
        index.upsert(WorkspaceEntry {
            id: workspace_id.clone(),
            repo_path: repository.root.display().to_string(),
            repo_identity: repository.identity(),
        });
        index.save(root_dir).unwrap();
        workspace_id
    }

    #[test]
    fn transfer_round_trips_between_two_root_dirs() {
        let fixture = Fixture::new("roundtrip");
        prepare_source(&fixture);
        let source_revision = WorkStore::open_at(&fixture.root_a, fixture.repository_a())
            .unwrap()
            .work_revision()
            .unwrap();

        let report = export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        assert_eq!(report.goals, 1);
        assert_eq!(report.decisions, 1);
        assert_eq!(report.checkpoints, 1);
        assert!(report.events >= 4);
        assert!(report.bytes > 0);

        let imported =
            import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false).unwrap();
        assert_eq!(imported.workspace_id, report.workspace_id);
        assert!(imported.latest_checkpoint_id.is_some());

        let store = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(store.workspace().id, report.workspace_id);
        assert_eq!(
            store.workspace().repo_identity,
            fixture.repository_b().identity()
        );
        assert_eq!(
            store.workspace().repo_path,
            fixture.repository_b().root.display().to_string()
        );
        assert_eq!(
            store.active_goal().unwrap().unwrap().statement,
            "Ship the transfer feature"
        );
        assert_eq!(store.list_decisions().unwrap().len(), 1);
        assert_eq!(store.list_checkpoints().unwrap().len(), 1);
        assert_eq!(store.work_revision().unwrap(), source_revision);
        let checkpoint = store.latest_checkpoint().unwrap().unwrap();
        assert_eq!(
            checkpoint.agent_env_ref.as_deref(),
            Some(r#"{"adapter":"codex","skills":["docs-writer"]}"#)
        );
    }

    #[test]
    fn tampered_payload_digest_refused() {
        let fixture = Fixture::new("tampered");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();

        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.out).unwrap()).unwrap();
        value["payload_sha256"] = serde_json::Value::String("0".repeat(64));
        std::fs::write(&fixture.out, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("integrity check failed"), "{error}");
        assert!(!db_has_any_workspace(&fixture.root_b));
    }

    #[test]
    fn newer_transfer_version_refused() {
        let fixture = Fixture::new("newer");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();

        let mut transfer = read_transfer(&fixture.out);
        transfer.rootws_version = ROOTWS_VERSION + 1;
        write_transfer(&fixture.out, transfer);

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("newer than this build supports"), "{error}");
    }

    #[test]
    fn secret_shaped_payload_refused() {
        let fixture = Fixture::new("secret");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();

        let mut transfer = read_transfer(&fixture.out);
        let workspace_id = transfer.workspace.id.clone();
        transfer.goals.push(GoalRecord {
            id: "root_goal_SECRET".to_string(),
            workspace_id,
            statement: "password = hunter2-secret".to_string(),
            status: model::GOAL_ACTIVE.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            provenance_id: None,
        });
        write_transfer(&fixture.out, transfer);

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret"), "{error}");
    }

    #[test]
    fn import_into_existing_workspace_refused() {
        let fixture = Fixture::new("existing");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        WorkStore::init_at(&fixture.root_b, fixture.repository_b()).unwrap();

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn export_unknown_checkpoint_refused() {
        let fixture = Fixture::new("unknown_cp");
        prepare_source(&fixture);

        let error = export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            Some("root_cp_MISSING"),
            &fixture.out,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Unknown checkpoint"), "{error}");
        assert!(!fixture.out.exists());
    }

    #[test]
    fn export_is_read_only() {
        let fixture = Fixture::new("export_ro");
        let workspace_id = seed_v2_workspace(&fixture.root_a, &fixture.repository_a());
        let db_path = paths::database_path(&fixture.root_a, &workspace_id);
        let before = std::fs::read(&db_path).unwrap();

        let error = export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("requires migration"), "{error}");
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
        assert!(!db_path.with_extension("db-wal").exists());
        assert!(!db_path.with_extension("db-shm").exists());
        assert!(!db_path
            .parent()
            .unwrap()
            .join("state.db.backup-v2")
            .exists());
        assert!(!fixture.out.exists());
    }

    /// Snapshot of every artifact an import may create, for before/after
    /// comparisons around a failed import.
    struct ImportBaseline {
        index: Option<Vec<u8>>,
        pointer: Option<Vec<u8>>,
        work_entries: Vec<String>,
    }

    impl ImportBaseline {
        fn capture(root_dir: &Path, repo: &Path) -> Self {
            Self {
                index: std::fs::read(paths::index_path(root_dir)).ok(),
                pointer: std::fs::read(paths::workspace_pointer_path(repo)).ok(),
                work_entries: dir_entry_names(&paths::work_dir(root_dir)),
            }
        }

        fn assert_unchanged(&self, root_dir: &Path, repo: &Path) {
            assert_eq!(
                std::fs::read(paths::index_path(root_dir)).ok(),
                self.index,
                "failed import must not change the workspace index"
            );
            assert_eq!(
                std::fs::read(paths::workspace_pointer_path(repo)).ok(),
                self.pointer,
                "failed import must not change the project pointer"
            );
            assert_eq!(
                dir_entry_names(&paths::work_dir(root_dir)),
                self.work_entries,
                "failed import must leave no workspace directories, databases, or temp files"
            );
            assert_no_staging_temps(root_dir);
        }
    }

    fn dir_entry_names(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// No staged `state.db.import-*.tmp` (or sidecar) may survive a failed
    /// import anywhere under `~/.root/work`.
    fn assert_no_staging_temps(root_dir: &Path) {
        let work = paths::work_dir(root_dir);
        let Ok(top) = std::fs::read_dir(&work) else {
            return;
        };
        for entry in top.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.contains(".import-"),
                "staging temp survived a failed import: {}",
                entry.path().display()
            );
            if entry.path().is_dir() {
                let Ok(inner) = std::fs::read_dir(entry.path()) else {
                    continue;
                };
                for inner_entry in inner.flatten() {
                    let name = inner_entry.file_name().to_string_lossy().into_owned();
                    assert!(
                        !name.contains(".import-"),
                        "staging temp survived a failed import: {}",
                        inner_entry.path().display()
                    );
                }
            }
        }
    }

    /// Clears a failpoint even if the test panics before its retry.
    struct FailpointGuard;

    impl FailpointGuard {
        fn set(step: u8) -> Self {
            failpoint::set(step);
            Self
        }
    }

    impl Drop for FailpointGuard {
        fn drop(&mut self) {
            failpoint::clear();
        }
    }

    #[test]
    fn import_is_all_or_nothing() {
        let fixture = Fixture::new("atomic");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();

        let mut transfer = read_transfer(&fixture.out);
        transfer.decisions[0].status = "bogus".to_string();
        let workspace_id = transfer.workspace.id.clone();
        write_transfer(&fixture.out, transfer);
        let baseline = ImportBaseline::capture(&fixture.root_b, &fixture.repo_b);

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(!error.is_empty());

        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        assert!(
            !db_path.exists(),
            "a failed import must leave no database at {}",
            db_path.display()
        );
        assert!(
            !paths::workspace_dir(&fixture.root_b, &workspace_id).exists(),
            "a failed import must leave no workspace directory"
        );
        baseline.assert_unchanged(&fixture.root_b, &fixture.repo_b);

        // Defense in depth: if a database somehow exists, it must be empty.
        if db_path.exists() {
            let conn = Connection::open(&db_path).unwrap();
            for table in ["workspaces", "decisions", "work_events", "checkpoints"] {
                let count: i64 = conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(count, 0, "{table} must be empty after rollback");
            }
        }

        // The corrected document imports immediately, with no cleanup.
        let mut transfer = read_transfer(&fixture.out);
        transfer.decisions[0].status = "active".to_string();
        write_transfer(&fixture.out, transfer);
        let report = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .expect("a failed import must be immediately retryable");
        assert_eq!(report.workspace_id, workspace_id);
        assert!(db_path.exists());
        let store = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(store.list_decisions().unwrap().len(), 1);
    }

    #[test]
    fn import_index_save_failure_leaves_no_artifacts_and_retries() {
        let fixture = Fixture::new("idxfail");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        let transfer = read_transfer(&fixture.out);
        let workspace_id = transfer.workspace.id.clone();
        let baseline = ImportBaseline::capture(&fixture.root_b, &fixture.repo_b);

        let guard = FailpointGuard::set(failpoint::INDEX_SAVE);
        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        drop(guard);
        assert!(error.contains("index save"), "{error}");

        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        assert!(
            !db_path.exists(),
            "index-save failure must remove the database"
        );
        assert!(
            !paths::workspace_dir(&fixture.root_b, &workspace_id).exists(),
            "index-save failure must remove the workspace directory"
        );
        baseline.assert_unchanged(&fixture.root_b, &fixture.repo_b);

        let report = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .expect("retry after an index-save failure must succeed");
        assert_eq!(report.workspace_id, workspace_id);
        let store = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(store.workspace().id, workspace_id);
    }

    #[test]
    fn import_pointer_save_failure_rolls_back_db_and_index() {
        let fixture = Fixture::new("ptrfail");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        let transfer = read_transfer(&fixture.out);
        let workspace_id = transfer.workspace.id.clone();

        // Real (non-hook) pointer-save failure: `.root` is occupied by a file,
        // so pointer::save refuses before writing anything.
        let root_marker = fixture.repo_b.join(".root");
        std::fs::write(&root_marker, b"occupied").unwrap();
        let baseline = ImportBaseline::capture(&fixture.root_b, &fixture.repo_b);

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, true)
            .unwrap_err()
            .to_string();
        assert!(error.to_lowercase().contains("pointer"), "{error}");

        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        assert!(
            !db_path.exists(),
            "pointer-save failure must remove the database"
        );
        assert!(
            !paths::workspace_dir(&fixture.root_b, &workspace_id).exists(),
            "pointer-save failure must remove the workspace directory"
        );
        baseline.assert_unchanged(&fixture.root_b, &fixture.repo_b);
        assert_eq!(
            std::fs::read(&root_marker).unwrap(),
            b"occupied",
            "pointer-save failure must not touch the existing .root entry"
        );

        // Clear the obstruction; the retry must publish everything.
        std::fs::remove_file(&root_marker).unwrap();
        let report = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, true)
            .expect("retry after a pointer-save failure must succeed");
        assert_eq!(report.workspace_id, workspace_id);
        assert!(db_path.exists());
        let pointer = pointer::load(&fixture.repo_b).unwrap().unwrap();
        assert_eq!(pointer.workspace_id, workspace_id);
    }

    #[test]
    fn leftover_empty_partial_database_does_not_poison_retry() {
        let fixture = Fixture::new("leftover_empty");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        let workspace_id = read_transfer(&fixture.out).workspace.id;

        // A pre-fix failed import could leave a migrated database with empty
        // tables at the final path (the transaction rolled back, the file did
        // not). It is not in the index, so it must be replaced, not refused.
        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(db::MIGRATION_V1).unwrap();
            conn.execute_batch(db::MIGRATION_V2).unwrap();
            conn.execute_batch(db::MIGRATION_V3).unwrap();
            conn.execute_batch(
                "CREATE TABLE work_schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'old');
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (2, 'old');
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (3, 'old');",
            )
            .unwrap();
        }

        let report = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .expect("an unindexed leftover database must not block the retry");
        assert_eq!(report.workspace_id, workspace_id);
        let store = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(store.workspace().id, workspace_id);
        assert_eq!(store.list_decisions().unwrap().len(), 1);
    }

    #[test]
    fn leftover_committed_partial_database_is_replaced_on_retry() {
        let fixture = Fixture::new("leftover_committed");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        let transfer = read_transfer(&fixture.out);
        let workspace_id = transfer.workspace.id.clone();
        let repository_b = fixture.repository_b();

        // A pre-fix crash between the index-less database commit and the index
        // save leaves a fully populated database with this repository's
        // identity — clearly a prior attempt of this same import.
        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        {
            let conn = db::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES (?1, 'partial', ?2, ?3, 'now', 'now')",
                params![
                    workspace_id,
                    repository_b.root.display().to_string(),
                    repository_b.identity()
                ],
            )
            .unwrap();
        }

        let report = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .expect("an unindexed committed partial must be replaced");
        assert_eq!(report.workspace_id, workspace_id);
        let store = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(store.workspace().name, transfer.workspace.name);
        assert_eq!(store.list_decisions().unwrap().len(), 1);
    }

    #[test]
    fn foreign_database_at_transfer_path_is_refused_untouched() {
        let fixture = Fixture::new("foreign_db");
        prepare_source(&fixture);
        export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &fixture.out,
            false,
        )
        .unwrap();
        let workspace_id = read_transfer(&fixture.out).workspace.id;

        // Same path, but the database belongs to a different workspace and
        // repository: not attributable to this import, so it must be refused
        // and left byte-for-byte intact.
        let db_path = paths::database_path(&fixture.root_b, &workspace_id);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        {
            let conn = db::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_FOREIGN', 'foreign', '/elsewhere', 'path:/elsewhere', 'now', 'now')",
                [],
            )
            .unwrap();
        }
        let before = std::fs::read(&db_path).unwrap();
        let baseline = ImportBaseline::capture(&fixture.root_b, &fixture.repo_b);

        let error = import_workspace(&fixture.root_b, &fixture.repo_b, &fixture.out, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already has work state"), "{error}");
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
        baseline.assert_unchanged(&fixture.root_b, &fixture.repo_b);
    }

    #[test]
    fn export_with_checkpoint_reconstructs_state_as_of_that_checkpoint() {
        let fixture = Fixture::new("as_of");
        WorkStore::init_at(&fixture.root_a, fixture.repository_a()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_a, fixture.repository_a()).unwrap();

        // State at checkpoint 1.
        store.set_goal("First goal").unwrap();
        store
            .add_decision("Decision before checkpoint", None)
            .unwrap();
        store
            .add_finding("Finding before checkpoint", None)
            .unwrap();
        store
            .add_artifact(NewArtifact {
                kind: model::ARTIFACT_FILE,
                uri: "before.md",
                fingerprint: Some("deadbeef"),
                evidence_ref: None,
            })
            .unwrap();
        store
            .start_session(Some("codex"), Some("agent-one"))
            .unwrap();
        let revision = store.work_revision().unwrap();
        let cp1 = store
            .create_checkpoint(NewCheckpoint {
                message: Some("checkpoint one"),
                work_revision: revision,
                git_head: Some("deadbeef"),
                git_branch: Some("main"),
                git_dirty: false,
                git_dirty_fingerprint: None,
                rootfile_digest: None,
                root_lock_digest: None,
                profile_reference: None,
                environment_status: model::ENV_OBSERVED,
                continuation_summary: "state one",
                snapshot: "{}",
                agent_env_ref: Some(r#"{"adapter":"codex","skills":[]}"#),
            })
            .unwrap();

        // Future work, after checkpoint 1.
        store.set_goal("Second goal").unwrap();
        store
            .add_decision("Decision after checkpoint", None)
            .unwrap();
        store.add_finding("Finding after checkpoint", None).unwrap();
        store
            .add_artifact(NewArtifact {
                kind: model::ARTIFACT_FILE,
                uri: "after.md",
                fingerprint: Some("cafebabe"),
                evidence_ref: None,
            })
            .unwrap();
        store
            .start_session(Some("claude"), Some("agent-two"))
            .unwrap();
        let revision = store.work_revision().unwrap();
        let cp2 = store
            .create_checkpoint(NewCheckpoint {
                message: Some("checkpoint two"),
                work_revision: revision,
                git_head: Some("feedface"),
                git_branch: Some("main"),
                git_dirty: false,
                git_dirty_fingerprint: None,
                rootfile_digest: None,
                root_lock_digest: None,
                profile_reference: None,
                environment_status: model::ENV_OBSERVED,
                continuation_summary: "state two",
                snapshot: "{}",
                agent_env_ref: None,
            })
            .unwrap();
        drop(store);

        // As-of export: only pre-checkpoint-1 state, internally consistent.
        let out_as_of = fixture.base.join("as_of.rootws.json");
        let report = export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            Some(&cp1.id),
            &out_as_of,
            false,
        )
        .unwrap();
        let doc = read_transfer(&out_as_of);
        assert_eq!(report.goals, 1, "{report:?}");
        assert_eq!(report.decisions, 1, "{report:?}");
        assert_eq!(report.findings, 1, "{report:?}");
        assert_eq!(report.artifacts, 1, "{report:?}");
        assert_eq!(report.checkpoints, 1, "{report:?}");

        assert_eq!(doc.goals.len(), 1);
        assert_eq!(doc.goals[0].statement, "First goal");
        assert_eq!(
            doc.goals[0].status,
            model::GOAL_ACTIVE,
            "the goal active at the checkpoint must export as active"
        );
        assert_eq!(doc.sessions.len(), 1);
        assert_eq!(doc.sessions[0].agent_identity.as_deref(), Some("agent-one"));
        assert_eq!(doc.decisions.len(), 1);
        assert_eq!(doc.decisions[0].statement, "Decision before checkpoint");
        assert_eq!(doc.findings.len(), 1);
        assert_eq!(doc.findings[0].statement, "Finding before checkpoint");
        assert_eq!(doc.artifacts.len(), 1);
        assert_eq!(doc.artifacts[0].uri, "before.md");
        assert_eq!(doc.checkpoints.len(), 1);
        assert_eq!(doc.checkpoints[0].id, cp1.id);
        assert_eq!(
            doc.checkpoints[0].agent_env_ref.as_deref(),
            Some(r#"{"adapter":"codex","skills":[]}"#),
            "agent env must travel with its checkpoint"
        );

        // The truncated ledger agrees with the filtered collections: it stops
        // at checkpoint 1 and only references entities present above.
        assert!(
            !doc.events
                .iter()
                .any(|e| e.entity_id.as_deref() == Some(cp2.id.as_str())),
            "checkpoint 2 must not appear in the as-of event ledger"
        );
        assert!(
            !doc.events
                .iter()
                .any(|e| e.event_type == "checkpoint.created"
                    && e.entity_id.is_some()
                    && e.entity_id.as_deref() != Some(cp1.id.as_str())),
            "only checkpoint 1's creation event may appear"
        );
        let mut ledger_ids: Vec<&str> = doc
            .events
            .iter()
            .filter_map(|e| e.entity_id.as_deref())
            .collect();
        ledger_ids.sort_unstable();
        ledger_ids.dedup();
        for id in ledger_ids {
            let known = doc.goals.iter().any(|g| g.id == id)
                || doc.decisions.iter().any(|d| d.id == id)
                || doc.findings.iter().any(|f| f.id == id)
                || doc.artifacts.iter().any(|a| a.id == id)
                || doc.sessions.iter().any(|s| s.id == id)
                || doc.checkpoints.iter().any(|c| c.id == id)
                || id == doc.workspace.id;
            assert!(
                known,
                "event references entity {id} that is absent from the as-of document"
            );
        }
        for provenance in doc.goals.iter().filter_map(|g| g.provenance_id.as_ref()) {
            assert!(
                doc.provenance.iter().any(|p| &p.id == provenance),
                "goal provenance {provenance} missing from the as-of document"
            );
        }

        // Full export (no --checkpoint) still contains everything.
        let out_full = fixture.base.join("full.rootws.json");
        let full_report = export_workspace(
            &fixture.root_a,
            &fixture.repository_a(),
            None,
            &out_full,
            false,
        )
        .unwrap();
        let full = read_transfer(&out_full);
        assert_eq!(full_report.goals, 2);
        assert_eq!(full_report.decisions, 2);
        assert_eq!(full_report.findings, 2);
        assert_eq!(full_report.artifacts, 2);
        assert_eq!(full_report.checkpoints, 2);
        assert!(full
            .sessions
            .iter()
            .any(|s| s.agent_identity.as_deref() == Some("agent-two")));
        assert!(full
            .goals
            .iter()
            .any(|g| g.statement == "Second goal" && g.status == model::GOAL_ACTIVE));
        assert!(full
            .goals
            .iter()
            .any(|g| g.statement == "First goal" && g.status == model::GOAL_SUPERSEDED));
        assert!(full
            .events
            .iter()
            .any(|e| e.entity_id.as_deref() == Some(cp2.id.as_str())));

        // Importing the as-of document reproduces exactly that state.
        let imported = import_workspace(&fixture.root_b, &fixture.repo_b, &out_as_of, false)
            .expect("the as-of document must import into a fresh Root directory");
        assert_eq!(imported.goals, 1);
        assert_eq!(imported.decisions, 1);
        assert_eq!(imported.findings, 1);
        assert_eq!(imported.artifacts, 1);
        assert_eq!(imported.checkpoints, 1);
        let store_b = WorkStore::open_at(&fixture.root_b, fixture.repository_b()).unwrap();
        assert_eq!(
            store_b.active_goal().unwrap().unwrap().statement,
            "First goal"
        );
        assert_eq!(store_b.list_decisions().unwrap().len(), 1);
        assert_eq!(store_b.list_findings().unwrap().len(), 1);
        assert_eq!(store_b.list_artifacts().unwrap().len(), 1);
        assert_eq!(store_b.list_checkpoints().unwrap().len(), 1);
        assert_eq!(
            store_b.latest_checkpoint().unwrap().unwrap().id,
            cp1.id,
            "the imported workspace's latest checkpoint must be checkpoint 1"
        );
        assert!(
            !store_b
                .events()
                .unwrap()
                .iter()
                .any(|e| e.entity_id.as_deref() == Some(cp2.id.as_str())),
            "checkpoint 2's event must not exist in the imported workspace"
        );
    }

    fn db_has_any_workspace(root_dir: &Path) -> bool {
        let work = paths::work_dir(root_dir);
        let Ok(entries) = std::fs::read_dir(&work) else {
            return false;
        };
        for entry in entries.flatten() {
            let db_path = entry.path().join("state.db");
            if !db_path.exists() {
                continue;
            }
            let Ok(conn) = Connection::open(&db_path) else {
                continue;
            };
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
                .unwrap_or(0);
            if count > 0 {
                return true;
            }
        }
        false
    }
}
