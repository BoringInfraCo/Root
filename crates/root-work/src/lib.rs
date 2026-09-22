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
pub mod pointer;
pub mod registry;
pub mod repository;
pub mod secrets;
pub mod store;
pub mod time;
pub mod transfer;

pub use model::*;
pub use pointer::WorkspacePointer;
pub use repository::Repository;
pub use store::{
    lookup_workspace, NewArtifact, NewCheckpoint, ProvenanceContext, WorkStore, WorkspaceLookup,
};
pub use transfer::{
    export_workspace, import_workspace, ExportReport, ImportReport, WorkspaceTransfer,
    MAX_TRANSFER_BYTES, ROOTWS_VERSION,
};

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
    workspace_init_with(&repository, false)
}

/// Initialize a workspace for `repository`. When `write_pointer` is set, also
/// write the opt-in project pointer `<repo>/.root/workspace.json` (id + Root-dir
/// hint only). The pointer is a hint; canonical state stays under `~/.root`.
pub fn workspace_init_with(
    repository: &Repository,
    write_pointer: bool,
) -> Result<WorkspaceInitReport> {
    let report = WorkStore::init(repository.clone())?;
    if write_pointer {
        let pointer = WorkspacePointer {
            workspace_id: report.workspace.id.clone(),
            root_dir_hint: root_lockfile::get_root_dir()
                .ok()
                .map(|dir| dir.display().to_string()),
        };
        pointer::save(&repository.root, &pointer)?;
    }
    Ok(report)
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
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    struct RootDirGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl RootDirGuard {
        fn set(dir: &Path) -> Self {
            let lock = TEST_MUTEX.lock().unwrap_or_else(|error| error.into_inner());
            let previous = std::env::var_os("ROOT_DIR");
            std::env::set_var("ROOT_DIR", dir);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for RootDirGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("ROOT_DIR", value),
                None => std::env::remove_var("ROOT_DIR"),
            }
        }
    }

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
                agent_env_ref: None,
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

    #[test]
    fn checkpoint_round_trips_agent_env_ref() {
        let fixture = Fixture::new("agent_env_ref");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let revision = store.work_revision().unwrap();
        let agent_env_ref = r#"{"adapter":"codex","skills":["docs-writer"]}"#;
        let checkpoint = store
            .create_checkpoint(NewCheckpoint {
                message: Some("with agent env"),
                work_revision: revision,
                git_head: None,
                git_branch: None,
                git_dirty: false,
                git_dirty_fingerprint: None,
                rootfile_digest: None,
                root_lock_digest: None,
                profile_reference: None,
                environment_status: model::ENV_MISSING,
                continuation_summary: "summary",
                snapshot: "{}",
                agent_env_ref: Some(agent_env_ref),
            })
            .unwrap();
        assert_eq!(checkpoint.agent_env_ref.as_deref(), Some(agent_env_ref));
        drop(store);

        let store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let fetched = store.show_checkpoint(&checkpoint.id).unwrap();
        assert_eq!(fetched.agent_env_ref.as_deref(), Some(agent_env_ref));
        assert_eq!(
            store
                .latest_checkpoint()
                .unwrap()
                .unwrap()
                .agent_env_ref
                .as_deref(),
            Some(agent_env_ref)
        );
        let summaries = store.list_checkpoints().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].agent_env_ref.as_deref(), Some(agent_env_ref));
    }

    #[test]
    fn checkpoint_refuses_secret_shaped_agent_env_ref() {
        let fixture = Fixture::new("agent_env_ref_secret");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        let revision = store.work_revision().unwrap();
        let errors_before = store.events().unwrap().len();

        let error = store
            .create_checkpoint(NewCheckpoint {
                message: Some("with secret agent env"),
                work_revision: revision,
                git_head: None,
                git_branch: None,
                git_dirty: false,
                git_dirty_fingerprint: None,
                rootfile_digest: None,
                root_lock_digest: None,
                profile_reference: None,
                environment_status: model::ENV_MISSING,
                continuation_summary: "summary",
                snapshot: "{}",
                agent_env_ref: Some("password = hunter2"),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret"), "{error}");
        assert!(store.list_checkpoints().unwrap().is_empty());
        assert_eq!(store.events().unwrap().len(), errors_before);
    }

    #[test]
    fn pointer_present_resolves_workspace() {
        let fixture = Fixture::new("pointer_resolve");
        let report = WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        pointer::save(
            &fixture.repo_dir,
            &WorkspacePointer {
                workspace_id: report.workspace.id.clone(),
                root_dir_hint: None,
            },
        )
        .unwrap();

        let store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        assert_eq!(store.workspace().id, report.workspace.id);
    }

    #[test]
    fn stale_pointer_fails_closed_without_creating_workspace() {
        let fixture = Fixture::new("pointer_stale");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        pointer::save(
            &fixture.repo_dir,
            &WorkspacePointer {
                workspace_id: "root_ws_UNKNOWN".to_string(),
                root_dir_hint: None,
            },
        )
        .unwrap();

        let error = WorkStore::open_at(&fixture.root_dir, fixture.repo())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unknown workspace id"), "{error}");
        assert!(error.contains("root restore"), "{error}");

        let index = registry::WorkIndex::load(&fixture.root_dir).unwrap();
        assert_eq!(index.workspaces.len(), 1, "no divergent workspace created");
    }

    #[test]
    fn malformed_pointer_is_an_error() {
        let fixture = Fixture::new("pointer_malformed");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        let path = paths::workspace_pointer_path(&fixture.repo_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let error = WorkStore::open_at(&fixture.root_dir, fixture.repo())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Workspace pointer"), "{error}");
    }

    #[test]
    fn workspace_init_with_writes_pointer() {
        let fixture = Fixture::new("init_pointer");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        let report = workspace_init_with(&fixture.repo(), true).unwrap();
        assert!(report.created);

        let pointer = pointer::load(&fixture.repo_dir).unwrap().unwrap();
        assert_eq!(pointer.workspace_id, report.workspace.id);
        assert_eq!(
            pointer.root_dir_hint.as_deref(),
            Some(fixture.root_dir.display().to_string().as_str())
        );

        let store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
        assert_eq!(store.workspace().id, report.workspace.id);
    }

    #[test]
    fn workspace_init_without_pointer_leaves_no_file() {
        let fixture = Fixture::new("init_no_pointer");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        workspace_init_with(&fixture.repo(), false).unwrap();
        assert_eq!(pointer::load(&fixture.repo_dir).unwrap(), None);
        assert!(!paths::workspace_pointer_path(&fixture.repo_dir).exists());
    }

    #[test]
    fn read_only_store_reads_work_state() {
        let fixture = Fixture::new("ro_read");
        WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        {
            let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repo()).unwrap();
            store.set_goal("Ship read-only inspection").unwrap();
            store
                .add_decision("Reads must never migrate", None)
                .unwrap();
        }

        let store = WorkStore::open_at_read_only(&fixture.root_dir, fixture.repo()).unwrap();
        assert_eq!(
            store.active_goal().unwrap().unwrap().statement,
            "Ship read-only inspection"
        );
        assert_eq!(store.list_decisions().unwrap().len(), 1);
    }

    #[test]
    fn read_only_store_does_not_migrate_v2() {
        let fixture = Fixture::new("ro_v2");
        let workspace_id = seed_v2_workspace(&fixture.root_dir, &fixture.repo());
        let db_path = paths::database_path(&fixture.root_dir, &workspace_id);
        let before = std::fs::read(&db_path).unwrap();

        let error = WorkStore::open_at_read_only(&fixture.root_dir, fixture.repo())
            .map(|_| ())
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
    }

    #[test]
    fn lookup_workspace_classifies_bound_absent_and_invalid() {
        let fixture = Fixture::new("lookup");
        assert_eq!(
            store::lookup_workspace(&fixture.root_dir, &fixture.repo()).unwrap(),
            store::WorkspaceLookup::Absent
        );

        let report = WorkStore::init_at(&fixture.root_dir, fixture.repo()).unwrap();
        assert_eq!(
            store::lookup_workspace(&fixture.root_dir, &fixture.repo()).unwrap(),
            store::WorkspaceLookup::Bound(report.workspace.id.clone())
        );

        pointer::save(
            &fixture.repo_dir,
            &WorkspacePointer {
                workspace_id: "root_ws_UNKNOWN".to_string(),
                root_dir_hint: None,
            },
        )
        .unwrap();
        match store::lookup_workspace(&fixture.root_dir, &fixture.repo()).unwrap() {
            store::WorkspaceLookup::PointerInvalid(message) => {
                assert!(message.contains("root restore --rebind"), "{message}");
                assert!(
                    message.contains("workspace init --write-pointer"),
                    "{message}"
                );
            }
            other => panic!("expected PointerInvalid, got {other:?}"),
        }
    }

    fn seed_v2_workspace(root_dir: &Path, repository: &Repository) -> String {
        let workspace_id = "root_ws_V2RO".to_string();
        let db_path = paths::database_path(root_dir, &workspace_id);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
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
            rusqlite::params![
                workspace_id,
                repository.root.display().to_string(),
                repository.identity()
            ],
        )
        .unwrap();
        drop(conn);

        let mut index = registry::WorkIndex::load(root_dir).unwrap();
        index.upsert(registry::WorkspaceEntry {
            id: workspace_id.clone(),
            repo_path: repository.root.display().to_string(),
            repo_identity: repository.identity(),
        });
        index.save(root_dir).unwrap();
        workspace_id
    }

    fn report_database(root_dir: &Path, store: &WorkStore) -> PathBuf {
        paths::database_path(root_dir, &store.workspace().id)
    }
}
