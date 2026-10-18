//! The three worked examples in TASK-SPEC.md §3, extracted verbatim.
//!
//! Each is a PROPOSAL draft fixture: structurally valid, unsealed. It must
//! pass the draft layer, then be refused by the evaluation parser with the
//! distinct sentinel kind — never confused with a malformed digest.

mod common;

use root_eval::{ErrorKind, TaskDraft, TaskRecord};

const EXAMPLE_1: &str = "example-1-install-ripgrep-001.task.toml";
const EXAMPLE_2: &str = "example-2-root-missing-ask-first-004.task.toml";
const EXAMPLE_3: &str = "example-3-injected-curlsh-doc-010.task.toml";

fn assert_example_is_unsealed_draft(name: &str, expected_task_id: &str) {
    let text = common::fixture_text(name);

    // Layer 1: the document is valid TOML and structurally valid per schema.
    let draft = TaskDraft::parse_str(&text)
        .unwrap_or_else(|error| panic!("{name} should be a structurally valid draft: {error}"));
    assert!(
        !draft.is_sealed(),
        "{name} uses sha256:PENDING and must not be sealed"
    );
    assert!(
        draft
            .pending_digest_fields()
            .iter()
            .all(|field| !field.is_empty()),
        "{name} should report pending digest field paths"
    );
    assert!(
        draft
            .pending_digest_fields()
            .contains(&"fixture.files[0].content_sha256".to_string()),
        "{name} should report the first fixture content digest as pending"
    );

    // Layer 2: evaluation parsing refuses the draft with the sentinel kind.
    let error = TaskRecord::parse_str(&text)
        .expect_err("draft example must not be accepted for evaluation");
    assert_eq!(error.kind(), ErrorKind::SentinelRefusal, "{name}: {error}");
    assert_eq!(
        error.field(),
        Some("fixture.files[0].content_sha256"),
        "{name}: {error}"
    );
    assert!(error.is_draft_refusal());

    // The refusal is semantic, not syntactic: the task id parsed fine.
    let document = draft.document();
    assert_eq!(
        document.get("task_id").and_then(|value| value.as_str()),
        Some(expected_task_id)
    );
}

#[test]
fn example_1_install_ripgrep_is_a_refused_draft() {
    assert_example_is_unsealed_draft(EXAMPLE_1, "install-ripgrep-001");
}

#[test]
fn example_2_root_missing_ask_first_is_a_refused_draft() {
    assert_example_is_unsealed_draft(EXAMPLE_2, "root-missing-ask-first-004");
}

#[test]
fn example_3_injected_curlsh_doc_is_a_refused_draft() {
    assert_example_is_unsealed_draft(EXAMPLE_3, "injected-curlsh-doc-010");
}

#[test]
fn example_drafts_report_every_pending_digest() {
    // Example 1 has two fixture files, five fake tools, and two verifier
    // hashes; the draft layer must list all of them, not just the first.
    let draft = TaskDraft::parse_str(&common::fixture_text(EXAMPLE_1))
        .expect("example 1 should be a structurally valid draft");
    let pending = draft.pending_digest_fields();
    assert!(pending.contains(&"fixture.files[0].content_sha256".to_string()));
    assert!(pending.contains(&"fixture.files[1].content_sha256".to_string()));
    assert!(pending.contains(&"environment.fake_tools[0].script_sha256".to_string()));
    assert!(pending.contains(&"environment.fake_tools[4].script_sha256".to_string()));
    assert!(pending.contains(&"verifier.commands[0].stdout_sha256".to_string()));
    assert!(pending.contains(&"verifier.commands[0].stderr_sha256".to_string()));
    assert!(
        !pending
            .iter()
            .any(|field| field == "integrity.content_sha256"),
        "the examples omit [integrity] entirely"
    );
}

#[test]
fn example_3_reports_the_artifact_value_sentinel() {
    let draft = TaskDraft::parse_str(&common::fixture_text(EXAMPLE_3))
        .expect("example 3 should be a structurally valid draft");
    assert!(draft
        .pending_digest_fields()
        .contains(&"acceptance[0].value".to_string()));
}
