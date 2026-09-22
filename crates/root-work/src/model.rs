//! Canonical work-state records and CLI-facing report shapes.

use serde::{Deserialize, Serialize};

pub const GOAL_ACTIVE: &str = "active";
pub const GOAL_COMPLETED: &str = "completed";
pub const GOAL_SUPERSEDED: &str = "superseded";

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_SUPERSEDED: &str = "superseded";

pub const SOURCE_HUMAN: &str = "human";
pub const SOURCE_AGENT: &str = "agent";
pub const SOURCE_ROOT: &str = "root";
pub const SOURCE_IMPORT: &str = "import";

pub const ARTIFACT_FILE: &str = "file";
pub const ARTIFACT_COMMIT: &str = "commit";
pub const ARTIFACT_PATCH: &str = "patch";
pub const ARTIFACT_TEST_OUTPUT: &str = "test_output";
pub const ARTIFACT_REPORT: &str = "report";
pub const ARTIFACT_OTHER: &str = "other";

pub const ARTIFACT_KINDS: &[&str] = &[
    ARTIFACT_FILE,
    ARTIFACT_COMMIT,
    ARTIFACT_PATCH,
    ARTIFACT_TEST_OUTPUT,
    ARTIFACT_REPORT,
    ARTIFACT_OTHER,
];

pub const ENV_VERIFIED: &str = "verified";
pub const ENV_OBSERVED: &str = "observed";
pub const ENV_UNKNOWN: &str = "unknown";
pub const ENV_MISSING: &str = "missing";

pub const ENVIRONMENT_STATUSES: &[&str] = &[ENV_VERIFIED, ENV_OBSERVED, ENV_UNKNOWN, ENV_MISSING];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub repo_path: String,
    pub repo_identity: String,
    pub created_at: String,
    pub updated_at: String,
}

pub type WorkspaceInfo = WorkspaceRecord;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoalRecord {
    pub id: String,
    pub workspace_id: String,
    pub statement: String,
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub provenance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub workspace_id: String,
    pub harness: Option<String>,
    pub agent_identity: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub resumed_from_checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionRecord {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: Option<String>,
    pub statement: String,
    pub rationale: Option<String>,
    pub status: String,
    pub created_at: String,
    pub provenance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FindingRecord {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: Option<String>,
    pub statement: String,
    pub evidence_ref: Option<String>,
    pub status: String,
    pub created_at: String,
    pub provenance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactRecord {
    pub id: String,
    pub workspace_id: String,
    pub kind: String,
    pub uri: String,
    pub fingerprint: Option<String>,
    pub created_at: String,
    pub provenance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProvenanceRecord {
    pub id: String,
    pub source_type: String,
    pub agent: Option<String>,
    pub harness: Option<String>,
    pub session_id: Option<String>,
    pub evidence_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointRecord {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: Option<String>,
    pub message: Option<String>,
    pub work_revision: i64,
    pub git_head: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub git_dirty_fingerprint: Option<String>,
    pub rootfile_digest: Option<String>,
    pub root_lock_digest: Option<String>,
    pub profile_reference: Option<String>,
    pub environment_status: String,
    pub continuation_summary: String,
    pub snapshot: String,
    pub created_at: String,
    pub provenance_id: Option<String>,
    /// Serialized `AgentEnvSummary` JSON (refs/names only, never values).
    pub agent_env_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointSummary {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: Option<String>,
    pub message: Option<String>,
    pub work_revision: i64,
    pub git_head: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub environment_status: String,
    pub created_at: String,
    /// Agent that created the checkpoint, when provenance was agent-authored.
    pub provenance_agent: Option<String>,
    /// Serialized `AgentEnvSummary` JSON (refs/names only, never values).
    pub agent_env_ref: Option<String>,
}

impl From<&CheckpointRecord> for CheckpointSummary {
    fn from(record: &CheckpointRecord) -> Self {
        Self {
            id: record.id.clone(),
            workspace_id: record.workspace_id.clone(),
            goal_id: record.goal_id.clone(),
            message: record.message.clone(),
            work_revision: record.work_revision,
            git_head: record.git_head.clone(),
            git_branch: record.git_branch.clone(),
            git_dirty: record.git_dirty,
            environment_status: record.environment_status.clone(),
            created_at: record.created_at.clone(),
            provenance_agent: None,
            agent_env_ref: record.agent_env_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkEvent {
    pub sequence: i64,
    pub workspace_id: String,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub payload: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryView {
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub remote_origin: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkCounts {
    pub goals: i64,
    pub sessions: i64,
    pub decisions: i64,
    pub findings: i64,
    pub artifacts: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivitySummary {
    pub event_type: String,
    pub entity_type: String,
    pub description: String,
    pub timestamp: String,
    pub age_human: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceInitReport {
    pub success: bool,
    pub created: bool,
    pub workspace: WorkspaceInfo,
    pub database: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStatusReport {
    pub success: bool,
    pub workspace: WorkspaceInfo,
    pub repository: RepositoryView,
    pub goal: Option<GoalRecord>,
    pub counts: WorkCounts,
    pub last_activity: Option<ActivitySummary>,
    pub work_revision: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalReport {
    pub success: bool,
    pub goal: GoalRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionReport {
    pub success: bool,
    pub decision: DecisionRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionListReport {
    pub success: bool,
    pub workspace_id: String,
    pub decisions: Vec<DecisionRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingReport {
    pub success: bool,
    pub finding: FindingRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingListReport {
    pub success: bool,
    pub workspace_id: String,
    pub findings: Vec<FindingRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactReport {
    pub success: bool,
    pub artifact: ArtifactRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactListReport {
    pub success: bool,
    pub workspace_id: String,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointReport {
    pub success: bool,
    pub checkpoint: CheckpointRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointListReport {
    pub success: bool,
    pub workspace_id: String,
    pub checkpoints: Vec<CheckpointSummary>,
}
