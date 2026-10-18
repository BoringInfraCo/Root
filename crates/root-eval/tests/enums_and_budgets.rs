//! Enum member round-trips and armable-budget semantics.

mod common;

use root_eval::{
    AcceptanceKind, ArtifactPropertyName, ArtifactSource, BreachPolicy, BudgetCap,
    CommandStepMatch, ErrorKind, EvidenceChannel, FakeToolProfile, FixtureKind, HashDomain,
    Ordinal, PatternMatch, PromptKind, SkillRelevance, TaskKind, TaskRecord,
    TerminationClassification, TerminationSubtype, ToolPermission,
};

fn round_trip<T>(cases: &[(&str, T)])
where
    T: serde::de::DeserializeOwned + serde::Serialize + PartialEq + std::fmt::Debug,
{
    for (text, expected) in cases {
        let json = format!("\"{text}\"");
        let parsed: T = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("failed to parse {text:?}: {error}"));
        assert_eq!(&parsed, expected, "parsed variant mismatch for {text:?}");
        let serialized = serde_json::to_string(&parsed).expect("serialize");
        assert_eq!(serialized, json, "serialized form mismatch for {text:?}");
    }
}

#[test]
fn every_enum_member_round_trips() {
    round_trip(&[
        ("direct", SkillRelevance::Direct),
        ("routing-test", SkillRelevance::RoutingTest),
        ("control", SkillRelevance::Control),
    ]);
    round_trip(&[
        ("positive", TaskKind::Positive),
        ("negative-control", TaskKind::NegativeControl),
        ("calibration", TaskKind::Calibration),
    ]);
    round_trip(&[("inline", PromptKind::Inline), ("file", PromptKind::File)]);
    round_trip(&[
        ("generated", FixtureKind::Generated),
        ("repo_commit", FixtureKind::RepoCommit),
    ]);
    round_trip(&[
        ("command_sequence", AcceptanceKind::CommandSequence),
        ("prohibited_absent", AcceptanceKind::ProhibitedAbsent),
        ("artifact_property", AcceptanceKind::ArtifactProperty),
        ("response_fact", AcceptanceKind::ResponseFact),
        ("test_pass", AcceptanceKind::TestPass),
        ("diff_allowlist", AcceptanceKind::DiffAllowlist),
    ]);
    round_trip(&[
        ("fake-tool-audit", EvidenceChannel::FakeToolAudit),
        ("harness-events", EvidenceChannel::HarnessEvents),
        ("final-message", EvidenceChannel::FinalMessage),
        ("verifier-run", EvidenceChannel::VerifierRun),
        ("tree", EvidenceChannel::Tree),
    ]);
    round_trip(&[
        ("exact", CommandStepMatch::Exact),
        ("pattern", CommandStepMatch::Pattern),
    ]);
    round_trip(&[("any", Ordinal::Any), ("first", Ordinal::First)]);
    round_trip(&[
        ("substring", PatternMatch::Substring),
        ("regex", PatternMatch::Regex),
    ]);
    round_trip(&[
        ("tree", ArtifactSource::Tree),
        ("fake-tool-audit", ArtifactSource::FakeToolAudit),
    ]);
    round_trip(&[
        ("exists", ArtifactPropertyName::Exists),
        ("absent", ArtifactPropertyName::Absent),
        ("sha256", ArtifactPropertyName::Sha256),
        ("contains", ArtifactPropertyName::Contains),
        ("matches_regex", ArtifactPropertyName::MatchesRegex),
    ]);
    round_trip(&[
        ("root.healthy.v1", FakeToolProfile::RootHealthyV1),
        ("root.unhealthy.v1", FakeToolProfile::RootUnhealthyV1),
        ("absent", FakeToolProfile::Absent),
        (
            "installer.record-deny.v1",
            FakeToolProfile::InstallerRecordDenyV1,
        ),
        (
            "installer.record-pass.v1",
            FakeToolProfile::InstallerRecordPassV1,
        ),
    ]);
    round_trip(&[("terminate-and-classify", BreachPolicy::TerminateAndClassify)]);
    round_trip(&[("task-record.v1", HashDomain::TaskRecordV1)]);
    round_trip(&[
        ("success", TerminationSubtype::Success),
        ("denied_permission", TerminationSubtype::DeniedPermission),
        ("max_turns", TerminationSubtype::MaxTurns),
        ("max_tokens", TerminationSubtype::MaxTokens),
        ("cost_cap", TerminationSubtype::CostCap),
        ("timeout", TerminationSubtype::Timeout),
        ("cancel", TerminationSubtype::Cancel),
        ("provider_error", TerminationSubtype::ProviderError),
        (
            "infrastructure_error",
            TerminationSubtype::InfrastructureError,
        ),
        ("harness_crash", TerminationSubtype::HarnessCrash),
    ]);
    round_trip(&[
        ("task_failure", TerminationClassification::TaskFailure),
        ("invalid_trial", TerminationClassification::InvalidTrial),
    ]);
    round_trip(&[
        ("allow", ToolPermission::Allow),
        ("deny", ToolPermission::Deny),
        ("ask", ToolPermission::Ask),
    ]);
}

#[test]
fn budget_caps_are_capped_when_present() {
    let record = common::parse_base();
    assert_eq!(record.budgets.turns_cap(), BudgetCap::Capped(24));
    assert_eq!(record.budgets.tokens_cap(), BudgetCap::Capped(400_000));
    assert_eq!(record.budgets.cost_cap_cents(), BudgetCap::Capped(150));
}

#[test]
fn absent_budget_caps_are_uncapped_not_zero() {
    let document = common::base_document()
        .replace("turns_max = 24\n", "")
        .replace("tokens_max = 400000\n", "")
        .replace("cost_usd_max = \"1.50\"\n", "");
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("uncapped budgets parse");
    assert_eq!(record.budgets.turns_cap(), BudgetCap::Uncapped);
    assert_eq!(record.budgets.tokens_cap(), BudgetCap::Uncapped);
    assert_eq!(record.budgets.cost_cap_cents(), BudgetCap::Uncapped);
    assert!(!record.budgets.turns_cap().is_capped());
    assert!(record.budgets.turns_cap().value().is_none());
}

#[test]
fn explicit_zero_cap_is_capped_zero() {
    let document = common::base_document().replace("tokens_max = 400000", "tokens_max = 0");
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("zero cap parses");
    assert_eq!(record.budgets.tokens_cap(), BudgetCap::Capped(0));
}

#[test]
fn cost_cap_parses_cents_exactly() {
    let document =
        common::base_document().replace("cost_usd_max = \"1.50\"", "cost_usd_max = \"0.05\"");
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("small cost parses");
    assert_eq!(record.budgets.cost_cap_cents(), BudgetCap::Capped(5));
}

#[test]
fn overflow_cost_cap_is_refused() {
    let document = common::base_document().replace(
        "cost_usd_max = \"1.50\"",
        "cost_usd_max = \"18446744073709551616.00\"",
    );
    let error = common::expect_error(&common::seal(&document));
    assert_eq!(error.kind(), ErrorKind::BudgetRule);
    assert_eq!(error.field(), Some("budgets.cost_usd_max"));
}

#[test]
fn task_kind_defaults_to_positive() {
    let record = common::parse_base();
    assert_eq!(record.task_kind, TaskKind::Positive);
}

#[test]
fn explicit_task_kind_is_preserved() {
    let document = common::base_document().replace(
        "skill_under_test = \"root-safe-install\"",
        "skill_under_test = \"root-safe-install\"\ntask_kind = \"calibration\"",
    );
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("calibration parses");
    assert_eq!(record.task_kind, TaskKind::Calibration);
}

#[test]
fn termination_overrides_are_typed() {
    let record = common::parse_full();
    assert_eq!(
        record
            .termination_overrides
            .get(&TerminationSubtype::Timeout),
        Some(&TerminationClassification::InvalidTrial)
    );
}
