//! Immutable snapshot of work state captured inside a checkpoint.

use anyhow::{Context, Result};
use root_agent_bundle::canonical::CanonicalEnv;
use root_work::{ArtifactRecord, DecisionRecord, FindingRecord, GoalRecord, WorkStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointSnapshot {
    pub goal: Option<GoalRecord>,
    pub decisions: Vec<DecisionRecord>,
    pub findings: Vec<FindingRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub work_revision: i64,
    /// Canonical agent-environment JSON captured with this checkpoint. Immutable
    /// once stored: resume derives its mapping from here, never from the live
    /// project file. `#[serde(default)]` keeps pre-Sprint-013 snapshots readable
    /// (field absent → `None`).
    #[serde(default)]
    pub agent_env: Option<serde_json::Value>,
    /// SHA-256 over the canonical agent-environment JSON (`agent_env`). Absent
    /// when no environment was captured.
    #[serde(default)]
    pub agent_env_sha256: Option<String>,
}

impl CheckpointSnapshot {
    /// Read the current durable work state into a snapshot (no agent env).
    pub fn capture(store: &WorkStore) -> Result<Self> {
        Self::capture_with_agent_env(store, None)
    }

    /// Read work state into a snapshot, binding an optional canonical agent
    /// environment and its content hash so the checkpoint is self-contained.
    pub fn capture_with_agent_env(
        store: &WorkStore,
        agent_env: Option<CanonicalEnv>,
    ) -> Result<Self> {
        let (agent_env, agent_env_sha256) = match agent_env {
            Some(env) => {
                let hash = env
                    .env_hash()
                    .context("Failed to hash the canonical agent environment")?;
                (
                    Some(
                        serde_json::to_value(&env)
                            .context("Failed to serialize the canonical agent environment")?,
                    ),
                    Some(hash),
                )
            }
            None => (None, None),
        };
        Ok(Self {
            goal: store.active_goal()?,
            decisions: store.list_decisions()?,
            findings: store.list_findings()?,
            artifacts: store.list_artifacts()?,
            work_revision: store.work_revision()?,
            agent_env,
            agent_env_sha256,
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).context("Failed to serialize checkpoint snapshot")
    }

    pub fn from_json(raw: &str) -> Result<Self> {
        serde_json::from_str(raw).context("Failed to parse checkpoint snapshot")
    }

    /// Active (non-superseded) decisions as of this snapshot.
    pub fn active_decisions(&self) -> impl Iterator<Item = &DecisionRecord> {
        self.decisions
            .iter()
            .filter(|decision| decision.status == root_work::model::STATUS_ACTIVE)
    }

    /// Active (non-superseded) findings as of this snapshot.
    pub fn active_findings(&self) -> impl Iterator<Item = &FindingRecord> {
        self.findings
            .iter()
            .filter(|finding| finding.status == root_work::model::STATUS_ACTIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn older_snapshot_without_agent_env_deserializes_to_none() {
        let raw = r#"{"goal":null,"decisions":[],"findings":[],"artifacts":[],"work_revision":3}"#;
        let snapshot = CheckpointSnapshot::from_json(raw).unwrap();
        assert!(snapshot.agent_env.is_none());
        assert!(snapshot.agent_env_sha256.is_none());
        assert_eq!(snapshot.work_revision, 3);
    }

    #[test]
    fn agent_env_round_trips_through_json() {
        let snapshot = CheckpointSnapshot {
            goal: None,
            decisions: Vec::new(),
            findings: Vec::new(),
            artifacts: Vec::new(),
            work_revision: 7,
            agent_env: Some(json!({"source_agent": "codex"})),
            agent_env_sha256: Some("abc123".to_string()),
        };
        let json = snapshot.to_json().unwrap();
        let parsed = CheckpointSnapshot::from_json(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }
}
