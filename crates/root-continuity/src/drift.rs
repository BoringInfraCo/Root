//! Deterministic drift detection between a checkpoint and current observed state.
//!
//! Drift compares recorded checkpoint facts against state Root can observe now.
//! It never mutates Git or the working tree, and it never promotes normal
//! changes to `blocking`.

use crate::environment::EnvironmentState;
use crate::git::{short_sha, GitState};
use crate::snapshot::CheckpointSnapshot;
use root_lockfile::hash_file;
use root_work::model::ARTIFACT_FILE;
use root_work::CheckpointRecord;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DRIFT_NONE: &str = "none";
pub const DRIFT_INFORMATIONAL: &str = "informational";
pub const DRIFT_WARNING: &str = "warning";
pub const DRIFT_BLOCKING: &str = "blocking";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriftItem {
    pub kind: String,
    pub level: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriftReport {
    pub level: String,
    pub items: Vec<DriftItem>,
}

impl DriftReport {
    fn from_items(items: Vec<DriftItem>) -> Self {
        let level = overall_level(&items).to_string();
        Self { level, items }
    }
}

/// Compare a checkpoint's recorded state to the current observed state.
pub fn detect(
    repo_root: &Path,
    checkpoint: &CheckpointRecord,
    snapshot: &CheckpointSnapshot,
    current_git: &GitState,
    current_environment: &EnvironmentState,
) -> DriftReport {
    let mut items = Vec::new();

    if checkpoint.git_head != current_git.head {
        items.push(item(
            "repository.head",
            DRIFT_WARNING,
            format!(
                "HEAD changed: {} -> {}",
                display_sha(&checkpoint.git_head),
                display_sha(&current_git.head)
            ),
        ));
    }

    if checkpoint.git_branch != current_git.branch {
        items.push(item(
            "repository.branch",
            DRIFT_WARNING,
            format!(
                "Branch changed: {} -> {}",
                display(&checkpoint.git_branch),
                display(&current_git.branch)
            ),
        ));
    }

    if checkpoint.git_dirty_fingerprint.is_some()
        && current_git.dirty_fingerprint.is_some()
        && checkpoint.git_dirty != current_git.dirty
    {
        items.push(item(
            "repository.dirty",
            DRIFT_INFORMATIONAL,
            format!(
                "Working tree dirty state changed: {} -> {}",
                dirty_label(checkpoint.git_dirty),
                dirty_label(current_git.dirty)
            ),
        ));
    }

    detect_artifact_drift(repo_root, snapshot, &mut items);
    detect_environment_drift(checkpoint, current_environment, &mut items);

    DriftReport::from_items(items)
}

fn detect_artifact_drift(
    repo_root: &Path,
    snapshot: &CheckpointSnapshot,
    items: &mut Vec<DriftItem>,
) {
    for artifact in &snapshot.artifacts {
        if artifact.kind != ARTIFACT_FILE {
            continue;
        }
        let path = resolve_uri(repo_root, &artifact.uri);
        if !path.exists() {
            items.push(item(
                "artifact.missing",
                DRIFT_WARNING,
                format!("Artifact missing: {}", artifact.uri),
            ));
            continue;
        }
        if let Some(recorded) = &artifact.fingerprint {
            if let Ok(current) = hash_file(&path) {
                if &current != recorded {
                    items.push(item(
                        "artifact.changed",
                        DRIFT_WARNING,
                        format!("Artifact changed: {}", artifact.uri),
                    ));
                }
            }
        }
    }
}

fn detect_environment_drift(
    checkpoint: &CheckpointRecord,
    current: &EnvironmentState,
    items: &mut Vec<DriftItem>,
) {
    if checkpoint.rootfile_digest != current.rootfile_digest {
        items.push(item(
            "environment.rootfile",
            DRIFT_WARNING,
            format!(
                "Rootfile digest changed: {} -> {}",
                display(&checkpoint.rootfile_digest),
                display(&current.rootfile_digest)
            ),
        ));
    }
    if checkpoint.root_lock_digest != current.root_lock_digest {
        items.push(item(
            "environment.lock",
            DRIFT_WARNING,
            format!(
                "root.lock digest changed: {} -> {}",
                display(&checkpoint.root_lock_digest),
                display(&current.root_lock_digest)
            ),
        ));
    }
    if checkpoint.profile_reference != current.profile_reference {
        items.push(item(
            "environment.profile",
            DRIFT_INFORMATIONAL,
            format!(
                "Root profile reference changed: {} -> {}",
                display(&checkpoint.profile_reference),
                display(&current.profile_reference)
            ),
        ));
    }
}

fn overall_level(items: &[DriftItem]) -> &'static str {
    if items.iter().any(|entry| entry.level == DRIFT_BLOCKING) {
        DRIFT_BLOCKING
    } else if items.iter().any(|entry| entry.level == DRIFT_WARNING) {
        DRIFT_WARNING
    } else if items.iter().any(|entry| entry.level == DRIFT_INFORMATIONAL) {
        DRIFT_INFORMATIONAL
    } else {
        DRIFT_NONE
    }
}

fn item(kind: &str, level: &str, detail: String) -> DriftItem {
    DriftItem {
        kind: kind.to_string(),
        level: level.to_string(),
        detail,
    }
}

pub fn resolve_uri(repo_root: &Path, uri: &str) -> PathBuf {
    let candidate = Path::new(uri);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repo_root.join(candidate)
    }
}

fn display(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("(none)")
}

fn display_sha(value: &Option<String>) -> String {
    match value {
        Some(sha) => short_sha(sha),
        None => "(none)".to_string(),
    }
}

fn dirty_label(dirty: bool) -> &'static str {
    if dirty {
        "dirty"
    } else {
        "clean"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_work::ArtifactRecord;

    fn base_checkpoint() -> CheckpointRecord {
        CheckpointRecord {
            id: "root_cp_test".into(),
            workspace_id: "root_ws_test".into(),
            goal_id: None,
            message: None,
            work_revision: 1,
            git_head: Some("aaaaaaaaaaaaaaaa".into()),
            git_branch: Some("main".into()),
            git_dirty: false,
            git_dirty_fingerprint: Some("cleanfp".into()),
            rootfile_digest: Some("rf".into()),
            root_lock_digest: Some("lock".into()),
            profile_reference: Some("/profiles/default".into()),
            environment_status: "observed".into(),
            continuation_summary: String::new(),
            snapshot: "{}".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            provenance_id: None,
            agent_env_ref: None,
        }
    }

    fn base_git() -> GitState {
        GitState {
            head: Some("aaaaaaaaaaaaaaaa".into()),
            branch: Some("main".into()),
            dirty: false,
            dirty_fingerprint: Some("cleanfp".into()),
        }
    }

    fn base_env() -> EnvironmentState {
        EnvironmentState {
            rootfile_digest: Some("rf".into()),
            root_lock_digest: Some("lock".into()),
            profile_reference: Some("/profiles/default".into()),
            status: "observed".into(),
        }
    }

    fn empty_snapshot() -> CheckpointSnapshot {
        CheckpointSnapshot {
            goal: None,
            decisions: Vec::new(),
            findings: Vec::new(),
            artifacts: Vec::new(),
            work_revision: 1,
            agent_env: None,
            agent_env_sha256: None,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("root_drift_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_artifact(uri: &str, fingerprint: Option<&str>) -> ArtifactRecord {
        ArtifactRecord {
            id: format!("root_art_{}", uri.len()),
            workspace_id: "root_ws_test".into(),
            kind: ARTIFACT_FILE.into(),
            uri: uri.into(),
            fingerprint: fingerprint.map(|value| value.to_string()),
            created_at: "2026-01-01T00:00:00Z".into(),
            provenance_id: None,
        }
    }

    #[test]
    fn no_drift_when_nothing_changed() {
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &base_git(),
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_NONE);
        assert!(report.items.is_empty());
    }

    #[test]
    fn head_change_is_warning() {
        let mut git = base_git();
        git.head = Some("bbbbbbbbbbbbbbbb".into());
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &git,
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "repository.head"));
    }

    #[test]
    fn branch_change_is_warning() {
        let mut git = base_git();
        git.branch = Some("feat/other".into());
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &git,
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "repository.branch"));
    }

    #[test]
    fn dirty_change_is_informational() {
        let mut git = base_git();
        git.dirty = true;
        git.dirty_fingerprint = Some("dirtyfp".into());
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &git,
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_INFORMATIONAL);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "repository.dirty"));
    }

    #[test]
    fn rootfile_change_is_warning() {
        let mut env = base_env();
        env.rootfile_digest = Some("rf2".into());
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &base_git(),
            &env,
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "environment.rootfile"));
    }

    #[test]
    fn lock_change_is_warning() {
        let mut env = base_env();
        env.root_lock_digest = Some("lock2".into());
        let report = detect(
            Path::new("/repo"),
            &base_checkpoint(),
            &empty_snapshot(),
            &base_git(),
            &env,
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "environment.lock"));
    }

    #[test]
    fn missing_artifact_is_warning() {
        let dir = temp_dir("missing");
        let mut snapshot = empty_snapshot();
        snapshot.artifacts = vec![file_artifact("src/missing.ts", Some("abc"))];
        let report = detect(
            &dir,
            &base_checkpoint(),
            &snapshot,
            &base_git(),
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "artifact.missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_artifact_is_warning() {
        let dir = temp_dir("changed");
        std::fs::write(dir.join("artifact.ts"), b"one").unwrap();
        let recorded = hash_file(&dir.join("artifact.ts")).unwrap();
        std::fs::write(dir.join("artifact.ts"), b"two").unwrap();

        let mut snapshot = empty_snapshot();
        snapshot.artifacts = vec![file_artifact("artifact.ts", Some(&recorded))];
        let report = detect(
            &dir,
            &base_checkpoint(),
            &snapshot,
            &base_git(),
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_WARNING);
        assert!(report
            .items
            .iter()
            .any(|item| item.kind == "artifact.changed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_artifact_is_not_drift() {
        let dir = temp_dir("unchanged");
        std::fs::write(dir.join("artifact.ts"), b"stable").unwrap();
        let recorded = hash_file(&dir.join("artifact.ts")).unwrap();

        let mut snapshot = empty_snapshot();
        snapshot.artifacts = vec![file_artifact("artifact.ts", Some(&recorded))];
        let report = detect(
            &dir,
            &base_checkpoint(),
            &snapshot,
            &base_git(),
            &base_env(),
        );
        assert_eq!(report.level, DRIFT_NONE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
