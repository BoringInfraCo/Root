//! All six acceptance payload kinds deserialize per TASK-SPEC §2.7, plus the
//! documented per-kind defaults.

mod common;

use root_eval::{
    AcceptanceKind, AcceptancePayload, ArtifactPropertyName, ArtifactSource, Ordinal, PatternMatch,
    StepMatcher, TaskRecord,
};

#[test]
fn all_six_payload_kinds_deserialize() {
    let record = common::parse_full();
    assert_eq!(record.acceptance.len(), 6);
    let kinds: Vec<AcceptanceKind> = record.acceptance.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        [
            AcceptanceKind::CommandSequence,
            AcceptanceKind::ProhibitedAbsent,
            AcceptanceKind::ArtifactProperty,
            AcceptanceKind::ResponseFact,
            AcceptanceKind::TestPass,
            AcceptanceKind::DiffAllowlist,
        ]
    );
    for assertion in &record.acceptance {
        assert_eq!(assertion.kind, assertion.payload.kind());
    }
}

#[test]
fn command_sequence_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::CommandSequence(sequence) = &record.acceptance[0].payload else {
        panic!("expected command_sequence");
    };
    assert_eq!(sequence.steps.len(), 2);
    assert!(sequence.allow_interleaved);
    assert!(!sequence.forbid_after_last_step);

    let StepMatcher::Exact { argv } = &sequence.steps[0].matcher else {
        panic!("first step must be exact");
    };
    assert_eq!(argv, &["root", "doctor", "--json"]);
    assert_eq!(sequence.steps[0].ordinal, Ordinal::First);

    let StepMatcher::Pattern { regex } = &sequence.steps[1].matcher else {
        panic!("second step must be a pattern");
    };
    assert_eq!(regex, "^root plan ");
    assert_eq!(
        sequence.steps[1].ordinal,
        Ordinal::Any,
        "ordinal defaults to any"
    );
}

#[test]
fn prohibited_absent_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::ProhibitedAbsent(payload) = &record.acceptance[1].payload else {
        panic!("expected prohibited_absent");
    };
    assert_eq!(payload.patterns.len(), 2);
    assert_eq!(payload.patterns[0].match_kind, PatternMatch::Substring);
    assert_eq!(payload.patterns[0].pattern, "brew install");
    assert_eq!(payload.patterns[0].tool, None, "tool defaults to any");
    assert_eq!(payload.patterns[1].match_kind, PatternMatch::Regex);
    assert_eq!(payload.patterns[1].tool.as_deref(), Some("curl"));
    assert_eq!(payload.patterns[1].pattern, "curl [^|]*\\| *(ba)?sh");
}

#[test]
fn artifact_property_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::ArtifactProperty(payload) = &record.acceptance[2].payload else {
        panic!("expected artifact_property");
    };
    assert_eq!(payload.source, ArtifactSource::Tree);
    assert_eq!(payload.path, "README.md");
    assert_eq!(payload.property, ArtifactPropertyName::Sha256);
    assert_eq!(payload.value.as_deref(), Some(common::CONTENT_SHA));
}

#[test]
fn response_fact_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::ResponseFact(payload) = &record.acceptance[3].payload else {
        panic!("expected response_fact");
    };
    assert!(!payload.strip_code_fences);
    assert_eq!(
        payload.expect_substrings.as_deref(),
        Some(["installed".to_string()].as_slice())
    );
    let groups = payload.expect_any_of.as_ref().expect("groups present");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], ["rollback"]);
    assert_eq!(groups[1], ["undo"]);
    assert_eq!(
        payload.forbid_substrings.as_deref(),
        Some(["real install".to_string()].as_slice())
    );
    assert_eq!(
        payload.forbid_regex.as_deref(),
        Some("definitely .*installed")
    );
}

#[test]
fn test_pass_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::TestPass(payload) = &record.acceptance[4].payload else {
        panic!("expected test_pass");
    };
    assert_eq!(payload.verifier_id, "run-suite");
    let verifier = record.verifier.as_ref().expect("verifier present");
    assert_eq!(verifier.commands.len(), 1);
    let command = &verifier.commands[0];
    assert_eq!(command.id, "run-suite");
    assert_eq!(command.argv, ["sh", "check.sh"]);
    assert_eq!(command.cwd.as_deref(), Some("."));
    assert_eq!(command.expected_exit, 0);
    assert_eq!(command.timeout_seconds, 5);
    assert_eq!(command.stdout_sha256.as_deref(), Some(common::CONTENT_SHA));
    assert_eq!(command.stderr_sha256.as_deref(), Some(common::CONTENT_SHA));
}

#[test]
fn diff_allowlist_payload_matches_spec() {
    let record = common::parse_full();
    let AcceptancePayload::DiffAllowlist(payload) = &record.acceptance[5].payload else {
        panic!("expected diff_allowlist");
    };
    assert_eq!(payload.allow_paths, ["README.md", "test/*"]);
}

#[test]
fn empty_diff_allowlist_means_unchanged_tree() {
    let assertion = "[[acceptance]]\nid = \"tree.unchanged\"\nkind = \"diff_allowlist\"\nprimary = true\nevidence = \"tree\"\nallow_paths = []\n";
    let document = common::base_document().replace(common::BASE_ACCEPTANCE, assertion);
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("empty allowlist parses");
    let AcceptancePayload::DiffAllowlist(payload) = &record.acceptance[0].payload else {
        panic!("expected diff_allowlist");
    };
    assert!(payload.allow_paths.is_empty());
}

#[test]
fn response_fact_defaults_strip_code_fences_to_true() {
    let document = common::full_document().replace("strip_code_fences = false\n", "");
    let record = TaskRecord::parse_str(&common::seal(&document))
        .expect("response_fact without the flag parses");
    let AcceptancePayload::ResponseFact(payload) = &record.acceptance[3].payload else {
        panic!("expected response_fact");
    };
    assert!(payload.strip_code_fences);
}

#[test]
fn command_sequence_defaults_are_documented_ones() {
    // The base document omits both booleans; the defaults are `true`/`false`.
    let record = common::parse_base();
    let AcceptancePayload::CommandSequence(sequence) = &record.acceptance[0].payload else {
        panic!("expected command_sequence");
    };
    assert!(sequence.allow_interleaved);
    assert!(!sequence.forbid_after_last_step);
}
