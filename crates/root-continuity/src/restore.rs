//! Work-state bind for deterministic restore (Sprint 013 §3.2 step 4).
//!
//! Restore always runs the environment restore first; this module only *binds*
//! the durable work state afterwards. It reports what exists and never
//! fabricates a checkpoint: an empty workspace yields `latest_checkpoint_id:
//! None`. Being outside a repository or having no bound workspace is reported
//! as `available: false` with a note, not an error, so environment restore can
//! still succeed.

use crate::checkpoint::parse_snapshot;
use crate::drift::{self, DriftReport};
use crate::environment::EnvironmentState;
use crate::git::GitState;
use anyhow::Result;
use root_lockfile::get_root_dir;
use root_work::store::{lookup_workspace, WorkspaceLookup};
use root_work::{Repository, WorkStore};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkBindReport {
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub latest_checkpoint_id: Option<String>,
    pub work_revision: Option<i64>,
    pub available: bool,
    pub drift: Option<DriftReport>,
    pub notes: Vec<String>,
}

/// Resolve and inspect the workspace bound to `cwd` without mutating anything.
///
/// The store is opened read-only so `root restore --dry-run` never creates or
/// migrates the work database. A genuinely absent workspace is reported as
/// `available: false`; a malformed or stale project pointer fails closed with
/// an error so the CLI can refuse rather than silently pretend no workspace
/// exists.
pub fn bind_workspace(cwd: &Path) -> Result<WorkBindReport> {
    let repository = match Repository::discover(cwd) {
        Ok(repository) => repository,
        Err(_) => {
            return Ok(unavailable(vec![
                "not inside a Git repository; no workspace bound".to_string(),
            ]))
        }
    };
    let root_dir = get_root_dir()?;
    match lookup_workspace(&root_dir, &repository)? {
        WorkspaceLookup::Bound(_) => {}
        WorkspaceLookup::Absent => {
            return Ok(unavailable(vec![
                "no workspace bound (run `root workspace init`)".to_string(),
            ]))
        }
        // Malformed or stale pointer: never downgrade to available:false.
        WorkspaceLookup::PointerInvalid(message) => anyhow::bail!("{message}"),
    }

    let store = WorkStore::open_at_read_only(&root_dir, repository.clone())?;

    let latest = store.latest_checkpoint()?;
    let work_revision = store.work_revision()?;
    let latest_checkpoint_id = latest.as_ref().map(|checkpoint| checkpoint.id.clone());
    let mut notes = Vec::new();
    let mut drift = None;

    match &latest {
        Some(checkpoint) => match parse_snapshot(checkpoint) {
            Ok(snapshot) => {
                let git = GitState::capture(&repository);
                let environment = EnvironmentState::capture_at(&root_dir)?;
                drift = Some(drift::detect(
                    &repository.root,
                    checkpoint,
                    &snapshot,
                    &git,
                    &environment,
                ));
            }
            Err(error) => notes.push(format!(
                "latest checkpoint '{}' has an unreadable snapshot: {error}",
                checkpoint.id
            )),
        },
        None => notes.push("no checkpoint recorded yet".to_string()),
    }

    Ok(WorkBindReport {
        workspace_id: Some(store.workspace().id.clone()),
        workspace_name: Some(store.workspace().name.clone()),
        latest_checkpoint_id,
        work_revision: Some(work_revision),
        available: true,
        drift,
        notes,
    })
}

/// Explicitly re-bind the repository's project pointer to a known workspace.
///
/// Delegates to `root_work::pointer::rebind`, which repairs a malformed pointer
/// when the repository identity matches a known workspace and returns `None`
/// when no workspace is known. This is the explicit `root restore --rebind`
/// flow — distinct from the read-only bind above.
pub fn rebind_workspace(root_dir: &Path, repository: &Repository) -> Result<Option<String>> {
    root_work::pointer::rebind(root_dir, repository)
}

fn unavailable(notes: Vec<String>) -> WorkBindReport {
    WorkBindReport {
        workspace_id: None,
        workspace_name: None,
        latest_checkpoint_id: None,
        work_revision: None,
        available: false,
        drift: None,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_lock;
    use root_work::{NewCheckpoint, ProvenanceContext};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    struct RootDirGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl RootDirGuard {
        fn set(dir: &Path) -> Self {
            let lock = test_lock();
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
        base: PathBuf,
        repo: PathBuf,
        root_dir: PathBuf,
        workspace_id: String,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_continuity_restore_{tag}_{}",
                std::process::id()
            ));
            let repo = base.join("campfire");
            let root_dir = base.join("root");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(&root_dir).unwrap();
            let repository = Repository::discover(&repo).unwrap();
            let init = WorkStore::init_at(&root_dir, repository).unwrap();
            Self {
                base,
                repo,
                root_dir,
                workspace_id: init.workspace.id,
            }
        }

        fn repository(&self) -> Repository {
            Repository::discover(&self.repo).unwrap()
        }

        fn database(&self) -> PathBuf {
            root_work::paths::database_path(&self.root_dir, &self.workspace_id)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn create_checkpoint(fixture: &Fixture, lock_digest: Option<&str>) {
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repository()).unwrap();
        store.set_goal("Bind work state").unwrap();
        let revision = store.work_revision().unwrap();
        let snapshot_json = {
            let snapshot = crate::snapshot::CheckpointSnapshot::capture(&store).unwrap();
            serde_json::to_string(&snapshot).unwrap()
        };
        store
            .create_checkpoint_with(
                NewCheckpoint {
                    message: Some("checkpoint"),
                    work_revision: revision,
                    git_head: None,
                    git_branch: None,
                    git_dirty: false,
                    git_dirty_fingerprint: None,
                    rootfile_digest: Some("rootfile"),
                    root_lock_digest: lock_digest,
                    profile_reference: None,
                    environment_status: root_work::model::ENV_OBSERVED,
                    continuation_summary: "summary",
                    snapshot: &snapshot_json,
                    agent_env_ref: None,
                },
                ProvenanceContext::default(),
            )
            .unwrap();
    }

    #[test]
    fn outside_repository_is_unavailable_not_error() {
        let base = std::env::temp_dir().join(format!(
            "root_continuity_restore_norepo_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let report = bind_workspace(&base).unwrap();
        assert!(!report.available);
        assert!(report.workspace_id.is_none());
        assert!(report.latest_checkpoint_id.is_none());
        assert!(!report.notes.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_store_never_fabricates_a_checkpoint() {
        let fixture = Fixture::new("empty");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        let report = bind_workspace(&fixture.repo).unwrap();
        assert!(report.available);
        assert!(report.workspace_id.is_some());
        assert!(report.latest_checkpoint_id.is_none());
        assert!(report.drift.is_none());
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("no checkpoint")));
    }

    #[test]
    fn divergent_lock_digest_surfaces_environment_drift() {
        let fixture = Fixture::new("drift");
        std::fs::write(fixture.root_dir.join("Rootfile"), "[packages]\n").unwrap();
        std::fs::write(fixture.root_dir.join("root.lock"), "one").unwrap();
        create_checkpoint(&fixture, Some("recorded-lock-digest"));
        std::fs::write(fixture.root_dir.join("root.lock"), "two").unwrap();

        let _guard = RootDirGuard::set(&fixture.root_dir);
        let report = bind_workspace(&fixture.repo).unwrap();
        assert!(report.available);
        assert!(report.latest_checkpoint_id.is_some());
        let drift = report.drift.expect("drift should be computed");
        assert!(
            drift
                .items
                .iter()
                .any(|item| item.kind == "environment.lock"),
            "items: {:?}",
            drift.items
        );
    }

    #[test]
    fn no_workspace_bound_is_unavailable_not_error() {
        let base = std::env::temp_dir().join(format!(
            "root_continuity_restore_unbound_{}",
            std::process::id()
        ));
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        let _guard = RootDirGuard::set(&root_dir);

        let report = bind_workspace(&repo).unwrap();
        assert!(!report.available);
        assert!(report.workspace_id.is_none());
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("no workspace bound")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stale_pointer_fails_closed() {
        let fixture = Fixture::new("stale_pointer");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        root_work::pointer::save(
            &fixture.repo,
            &root_work::WorkspacePointer {
                workspace_id: "root_ws_UNKNOWN".to_string(),
                root_dir_hint: None,
            },
        )
        .unwrap();

        let error = bind_workspace(&fixture.repo)
            .map(|_| ())
            .expect_err("stale pointer must fail closed, not be downgraded");
        assert!(
            error.to_string().contains("Unknown workspace id"),
            "{error}"
        );
    }

    #[test]
    fn malformed_pointer_fails_closed() {
        let fixture = Fixture::new("malformed_pointer");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        let path = root_work::paths::workspace_pointer_path(&fixture.repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let error = bind_workspace(&fixture.repo)
            .map(|_| ())
            .expect_err("malformed pointer must fail closed, not be downgraded");
        assert!(
            error.to_string().contains("pointer")
                || error.to_string().contains("Workspace pointer"),
            "{error}"
        );
    }

    #[test]
    fn read_only_bind_does_not_mutate_database_files() {
        let fixture = Fixture::new("read_only");
        create_checkpoint(&fixture, Some("lock"));
        let _guard = RootDirGuard::set(&fixture.root_dir);

        let db = fixture.database();
        let before = root_work::fingerprint_file(&db).unwrap();
        let before_mtime = std::fs::metadata(&db).unwrap().modified().unwrap();

        let report = bind_workspace(&fixture.repo).unwrap();
        assert!(report.available);

        assert_eq!(
            root_work::fingerprint_file(&db).unwrap(),
            before,
            "read-only bind must not change the database bytes"
        );
        assert_eq!(
            std::fs::metadata(&db).unwrap().modified().unwrap(),
            before_mtime,
            "read-only bind must not change the database mtime"
        );
        let dir = db.parent().unwrap();
        assert!(
            !dir.join("state.db-wal").exists(),
            "bind created a -wal file"
        );
        assert!(
            !dir.join("state.db-shm").exists(),
            "bind created a -shm file"
        );
    }

    #[test]
    fn rebind_repairs_malformed_pointer_when_identity_matches() {
        let fixture = Fixture::new("rebind_repair");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        let path = root_work::paths::workspace_pointer_path(&fixture.repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let returned = rebind_workspace(&fixture.root_dir, &fixture.repository()).unwrap();
        assert_eq!(returned.as_deref(), Some(fixture.workspace_id.as_str()));
        let pointer = root_work::pointer::load(&fixture.repo).unwrap().unwrap();
        assert_eq!(pointer.workspace_id, fixture.workspace_id);
    }

    #[test]
    fn rebind_unknown_identity_returns_none() {
        let base = std::env::temp_dir().join(format!(
            "root_continuity_restore_rebind_none_{}",
            std::process::id()
        ));
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        let _guard = RootDirGuard::set(&root_dir);
        let repository = Repository::discover(&repo).unwrap();

        assert_eq!(rebind_workspace(&root_dir, &repository).unwrap(), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
