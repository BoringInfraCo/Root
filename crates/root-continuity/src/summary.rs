//! Deterministic, LLM-free continuation summary generation.

use crate::environment::EnvironmentState;
use crate::git::{short_sha, GitState};
use crate::snapshot::CheckpointSnapshot;
use root_work::model::{ENV_MISSING, ENV_OBSERVED, ENV_UNKNOWN, ENV_VERIFIED};

/// Build a concise, deterministic continuation summary from structured state.
pub fn continuation_summary(
    snapshot: &CheckpointSnapshot,
    git: &GitState,
    environment: &EnvironmentState,
) -> String {
    let goal = snapshot
        .goal
        .as_ref()
        .map(|goal| goal.statement.clone())
        .unwrap_or_else(|| "(none)".to_string());

    let latest_finding = snapshot
        .findings
        .first()
        .map(|finding| finding.statement.clone())
        .unwrap_or_else(|| "(none)".to_string());

    let branch = git.branch.as_deref().unwrap_or("(no branch)");
    let head = git
        .head
        .as_deref()
        .map(short_sha)
        .unwrap_or_else(|| "(no commits)".to_string());
    let dirty = if git.dirty_fingerprint.is_none() {
        "unknown"
    } else if git.dirty {
        "dirty"
    } else {
        "clean"
    };

    let environment_line = match environment.status.as_str() {
        ENV_VERIFIED => "Environment verified.",
        ENV_OBSERVED => "Environment observed (Rootfile and root.lock present).",
        ENV_UNKNOWN => "Environment partially observed.",
        ENV_MISSING => "No Root environment declared.",
        _ => "Environment status unknown.",
    };

    format!(
        "Goal:\n{goal}\n\n\
         Current work:\n{decisions} decisions, {findings} findings, {artifacts} artifacts.\n\n\
         Latest finding:\n{latest_finding}\n\n\
         Repository:\n{branch} @ {head}\n{dirty}\n\n\
         Environment:\n{environment_line}",
        goal = goal,
        decisions = snapshot.decisions.len(),
        findings = snapshot.findings.len(),
        artifacts = snapshot.artifacts.len(),
        latest_finding = latest_finding,
        branch = branch,
        head = head,
        dirty = dirty,
        environment_line = environment_line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_work::FindingRecord;

    fn snapshot_with_finding(statement: &str) -> CheckpointSnapshot {
        CheckpointSnapshot {
            goal: None,
            decisions: Vec::new(),
            findings: vec![FindingRecord {
                id: "root_find_1".into(),
                workspace_id: "root_ws_1".into(),
                goal_id: None,
                statement: statement.into(),
                evidence_ref: None,
                status: "active".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                provenance_id: None,
            }],
            artifacts: Vec::new(),
            work_revision: 4,
        }
    }

    #[test]
    fn summary_is_deterministic_and_includes_latest_finding() {
        let snapshot = snapshot_with_finding("Invite consumption fails");
        let git = GitState {
            head: Some("2fc193e0aabbcc".into()),
            branch: Some("feat/x".into()),
            dirty: true,
            dirty_fingerprint: Some("fp".into()),
        };
        let environment = EnvironmentState {
            rootfile_digest: Some("a".into()),
            root_lock_digest: Some("b".into()),
            profile_reference: None,
            status: ENV_OBSERVED.into(),
        };
        let summary = continuation_summary(&snapshot, &git, &environment);
        assert!(summary.contains("Invite consumption fails"));
        assert!(summary.contains("feat/x @ 2fc193e"));
        assert!(summary.contains("dirty"));
        assert!(summary.contains("0 decisions, 1 findings, 0 artifacts"));
        assert_eq!(summary, continuation_summary(&snapshot, &git, &environment));
    }
}
