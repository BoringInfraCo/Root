use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::canonical::CanonicalRecord;

// ---------------------------------------------------------------------------
// Closed enums (schema §2)
// ---------------------------------------------------------------------------

/// Experimental role of the task (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillRelevance {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "routing-test")]
    RoutingTest,
    #[serde(rename = "control")]
    Control,
}

/// Interaction with the skill under test (§2.2). Defaults to `positive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TaskKind {
    #[serde(rename = "positive")]
    #[default]
    Positive,
    #[serde(rename = "negative-control")]
    NegativeControl,
    #[serde(rename = "calibration")]
    Calibration,
}

/// Prompt representation (§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PromptKind {
    #[serde(rename = "inline")]
    Inline,
    #[serde(rename = "file")]
    File,
}

/// Fixture representation (§2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FixtureKind {
    #[serde(rename = "generated")]
    Generated,
    #[serde(rename = "repo_commit")]
    RepoCommit,
}

/// The six acceptance payload shapes (§2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AcceptanceKind {
    #[serde(rename = "command_sequence")]
    CommandSequence,
    #[serde(rename = "prohibited_absent")]
    ProhibitedAbsent,
    #[serde(rename = "artifact_property")]
    ArtifactProperty,
    #[serde(rename = "response_fact")]
    ResponseFact,
    #[serde(rename = "test_pass")]
    TestPass,
    #[serde(rename = "diff_allowlist")]
    DiffAllowlist,
}

/// Evidence channel an assertion evaluates (§2.7.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvidenceChannel {
    #[serde(rename = "fake-tool-audit")]
    FakeToolAudit,
    #[serde(rename = "harness-events")]
    HarnessEvents,
    #[serde(rename = "final-message")]
    FinalMessage,
    #[serde(rename = "verifier-run")]
    VerifierRun,
    #[serde(rename = "tree")]
    Tree,
}

/// How a `command_sequence` step matches (§2.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandStepMatch {
    #[serde(rename = "exact")]
    Exact,
    #[serde(rename = "pattern")]
    Pattern,
}

/// Ordinal placement of a `command_sequence` step (§2.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Ordinal {
    #[serde(rename = "any")]
    #[default]
    Any,
    #[serde(rename = "first")]
    First,
}

/// How a `prohibited_absent` pattern matches (§2.7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PatternMatch {
    #[serde(rename = "substring")]
    Substring,
    #[serde(rename = "regex")]
    Regex,
}

/// Target of an `artifact_property` assertion (§2.7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtifactSource {
    #[serde(rename = "tree")]
    Tree,
    #[serde(rename = "fake-tool-audit")]
    FakeToolAudit,
}

/// Property checked by an `artifact_property` assertion (§2.7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtifactPropertyName {
    #[serde(rename = "exists")]
    Exists,
    #[serde(rename = "absent")]
    Absent,
    #[serde(rename = "sha256")]
    Sha256,
    #[serde(rename = "contains")]
    Contains,
    #[serde(rename = "matches_regex")]
    MatchesRegex,
}

/// Registered deterministic fake-tool profiles (§2.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FakeToolProfile {
    #[serde(rename = "root.healthy.v1")]
    RootHealthyV1,
    #[serde(rename = "root.unhealthy.v1")]
    RootUnhealthyV1,
    #[serde(rename = "absent")]
    Absent,
    #[serde(rename = "installer.record-deny.v1")]
    InstallerRecordDenyV1,
    #[serde(rename = "installer.record-pass.v1")]
    InstallerRecordPassV1,
}

/// Budget breach policy (§2.6). v1 has exactly one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BreachPolicy {
    #[serde(rename = "terminate-and-classify")]
    TerminateAndClassify,
}

/// Integrity hash-domain constant (§2.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashDomain {
    #[serde(rename = "task-record.v1")]
    TaskRecordV1,
}

/// Closed termination subtype set (§2.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TerminationSubtype {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "denied_permission")]
    DeniedPermission,
    #[serde(rename = "max_turns")]
    MaxTurns,
    #[serde(rename = "max_tokens")]
    MaxTokens,
    #[serde(rename = "cost_cap")]
    CostCap,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "cancel")]
    Cancel,
    #[serde(rename = "provider_error")]
    ProviderError,
    #[serde(rename = "infrastructure_error")]
    InfrastructureError,
    #[serde(rename = "harness_crash")]
    HarnessCrash,
}

/// Classification an override may assign (§2.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TerminationClassification {
    #[serde(rename = "task_failure")]
    TaskFailure,
    #[serde(rename = "invalid_trial")]
    InvalidTrial,
}

/// Harness permission intent for a tool (§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolPermission {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
    #[serde(rename = "ask")]
    Ask,
}

// ---------------------------------------------------------------------------
// Identity and budgets
// ---------------------------------------------------------------------------

/// Stable identity of a task record: the `(task_id, version)` pair.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskIdentity {
    pub task_id: String,
    pub version: u32,
}

/// Explicit cap semantics for armable budgets.
///
/// `Uncapped` is the deliberate representation of an absent cap: the task spec
/// requires "missing is not zero" and the runner must not impute a zero cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetCap<T> {
    Uncapped,
    Capped(T),
}

impl<T> BudgetCap<T> {
    pub fn is_capped(&self) -> bool {
        matches!(self, Self::Capped(_))
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Uncapped => None,
            Self::Capped(value) => Some(value),
        }
    }
}

/// Task-level budgets (§2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub wall_time_seconds_max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_max: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd_max: Option<String>,
    pub breach_policy: BreachPolicy,
}

impl Budgets {
    pub fn turns_cap(&self) -> BudgetCap<u32> {
        self.turns_max
            .map_or(BudgetCap::Uncapped, BudgetCap::Capped)
    }

    pub fn tokens_cap(&self) -> BudgetCap<u64> {
        self.tokens_max
            .map_or(BudgetCap::Uncapped, BudgetCap::Capped)
    }

    /// Cost cap in integer cents, parsed from the fixed two-decimal string.
    ///
    /// Parsing succeeded at task-validation time, so a present `cost_usd_max`
    /// always yields `Some`.
    pub fn cost_cap_cents(&self) -> BudgetCap<u64> {
        self.cost_usd_max
            .as_deref()
            .and_then(cost_usd_cents)
            .map_or(BudgetCap::Uncapped, BudgetCap::Capped)
    }
}

/// Parse a canonical `^[0-9]+\.[0-9]{2}$` cost string into cents.
pub fn cost_usd_cents(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.')?;
    if whole.is_empty() || fraction.len() != 2 {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let fraction: u64 = fraction.parse().ok()?;
    whole.checked_mul(100)?.checked_add(fraction)
}

// ---------------------------------------------------------------------------
// Prompt, fixture, environment
// ---------------------------------------------------------------------------

/// The exact user turn handed to the harness (§2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    Inline { inline: String },
    File { file: String, file_sha256: String },
}

impl Prompt {
    pub fn kind(&self) -> PromptKind {
        match self {
            Self::Inline { .. } => PromptKind::Inline,
            Self::File { .. } => PromptKind::File,
        }
    }
}

/// One declared file in a `generated` fixture (§2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    pub path: String,
    pub content: String,
    pub content_sha256: String,
}

/// A `generated` fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFixture {
    pub entry_point: String,
    pub max_tree_bytes: u64,
    pub files: Vec<FixtureFile>,
}

/// A `repo_commit` fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCommitFixture {
    pub entry_point: String,
    pub max_tree_bytes: u64,
    pub repo: String,
    pub commit: String,
    pub tree_sha256: String,
}

/// Fixture declaration (§2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fixture {
    Generated(GeneratedFixture),
    RepoCommit(RepoCommitFixture),
}

impl Fixture {
    pub fn kind(&self) -> FixtureKind {
        match self {
            Self::Generated(_) => FixtureKind::Generated,
            Self::RepoCommit(_) => FixtureKind::RepoCommit,
        }
    }

    pub fn entry_point(&self) -> &str {
        match self {
            Self::Generated(fixture) => &fixture.entry_point,
            Self::RepoCommit(fixture) => &fixture.entry_point,
        }
    }

    pub fn max_tree_bytes(&self) -> u64 {
        match self {
            Self::Generated(fixture) => fixture.max_tree_bytes,
            Self::RepoCommit(fixture) => fixture.max_tree_bytes,
        }
    }
}

/// Deterministic fake-tool declaration (§2.5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeTool {
    pub provides: String,
    pub profile: FakeToolProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_sha256: Option<String>,
}

/// Everything the harness process may see and do (§2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_permissions: BTreeMap<String, ToolPermission>,
    pub env_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fake_tools: Vec<FakeTool>,
}

// ---------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------

/// One step of a `command_sequence` assertion (§2.7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStep {
    pub matcher: StepMatcher,
    pub ordinal: Ordinal,
}

/// Exact-argv or pattern matcher for a command step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepMatcher {
    Exact { argv: Vec<String> },
    Pattern { regex: String },
}

/// `command_sequence` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSequence {
    pub steps: Vec<CommandStep>,
    pub allow_interleaved: bool,
    pub forbid_after_last_step: bool,
}

/// One prohibited pattern (§2.7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProhibitedPattern {
    pub pattern: String,
    pub match_kind: PatternMatch,
    pub tool: Option<String>,
}

/// `prohibited_absent` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProhibitedAbsent {
    pub patterns: Vec<ProhibitedPattern>,
}

/// `artifact_property` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactProperty {
    pub source: ArtifactSource,
    pub path: String,
    pub property: ArtifactPropertyName,
    pub value: Option<String>,
}

/// `response_fact` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseFact {
    pub strip_code_fences: bool,
    pub expect_substrings: Option<Vec<String>>,
    pub expect_any_of: Option<Vec<Vec<String>>>,
    pub forbid_substrings: Option<Vec<String>>,
    pub forbid_regex: Option<String>,
}

/// `test_pass` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPass {
    pub verifier_id: String,
}

/// `diff_allowlist` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffAllowlist {
    pub allow_paths: Vec<String>,
}

/// Typed payload of an acceptance assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptancePayload {
    CommandSequence(CommandSequence),
    ProhibitedAbsent(ProhibitedAbsent),
    ArtifactProperty(ArtifactProperty),
    ResponseFact(ResponseFact),
    TestPass(TestPass),
    DiffAllowlist(DiffAllowlist),
}

impl AcceptancePayload {
    pub fn kind(&self) -> AcceptanceKind {
        match self {
            Self::CommandSequence(_) => AcceptanceKind::CommandSequence,
            Self::ProhibitedAbsent(_) => AcceptanceKind::ProhibitedAbsent,
            Self::ArtifactProperty(_) => AcceptanceKind::ArtifactProperty,
            Self::ResponseFact(_) => AcceptanceKind::ResponseFact,
            Self::TestPass(_) => AcceptanceKind::TestPass,
            Self::DiffAllowlist(_) => AcceptanceKind::DiffAllowlist,
        }
    }
}

/// One ordered acceptance assertion (§2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acceptance {
    pub id: String,
    pub kind: AcceptanceKind,
    pub primary: bool,
    pub evidence: EvidenceChannel,
    pub payload: AcceptancePayload,
}

// ---------------------------------------------------------------------------
// Verifier and integrity
// ---------------------------------------------------------------------------

/// One external verifier command (§2.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierCommand {
    pub id: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub expected_exit: i32,
    pub timeout_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_sha256: Option<String>,
}

/// Optional external verifier commands (§2.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verifier {
    #[serde(default)]
    pub commands: Vec<VerifierCommand>,
}

/// Integrity block (§2.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub content_sha256: String,
    pub hash_domain: HashDomain,
}

/// A fully parsed and validated benchmark task record (schema v1).
///
/// Records are constructed only through [`crate::TaskRecord::parse_str`] or
/// [`crate::TaskRecord::parse_bytes`]. The canonical bytes and digest are
/// computed from the parsed document minus `[integrity]` at parse time and
/// are available through [`crate::TaskRecord::canonical_bytes`] and
/// [`crate::TaskRecord::canonical_hash`].
///
/// The private canonical fields make struct-literal construction outside this
/// crate impossible, preserving the parse-time integrity invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub schema_version: u32,
    pub task_id: String,
    pub version: u32,
    pub title: String,
    pub skill_relevance: SkillRelevance,
    pub skill_under_test: String,
    pub task_kind: TaskKind,
    pub prompt: Prompt,
    pub fixture: Fixture,
    pub environment: Environment,
    pub budgets: Budgets,
    pub acceptance: Vec<Acceptance>,
    pub verifier: Option<Verifier>,
    pub termination_overrides: BTreeMap<TerminationSubtype, TerminationClassification>,
    pub integrity: Integrity,
    pub(crate) canonical: CanonicalRecord,
}
