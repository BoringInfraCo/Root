use std::collections::BTreeMap;

use serde::Deserialize;

use crate::schema::{
    AcceptanceKind, ArtifactPropertyName, ArtifactSource, Budgets, CommandStepMatch, Environment,
    EvidenceChannel, FixtureFile, FixtureKind, Integrity, Ordinal, PatternMatch, PromptKind,
    SkillRelevance, TaskKind, TerminationClassification, TerminationSubtype, Verifier,
};

/// Raw, exact-key-set-checked shape for a task record (§2).
///
/// This layer exists so the `deny_unknown_fields` contract is enforced twice:
/// once by the explicit per-level key sets in `parse.rs`, and once by serde.
/// Presence rules that vary by discriminated variant are validated in
/// `parse.rs`; this struct is allowed to hold `Option` fields for them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTaskRecord {
    pub schema_version: u32,
    pub task_id: String,
    pub version: u32,
    pub title: String,
    pub skill_relevance: SkillRelevance,
    pub skill_under_test: String,
    #[serde(default)]
    pub task_kind: TaskKind,
    pub prompt: RawPrompt,
    pub fixture: RawFixture,
    pub environment: Environment,
    pub budgets: Budgets,
    pub acceptance: Vec<RawAcceptance>,
    #[serde(default)]
    pub verifier: Option<Verifier>,
    #[serde(default)]
    pub termination_overrides: BTreeMap<TerminationSubtype, TerminationClassification>,
    pub integrity: Integrity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPrompt {
    pub kind: PromptKind,
    pub inline: Option<String>,
    pub file: Option<String>,
    pub file_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFixture {
    pub kind: FixtureKind,
    pub entry_point: String,
    pub max_tree_bytes: u64,
    pub tree_sha256: Option<String>,
    pub files: Option<Vec<FixtureFile>>,
    pub repo: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAcceptance {
    pub id: String,
    pub kind: AcceptanceKind,
    pub primary: bool,
    pub evidence: EvidenceChannel,

    // command_sequence
    pub steps: Option<Vec<RawCommandStep>>,
    pub allow_interleaved: Option<bool>,
    pub forbid_after_last_step: Option<bool>,

    // prohibited_absent
    pub patterns: Option<Vec<RawProhibitedPattern>>,

    // artifact_property
    pub source: Option<ArtifactSource>,
    pub path: Option<String>,
    pub property: Option<ArtifactPropertyName>,
    pub value: Option<String>,

    // response_fact
    pub strip_code_fences: Option<bool>,
    pub expect_substrings: Option<Vec<String>>,
    pub expect_any_of: Option<Vec<Vec<String>>>,
    pub forbid_substrings: Option<Vec<String>>,
    pub forbid_regex: Option<String>,

    // test_pass
    pub verifier_id: Option<String>,

    // diff_allowlist
    pub allow_paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCommandStep {
    #[serde(rename = "match")]
    pub match_kind: CommandStepMatch,
    pub argv: Option<Vec<String>>,
    pub regex: Option<String>,
    pub ordinal: Option<Ordinal>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProhibitedPattern {
    pub pattern: String,
    #[serde(rename = "match")]
    pub match_kind: PatternMatch,
    pub tool: Option<String>,
}
