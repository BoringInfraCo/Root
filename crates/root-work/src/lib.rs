//! Root Work State Engine.
//!
//! Root v0.5 gives engineering work a durable, harness-independent home. This
//! crate owns the canonical work model — workspace, goal, sessions,
//! decisions, findings, artifacts, provenance, and the append-only work event
//! ledger — backed by SQLite under `~/.root/work/`.
//!
//! Sprint 005 deliberately stops at persistence and inspection. It does not
//! implement checkpoints, resume, handoff, recovery, MCP, or agent capture.

pub mod db;
pub mod id;
pub mod model;
pub mod paths;
pub mod registry;
pub mod repository;
pub mod secrets;
pub mod store;
pub mod time;

pub use model::*;
pub use repository::Repository;
pub use store::{NewArtifact, NewCheckpoint, ProvenanceContext, WorkStore};

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn open_workspace(cwd: &Path) -> Result<WorkStore> {
    let repository = Repository::discover(cwd)?;
    WorkStore::open(repository)
}

/// Compute the deterministic content fingerprint Root stores for a file artifact.
pub fn fingerprint_file(path: &Path) -> Result<String> {
    root_lockfile::hash_file(path)
        .with_context(|| format!("Failed to fingerprint {}", path.display()))
}

/// Resolve a user-supplied artifact path relative to the current directory.
fn resolve_artifact_path(cwd: &Path, input: &str) -> PathBuf {
    let candidate = Path::new(input);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

pub fn workspace_init(cwd: &Path) -> Result<WorkspaceInitReport> {
    let repository = Repository::discover(cwd)?;
    WorkStore::init(repository)
}

pub fn workspace_status(cwd: &Path) -> Result<WorkspaceStatusReport> {
    open_workspace(cwd)?.status()
}

pub fn goal_set(cwd: &Path, statement: &str) -> Result<GoalReport> {
    let mut store = open_workspace(cwd)?;
    store.set_goal(statement)
}

pub fn goal_show(cwd: &Path) -> Result<GoalReport> {
    open_workspace(cwd)?.show_goal()
}

pub fn decision_add(
    cwd: &Path,
    statement: &str,
    rationale: Option<&str>,
) -> Result<DecisionReport> {
    let mut store = open_workspace(cwd)?;
    let decision = store.add_decision(statement, rationale)?;
    Ok(DecisionReport {
        success: true,
        decision,
    })
}

pub fn decision_list(cwd: &Path) -> Result<DecisionListReport> {
    let store = open_workspace(cwd)?;
    Ok(DecisionListReport {
        success: true,
        workspace_id: store.workspace().id.clone(),
        decisions: store.list_decisions()?,
    })
}

pub fn decision_show(cwd: &Path, decision_id: &str) -> Result<DecisionReport> {
    let store = open_workspace(cwd)?;
    Ok(DecisionReport {
        success: true,
        decision: store.show_decision(decision_id)?,
    })
}

pub fn finding_add(
    cwd: &Path,
    statement: &str,
    evidence_ref: Option<&str>,
) -> Result<FindingReport> {
    let mut store = open_workspace(cwd)?;
    let finding = store.add_finding(statement, evidence_ref)?;
    Ok(FindingReport {
        success: true,
        finding,
    })
}

pub fn finding_list(cwd: &Path) -> Result<FindingListReport> {
    let store = open_workspace(cwd)?;
    Ok(FindingListReport {
        success: true,
        workspace_id: store.workspace().id.clone(),
        findings: store.list_findings()?,
    })
}

pub fn finding_show(cwd: &Path, finding_id: &str) -> Result<FindingReport> {
    let store = open_workspace(cwd)?;
    Ok(FindingReport {
        success: true,
        finding: store.show_finding(finding_id)?,
    })
}

pub fn artifact_add(cwd: &Path, path: &str) -> Result<ArtifactReport> {
    let input = path.trim();
    if input.is_empty() {
        bail!("An artifact path is required.");
    }
    let resolved = resolve_artifact_path(cwd, input);
    if !resolved.exists() {
        bail!(
            "Artifact path missing: {}\n\n\
             Root records references to existing files; it does not create them.",
            input
        );
    }
    if resolved.is_dir() {
        bail!(
            "Artifact path is a directory: {}\n\n\
             Sprint 005 records file artifacts. Point at a file instead.",
            input
        );
    }
    let fingerprint = fingerprint_file(&resolved)?;

    let mut store = open_workspace(cwd)?;
    let artifact = store.add_artifact(NewArtifact {
        kind: model::ARTIFACT_FILE,
        uri: input,
        fingerprint: Some(&fingerprint),
        evidence_ref: None,
    })?;
    Ok(ArtifactReport {
        success: true,
        artifact,
    })
}

pub fn artifact_list(cwd: &Path) -> Result<ArtifactListReport> {
    let store = open_workspace(cwd)?;
    Ok(ArtifactListReport {
        success: true,
        workspace_id: store.workspace().id.clone(),
        artifacts: store.list_artifacts()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root_dir: PathBuf,
        repo_dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("root_work_lib_{}_{}", tag, std::process::id()));
            let root_dir = base.join("root");
            let repo_dir = base.join("repo");
            std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
            Self { root_dir, repo_dir }
        }

        fn repo(&self) -> Repository {
            Repository::discover(&self.repo_dir).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(base) = self.root_dir.parent() {
                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    #[test]
    fn workspace_persists_across_reopen() {
        let fixture = Fixture::new("persist");
        let report = WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        assert!(report.created);
        assert!(report.database.ends_with("state.db"));
        let workspace_id = report.workspace.id.clone();

        {
            let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
            store.set_goal("Implement workspace invitations").unwrap();
            store
                .add_decision("Tokens expire after 24 hours", None)
                .unwrap();
        }

        let store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        assert_eq!(store.workspace().id, workspace_id);
        let status = store.status().unwrap();
        assert_eq!(
            status.goal.as_ref().unwrap().statement,
            "Implement workspace invitations"
        );
        assert_eq!(status.counts.decisions, 1);
        assert!(status.work_revision >= 3);
    }

    #[test]
    fn setting_a_goal_supersedes_but_keeps_previous() {
        let fixture = Fixture::new("goal");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();

        let first = store.set_goal("First goal").unwrap().goal;
        let second = store.set_goal("Second goal").unwrap().goal;
        assert_ne!(first.id, second.id);

        let status = store.status().unwrap();
        assert_eq!(status.goal.as_ref().unwrap().statement, "Second goal");
        assert_eq!(status.counts.goals, 2);

        let db_path = report_database(&fixture.root_dir, &store);
        let conn = rusqlite::Connection::open(db_path).unwrap();
        let first_status: String = conn
            .query_row(
                "SELECT status FROM goals WHERE id = ?1",
                rusqlite::params![first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_status, model::GOAL_SUPERSEDED);
    }

    #[test]
    fn mutations_create_human_provenance_and_events() {
        let fixture = Fixture::new("provenance");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let finding = store
            .add_finding(
                "Consumption fails inside the transaction",
                Some("test output"),
            )
            .unwrap();

        assert!(finding.provenance_id.is_some());
        let events = store.events().unwrap();
        assert!(events.iter().any(|e| e.event_type == "finding.created"));
        let created = events
            .iter()
            .find(|e| e.event_type == "finding.created")
            .unwrap();
        assert_eq!(created.entity_id.as_deref(), Some(finding.id.as_str()));

        let conn = rusqlite::Connection::open(report_database(&fixture.root_dir, &store)).unwrap();
        let source_type: String = conn
            .query_row(
                "SELECT source_type FROM provenance WHERE id = ?1",
                rusqlite::params![finding.provenance_id.unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_type, model::SOURCE_HUMAN);
    }

    #[test]
    fn artifact_records_fingerprint() {
        let fixture = Fixture::new("artifact");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let artifact = store
            .add_artifact(NewArtifact {
                kind: model::ARTIFACT_FILE,
                uri: "src/create.ts",
                fingerprint: Some("deadbeef"),
                evidence_ref: None,
            })
            .unwrap();
        assert_eq!(artifact.kind, model::ARTIFACT_FILE);
        assert_eq!(artifact.fingerprint.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn fingerprint_is_content_addressed() {
        let dir = std::env::temp_dir().join(format!("root_work_fp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let fingerprint = fingerprint_file(&path).unwrap();
        assert_eq!(
            fingerprint,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_double_init() {
        let fixture = Fixture::new("double");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let err = WorkStore::init_at(&fixture.root_dir, fixture.repo())
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn rejected_mutations_create_no_row_or_event() {
        let fixture = Fixture::new("atomic");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();

        let events_before = store.events().unwrap().len();
        let decisions_before = store.list_decisions().unwrap().len();
        let findings_before = store.list_findings().unwrap().len();
        let artifacts_before = store.list_artifacts().unwrap().len();
        let database = report_database(&fixture.root_dir, &store);
        let provenance_before: i64 = rusqlite::Connection::open(&database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM provenance", [], |row| row.get(0))
            .unwrap();

        assert!(store.add_decision("   ", None).is_err());
        assert!(store
            .add_decision("password = hunter2-secret", None)
            .is_err());
        assert!(store.add_finding("", None).is_err());
        assert!(store.add_finding("token: abcdef123456", None).is_err());
        assert!(store
            .add_artifact(NewArtifact {
                kind: "unsupported",
                uri: "src/create.ts",
                fingerprint: None,
                evidence_ref: None,
            })
            .is_err());

        assert_eq!(store.events().unwrap().len(), events_before);
        assert_eq!(store.list_decisions().unwrap().len(), decisions_before);
        assert_eq!(store.list_findings().unwrap().len(), findings_before);
        assert_eq!(store.list_artifacts().unwrap().len(), artifacts_before);
        let provenance_after: i64 = rusqlite::Connection::open(&database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM provenance", [], |row| row.get(0))
            .unwrap();
        assert_eq!(provenance_after, provenance_before);
    }

    #[test]
    fn checkpoints_are_immutable_and_persisted() {
        let fixture = Fixture::new("checkpoint");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        store.set_goal("Goal").unwrap();
        let revision = store.work_revision().unwrap();
        let checkpoint = store
            .create_checkpoint(NewCheckpoint {
                message: Some("first checkpoint"),
                work_revision: revision,
                git_head: Some("abc123"),
                git_branch: Some("feat/x"),
                git_dirty: true,
                git_dirty_fingerprint: Some("fingerprint"),
                rootfile_digest: Some("rootfile"),
                root_lock_digest: Some("lock"),
                profile_reference: Some("/nix/store/profile"),
                environment_status: model::ENV_OBSERVED,
                continuation_summary: "summary",
                snapshot: "{}",
            })
            .unwrap();
        assert!(checkpoint.id.starts_with("root_cp_"));
        assert_eq!(checkpoint.git_head.as_deref(), Some("abc123"));

        // Durable work changes after the checkpoint.
        store.add_decision("later decision", None).unwrap();
        drop(store);

        let store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let fetched = store.show_checkpoint(&checkpoint.id).unwrap();
        assert_eq!(fetched.work_revision, revision);
        assert_eq!(fetched.continuation_summary, "summary");
        assert_eq!(
            store.latest_checkpoint().unwrap().unwrap().id,
            checkpoint.id
        );
        assert_eq!(store.list_checkpoints().unwrap().len(), 1);
        assert!(store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.event_type == "checkpoint.created"));
    }

    fn report_database(root_dir: &Path, store: &WorkStore) -> PathBuf {
        paths::database_path(root_dir, &store.workspace().id)
    }
}
