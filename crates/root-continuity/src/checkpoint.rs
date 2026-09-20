//! Durable checkpoint creation and inspection.
//!
//! A checkpoint is an honest record of state Root can observe: normalized work
//! state, observable Git state, observable environment references, and a
//! deterministic continuation summary. Checkpoints are immutable.

use crate::environment::EnvironmentState;
use crate::git::GitState;
use crate::snapshot::CheckpointSnapshot;
use crate::summary::continuation_summary;
use anyhow::{bail, Result};
use root_work::{
    CheckpointListReport, CheckpointReport, NewCheckpoint, ProvenanceContext, Repository, WorkStore,
};
use std::path::Path;

/// Create a checkpoint for the workspace associated with `cwd`.
pub fn create(cwd: &Path, message: Option<&str>) -> Result<CheckpointReport> {
    create_with_provenance(cwd, message, ProvenanceContext::default())
}

/// Create a checkpoint attributed to an explicit provenance source.
pub fn create_with_provenance(
    cwd: &Path,
    message: Option<&str>,
    context: ProvenanceContext<'_>,
) -> Result<CheckpointReport> {
    let repository = Repository::discover(cwd)?;
    let mut store = WorkStore::open(repository.clone())?;
    let root_dir = root_lockfile::get_root_dir()?;
    create_on_store(&mut store, &repository, &root_dir, message, context)
}

/// Create a checkpoint using an already-open workspace store.
pub fn create_on_store(
    store: &mut WorkStore,
    repository: &Repository,
    root_dir: &Path,
    message: Option<&str>,
    context: ProvenanceContext<'_>,
) -> Result<CheckpointReport> {
    let git = GitState::capture(repository);
    let environment = EnvironmentState::capture_at(root_dir)?;
    let snapshot = CheckpointSnapshot::capture(store)?;
    let summary = continuation_summary(&snapshot, &git, &environment);

    let checkpoint = store.create_checkpoint_with(
        NewCheckpoint {
            message,
            work_revision: snapshot.work_revision,
            git_head: git.head.as_deref(),
            git_branch: git.branch.as_deref(),
            git_dirty: git.dirty,
            git_dirty_fingerprint: git.dirty_fingerprint.as_deref(),
            rootfile_digest: environment.rootfile_digest.as_deref(),
            root_lock_digest: environment.root_lock_digest.as_deref(),
            profile_reference: environment.profile_reference.as_deref(),
            environment_status: &environment.status,
            continuation_summary: &summary,
            snapshot: &snapshot.to_json()?,
        },
        context,
    )?;

    Ok(CheckpointReport {
        success: true,
        checkpoint,
    })
}

pub fn list(cwd: &Path) -> Result<CheckpointListReport> {
    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository)?;
    Ok(CheckpointListReport {
        success: true,
        workspace_id: store.workspace().id.clone(),
        checkpoints: store.list_checkpoints()?,
    })
}

pub fn show(cwd: &Path, checkpoint_id: &str) -> Result<CheckpointReport> {
    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository)?;
    Ok(CheckpointReport {
        success: true,
        checkpoint: store.show_checkpoint(checkpoint_id)?,
    })
}

pub fn show_last(cwd: &Path) -> Result<CheckpointReport> {
    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository)?;
    let checkpoint = store.latest_checkpoint()?.ok_or_else(|| {
        anyhow::anyhow!(
            "No checkpoints exist for this workspace.\n\n\
             Create one with:  root checkpoint create"
        )
    })?;
    Ok(CheckpointReport {
        success: true,
        checkpoint,
    })
}

/// Parse a stored checkpoint snapshot, validating that it is well-formed.
pub fn parse_snapshot(checkpoint: &root_work::CheckpointRecord) -> Result<CheckpointSnapshot> {
    if checkpoint.snapshot.trim().is_empty() {
        bail!("Checkpoint '{}' has no stored snapshot.", checkpoint.id);
    }
    CheckpointSnapshot::from_json(&checkpoint.snapshot)
}
