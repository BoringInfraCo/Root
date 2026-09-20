//! SQLite persistence for canonical work state.
//!
//! The database owns queryable *current* state. The `work_events` table owns
//! the append-only history of what happened. Both live in the same file but
//! serve different purposes.

use crate::time::now_rfc3339;
use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Current canonical work schema version.
pub const WORK_SCHEMA_VERSION: i64 = 2;

/// Open (creating if needed) and migrate a work-state database.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create work directory {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("Database unavailable at {}", path.display()))?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )
    .context("Failed to configure the work database")?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS work_schema_migrations (
             version INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL
         );",
    )
    .context("Failed to initialize the work migration table")?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
            [],
            |row| row.get(0),
        )
        .context("Failed to read the work schema version")?;

    if current > WORK_SCHEMA_VERSION {
        bail!(
            "Work database schema version {} is newer than this build supports ({}).\n\n\
             Upgrade Root before opening this workspace.",
            current,
            WORK_SCHEMA_VERSION
        );
    }

    if current < 1 {
        apply_migration(conn, 1, MIGRATION_V1)?;
    }

    if current < 2 {
        apply_migration(conn, 2, MIGRATION_V2)?;
    }

    Ok(())
}

/// Apply one schema migration and its version marker atomically. If any
/// statement fails, the transaction rolls back and no partial tables or
/// version rows remain, so the migration can be retried.
fn apply_migration(conn: &Connection, version: i64, sql: &str) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .with_context(|| format!("Failed to start work schema migration to version {version}"))?;
    tx.execute_batch(sql)
        .with_context(|| format!("Work database migration to version {version} failed"))?;
    tx.execute(
        "INSERT INTO work_schema_migrations (version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![version, now_rfc3339()],
    )?;
    tx.commit()
        .with_context(|| format!("Failed to commit work schema migration to version {version}"))?;
    Ok(())
}

pub const MIGRATION_V1: &str = r#"
CREATE TABLE workspaces (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    repo_path     TEXT NOT NULL,
    repo_identity TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE provenance (
    id           TEXT PRIMARY KEY,
    source_type  TEXT NOT NULL CHECK (source_type IN ('human', 'agent', 'root', 'import')),
    agent        TEXT,
    harness      TEXT,
    session_id   TEXT,
    evidence_ref TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE goals (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    statement     TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('active', 'completed', 'superseded')),
    created_at    TEXT NOT NULL,
    completed_at  TEXT,
    provenance_id TEXT REFERENCES provenance(id)
);
CREATE INDEX idx_goals_workspace_status ON goals(workspace_id, status);

CREATE TABLE sessions (
    id                         TEXT PRIMARY KEY,
    workspace_id               TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    harness                    TEXT,
    agent_identity             TEXT,
    started_at                 TEXT NOT NULL,
    ended_at                   TEXT,
    resumed_from_checkpoint_id TEXT
);
CREATE INDEX idx_sessions_workspace ON sessions(workspace_id);

CREATE TABLE decisions (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    goal_id       TEXT REFERENCES goals(id),
    statement     TEXT NOT NULL,
    rationale     TEXT,
    status        TEXT NOT NULL CHECK (status IN ('active', 'superseded')),
    created_at    TEXT NOT NULL,
    provenance_id TEXT REFERENCES provenance(id)
);
CREATE INDEX idx_decisions_workspace ON decisions(workspace_id, created_at);

CREATE TABLE findings (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    goal_id       TEXT REFERENCES goals(id),
    statement     TEXT NOT NULL,
    evidence_ref  TEXT,
    status        TEXT NOT NULL CHECK (status IN ('active', 'superseded')),
    created_at    TEXT NOT NULL,
    provenance_id TEXT REFERENCES provenance(id)
);
CREATE INDEX idx_findings_workspace ON findings(workspace_id, created_at);

CREATE TABLE artifacts (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL,
    uri           TEXT NOT NULL,
    fingerprint   TEXT,
    created_at    TEXT NOT NULL,
    provenance_id TEXT REFERENCES provenance(id)
);
CREATE INDEX idx_artifacts_workspace ON artifacts(workspace_id, created_at);

CREATE TABLE work_events (
    sequence     INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    event_type   TEXT NOT NULL,
    entity_type  TEXT NOT NULL,
    entity_id    TEXT,
    payload      TEXT,
    timestamp    TEXT NOT NULL
);
CREATE INDEX idx_work_events_workspace ON work_events(workspace_id, sequence);
"#;

pub const MIGRATION_V2: &str = r#"
CREATE TABLE checkpoints (
    id                     TEXT PRIMARY KEY,
    workspace_id           TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    goal_id                TEXT REFERENCES goals(id),
    message                TEXT,
    work_revision          INTEGER NOT NULL,
    git_head               TEXT,
    git_branch             TEXT,
    git_dirty              INTEGER NOT NULL,
    git_dirty_fingerprint  TEXT,
    rootfile_digest        TEXT,
    root_lock_digest       TEXT,
    profile_reference      TEXT,
    environment_status     TEXT NOT NULL,
    continuation_summary   TEXT NOT NULL,
    snapshot               TEXT NOT NULL,
    created_at             TEXT NOT NULL,
    provenance_id          TEXT REFERENCES provenance(id)
);
CREATE INDEX idx_checkpoints_workspace ON checkpoints(workspace_id, created_at);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("root_work_db_{}_{}", tag, std::process::id()))
            .join("state.db")
    }

    #[test]
    fn creates_schema_and_records_version() {
        let path = temp_db("create");
        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('workspaces','goals','sessions','decisions','findings','artifacts','provenance','work_events','checkpoints')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 9);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migration_is_idempotent_and_enables_foreign_keys() {
        let path = temp_db("idempotent");
        let conn = open(&path).unwrap();
        drop(conn);
        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);

        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrates_v1_to_v2_without_data_loss() {
        let path = temp_db("v1_to_v2");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute_batch(
                "CREATE TABLE work_schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
                 INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_EXISTING', 'campfire', '/tmp/campfire', 'path:/tmp/campfire', 'now', 'now');",
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);
        let name: String = conn
            .query_row(
                "SELECT name FROM workspaces WHERE id = 'root_ws_EXISTING'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "campfire");
        let has_checkpoints: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'checkpoints'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_checkpoints, 1);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_newer_schema_version() {
        let path = temp_db("newer");
        let conn = open(&path).unwrap();
        conn.execute(
            "INSERT INTO work_schema_migrations (version, applied_at) VALUES (999, 'now')",
            [],
        )
        .unwrap();
        drop(conn);
        let err = open(&path).unwrap_err().to_string();
        assert!(err.contains("newer than this build supports"), "{err}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn failed_migration_rolls_back_and_retries_cleanly() {
        let path = temp_db("retry");
        let conn = open(&path).unwrap();

        let failing = "CREATE TABLE migration_probe (id INTEGER);
                        INSERT INTO missing_table (id) VALUES (1);";
        let err = apply_migration(&conn, 3, failing).unwrap_err().to_string();
        assert!(err.contains("version 3"), "{err}");

        let probe: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'migration_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(probe, 0, "partial table survived a failed migration");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);

        apply_migration(&conn, 3, "CREATE TABLE migration_probe (id INTEGER);").unwrap();
        let probe: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'migration_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(probe, 1);
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 3);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn failed_v2_migration_is_retryable_without_loss() {
        let path = temp_db("retry_v2");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute_batch(
                "CREATE TABLE work_schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
                 INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_EXISTING', 'campfire', '/tmp/campfire', 'path:/tmp/campfire', 'now', 'now');
                 CREATE TABLE checkpoints (bogus TEXT);",
            )
            .unwrap();
        }

        let err = open(&path).unwrap_err().to_string();
        assert!(err.contains("version 2"), "{err}");

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1, "failed migration must not advance the version");
        let name: String = conn
            .query_row(
                "SELECT name FROM workspaces WHERE id = 'root_ws_EXISTING'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "campfire");
        conn.execute_batch("DROP TABLE checkpoints").unwrap();
        drop(conn);

        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);
        let has_snapshot: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'snapshot'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_snapshot, 1);

        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
