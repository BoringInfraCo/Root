//! The canonical Work State Engine store.
//!
//! `WorkStore` owns an open SQLite connection bound to exactly one Root
//! workspace. Every durable mutation runs inside a transaction and appends an
//! entry to the append-only `work_events` ledger.

use crate::db;
use crate::id;
use crate::model::*;
use crate::paths;
use crate::registry::{WorkIndex, WorkspaceEntry};
use crate::repository::Repository;
use crate::secrets;
use crate::time::{humanize_age, now_rfc3339};
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};

pub struct WorkStore {
    conn: Connection,
    workspace: WorkspaceRecord,
    repository: Repository,
}

impl WorkStore {
    /// Initialize a new workspace inside `repository`.
    pub fn init(repository: Repository) -> Result<WorkspaceInitReport> {
        let root_dir = root_dir()?;
        Self::init_at(&root_dir, repository)
    }

    /// Initialize a new workspace using an explicit Root directory.
    pub fn init_at(root_dir: &Path, repository: Repository) -> Result<WorkspaceInitReport> {
        let mut index = WorkIndex::load(root_dir)?;
        let repo_path = repository.root.display().to_string();
        let identity = repository.identity();

        if let Some(entry) = index
            .find_by_identity(&identity)
            .or_else(|| index.find_by_path(&repo_path))
        {
            if paths::database_path(root_dir, &entry.id).exists() {
                bail!(
                    "A Root workspace already exists for this repository ({}).\n\n\
                     Use `root workspace status` to inspect it.",
                    entry.id
                );
            }
        }

        let workspace_id = id::workspace_id();
        if !id::is_filesystem_safe(&workspace_id) {
            bail!("Generated workspace ID is not filesystem-safe");
        }

        std::fs::create_dir_all(paths::exports_dir(root_dir, &workspace_id))?;
        let db_path = paths::database_path(root_dir, &workspace_id);
        let conn = db::open(&db_path)?;

        let now = now_rfc3339();
        let workspace = WorkspaceRecord {
            id: workspace_id.clone(),
            name: repository.name(),
            repo_path: repo_path.clone(),
            repo_identity: identity.clone(),
            created_at: now.clone(),
            updated_at: now,
        };

        {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    workspace.id,
                    workspace.name,
                    workspace.repo_path,
                    workspace.repo_identity,
                    workspace.created_at,
                    workspace.updated_at
                ],
            )?;
            let provenance = insert_provenance(&tx, SOURCE_ROOT, None, None, None, None)?;
            let payload = serde_json::json!({
                "name": workspace.name,
                "repo_path": workspace.repo_path,
                "repo_identity": workspace.repo_identity,
                "provenance_id": provenance,
            });
            insert_event(
                &tx,
                &workspace.id,
                "workspace.created",
                "workspace",
                Some(&workspace.id),
                payload,
            )?;
            tx.commit()?;
        }

        index.upsert(WorkspaceEntry {
            id: workspace_id.clone(),
            repo_path,
            repo_identity: identity,
        });
        index.save(root_dir)?;

        Ok(WorkspaceInitReport {
            success: true,
            created: true,
            workspace,
            database: db_path.display().to_string(),
        })
    }

    /// Open the existing workspace associated with `repository`.
    pub fn open(repository: Repository) -> Result<Self> {
        let root_dir = root_dir()?;
        Self::open_at(&root_dir, repository)
    }

    /// Open the existing workspace using an explicit Root directory.
    pub fn open_at(root_dir: &Path, repository: Repository) -> Result<Self> {
        let workspace_id = locate_workspace_id(root_dir, &repository)?;
        let db_path = paths::database_path(root_dir, &workspace_id);
        if !db_path.exists() {
            bail!(
                "Workspace metadata is present but the database is missing.\n\n\
                 Expected: {}\n\
                 Run `root workspace init` to recreate work state for this repository.",
                db_path.display()
            );
        }
        let conn = db::open(&db_path)?;
        let workspace = load_workspace(&conn, &workspace_id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Workspace metadata points at '{}', but no matching workspace exists in {}.",
                workspace_id,
                db_path.display()
            )
        })?;
        Ok(Self {
            conn,
            workspace,
            repository,
        })
    }

    pub fn workspace(&self) -> &WorkspaceRecord {
        &self.workspace
    }

    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    fn active_goal_record(&self) -> Result<Option<GoalRecord>> {
        let goal = self
            .conn
            .query_row(
                "SELECT id, workspace_id, statement, status, created_at, completed_at, provenance_id
                 FROM goals WHERE workspace_id = ?1 AND status = 'active'
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![self.workspace.id],
                row_to_goal,
            )
            .optional()?;
        Ok(goal)
    }

    /// The active goal, if one is set.
    pub fn active_goal(&self) -> Result<Option<GoalRecord>> {
        self.active_goal_record()
    }

    pub fn status(&self) -> Result<WorkspaceStatusReport> {
        let goal = self.active_goal_record()?;
        let counts = WorkCounts {
            goals: self.count("goals")?,
            sessions: self.count("sessions")?,
            decisions: self.count("decisions")?,
            findings: self.count("findings")?,
            artifacts: self.count("artifacts")?,
        };
        let last_activity = self.last_activity()?;
        let work_revision = self.count("work_events")?;
        Ok(WorkspaceStatusReport {
            success: true,
            workspace: self.workspace.clone(),
            repository: RepositoryView {
                path: self.repository.root.display().to_string(),
                branch: self.repository.branch.clone(),
                head: self.repository.head.clone(),
                remote_origin: self.repository.remote_origin.clone(),
            },
            goal,
            counts,
            last_activity,
            work_revision,
        })
    }

    fn count(&self, table: &str) -> Result<i64> {
        let value = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {} WHERE workspace_id = ?1", table),
            params![self.workspace.id],
            |row| row.get(0),
        )?;
        Ok(value)
    }

    fn last_activity(&self) -> Result<Option<ActivitySummary>> {
        let event = self
            .conn
            .query_row(
                "SELECT sequence, workspace_id, event_type, entity_type, entity_id, payload, timestamp
                 FROM work_events WHERE workspace_id = ?1 ORDER BY sequence DESC LIMIT 1",
                params![self.workspace.id],
                row_to_event,
            )
            .optional()?;
        Ok(event.map(|event| ActivitySummary {
            description: describe_event(&event),
            age_human: humanize_age(&event.timestamp),
            event_type: event.event_type,
            entity_type: event.entity_type,
            timestamp: event.timestamp,
        }))
    }

    /// Set a new active goal. Any previous active goal is superseded, not deleted.
    pub fn set_goal(&mut self, statement: &str) -> Result<GoalReport> {
        let statement = statement.trim();
        if statement.is_empty() {
            bail!("A goal statement is required.");
        }
        reject_secret(statement)?;
        let tx = self.conn.transaction()?;
        let provenance = insert_provenance(&tx, SOURCE_HUMAN, None, None, None, None)?;
        let now = now_rfc3339();

        if let Some(previous) = tx
            .query_row(
                "SELECT id FROM goals WHERE workspace_id = ?1 AND status = 'active'",
                params![self.workspace.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            tx.execute(
                "UPDATE goals SET status = 'superseded' WHERE id = ?1",
                params![previous],
            )?;
        }

        let goal = GoalRecord {
            id: id::goal_id(),
            workspace_id: self.workspace.id.clone(),
            statement: statement.to_string(),
            status: GOAL_ACTIVE.to_string(),
            created_at: now,
            completed_at: None,
            provenance_id: Some(provenance.clone()),
        };
        tx.execute(
            "INSERT INTO goals (id, workspace_id, statement, status, created_at, completed_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                goal.id,
                goal.workspace_id,
                goal.statement,
                goal.status,
                goal.created_at,
                goal.completed_at,
                goal.provenance_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &goal.created_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "goal.set",
            "goal",
            Some(&goal.id),
            serde_json::json!({ "statement": goal.statement, "provenance_id": provenance }),
        )?;
        tx.commit()?;
        Ok(GoalReport {
            success: true,
            goal,
        })
    }

    pub fn show_goal(&self) -> Result<GoalReport> {
        let goal = self.active_goal_record()?.ok_or_else(|| {
            anyhow::anyhow!("No active goal.\n\nSet one with:  root goal set \"<goal>\"")
        })?;
        Ok(GoalReport {
            success: true,
            goal,
        })
    }

    pub fn add_decision(
        &mut self,
        statement: &str,
        rationale: Option<&str>,
    ) -> Result<DecisionRecord> {
        self.add_decision_with(statement, rationale, ProvenanceContext::default())
    }

    /// Record a decision attributed to an explicit provenance source.
    pub fn add_decision_with(
        &mut self,
        statement: &str,
        rationale: Option<&str>,
        context: ProvenanceContext<'_>,
    ) -> Result<DecisionRecord> {
        let statement = statement.trim();
        if statement.is_empty() {
            bail!("A decision statement is required.");
        }
        reject_secret(statement)?;
        if let Some(rationale) = rationale {
            reject_secret(rationale)?;
        }
        let goal_id = self.active_goal_record()?.map(|goal| goal.id);
        let tx = self.conn.transaction()?;
        let provenance = insert_provenance_ctx(&tx, &context)?;
        let decision = DecisionRecord {
            id: id::decision_id(),
            workspace_id: self.workspace.id.clone(),
            goal_id,
            statement: statement.to_string(),
            rationale: rationale.map(|value| value.to_string()),
            status: STATUS_ACTIVE.to_string(),
            created_at: now_rfc3339(),
            provenance_id: Some(provenance.clone()),
        };
        tx.execute(
            "INSERT INTO decisions (id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                decision.id,
                decision.workspace_id,
                decision.goal_id,
                decision.statement,
                decision.rationale,
                decision.status,
                decision.created_at,
                decision.provenance_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &decision.created_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "decision.created",
            "decision",
            Some(&decision.id),
            serde_json::json!({
                "statement": decision.statement,
                "rationale": decision.rationale,
                "provenance_id": provenance,
            }),
        )?;
        tx.commit()?;
        Ok(decision)
    }

    pub fn list_decisions(&self) -> Result<Vec<DecisionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id
             FROM decisions WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map(params![self.workspace.id], row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn show_decision(&self, decision_id: &str) -> Result<DecisionRecord> {
        self.conn
            .query_row(
                "SELECT id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id
                 FROM decisions WHERE id = ?1 AND workspace_id = ?2",
                params![decision_id, self.workspace.id],
                row_to_decision,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Decision '{}' not found.", decision_id))
    }

    pub fn add_finding(
        &mut self,
        statement: &str,
        evidence_ref: Option<&str>,
    ) -> Result<FindingRecord> {
        self.add_finding_with(statement, evidence_ref, ProvenanceContext::default())
    }

    /// Record a finding attributed to an explicit provenance source.
    pub fn add_finding_with(
        &mut self,
        statement: &str,
        evidence_ref: Option<&str>,
        context: ProvenanceContext<'_>,
    ) -> Result<FindingRecord> {
        let statement = statement.trim();
        if statement.is_empty() {
            bail!("A finding statement is required.");
        }
        reject_secret(statement)?;
        let goal_id = self.active_goal_record()?.map(|goal| goal.id);
        let tx = self.conn.transaction()?;
        let provenance = insert_provenance_ctx(&tx, &context)?;
        let finding = FindingRecord {
            id: id::finding_id(),
            workspace_id: self.workspace.id.clone(),
            goal_id,
            statement: statement.to_string(),
            evidence_ref: evidence_ref
                .or(context.evidence_ref)
                .map(|value| value.to_string()),
            status: STATUS_ACTIVE.to_string(),
            created_at: now_rfc3339(),
            provenance_id: Some(provenance.clone()),
        };
        tx.execute(
            "INSERT INTO findings (id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                finding.id,
                finding.workspace_id,
                finding.goal_id,
                finding.statement,
                finding.evidence_ref,
                finding.status,
                finding.created_at,
                finding.provenance_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &finding.created_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "finding.created",
            "finding",
            Some(&finding.id),
            serde_json::json!({
                "statement": finding.statement,
                "evidence_ref": finding.evidence_ref,
                "provenance_id": provenance,
            }),
        )?;
        tx.commit()?;
        Ok(finding)
    }

    pub fn list_findings(&self) -> Result<Vec<FindingRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id
             FROM findings WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map(params![self.workspace.id], row_to_finding)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn show_finding(&self, finding_id: &str) -> Result<FindingRecord> {
        self.conn
            .query_row(
                "SELECT id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id
                 FROM findings WHERE id = ?1 AND workspace_id = ?2",
                params![finding_id, self.workspace.id],
                row_to_finding,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Finding '{}' not found.", finding_id))
    }

    pub fn add_artifact(&mut self, artifact: NewArtifact<'_>) -> Result<ArtifactRecord> {
        if !ARTIFACT_KINDS.contains(&artifact.kind) {
            bail!(
                "Unsupported artifact kind '{}'. Expected one of: {}.",
                artifact.kind,
                ARTIFACT_KINDS.join(", ")
            );
        }
        if artifact.uri.trim().is_empty() {
            bail!("An artifact URI is required.");
        }
        let tx = self.conn.transaction()?;
        let provenance =
            insert_provenance(&tx, SOURCE_HUMAN, None, None, None, artifact.evidence_ref)?;
        let record = ArtifactRecord {
            id: id::artifact_id(),
            workspace_id: self.workspace.id.clone(),
            kind: artifact.kind.to_string(),
            uri: artifact.uri.to_string(),
            fingerprint: artifact.fingerprint.map(|value| value.to_string()),
            created_at: now_rfc3339(),
            provenance_id: Some(provenance.clone()),
        };
        tx.execute(
            "INSERT INTO artifacts (id, workspace_id, kind, uri, fingerprint, created_at, provenance_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.workspace_id,
                record.kind,
                record.uri,
                record.fingerprint,
                record.created_at,
                record.provenance_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &record.created_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "artifact.created",
            "artifact",
            Some(&record.id),
            serde_json::json!({
                "kind": record.kind,
                "uri": record.uri,
                "fingerprint": record.fingerprint,
                "provenance_id": provenance,
            }),
        )?;
        tx.commit()?;
        Ok(record)
    }

    pub fn list_artifacts(&self) -> Result<Vec<ArtifactRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, kind, uri, fingerprint, created_at, provenance_id
             FROM artifacts WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map(params![self.workspace.id], row_to_artifact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Create an explicitly invoked agent/human session record.
    pub fn start_session(
        &mut self,
        harness: Option<&str>,
        agent_identity: Option<&str>,
    ) -> Result<SessionRecord> {
        let tx = self.conn.transaction()?;
        let session = SessionRecord {
            id: id::session_id(),
            workspace_id: self.workspace.id.clone(),
            harness: harness.map(|value| value.to_string()),
            agent_identity: agent_identity.map(|value| value.to_string()),
            started_at: now_rfc3339(),
            ended_at: None,
            resumed_from_checkpoint_id: None,
        };
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, harness, agent_identity, started_at, ended_at, resumed_from_checkpoint_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session.id,
                session.workspace_id,
                session.harness,
                session.agent_identity,
                session.started_at,
                session.ended_at,
                session.resumed_from_checkpoint_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &session.started_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "session.started",
            "session",
            Some(&session.id),
            serde_json::json!({ "harness": session.harness, "agent": session.agent_identity }),
        )?;
        tx.commit()?;
        Ok(session)
    }

    /// Create an immutable checkpoint over the current durable work state.
    pub fn create_checkpoint(&mut self, input: NewCheckpoint<'_>) -> Result<CheckpointRecord> {
        self.create_checkpoint_with(input, ProvenanceContext::default())
    }

    /// Create an immutable checkpoint attributed to an explicit provenance source.
    pub fn create_checkpoint_with(
        &mut self,
        input: NewCheckpoint<'_>,
        context: ProvenanceContext<'_>,
    ) -> Result<CheckpointRecord> {
        if !ENVIRONMENT_STATUSES.contains(&input.environment_status) {
            bail!(
                "Unsupported environment status '{}'. Expected one of: {}.",
                input.environment_status,
                ENVIRONMENT_STATUSES.join(", ")
            );
        }
        if let Some(message) = input.message {
            reject_secret(message)?;
        }
        reject_secret(input.continuation_summary)?;
        let goal_id = self.active_goal_record()?.map(|goal| goal.id);
        let tx = self.conn.transaction()?;
        let provenance = insert_provenance_ctx(&tx, &context)?;
        let record = CheckpointRecord {
            id: id::checkpoint_id(),
            workspace_id: self.workspace.id.clone(),
            goal_id,
            message: input.message.map(|value| value.to_string()),
            work_revision: input.work_revision,
            git_head: input.git_head.map(|value| value.to_string()),
            git_branch: input.git_branch.map(|value| value.to_string()),
            git_dirty: input.git_dirty,
            git_dirty_fingerprint: input.git_dirty_fingerprint.map(|value| value.to_string()),
            rootfile_digest: input.rootfile_digest.map(|value| value.to_string()),
            root_lock_digest: input.root_lock_digest.map(|value| value.to_string()),
            profile_reference: input.profile_reference.map(|value| value.to_string()),
            environment_status: input.environment_status.to_string(),
            continuation_summary: input.continuation_summary.to_string(),
            snapshot: input.snapshot.to_string(),
            created_at: now_rfc3339(),
            provenance_id: Some(provenance.clone()),
        };
        tx.execute(
            "INSERT INTO checkpoints (
                id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                profile_reference, environment_status, continuation_summary, snapshot,
                created_at, provenance_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                record.id,
                record.workspace_id,
                record.goal_id,
                record.message,
                record.work_revision,
                record.git_head,
                record.git_branch,
                record.git_dirty as i64,
                record.git_dirty_fingerprint,
                record.rootfile_digest,
                record.root_lock_digest,
                record.profile_reference,
                record.environment_status,
                record.continuation_summary,
                record.snapshot,
                record.created_at,
                record.provenance_id
            ],
        )?;
        touch_workspace(&tx, &self.workspace.id, &record.created_at)?;
        insert_event(
            &tx,
            &self.workspace.id,
            "checkpoint.created",
            "checkpoint",
            Some(&record.id),
            serde_json::json!({
                "message": record.message,
                "work_revision": record.work_revision,
                "environment_status": record.environment_status,
                "provenance_id": provenance,
            }),
        )?;
        tx.commit()?;
        Ok(record)
    }

    pub fn list_checkpoints(&self) -> Result<Vec<CheckpointSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                    git_dirty, environment_status, created_at
             FROM checkpoints WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map(params![self.workspace.id], row_to_checkpoint_summary)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn show_checkpoint(&self, checkpoint_id: &str) -> Result<CheckpointRecord> {
        self.conn
            .query_row(
                "SELECT id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                        git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                        profile_reference, environment_status, continuation_summary, snapshot,
                        created_at, provenance_id
                 FROM checkpoints WHERE id = ?1 AND workspace_id = ?2",
                params![checkpoint_id, self.workspace.id],
                row_to_checkpoint,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Checkpoint '{}' not found.", checkpoint_id))
    }

    pub fn latest_checkpoint(&self) -> Result<Option<CheckpointRecord>> {
        let record = self
            .conn
            .query_row(
                "SELECT id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                        git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                        profile_reference, environment_status, continuation_summary, snapshot,
                        created_at, provenance_id
                 FROM checkpoints WHERE workspace_id = ?1
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![self.workspace.id],
                row_to_checkpoint,
            )
            .optional()?;
        Ok(record)
    }

    /// Load a provenance record by ID.
    pub fn provenance(&self, provenance_id: &str) -> Result<ProvenanceRecord> {
        self.conn
            .query_row(
                "SELECT id, source_type, agent, harness, session_id, evidence_ref, created_at
                 FROM provenance WHERE id = ?1",
                params![provenance_id],
                row_to_provenance,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Provenance '{}' not found.", provenance_id))
    }

    pub fn events(&self) -> Result<Vec<WorkEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT sequence, workspace_id, event_type, entity_type, entity_id, payload, timestamp
             FROM work_events WHERE workspace_id = ?1 ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(params![self.workspace.id], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn work_revision(&self) -> Result<i64> {
        self.count("work_events")
    }
}

/// Input for creating an artifact reference.
pub struct NewArtifact<'a> {
    pub kind: &'a str,
    pub uri: &'a str,
    pub fingerprint: Option<&'a str>,
    pub evidence_ref: Option<&'a str>,
}

/// Input for creating an immutable checkpoint.
pub struct NewCheckpoint<'a> {
    pub message: Option<&'a str>,
    pub work_revision: i64,
    pub git_head: Option<&'a str>,
    pub git_branch: Option<&'a str>,
    pub git_dirty: bool,
    pub git_dirty_fingerprint: Option<&'a str>,
    pub rootfile_digest: Option<&'a str>,
    pub root_lock_digest: Option<&'a str>,
    pub profile_reference: Option<&'a str>,
    pub environment_status: &'a str,
    pub continuation_summary: &'a str,
    pub snapshot: &'a str,
}

/// Attribution for durable work objects. CLI-created records default to human.
#[derive(Debug, Clone)]
pub struct ProvenanceContext<'a> {
    pub source_type: &'a str,
    pub agent: Option<&'a str>,
    pub harness: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub evidence_ref: Option<&'a str>,
}

impl Default for ProvenanceContext<'_> {
    fn default() -> Self {
        Self {
            source_type: SOURCE_HUMAN,
            agent: None,
            harness: None,
            session_id: None,
            evidence_ref: None,
        }
    }
}

fn insert_provenance_ctx(conn: &Connection, context: &ProvenanceContext<'_>) -> Result<String> {
    insert_provenance(
        conn,
        context.source_type,
        context.agent,
        context.harness,
        context.session_id,
        context.evidence_ref,
    )
}

fn root_dir() -> Result<PathBuf> {
    root_lockfile::get_root_dir()
}

fn reject_secret(value: &str) -> Result<()> {
    if let Some(label) = secrets::detect(value) {
        bail!("{}", secrets::refusal(label));
    }
    Ok(())
}

fn locate_workspace_id(root_dir: &Path, repository: &Repository) -> Result<String> {
    let index = WorkIndex::load(root_dir)?;
    let repo_path = repository.root.display().to_string();
    if let Some(entry) = index
        .find_by_identity(&repository.identity())
        .or_else(|| index.find_by_path(&repo_path))
    {
        return Ok(entry.id.clone());
    }
    bail!(
        "No Root workspace found for this repository.\n\n\
         Initialize one with:  root workspace init"
    )
}

fn load_workspace(conn: &Connection, workspace_id: &str) -> Result<Option<WorkspaceRecord>> {
    let workspace = conn
        .query_row(
            "SELECT id, name, repo_path, repo_identity, created_at, updated_at
             FROM workspaces WHERE id = ?1",
            params![workspace_id],
            row_to_workspace,
        )
        .optional()?;
    Ok(workspace)
}

fn insert_provenance(
    conn: &Connection,
    source_type: &str,
    agent: Option<&str>,
    harness: Option<&str>,
    session_id: Option<&str>,
    evidence_ref: Option<&str>,
) -> Result<String> {
    let provenance_id = id::provenance_id();
    conn.execute(
        "INSERT INTO provenance (id, source_type, agent, harness, session_id, evidence_ref, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            provenance_id,
            source_type,
            agent,
            harness,
            session_id,
            evidence_ref,
            now_rfc3339()
        ],
    )?;
    Ok(provenance_id)
}

fn insert_event(
    conn: &Connection,
    workspace_id: &str,
    event_type: &str,
    entity_type: &str,
    entity_id: Option<&str>,
    payload: serde_json::Value,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO work_events (workspace_id, event_type, entity_type, entity_id, payload, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            workspace_id,
            event_type,
            entity_type,
            entity_id,
            payload.to_string(),
            now_rfc3339()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn touch_workspace(conn: &Connection, workspace_id: &str, timestamp: &str) -> Result<()> {
    conn.execute(
        "UPDATE workspaces SET updated_at = ?2 WHERE id = ?1",
        params![workspace_id, timestamp],
    )?;
    Ok(())
}

fn describe_event(event: &WorkEvent) -> String {
    match event.event_type.as_str() {
        "workspace.created" => "Workspace created".to_string(),
        "goal.set" => "Goal set".to_string(),
        "decision.created" => "Decision added".to_string(),
        "finding.created" => "Finding added".to_string(),
        "artifact.created" => "Artifact added".to_string(),
        "session.started" => "Session started".to_string(),
        "session.ended" => "Session ended".to_string(),
        other => other.to_string(),
    }
}

fn row_to_workspace(row: &Row) -> rusqlite::Result<WorkspaceRecord> {
    Ok(WorkspaceRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        repo_path: row.get(2)?,
        repo_identity: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn row_to_goal(row: &Row) -> rusqlite::Result<GoalRecord> {
    Ok(GoalRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        statement: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        completed_at: row.get(5)?,
        provenance_id: row.get(6)?,
    })
}

fn row_to_decision(row: &Row) -> rusqlite::Result<DecisionRecord> {
    Ok(DecisionRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        goal_id: row.get(2)?,
        statement: row.get(3)?,
        rationale: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        provenance_id: row.get(7)?,
    })
}

fn row_to_finding(row: &Row) -> rusqlite::Result<FindingRecord> {
    Ok(FindingRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        goal_id: row.get(2)?,
        statement: row.get(3)?,
        evidence_ref: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        provenance_id: row.get(7)?,
    })
}

fn row_to_artifact(row: &Row) -> rusqlite::Result<ArtifactRecord> {
    Ok(ArtifactRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        kind: row.get(2)?,
        uri: row.get(3)?,
        fingerprint: row.get(4)?,
        created_at: row.get(5)?,
        provenance_id: row.get(6)?,
    })
}

fn row_to_checkpoint(row: &Row) -> rusqlite::Result<CheckpointRecord> {
    Ok(CheckpointRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        goal_id: row.get(2)?,
        message: row.get(3)?,
        work_revision: row.get(4)?,
        git_head: row.get(5)?,
        git_branch: row.get(6)?,
        git_dirty: row.get::<_, i64>(7)? != 0,
        git_dirty_fingerprint: row.get(8)?,
        rootfile_digest: row.get(9)?,
        root_lock_digest: row.get(10)?,
        profile_reference: row.get(11)?,
        environment_status: row.get(12)?,
        continuation_summary: row.get(13)?,
        snapshot: row.get(14)?,
        created_at: row.get(15)?,
        provenance_id: row.get(16)?,
    })
}

fn row_to_checkpoint_summary(row: &Row) -> rusqlite::Result<CheckpointSummary> {
    Ok(CheckpointSummary {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        goal_id: row.get(2)?,
        message: row.get(3)?,
        work_revision: row.get(4)?,
        git_head: row.get(5)?,
        git_branch: row.get(6)?,
        git_dirty: row.get::<_, i64>(7)? != 0,
        environment_status: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn row_to_provenance(row: &Row) -> rusqlite::Result<ProvenanceRecord> {
    Ok(ProvenanceRecord {
        id: row.get(0)?,
        source_type: row.get(1)?,
        agent: row.get(2)?,
        harness: row.get(3)?,
        session_id: row.get(4)?,
        evidence_ref: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn row_to_event(row: &Row) -> rusqlite::Result<WorkEvent> {
    Ok(WorkEvent {
        sequence: row.get(0)?,
        workspace_id: row.get(1)?,
        event_type: row.get(2)?,
        entity_type: row.get(3)?,
        entity_id: row.get(4)?,
        payload: row.get(5)?,
        timestamp: row.get(6)?,
    })
}
