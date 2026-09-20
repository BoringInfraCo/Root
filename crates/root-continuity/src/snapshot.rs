//! Immutable snapshot of work state captured inside a checkpoint.

use anyhow::{Context, Result};
use root_work::{ArtifactRecord, DecisionRecord, FindingRecord, GoalRecord, WorkStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointSnapshot {
    pub goal: Option<GoalRecord>,
    pub decisions: Vec<DecisionRecord>,
    pub findings: Vec<FindingRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub work_revision: i64,
}

impl CheckpointSnapshot {
    /// Read the current durable work state into a snapshot.
    pub fn capture(store: &WorkStore) -> Result<Self> {
        Ok(Self {
            goal: store.active_goal()?,
            decisions: store.list_decisions()?,
            findings: store.list_findings()?,
            artifacts: store.list_artifacts()?,
            work_revision: store.work_revision()?,
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
