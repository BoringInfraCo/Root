//! Deterministic resume projection: turn a checkpoint plus current observed
//! state into a minimal, inspectable continuation package.
//!
//! Resume is minimal (active work only), evidence-aware (recorded facts are
//! separated from generated suggestions), and deterministic: the same inputs
//! always produce the same projection. No LLM is involved.

use crate::checkpoint::parse_snapshot;
use crate::drift::{self, DriftReport};
use crate::environment::EnvironmentState;
use crate::git::GitState;
use crate::snapshot::CheckpointSnapshot;
use anyhow::{anyhow, Result};
use root_work::model::STATUS_ACTIVE;
use root_work::{
    ArtifactRecord, CheckpointRecord, DecisionRecord, FindingRecord, GoalRecord, Repository,
    WorkStore, WorkspaceRecord,
};
use serde::Serialize;
use std::path::Path;

/// Maximum number of decisions included in a resume package.
pub const DECISION_CAP: usize = 10;

/// Maximum number of findings included in a resume package.
pub const FINDING_CAP: usize = 10;

/// Maximum number of artifacts included in a resume package.
pub const ARTIFACT_CAP: usize = 20;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeWorkspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeCheckpoint {
    pub id: String,
    pub message: Option<String>,
    pub created_at: String,
    pub work_revision: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeCurrentState {
    pub active_decisions: usize,
    pub active_findings: usize,
    pub artifacts: usize,
    pub work_revision: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeProvenance {
    pub checkpoint_provenance_id: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeReport {
    pub workspace: ResumeWorkspace,
    pub goal: Option<GoalRecord>,
    pub checkpoint: ResumeCheckpoint,
    pub current_state: ResumeCurrentState,
    pub unresolved_work: Vec<String>,
    pub decisions: Vec<DecisionRecord>,
    pub findings: Vec<FindingRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub decisions_omitted: usize,
    pub findings_omitted: usize,
    pub artifacts_omitted: usize,
    pub repository_state: GitState,
    pub environment_state: EnvironmentState,
    pub drift: DriftReport,
    pub suggested_continuation: Vec<String>,
    pub provenance: ResumeProvenance,
}

/// Build a continuation package for `cwd` from an explicit checkpoint or the
/// most recent one.
pub fn resume(cwd: &Path, checkpoint_id: Option<&str>) -> Result<ResumeReport> {
    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository.clone())?;

    let checkpoint = match checkpoint_id {
        Some(id) => store.show_checkpoint(id)?,
        None => store.latest_checkpoint()?.ok_or_else(|| {
            anyhow!(
                "No checkpoints exist for this workspace.\n\n\
                 Create one with:  root checkpoint create"
            )
        })?,
    };

    let snapshot = parse_snapshot(&checkpoint)?;
    let git = GitState::capture(&repository);
    let environment = EnvironmentState::capture()?;
    let current = current_work(&store)?;

    Ok(build_report(
        store.workspace(),
        &checkpoint,
        &snapshot,
        &git,
        &environment,
        &repository.root,
        &current,
    ))
}

fn current_work(store: &WorkStore) -> Result<ResumeCurrentState> {
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
    let artifact_count = artifacts.len();
    let work_revision = store.work_revision()?;

    Ok(ResumeCurrentState {
        active_decisions,
        active_findings,
        artifacts: artifact_count,
        work_revision,
        summary: format!(
            "{active_decisions} active decisions, {active_findings} active findings, {artifact_count} artifacts recorded."
        ),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_report(
    workspace: &WorkspaceRecord,
    checkpoint: &CheckpointRecord,
    snapshot: &CheckpointSnapshot,
    git: &GitState,
    environment: &EnvironmentState,
    repo_root: &Path,
    current: &ResumeCurrentState,
) -> ResumeReport {
    let active_decisions: Vec<DecisionRecord> = snapshot.active_decisions().cloned().collect();
    let active_findings: Vec<FindingRecord> = snapshot.active_findings().cloned().collect();

    let (decisions, decisions_omitted) = cap_records(&active_decisions, DECISION_CAP, |decision| {
        (decision.created_at.as_str(), decision.id.as_str())
    });
    let (findings, findings_omitted) = cap_records(&active_findings, FINDING_CAP, |finding| {
        (finding.created_at.as_str(), finding.id.as_str())
    });
    let (artifacts, artifacts_omitted) =
        cap_records(&snapshot.artifacts, ARTIFACT_CAP, |artifact| {
            (artifact.created_at.as_str(), artifact.id.as_str())
        });

    let unresolved_work = findings
        .iter()
        .map(|finding| format!("Recorded observation: {}", finding.statement))
        .collect();

    let drift = drift::detect(repo_root, checkpoint, snapshot, git, environment);
    let suggested_continuation = suggestions(&findings);

    ResumeReport {
        workspace: ResumeWorkspace {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
        },
        goal: snapshot.goal.clone(),
        checkpoint: ResumeCheckpoint {
            id: checkpoint.id.clone(),
            message: checkpoint.message.clone(),
            created_at: checkpoint.created_at.clone(),
            work_revision: checkpoint.work_revision,
        },
        current_state: current.clone(),
        unresolved_work,
        decisions,
        findings,
        artifacts,
        decisions_omitted,
        findings_omitted,
        artifacts_omitted,
        repository_state: git.clone(),
        environment_state: environment.clone(),
        drift,
        suggested_continuation,
        provenance: ResumeProvenance {
            checkpoint_provenance_id: checkpoint.provenance_id.clone(),
            source: "checkpoint".to_string(),
        },
    }
}

/// Sort newest-first (created_at desc, id asc for deterministic ties) and cap.
fn cap_records<T: Clone>(
    items: &[T],
    cap: usize,
    key: impl for<'a> Fn(&'a T) -> (&'a str, &'a str),
) -> (Vec<T>, usize) {
    let mut ordered = items.to_vec();
    ordered.sort_by(|a, b| {
        let (a_time, a_id) = key(a);
        let (b_time, b_id) = key(b);
        b_time.cmp(a_time).then_with(|| a_id.cmp(b_id))
    });
    let omitted = ordered.len().saturating_sub(cap);
    ordered.truncate(cap);
    (ordered, omitted)
}

fn suggestions(findings: &[FindingRecord]) -> Vec<String> {
    match findings.first() {
        Some(finding) => vec![format!("Investigate: {}", finding.statement)],
        None => vec!["Review the active goal and recorded decisions.".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_work::model::{STATUS_ACTIVE, STATUS_SUPERSEDED};

    fn workspace() -> WorkspaceRecord {
        WorkspaceRecord {
            id: "root_ws_test".into(),
            name: "campfire".into(),
            repo_path: "/repo".into(),
            repo_identity: "path:/repo".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn checkpoint() -> CheckpointRecord {
        CheckpointRecord {
            id: "root_cp_test".into(),
            workspace_id: "root_ws_test".into(),
            goal_id: Some("root_goal_test".into()),
            message: Some("endpoint implemented".into()),
            work_revision: 5,
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
            provenance_id: Some("root_prov_test".into()),
        }
    }

    fn decision(id: &str, status: &str, statement: &str) -> DecisionRecord {
        DecisionRecord {
            id: id.into(),
            workspace_id: "root_ws_test".into(),
            goal_id: None,
            statement: statement.into(),
            rationale: None,
            status: status.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            provenance_id: None,
        }
    }

    fn finding(id: &str, status: &str, statement: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            workspace_id: "root_ws_test".into(),
            goal_id: None,
            statement: statement.into(),
            evidence_ref: None,
            status: status.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            provenance_id: None,
        }
    }

    fn git() -> GitState {
        GitState {
            head: Some("aaaaaaaaaaaaaaaa".into()),
            branch: Some("main".into()),
            dirty: false,
            dirty_fingerprint: Some("cleanfp".into()),
        }
    }

    fn environment() -> EnvironmentState {
        EnvironmentState {
            rootfile_digest: Some("rf".into()),
            root_lock_digest: Some("lock".into()),
            profile_reference: Some("/profiles/default".into()),
            status: "observed".into(),
        }
    }

    fn current_state() -> ResumeCurrentState {
        ResumeCurrentState {
            active_decisions: 1,
            active_findings: 1,
            artifacts: 0,
            work_revision: 7,
            summary: "1 active decisions, 1 active findings, 0 artifacts recorded.".into(),
        }
    }

    fn snapshot(
        decisions: Vec<DecisionRecord>,
        findings: Vec<FindingRecord>,
        artifacts: Vec<ArtifactRecord>,
    ) -> CheckpointSnapshot {
        CheckpointSnapshot {
            goal: Some(GoalRecord {
                id: "root_goal_test".into(),
                workspace_id: "root_ws_test".into(),
                statement: "Implement invitations".into(),
                status: "active".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                completed_at: None,
                provenance_id: None,
            }),
            decisions,
            findings,
            artifacts,
            work_revision: 5,
        }
    }

    #[test]
    fn superseded_decision_and_finding_are_excluded() {
        let snapshot = snapshot(
            vec![
                decision("root_dec_1", STATUS_ACTIVE, "Tokens expire after 24h"),
                decision("root_dec_2", STATUS_SUPERSEDED, "Tokens never expire"),
            ],
            vec![
                finding("root_find_1", STATUS_ACTIVE, "Consumption fails"),
                finding("root_find_2", STATUS_SUPERSEDED, "Old behavior"),
            ],
            Vec::new(),
        );

        let report = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );

        assert_eq!(report.decisions.len(), 1);
        assert_eq!(report.decisions[0].id, "root_dec_1");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].id, "root_find_1");
        assert_eq!(report.unresolved_work.len(), 1);
        assert!(report.unresolved_work[0].starts_with("Recorded observation:"));
    }

    #[test]
    fn projection_is_deterministic() {
        let snapshot = snapshot(
            vec![decision("root_dec_1", STATUS_ACTIVE, "Tokens hashed")],
            vec![finding("root_find_1", STATUS_ACTIVE, "Consumption fails")],
            Vec::new(),
        );
        let first = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );
        let second = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }

    #[test]
    fn suggestion_prefers_latest_finding() {
        let snapshot = snapshot(
            Vec::new(),
            vec![finding("root_find_1", STATUS_ACTIVE, "Consumption fails")],
            Vec::new(),
        );
        let report = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );
        assert_eq!(
            report.suggested_continuation,
            vec!["Investigate: Consumption fails".to_string()]
        );
    }

    #[test]
    fn suggestion_falls_back_without_findings() {
        let snapshot = snapshot(Vec::new(), Vec::new(), Vec::new());
        let report = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );
        assert_eq!(
            report.suggested_continuation,
            vec!["Review the active goal and recorded decisions.".to_string()]
        );
    }

    #[test]
    fn artifacts_are_capped_and_ordered_newest_first() {
        let mut artifacts = Vec::new();
        for index in 0..25 {
            artifacts.push(ArtifactRecord {
                id: format!("root_art_{index:02}"),
                workspace_id: "root_ws_test".into(),
                kind: "file".into(),
                uri: format!("src/file_{index}.ts"),
                fingerprint: None,
                created_at: format!("2026-01-01T00:{index:02}:00Z"),
                provenance_id: None,
            });
        }
        let snapshot = snapshot(Vec::new(), Vec::new(), artifacts);
        let report = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );
        assert_eq!(report.artifacts.len(), ARTIFACT_CAP);
        assert_eq!(report.artifacts_omitted, 5);
        assert_eq!(report.artifacts[0].id, "root_art_24");
    }

    #[test]
    fn decisions_and_findings_are_capped_and_ordered_newest_first() {
        let mut decisions = Vec::new();
        let mut findings = Vec::new();
        for index in 0..25 {
            let mut decision_record =
                decision(&format!("root_dec_{index:02}"), STATUS_ACTIVE, "Statement");
            decision_record.created_at = format!("2026-01-01T00:{index:02}:00Z");
            decisions.push(decision_record);

            let mut finding_record =
                finding(&format!("root_find_{index:02}"), STATUS_ACTIVE, "Statement");
            finding_record.created_at = format!("2026-01-01T00:{index:02}:00Z");
            findings.push(finding_record);
        }

        let snapshot = snapshot(decisions, findings, Vec::new());
        let report = build_report(
            &workspace(),
            &checkpoint(),
            &snapshot,
            &git(),
            &environment(),
            Path::new("/repo"),
            &current_state(),
        );

        assert_eq!(report.decisions.len(), DECISION_CAP);
        assert_eq!(report.decisions_omitted, 15);
        assert_eq!(report.decisions[0].id, "root_dec_24");
        assert_eq!(report.findings.len(), FINDING_CAP);
        assert_eq!(report.findings_omitted, 15);
        assert_eq!(report.findings[0].id, "root_find_24");
        assert_eq!(report.unresolved_work.len(), FINDING_CAP);
    }
}
