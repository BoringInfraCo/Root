//! Positive coverage for the schema fields the shared base/full records do not
//! exercise: the `file` prompt variant, the `repo_commit` fixture variant, the
//! fake-tool manifest, the permission map, and termination overrides.
//!
//! Together with `valid-spine.toml` (30-field mandatory spine) and
//! `common::full_document` (all six acceptance payloads), these records
//! positively parse every one of the 65 distinct field names counted by
//! TASK-SPEC §2.11 (nested array-element fields counted once; array container
//! names excluded).

mod common;

use root_eval::{
    sha256_digest, AcceptancePayload, BudgetCap, EvidenceChannel, FakeToolProfile, PatternMatch,
    Prompt, PromptKind, SkillRelevance, TaskKind, TaskRecord, TerminationClassification,
    TerminationSubtype, ToolPermission,
};

fn coverage_document() -> String {
    let digest = sha256_digest(b"coverage script");
    format!(
        r#"schema_version = 1
task_id = "coverage-task-001"
version = 3
title = "Coverage record"
skill_relevance = "routing-test"
skill_under_test = "root-safe-install"
task_kind = "negative-control"

[prompt]
kind = "file"
file = "prompts/task.md"
file_sha256 = "{digest}"

[fixture]
kind = "repo_commit"
entry_point = "repo"
max_tree_bytes = 4096
repo = "https://example.invalid/repo.git"
commit = "0123456789abcdef0123456789abcdef01234567"
tree_sha256 = "{digest}"

[environment]
allowed_tools = ["bash", "read", "skill"]
tool_permissions = {{ webfetch = "deny", bash = "allow", edit = "ask" }}
env_allowlist = ["PATH", "HOME", "TMPDIR", "XDG_*"]

[[environment.fake_tools]]
provides = "root"
profile = "root.unhealthy.v1"
script_sha256 = "{digest}"

[[environment.fake_tools]]
provides = "brew"
profile = "installer.record-deny.v1"
script_sha256 = "{digest}"

[[environment.fake_tools]]
provides = "curl"
profile = "absent"

[budgets]
wall_time_seconds_max = 600
breach_policy = "terminate-and-classify"

[termination_overrides]
timeout = "invalid_trial"
cancel = "task_failure"

[[acceptance]]
id = "no.banned"
kind = "prohibited_absent"
primary = true
evidence = "fake-tool-audit"
patterns = [
  {{ match = "substring", pattern = "brew install" }},
]

[integrity]
content_sha256 = "sha256:PENDING"
hash_domain = "task-record.v1"
"#
    )
}

#[test]
fn file_prompt_and_repo_commit_variants_parse_positively() {
    let record = TaskRecord::parse_str(&common::seal(&coverage_document()))
        .expect("coverage record should parse");

    assert_eq!(record.task_id, "coverage-task-001");
    assert_eq!(record.version, 3);
    assert_eq!(record.skill_relevance, SkillRelevance::RoutingTest);
    assert_eq!(record.task_kind, TaskKind::NegativeControl);

    // prompt.file variant (three fields)
    assert_eq!(record.prompt.kind(), PromptKind::File);
    let Prompt::File { file, file_sha256 } = &record.prompt else {
        panic!("expected the file prompt variant");
    };
    assert_eq!(file, "prompts/task.md");
    assert_eq!(file_sha256, &sha256_digest(b"coverage script"));

    // fixture.repo_commit variant (five fields incl. tree_sha256)
    let root_eval::Fixture::RepoCommit(fixture) = &record.fixture else {
        panic!("expected the repo_commit fixture variant");
    };
    assert_eq!(fixture.entry_point, "repo");
    assert_eq!(fixture.max_tree_bytes, 4096);
    assert_eq!(fixture.repo, "https://example.invalid/repo.git");
    assert_eq!(fixture.commit, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(fixture.tree_sha256, sha256_digest(b"coverage script"));
}

#[test]
fn fake_tool_manifest_and_permission_map_parse_positively() {
    let record = TaskRecord::parse_str(&common::seal(&coverage_document())).expect("parse");

    assert_eq!(record.environment.tool_permissions.len(), 3);
    assert_eq!(
        record.environment.tool_permissions.get("webfetch"),
        Some(&ToolPermission::Deny)
    );
    assert_eq!(
        record.environment.tool_permissions.get("bash"),
        Some(&ToolPermission::Allow)
    );
    assert_eq!(
        record.environment.tool_permissions.get("edit"),
        Some(&ToolPermission::Ask)
    );
    assert_eq!(record.environment.env_allowlist.last().unwrap(), "XDG_*");

    let tools = &record.environment.fake_tools;
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0].profile, FakeToolProfile::RootUnhealthyV1);
    assert!(tools[0].script_sha256.is_some());
    assert_eq!(tools[1].profile, FakeToolProfile::InstallerRecordDenyV1);
    assert_eq!(tools[2].profile, FakeToolProfile::Absent);
    assert!(tools[2].script_sha256.is_none());
}

#[test]
fn termination_overrides_and_armable_budgets_parse_positively() {
    let record = TaskRecord::parse_str(&common::seal(&coverage_document())).expect("parse");

    // Only one budget is capped; the others are explicitly uncapped.
    assert_eq!(record.budgets.turns_cap(), BudgetCap::Uncapped);
    assert_eq!(record.budgets.tokens_cap(), BudgetCap::Uncapped);
    assert_eq!(record.budgets.cost_cap_cents(), BudgetCap::Uncapped);

    assert_eq!(
        record
            .termination_overrides
            .get(&TerminationSubtype::Timeout),
        Some(&TerminationClassification::InvalidTrial)
    );
    assert_eq!(
        record
            .termination_overrides
            .get(&TerminationSubtype::Cancel),
        Some(&TerminationClassification::TaskFailure)
    );

    let payload = &record.acceptance[0].payload;
    let AcceptancePayload::ProhibitedAbsent(payload) = payload else {
        panic!("expected prohibited_absent");
    };
    assert_eq!(payload.patterns[0].match_kind, PatternMatch::Substring);
    assert_eq!(
        record.acceptance[0].evidence,
        EvidenceChannel::FakeToolAudit
    );
}
