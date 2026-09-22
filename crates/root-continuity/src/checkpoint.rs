//! Durable checkpoint creation and inspection.
//!
//! A checkpoint is an honest record of state Root can observe: normalized work
//! state, observable Git state, observable environment references, and a
//! deterministic continuation summary. Checkpoints are immutable.

use crate::agent_env::CaptureOutcome;
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
    // Agent-environment capture is all-or-nothing. A genuine absence is
    // recorded as no reference; a present-but-unusable environment (malformed,
    // unreadable, secret-shaped, oversized) aborts the checkpoint before any
    // row or event is written, so a partial capture can never present as
    // complete.
    let (agent_env_ref, agent_env) = match crate::agent_env::capture(&repository.root, None)? {
        CaptureOutcome::Absent => (None, None),
        CaptureOutcome::Captured { summary, canonical } => {
            let reference = serde_json::to_string(&summary).map_err(|error| {
                anyhow::anyhow!("Failed to serialize agent env summary: {error}")
            })?;
            (Some(reference), Some(canonical))
        }
    };
    let snapshot = CheckpointSnapshot::capture_with_agent_env(store, agent_env)?;
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
            agent_env_ref: agent_env_ref.as_deref(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use root_agent_bundle::canonical::{
        CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalProvenance,
        CanonicalSkill, CANONICAL_SCHEMA_VERSION,
    };
    use root_agent_bundle::manifest::SECRET_DISCLOSURE;
    use root_agent_bundle::project::emit_agent_toml;
    use root_work::{ProvenanceContext, WorkStore};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    struct HomeGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let lock = crate::test_lock();
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", home);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    struct Fixture {
        base: PathBuf,
        repo: PathBuf,
        root_dir: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_continuity_checkpoint_{tag}_{}",
                std::process::id()
            ));
            let repo = base.join("campfire");
            let root_dir = base.join("root");
            let home = base.join("home");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(&root_dir).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            let repository = Repository::discover(&repo).unwrap();
            WorkStore::init_at(&root_dir, repository).unwrap();
            Self {
                base,
                repo,
                root_dir,
                home,
            }
        }

        fn repository(&self) -> Repository {
            Repository::discover(&self.repo).unwrap()
        }

        fn open(&self) -> WorkStore {
            WorkStore::open_at(&self.root_dir, self.repository()).unwrap()
        }

        fn try_create(&self) -> Result<root_work::CheckpointRecord> {
            let mut store = self.open();
            let repository = self.repository();
            Ok(create_on_store(
                &mut store,
                &repository,
                &self.root_dir,
                Some("checkpoint"),
                ProvenanceContext::default(),
            )?
            .checkpoint)
        }

        fn create(&self) -> root_work::CheckpointRecord {
            self.try_create().unwrap()
        }

        fn write_env(&self, text: &str) {
            let dir = self.repo.join(".root");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("agent.toml"), text).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn canonical_text() -> String {
        let env = CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
            source_agent: "codex".to_string(),
            source_agent_version: "0.150.1".to_string(),
            instructions: CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: vec![CanonicalSkill {
                name: "docs-writer".to_string(),
                files: vec!["SKILL.md".to_string()],
            }],
            tools: Vec::new(),
            mcp_servers: BTreeMap::new(),
            policies: Vec::new(),
            environment: CanonicalEnvironment { needs_env: vec![] },
            provenance: CanonicalProvenance {
                captured_at: "1234567890".to_string(),
                bundle_hash: String::new(),
                disclosure: SECRET_DISCLOSURE.to_string(),
            },
        };
        emit_agent_toml(&env).unwrap()
    }

    /// Best-effort snapshot parse for assertions (snapshot JSON is stored raw).
    fn snapshot_of(checkpoint: &root_work::CheckpointRecord) -> CheckpointSnapshot {
        CheckpointSnapshot::from_json(&checkpoint.snapshot).unwrap()
    }

    fn counts(fixture: &Fixture) -> (usize, usize) {
        let store = fixture.open();
        (
            store.events().unwrap().len(),
            store.list_checkpoints().unwrap().len(),
        )
    }

    #[test]
    fn checkpoint_with_agent_toml_has_populated_agent_env_ref_and_snapshot() {
        let fixture = Fixture::new("populated");
        let _guard = HomeGuard::set(&fixture.home);
        fixture.write_env(&canonical_text());

        let checkpoint = fixture.create();
        let raw = checkpoint
            .agent_env_ref
            .clone()
            .expect("agent_env_ref populated");
        let summary: crate::agent_env::AgentEnvSummary = serde_json::from_str(&raw).unwrap();
        assert_eq!(summary.adapter.as_deref(), Some("codex"));
        assert_eq!(summary.skills, vec!["docs-writer".to_string()]);

        // The canonical environment is embedded in the immutable snapshot too.
        let snapshot = snapshot_of(&checkpoint);
        let stored = snapshot
            .agent_env
            .clone()
            .expect("snapshot.agent_env populated");
        let canonical: CanonicalEnv = serde_json::from_value(stored).unwrap();
        assert_eq!(canonical.source_agent, "codex");
        assert_eq!(canonical.skills[0].name, "docs-writer");
        assert!(snapshot.agent_env_sha256.is_some());

        let store = fixture.open();
        let fetched = store.show_checkpoint(&checkpoint.id).unwrap();
        assert_eq!(fetched.agent_env_ref.as_deref(), Some(raw.as_str()));
        let refetched = snapshot_of(&fetched);
        assert_eq!(refetched.agent_env, snapshot.agent_env);
    }

    #[test]
    fn checkpoint_without_agent_env_has_no_ref_or_snapshot() {
        let fixture = Fixture::new("absent");
        let _guard = HomeGuard::set(&fixture.home);

        let checkpoint = fixture.create();
        assert!(checkpoint.agent_env_ref.is_none());
        let snapshot = snapshot_of(&checkpoint);
        assert!(snapshot.agent_env.is_none());
        assert!(snapshot.agent_env_sha256.is_none());
    }

    #[test]
    fn malformed_agent_env_fails_checkpoint_atomically() {
        let fixture = Fixture::new("malformed");
        let _guard = HomeGuard::set(&fixture.home);
        fixture.write_env("{ this is not valid toml");
        let (events_before, checkpoints_before) = counts(&fixture);

        let error = fixture.try_create().unwrap_err().to_string();
        assert!(
            error.contains("malformed or unreadable"),
            "sanitized error expected, got: {error}"
        );
        let (events_after, checkpoints_after) = counts(&fixture);
        assert_eq!(events_after, events_before, "no event must be appended");
        assert_eq!(
            checkpoints_after, checkpoints_before,
            "no checkpoint row must be written"
        );
    }

    #[test]
    fn secret_shaped_agent_env_fails_checkpoint_without_leaking_value() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        let fixture = Fixture::new("secret");
        let _guard = HomeGuard::set(&fixture.home);
        // Valid TOML carrying a secret-shaped tool value: parsing reaches the
        // free-text secret guard, which refuses without echoing the value.
        let toml = format!(
            "schema_version = 1\n\
             source_agent = \"codex\"\n\
             source_agent_version = \"0.150.1\"\n\
             [instructions]\n\
             main = \"AGENTS.md\"\n\
             [[tools]]\n\
             name = \"model\"\n\
             value = \"{secret}\"\n\
             [environment]\n\
             needs_env = []\n\
             [provenance]\n\
             captured_at = \"1234567890\"\n\
             bundle_hash = \"\"\n\
             disclosure = \"{SECRET_DISCLOSURE}\"\n"
        );
        fixture.write_env(&toml);

        let (events_before, checkpoints_before) = counts(&fixture);
        let error = fixture.try_create().unwrap_err().to_string();
        assert!(
            error.contains("looks like a secret"),
            "sanitized refusal expected, got: {error}"
        );
        assert!(!error.contains(secret), "secret value leaked: {error}");
        let (events_after, checkpoints_after) = counts(&fixture);
        assert_eq!(events_after, events_before);
        assert_eq!(checkpoints_after, checkpoints_before);
    }
}
