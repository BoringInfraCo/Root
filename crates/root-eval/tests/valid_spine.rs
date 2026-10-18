//! `valid-spine.toml`: the minimal positive record and its pinned hash.
//!
//! The fixture exercises the 30-field mandatory spine from TASK-SPEC §2.11 and
//! is fully sealed (real content digest, real integrity digest).

mod common;

use root_eval::{
    AcceptanceKind, AcceptancePayload, PromptKind, StepMatcher, TaskDraft, TaskRecord,
};

/// Pinned canonical hash of `tests/fixtures/tasks/valid-spine.toml`.
///
/// If this test fails, either the fixture changed (which requires a new
/// `version`) or canonicalization drifted. Both are seal-breaking events.
const SPINE_HASH: &str = "sha256:c8794c59e63a56e82e7fa45f29cd03cda05314a095fedcbe91e2526f3cd52b8e";

const SPINE_FILE: &str = "valid-spine.toml";

#[test]
fn spine_parses_and_matches_its_pinned_hash() {
    let record = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE))
        .expect("valid-spine.toml should parse as a sealed record");
    assert_eq!(record.canonical_hash(), SPINE_HASH);
    assert_eq!(record.content_sha256(), SPINE_HASH);
    assert_eq!(record.integrity.content_sha256, SPINE_HASH);
}

#[test]
fn spine_exercises_the_thirty_field_mandatory_spine() {
    let record = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE)).expect("parse");

    // Identity (6)
    assert_eq!(record.schema_version, 1);
    assert_eq!(record.task_id, "valid-spine-001");
    assert_eq!(record.version, 1);
    assert!(!record.title.is_empty());
    assert_eq!(record.skill_relevance, root_eval::SkillRelevance::Direct);
    assert_eq!(record.skill_under_test, "root-safe-install");

    // Prompt (2)
    let PromptKind::Inline = record.prompt.kind() else {
        panic!("spine prompt must be inline");
    };
    let root_eval::Prompt::Inline { inline } = &record.prompt else {
        panic!("spine prompt must be inline");
    };
    assert!(!inline.is_empty());

    // Fixture (3 + 3 file fields)
    assert_eq!(record.fixture.kind(), root_eval::FixtureKind::Generated);
    let root_eval::Fixture::Generated(generated) = &record.fixture else {
        panic!("spine fixture must be generated");
    };
    assert_eq!(generated.entry_point, ".");
    assert_eq!(generated.max_tree_bytes, 1000);
    assert_eq!(generated.files.len(), 1);
    let file = &generated.files[0];
    assert_eq!(file.path, "README.md");
    assert_eq!(file.content, common::CONTENT);
    assert_eq!(file.content_sha256, common::CONTENT_SHA);

    // Environment (2 of 4 fields mandatory)
    assert_eq!(record.environment.allowed_tools, ["bash", "read"]);
    assert_eq!(record.environment.env_allowlist, ["PATH", "HOME", "TMPDIR"]);

    // Budgets (5)
    assert_eq!(record.budgets.wall_time_seconds_max, 60);
    assert_eq!(record.budgets.turns_max, Some(8));
    assert_eq!(record.budgets.tokens_max, Some(1000));
    assert_eq!(record.budgets.cost_usd_max.as_deref(), Some("0.50"));
    assert_eq!(
        record.budgets.breach_policy,
        root_eval::BreachPolicy::TerminateAndClassify
    );

    // Acceptance (id, kind, primary, evidence, steps, steps[].argv, steps[].match)
    assert_eq!(record.acceptance.len(), 1);
    let assertion = &record.acceptance[0];
    assert_eq!(assertion.id, "seq.root");
    assert_eq!(assertion.kind, AcceptanceKind::CommandSequence);
    assert!(assertion.primary);
    assert_eq!(
        assertion.evidence,
        root_eval::EvidenceChannel::FakeToolAudit
    );
    let AcceptancePayload::CommandSequence(sequence) = &assertion.payload else {
        panic!("spine assertion must be a command sequence");
    };
    assert_eq!(sequence.steps.len(), 1);
    let StepMatcher::Exact { argv } = &sequence.steps[0].matcher else {
        panic!("spine step must match exact argv");
    };
    assert_eq!(argv, &["root", "doctor", "--json"]);

    // Integrity (2)
    assert_eq!(record.integrity.content_sha256, SPINE_HASH);
    assert_eq!(
        record.integrity.hash_domain,
        root_eval::HashDomain::TaskRecordV1
    );

    // Defaults that the spine deliberately leaves implicit.
    assert_eq!(record.task_kind, root_eval::TaskKind::Positive);
    assert!(sequence.allow_interleaved);
    assert!(!sequence.forbid_after_last_step);
    assert_eq!(sequence.steps[0].ordinal, root_eval::Ordinal::First);
    assert!(record.verifier.is_none());
    assert!(record.termination_overrides.is_empty());
}

#[test]
fn spine_canonical_bytes_are_stable_and_self_describing() {
    let record = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE)).expect("parse");
    let bytes = record.canonical_bytes();
    let text = std::str::from_utf8(bytes).expect("canonical bytes are UTF-8");
    assert!(text.ends_with('\n'));
    assert!(!text.contains("[integrity]"));
    assert!(!text.contains("sha256:PENDING"));

    // The same input parses to the same canonical bytes twice.
    let second = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE)).expect("parse");
    assert_eq!(second.canonical_bytes(), bytes);
    assert_eq!(second.canonical_hash(), record.canonical_hash());
}

#[test]
fn spine_canonical_bytes_reparse_to_the_same_hash() {
    let record = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE)).expect("parse");
    let document = format!(
        "{}\n[integrity]\ncontent_sha256 = \"{}\"\nhash_domain = \"task-record.v1\"\n",
        std::str::from_utf8(record.canonical_bytes()).expect("utf-8"),
        record.canonical_hash()
    );
    let reparsed = TaskRecord::parse_str(&document).expect("canonical bytes re-parse");
    assert_eq!(reparsed.canonical_bytes(), record.canonical_bytes());
    assert_eq!(reparsed.canonical_hash(), record.canonical_hash());
}

#[test]
fn spine_is_reported_as_sealed_by_the_draft_layer() {
    let draft = TaskDraft::parse_str(&common::fixture_text(SPINE_FILE)).expect("draft");
    assert!(draft.is_sealed());
    assert!(draft.pending_digest_fields().is_empty());
    assert_eq!(draft.canonical_hash(), SPINE_HASH);
    assert_eq!(draft.schema_version(), 1);
}

#[test]
fn spine_identity_is_task_id_and_version() {
    let record = TaskRecord::parse_str(&common::fixture_text(SPINE_FILE)).expect("parse");
    let identity = record.identity();
    assert_eq!(identity.task_id, "valid-spine-001");
    assert_eq!(identity.version, 1);
}
