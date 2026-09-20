//! Recovery inspection: what durable state exists after an interruption and
//! what Root can safely continue from.
//!
//! Recovery never claims to restore unobserved state. It reports only the work,
//! checkpoint, repository, and environment facts Root actually persisted, and
//! lists what it cannot recover explicitly.

use crate::checkpoint::parse_snapshot;
use crate::drift::{self, DriftReport, DRIFT_BLOCKING, DRIFT_NONE, DRIFT_WARNING};
use crate::environment::EnvironmentState;
use crate::git::{short_sha, GitState};
use anyhow::Result;
use root_lockfile::get_root_dir;
use root_work::model::{ENV_MISSING, ENV_OBSERVED, ENV_UNKNOWN, ENV_VERIFIED, STATUS_ACTIVE};
use root_work::{Repository, WorkStore};
use serde::Serialize;
use std::path::Path;

/// State Root never captured and therefore cannot recover. This list is fixed
/// and honest; it does not depend on what happened to be running.
pub const NOT_RECOVERABLE: [&str; 3] = [
    "unrecorded agent conversation",
    "unsaved editor state",
    "commands not observed by Root",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecoverWorkspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecoverCheckpoint {
    pub id: String,
    pub message: Option<String>,
    pub created_at: String,
    pub age_human: String,
    pub work_revision: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecoverRepository {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecoverWorkState {
    pub available: bool,
    pub active_decisions: usize,
    pub active_findings: usize,
    pub artifacts: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecoverReport {
    pub workspace: RecoverWorkspace,
    pub last_checkpoint: Option<RecoverCheckpoint>,
    pub repository: RecoverRepository,
    pub environment: EnvironmentState,
    pub drift: DriftReport,
    pub work_state: RecoverWorkState,
    pub recoverable: Vec<String>,
    pub not_recoverable: Vec<String>,
    pub recommended_action: String,
}

/// Inspect durable state for the workspace associated with `cwd`.
pub fn recover(cwd: &Path) -> Result<RecoverReport> {
    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository.clone())?;
    let root_dir = get_root_dir()?;
    let environment = EnvironmentState::capture_at(&root_dir)?;
    let git = GitState::capture(&repository);
    build_recover(&store, &git, &environment)
}

/// Build a recovery report from an already-open store and observed state.
pub fn build_recover(
    store: &WorkStore,
    git: &GitState,
    environment: &EnvironmentState,
) -> Result<RecoverReport> {
    let latest = store.latest_checkpoint()?;
    let mut drift = DriftReport {
        level: DRIFT_NONE.to_string(),
        items: Vec::new(),
    };
    let last_checkpoint = match &latest {
        Some(checkpoint) => {
            let snapshot = parse_snapshot(checkpoint)?;
            drift = drift::detect(
                &store.repository().root,
                checkpoint,
                &snapshot,
                git,
                environment,
            );
            Some(RecoverCheckpoint {
                id: checkpoint.id.clone(),
                message: checkpoint.message.clone(),
                created_at: checkpoint.created_at.clone(),
                age_human: root_work::time::humanize_age(&checkpoint.created_at),
                work_revision: checkpoint.work_revision,
            })
        }
        None => None,
    };

    let decisions = store.list_decisions()?;
    let findings = store.list_findings()?;
    let artifacts = store.list_artifacts()?;
    let active_decisions = decisions
        .iter()
        .filter(|decision| decision.status == STATUS_ACTIVE)
        .count();
    let active_findings = findings
        .iter()
        .filter(|finding| finding.status == STATUS_ACTIVE)
        .count();

    let mut recoverable = Vec::new();
    if store.active_goal()?.is_some() {
        recoverable.push("goal".to_string());
    }
    if !decisions.is_empty() {
        recoverable.push("decisions".to_string());
    }
    if !findings.is_empty() {
        recoverable.push("findings".to_string());
    }
    if !artifacts.is_empty() {
        recoverable.push("artifact references".to_string());
    }
    if last_checkpoint.is_some() {
        recoverable.push("checkpoint reference".to_string());
    }
    if environment_has_reference(environment) {
        recoverable.push("environment reference".to_string());
    }

    let recommended_action = if drift.level == DRIFT_WARNING || drift.level == DRIFT_BLOCKING {
        "Inspect drift before resuming.".to_string()
    } else if let Some(checkpoint) = &last_checkpoint {
        format!("Resume from {}.", checkpoint.id)
    } else {
        "Create a checkpoint to establish a durable continuation point.".to_string()
    };

    Ok(RecoverReport {
        workspace: RecoverWorkspace {
            id: store.workspace().id.clone(),
            name: store.workspace().name.clone(),
        },
        last_checkpoint,
        repository: RecoverRepository {
            branch: git.branch.clone(),
            head: git.head.clone(),
            dirty: git.dirty,
        },
        environment: environment.clone(),
        drift,
        work_state: RecoverWorkState {
            available: true,
            active_decisions,
            active_findings,
            artifacts: artifacts.len(),
        },
        recoverable,
        not_recoverable: NOT_RECOVERABLE
            .iter()
            .map(|item| item.to_string())
            .collect(),
        recommended_action,
    })
}

fn environment_has_reference(environment: &EnvironmentState) -> bool {
    environment.rootfile_digest.is_some()
        || environment.root_lock_digest.is_some()
        || environment.profile_reference.is_some()
}

/// Render a human-inspectable recovery report in the Sprint 010 output style.
pub fn render_recover(report: &RecoverReport) -> String {
    let mut out = String::from("Root Recovery\n");
    out.push_str(&format!("\nWorkspace\n  {}\n", report.workspace.name));

    out.push_str("\nLast durable checkpoint\n");
    match &report.last_checkpoint {
        Some(checkpoint) => {
            out.push_str(&format!(
                "  {}\n  {}\n",
                checkpoint.id, checkpoint.age_human
            ));
            if let Some(message) = &checkpoint.message {
                out.push_str(&format!("  {message}\n"));
            }
        }
        None => out.push_str("  (none)\n"),
    }

    out.push_str("\nRepository\n");
    let branch = report.repository.branch.as_deref().unwrap_or("(no branch)");
    let head = report
        .repository
        .head
        .as_deref()
        .map(short_sha)
        .unwrap_or_else(|| "(no commits)".to_string());
    out.push_str(&format!("  {branch} @ {head}\n"));
    out.push_str(&format!(
        "  {}\n",
        if report.repository.dirty {
            "working tree dirty"
        } else {
            "working tree clean"
        }
    ));

    let mut artifact_changes = 0usize;
    for item in &report.drift.items {
        match item.kind.as_str() {
            "repository.head" => out.push_str("  HEAD changed since checkpoint\n"),
            "repository.branch" => out.push_str("  Branch changed since checkpoint\n"),
            "repository.dirty" => out.push_str("  Working tree dirty state changed\n"),
            "artifact.missing" | "artifact.changed" => artifact_changes += 1,
            _ => {}
        }
    }
    if artifact_changes > 0 {
        out.push_str(&format!(
            "  {artifact_changes} referenced artifacts modified\n"
        ));
    }

    out.push_str("\nEnvironment\n");
    out.push_str(&format!(
        "  {}\n",
        environment_label(&report.environment.status)
    ));

    out.push_str("\nWork state\n");
    out.push_str(if report.work_state.available {
        "  available\n"
    } else {
        "  unavailable\n"
    });
    out.push_str(&format!(
        "  {} active decisions, {} active findings, {} artifacts\n",
        report.work_state.active_decisions,
        report.work_state.active_findings,
        report.work_state.artifacts
    ));

    out.push_str("\nRecoverable\n");
    if report.recoverable.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for line in &report.recoverable {
            out.push_str(&format!("  {line}\n"));
        }
    }

    out.push_str("\nNot recoverable\n");
    for line in &report.not_recoverable {
        out.push_str(&format!("  {line}\n"));
    }

    out.push_str("\nRecommended action\n");
    out.push_str(&format!("  {}\n", report.recommended_action));
    out
}

fn environment_label(status: &str) -> &'static str {
    match status {
        ENV_VERIFIED => "environment verified",
        ENV_OBSERVED => "environment observed",
        ENV_UNKNOWN => "environment partially observed",
        ENV_MISSING => "no Root environment declared",
        _ => "environment status unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CheckpointSnapshot;
    use root_work::{NewCheckpoint, ProvenanceContext};
    use std::path::PathBuf;

    struct Fixture {
        base: PathBuf,
        repo: PathBuf,
        root_dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_continuity_recover_{tag}_{}",
                std::process::id()
            ));
            let repo = base.join("campfire");
            let root_dir = base.join("root");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(&root_dir).unwrap();
            let repository = Repository::discover(&repo).unwrap();
            WorkStore::init_at(&root_dir, repository).unwrap();
            Self {
                base,
                repo,
                root_dir,
            }
        }

        fn store(&self) -> WorkStore {
            let repository = Repository::discover(&self.repo).unwrap();
            WorkStore::open_at(&self.root_dir, repository).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn git(head: &str) -> GitState {
        GitState {
            head: Some(head.to_string()),
            branch: Some("main".to_string()),
            dirty: false,
            dirty_fingerprint: Some("cleanfp".to_string()),
        }
    }

    fn environment() -> EnvironmentState {
        EnvironmentState {
            rootfile_digest: None,
            root_lock_digest: None,
            profile_reference: None,
            status: ENV_MISSING.to_string(),
        }
    }

    fn checkpoint(store: &mut WorkStore) {
        let snapshot = CheckpointSnapshot::capture(store).unwrap();
        let snapshot_json = snapshot.to_json().unwrap();
        store
            .create_checkpoint_with(
                NewCheckpoint {
                    message: Some("checkpoint message"),
                    work_revision: snapshot.work_revision,
                    git_head: Some("aaaaaaaaaaaaaaaa"),
                    git_branch: Some("main"),
                    git_dirty: false,
                    git_dirty_fingerprint: Some("cleanfp"),
                    rootfile_digest: None,
                    root_lock_digest: None,
                    profile_reference: None,
                    environment_status: ENV_MISSING,
                    continuation_summary: "summary",
                    snapshot: &snapshot_json,
                },
                ProvenanceContext::default(),
            )
            .unwrap();
    }

    #[test]
    fn no_checkpoint_reports_work_state_without_error() {
        let fixture = Fixture::new("nocheckpoint");
        let mut store = fixture.store();
        store.set_goal("Implement invitations").unwrap();
        store.add_decision("Tokens expire after 24h", None).unwrap();

        let report = build_recover(&store, &git("aaaaaaaaaaaaaaaa"), &environment()).unwrap();
        assert!(report.last_checkpoint.is_none());
        assert_eq!(report.drift.level, DRIFT_NONE);
        assert!(report.recoverable.contains(&"goal".to_string()));
        assert!(report.recoverable.contains(&"decisions".to_string()));
        assert!(!report
            .recoverable
            .contains(&"checkpoint reference".to_string()));
        assert_eq!(
            report.recommended_action,
            "Create a checkpoint to establish a durable continuation point."
        );
    }

    #[test]
    fn checkpoint_enables_resume_recommendation() {
        let fixture = Fixture::new("checkpoint");
        let mut store = fixture.store();
        store.set_goal("Implement invitations").unwrap();
        store.add_decision("Tokens expire after 24h", None).unwrap();
        checkpoint(&mut store);

        let report = build_recover(&store, &git("aaaaaaaaaaaaaaaa"), &environment()).unwrap();
        let last = report.last_checkpoint.as_ref().unwrap();
        assert_eq!(last.work_revision, 3);
        assert!(report
            .recoverable
            .contains(&"checkpoint reference".to_string()));
        assert_eq!(
            report.recommended_action,
            format!("Resume from {}.", last.id)
        );
    }

    #[test]
    fn head_change_recommends_inspecting_drift() {
        let fixture = Fixture::new("drift");
        let mut store = fixture.store();
        store.set_goal("Implement invitations").unwrap();
        checkpoint(&mut store);

        let report = build_recover(&store, &git("bbbbbbbbbbbbbbbb"), &environment()).unwrap();
        assert_eq!(report.drift.level, DRIFT_WARNING);
        assert_eq!(report.recommended_action, "Inspect drift before resuming.");
    }

    #[test]
    fn not_recoverable_is_fixed_and_honest() {
        let fixture = Fixture::new("honest");
        let store = fixture.store();
        let report = build_recover(&store, &git("aaaaaaaaaaaaaaaa"), &environment()).unwrap();
        assert_eq!(report.not_recoverable.len(), 3);
        for item in NOT_RECOVERABLE {
            assert!(report.not_recoverable.contains(&item.to_string()));
        }
    }

    #[test]
    fn render_includes_expected_sections() {
        let fixture = Fixture::new("render");
        let mut store = fixture.store();
        store.set_goal("Implement invitations").unwrap();
        checkpoint(&mut store);
        let report = build_recover(&store, &git("aaaaaaaaaaaaaaaa"), &environment()).unwrap();
        let rendered = render_recover(&report);
        for section in [
            "Root Recovery",
            "Workspace",
            "Last durable checkpoint",
            "Repository",
            "Environment",
            "Work state",
            "Recoverable",
            "Not recoverable",
            "Recommended action",
        ] {
            assert!(rendered.contains(section), "missing {section}:\n{rendered}");
        }
        assert!(rendered.contains("commands not observed by Root"));
    }
}
