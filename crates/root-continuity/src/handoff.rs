//! Portable handoff projection: a deterministic view of canonical Root state
//! intended for another agent or human.
//!
//! Handoff is a projection, not a transcript conversion. It reuses `resume` to
//! select the latest checkpoint and active work, then adds provenance (`from`),
//! a normalized target (`to`), and harness-specific instructions.

use crate::drift::DriftReport;
use crate::environment::EnvironmentState;
use crate::resume::{resume, ResumeReport};
use crate::with::{target_instruction_text, SUPPORTED_TARGETS};
use anyhow::{bail, Result};
use root_work::model::{
    ENV_MISSING, ENV_OBSERVED, ENV_UNKNOWN, ENV_VERIFIED, SOURCE_HUMAN, SOURCE_ROOT,
};
use root_work::{ArtifactRecord, DecisionRecord, FindingRecord, GoalRecord, Repository, WorkStore};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HandoffCheckpoint {
    pub id: String,
    pub message: Option<String>,
    pub created_at: String,
    pub work_revision: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HandoffReport {
    pub handoff_id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub goal: Option<GoalRecord>,
    pub checkpoint: Option<HandoffCheckpoint>,
    pub state: Vec<String>,
    pub decisions: Vec<DecisionRecord>,
    pub findings: Vec<FindingRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub environment: EnvironmentState,
    pub drift: DriftReport,
    pub suggested_continuation: Vec<String>,
    pub instructions: Option<String>,
}

/// Build a handoff package for `cwd` from the most recent checkpoint.
pub fn handoff(cwd: &Path, to: Option<&str>) -> Result<HandoffReport> {
    let target = normalize_target(to)?;
    let report = resume(cwd, None)?;

    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository)?;
    let checkpoint = store.show_checkpoint(&report.checkpoint.id)?;
    let from = origin(&store, checkpoint.provenance_id.as_deref());

    let instructions = match &target {
        Some(id) => Some(target_instruction_text(id)?),
        None => None,
    };

    Ok(project(
        report,
        root_work::id::handoff_id(),
        from,
        target,
        instructions,
    ))
}

fn normalize_target(to: Option<&str>) -> Result<Option<String>> {
    match to {
        None => Ok(None),
        Some(raw) => {
            let id = raw.trim().to_ascii_lowercase();
            if id.is_empty() {
                bail!("A handoff target id is required after --to.");
            }
            if !SUPPORTED_TARGETS.contains(&id.as_str()) {
                bail!(
                    "Unsupported handoff target '{}'.\n\nSupported adapters: {}.",
                    raw.trim(),
                    SUPPORTED_TARGETS.join(", ")
                );
            }
            Ok(Some(id))
        }
    }
}

fn origin(store: &WorkStore, provenance_id: Option<&str>) -> Option<String> {
    let provenance_id = provenance_id?;
    let record = store.provenance(provenance_id).ok()?;
    if record.source_type == SOURCE_HUMAN || record.source_type == SOURCE_ROOT {
        return None;
    }
    record.harness.or(record.agent)
}

fn project(
    report: ResumeReport,
    handoff_id: String,
    from: Option<String>,
    to: Option<String>,
    instructions: Option<String>,
) -> HandoffReport {
    let mut state = vec![report.current_state.summary.clone()];
    state.extend(report.unresolved_work.iter().cloned());

    let suggested_continuation = report
        .suggested_continuation
        .iter()
        .map(|suggestion| format!("Suggestion (not verified): {suggestion}"))
        .collect();

    HandoffReport {
        handoff_id,
        from,
        to,
        goal: report.goal,
        checkpoint: Some(HandoffCheckpoint {
            id: report.checkpoint.id,
            message: report.checkpoint.message,
            created_at: report.checkpoint.created_at,
            work_revision: report.checkpoint.work_revision,
        }),
        state,
        decisions: report.decisions,
        findings: report.findings,
        artifacts: report.artifacts,
        environment: report.environment_state,
        drift: report.drift,
        suggested_continuation,
        instructions,
    }
}

/// Render a human-inspectable handoff in the Sprint 009 output style.
pub fn render_handoff(report: &HandoffReport) -> String {
    let mut out = String::from("Handoff\n");
    out.push_str(&format!("  {}\n", report.handoff_id));

    out.push_str("\nFrom\n");
    out.push_str(&format!("  {}\n", agent_label(&report.from)));

    out.push_str("\nTo\n");
    out.push_str(&format!("  {}\n", agent_label(&report.to)));

    out.push_str("\nGoal\n");
    match &report.goal {
        Some(goal) => out.push_str(&format!("  {}\n", goal.statement)),
        None => out.push_str("  (none)\n"),
    }

    out.push_str("\nCheckpoint\n");
    match &report.checkpoint {
        Some(checkpoint) => {
            out.push_str(&format!("  {}\n", checkpoint.id));
            if let Some(message) = &checkpoint.message {
                out.push_str(&format!("  {message}\n"));
            }
        }
        None => out.push_str("  (none)\n"),
    }

    out.push_str("\nState\n");
    if report.state.is_empty() {
        out.push_str("  (none recorded)\n");
    } else {
        for line in &report.state {
            out.push_str(&format!("  {line}\n"));
        }
    }

    out.push_str("\nImportant decisions\n");
    if report.decisions.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for decision in &report.decisions {
            out.push_str(&format!("  {}\n", decision.statement));
        }
    }

    out.push_str("\nLatest finding\n");
    match report.findings.first() {
        Some(finding) => out.push_str(&format!("  {}\n", finding.statement)),
        None => out.push_str("  (none)\n"),
    }

    out.push_str("\nEnvironment\n");
    out.push_str(&format!(
        "  {}\n",
        environment_label(&report.environment.status)
    ));

    out.push_str("\nDrift\n");
    if report.drift.items.is_empty() {
        out.push_str("  None.\n");
    } else {
        for item in &report.drift.items {
            out.push_str(&format!("  [{}] {}\n", item.level, item.detail));
        }
    }

    out
}

fn agent_label(value: &Option<String>) -> String {
    match value {
        Some(id) => root_adapters::display_name(id),
        None => "(none)".to_string(),
    }
}

fn environment_label(status: &str) -> &'static str {
    match status {
        ENV_VERIFIED => "Verified.",
        ENV_OBSERVED => "Observed (Rootfile and root.lock present).",
        ENV_UNKNOWN => "Partially observed.",
        ENV_MISSING => "Missing.",
        _ => "Unknown.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resume::{build_report, ResumeCurrentState};
    use crate::snapshot::CheckpointSnapshot;
    use root_work::model::{STATUS_ACTIVE, STATUS_SUPERSEDED};
    use root_work::{ProvenanceContext, WorkspaceRecord};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    struct RootDirGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl RootDirGuard {
        fn set(dir: &Path) -> Self {
            // Shared crate lock: serialize with every other ROOT_DIR test.
            let lock = crate::test_lock();
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
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_continuity_handoff_{tag}_{}",
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

        fn repository(&self) -> Repository {
            Repository::discover(&self.repo).unwrap()
        }

        fn store(&self) -> WorkStore {
            WorkStore::open_at(&self.root_dir, self.repository()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

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

    fn checkpoint_record() -> root_work::CheckpointRecord {
        root_work::CheckpointRecord {
            id: "root_cp_test".into(),
            workspace_id: "root_ws_test".into(),
            goal_id: Some("root_goal_test".into()),
            message: Some("endpoint implemented".into()),
            work_revision: 5,
            git_head: Some("aaaaaaaaaaaaaaaa".into()),
            git_branch: Some("main".into()),
            git_dirty: false,
            git_dirty_fingerprint: Some("cleanfp".into()),
            rootfile_digest: None,
            root_lock_digest: None,
            profile_reference: None,
            environment_status: ENV_MISSING.into(),
            continuation_summary: String::new(),
            snapshot: "{}".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            provenance_id: Some("root_prov_test".into()),
            agent_env_ref: None,
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

    fn git_state() -> crate::git::GitState {
        crate::git::GitState {
            head: Some("aaaaaaaaaaaaaaaa".into()),
            branch: Some("main".into()),
            dirty: false,
            dirty_fingerprint: Some("cleanfp".into()),
        }
    }

    fn environment_state() -> EnvironmentState {
        EnvironmentState {
            rootfile_digest: None,
            root_lock_digest: None,
            profile_reference: None,
            status: ENV_MISSING.into(),
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

    fn snapshot_with(
        decisions: Vec<DecisionRecord>,
        findings: Vec<FindingRecord>,
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
            artifacts: Vec::new(),
            work_revision: 5,
            agent_env: None,
            agent_env_sha256: None,
        }
    }

    fn resume_report(decisions: Vec<DecisionRecord>, findings: Vec<FindingRecord>) -> ResumeReport {
        let snapshot = snapshot_with(decisions, findings);
        build_report(
            &workspace(),
            &checkpoint_record(),
            &snapshot,
            &git_state(),
            &environment_state(),
            Path::new("/repo"),
            &current_state(),
        )
    }

    #[test]
    fn handoff_requires_a_checkpoint() {
        let fixture = Fixture::new("nocheckpoint");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        let error = handoff(&fixture.repo, None).unwrap_err().to_string();
        assert!(error.contains("root checkpoint create"), "{error}");
    }

    #[test]
    fn from_is_agent_for_agent_checkpoint_and_none_for_human() {
        let fixture = Fixture::new("from");
        let _guard = RootDirGuard::set(&fixture.root_dir);

        let mut store = fixture.store();
        store.set_goal("Implement invitations").unwrap();
        drop(store);

        crate::create(&fixture.repo, Some("human checkpoint")).unwrap();
        let human = handoff(&fixture.repo, None).unwrap();
        assert_eq!(human.from, None);

        let context = ProvenanceContext {
            source_type: root_work::model::SOURCE_AGENT,
            agent: Some("codex"),
            harness: None,
            session_id: Some("root_sess_test"),
            evidence_ref: None,
        };
        crate::create_with_provenance(&fixture.repo, Some("agent checkpoint"), context).unwrap();

        let agent = handoff(&fixture.repo, Some("CLAUDE")).unwrap();
        assert_eq!(agent.from.as_deref(), Some("codex"));
        assert_eq!(agent.to.as_deref(), Some("claude"));
        assert!(agent
            .instructions
            .as_deref()
            .unwrap()
            .to_lowercase()
            .contains("checkpoint"));
    }

    #[test]
    fn handoff_to_opencode_uses_the_shared_opencode_instructions() {
        let fixture = Fixture::new("opencode");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        crate::create(&fixture.repo, Some("opencode checkpoint")).unwrap();

        let report = handoff(&fixture.repo, Some("OPENCODE")).unwrap();
        assert_eq!(report.to.as_deref(), Some("opencode"));
        let instructions = report.instructions.as_deref().expect("instructions");
        assert!(
            instructions.contains("Root continuity instructions (OpenCode)"),
            "{instructions}"
        );
        assert!(instructions.to_lowercase().contains("checkpoint"));
    }

    #[test]
    fn handoff_codex_and_claude_instructions_are_byte_identical() {
        let fixture = Fixture::new("unchanged");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        crate::create(&fixture.repo, Some("checkpoint")).unwrap();

        for id in ["codex", "claude"] {
            let expected = root_adapters::root_instructions(id).unwrap();
            let report = handoff(&fixture.repo, Some(id)).unwrap();
            assert_eq!(report.instructions.as_deref(), Some(expected.as_str()));
        }
    }

    #[test]
    fn handoff_unknown_target_fails_closed_listing_canonical_targets() {
        let fixture = Fixture::new("unknown_target");
        let _guard = RootDirGuard::set(&fixture.root_dir);
        crate::create(&fixture.repo, Some("checkpoint")).unwrap();

        let error = handoff(&fixture.repo, Some("gemini"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unsupported handoff target"), "{error}");
        assert!(error.contains("codex, opencode, claude"), "{error}");
    }

    #[test]
    fn superseded_decisions_and_findings_are_excluded() {
        let report = resume_report(
            vec![
                decision("root_dec_1", STATUS_ACTIVE, "Tokens expire after 24h"),
                decision("root_dec_2", STATUS_SUPERSEDED, "Tokens never expire"),
            ],
            vec![
                finding("root_find_1", STATUS_ACTIVE, "Consumption fails"),
                finding("root_find_2", STATUS_SUPERSEDED, "Old behavior"),
            ],
        );
        let handoff = project(report, "root_handoff_test".into(), None, None, None);
        assert_eq!(handoff.decisions.len(), 1);
        assert_eq!(handoff.decisions[0].id, "root_dec_1");
        assert_eq!(handoff.findings.len(), 1);
        assert_eq!(handoff.findings[0].id, "root_find_1");
        assert!(handoff.suggested_continuation[0].starts_with("Suggestion (not verified):"));
    }

    #[test]
    fn projection_is_deterministic_for_fixed_id() {
        let first = project(
            resume_report(Vec::new(), Vec::new()),
            "root_handoff_test".into(),
            Some("codex".into()),
            Some("claude".into()),
            Some("instructions".into()),
        );
        let second = project(
            resume_report(Vec::new(), Vec::new()),
            "root_handoff_test".into(),
            Some("codex".into()),
            Some("claude".into()),
            Some("instructions".into()),
        );
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }

    #[test]
    fn instructions_present_only_when_target_given() {
        let without = project(
            resume_report(Vec::new(), Vec::new()),
            "root_handoff_test".into(),
            None,
            None,
            None,
        );
        assert!(without.instructions.is_none());

        let with = project(
            resume_report(Vec::new(), Vec::new()),
            "root_handoff_test".into(),
            None,
            Some("claude".into()),
            Some(root_adapters::root_instructions("claude").unwrap()),
        );
        assert!(with.instructions.as_deref().unwrap().contains("resume"));
    }

    #[test]
    fn render_includes_the_expected_sections() {
        let report = project(
            resume_report(
                vec![decision("root_dec_1", STATUS_ACTIVE, "Tokens are hashed")],
                vec![finding("root_find_1", STATUS_ACTIVE, "Consumption fails")],
            ),
            "root_handoff_test".into(),
            Some("codex".into()),
            Some("claude".into()),
            None,
        );
        let rendered = render_handoff(&report);
        for section in [
            "Handoff",
            "From",
            "To",
            "Goal",
            "Checkpoint",
            "State",
            "Important decisions",
            "Latest finding",
            "Environment",
            "Drift",
        ] {
            assert!(rendered.contains(section), "missing {section}:\n{rendered}");
        }
        assert!(rendered.contains("Tokens are hashed"));
        assert!(rendered.contains("Consumption fails"));
        assert!(rendered.contains("Claude Code"));
    }
}
