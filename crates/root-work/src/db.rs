//! SQLite persistence for canonical work state.
//!
//! The database owns queryable *current* state. The `work_events` table owns
//! the append-only history of what happened. Both live in the same file but
//! serve different purposes.

use crate::time::now_rfc3339;
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Current canonical work schema version.
pub const WORK_SCHEMA_VERSION: i64 = 3;

/// Open (creating if needed) and migrate a work-state database.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create work directory {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("Database unavailable at {}", path.display()))?;
    configure(&conn)?;

    let current = schema_version(&conn)?;
    if current > 0 && current < WORK_SCHEMA_VERSION {
        create_pre_migration_backup(&conn, path, current)?;
    }

    migrate(&conn)?;
    Ok(conn)
}

/// Open an existing work-state database strictly read-only.
///
/// Never creates the file or its parent directory, never migrates, and never
/// enables WAL — so it does not create `-wal`/`-shm` sidecars. A database whose
/// schema is older than this build is refused with a clear migration hint.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    // A cleanly closed WAL database has no live `-wal`, so it can be opened
    // `immutable` — that keeps SQLite from creating `-wal`/`-shm` sidecars. If
    // a writer currently holds uncheckpointed frames, open the existing WAL
    // read-only instead (the sidecars already exist; nothing new is created).
    let conn = if wal_has_frames(path) {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    } else {
        Connection::open_with_flags(
            sqlite_immutable_uri(path),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
    }
    .with_context(|| format!("Work database unavailable at {}", path.display()))?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA foreign_keys = ON;")
        .context("Failed to configure the read-only work database")?;

    let current = schema_version(&conn)?;
    if current > WORK_SCHEMA_VERSION {
        bail!("{}", newer_schema_message(current));
    }
    if current < WORK_SCHEMA_VERSION {
        bail!(
            "Work database schema version {} requires migration before read-only inspection \
             (current {}). Open it read-write (any mutating Root command) to migrate.",
            current,
            WORK_SCHEMA_VERSION
        );
    }
    Ok(conn)
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!("-{suffix}"));
    PathBuf::from(name)
}

fn wal_has_frames(path: &Path) -> bool {
    std::fs::metadata(sidecar_path(path, "wal"))
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

/// Build a `file:` URI that opens `path` immutable, so SQLite never creates
/// WAL shared-memory sidecars. Characters outside the URI-unreserved set are
/// percent-encoded.
fn sqlite_immutable_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        let character = byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.' | '~') {
            uri.push(character);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    uri
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

fn newer_schema_message(current: i64) -> String {
    format!(
        "Work database schema version {} is newer than this build supports ({}).\n\n\
         Upgrade Root before opening this workspace.",
        current, WORK_SCHEMA_VERSION
    )
}

/// Read the applied schema version, treating a missing migration table as `0`.
fn schema_version(conn: &Connection) -> Result<i64> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'work_schema_migrations'",
            [],
            |row| row.get(0),
        )
        .context("Failed to inspect the work migration table")?;
    if present == 0 {
        return Ok(0);
    }
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
        [],
        |row| row.get(0),
    )
    .context("Failed to read the work schema version")
}

/// `<db>.backup-v<from>`, e.g. `state.db.backup-v2`.
fn backup_path(path: &Path, from: i64) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".backup-v{from}"));
    PathBuf::from(name)
}

/// Create a consistent pre-migration snapshot with `VACUUM INTO`.
///
/// A symlinked backup path is refused (fail closed). An existing regular-file
/// backup is left untouched. A fresh path is populated atomically by SQLite.
fn create_pre_migration_backup(conn: &Connection, path: &Path, from: i64) -> Result<()> {
    let backup = backup_path(path, from);
    match std::fs::symlink_metadata(&backup) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!(
                "Refusing to migrate: the pre-migration backup path {} is a symlink.",
                backup.display()
            );
        }
        Ok(meta) if meta.is_file() => {
            return Ok(());
        }
        Ok(_) => {
            bail!(
                "Refusing to migrate: the pre-migration backup path {} is not a regular file.",
                backup.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect backup path {}", backup.display()));
        }
    }

    let escaped = backup.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{}'", escaped))
        .with_context(|| {
            format!(
                "Failed to create a pre-migration backup for schema version {from} at {}",
                backup.display()
            )
        })?;
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

    let current = schema_version(conn)?;

    if current > WORK_SCHEMA_VERSION {
        bail!("{}", newer_schema_message(current));
    }

    if current < 1 {
        apply_migration(conn, 1, MIGRATION_V1)?;
    }

    if current < 2 {
        apply_migration(conn, 2, MIGRATION_V2)?;
    }

    if current < 3 {
        apply_migration(conn, 3, MIGRATION_V3)?;
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

/// Sprint 013: additive agent-environment reference on checkpoints. Stores the
/// serialized `AgentEnvSummary` JSON (adapter/skills/MCP/credential *names*
/// only — never secret values). Nullable, so v2 rows migrate with no data loss.
pub const MIGRATION_V3: &str = "ALTER TABLE checkpoints ADD COLUMN agent_env_ref TEXT";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("root_work_db_{}_{}", tag, std::process::id()))
            .join("state.db")
    }

    fn seed_v2_db(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        conn.execute_batch(
            "CREATE TABLE work_schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
             INSERT INTO work_schema_migrations (version, applied_at) VALUES (2, 'now');
             INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
             VALUES ('root_ws_V2', 'campfire', '/tmp/campfire', 'path:/tmp/campfire', 'now', 'now');",
        )
        .unwrap();
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

        let has_agent_env_ref: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'agent_env_ref'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_agent_env_ref, 1, "v3 must add agent_env_ref");

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
        let err = apply_migration(&conn, 99, failing).unwrap_err().to_string();
        assert!(err.contains("version 99"), "{err}");

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

        apply_migration(&conn, 99, "CREATE TABLE migration_probe (id INTEGER);").unwrap();
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
        assert_eq!(version, 99);

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

    #[test]
    fn fresh_database_is_v3_latest_version() {
        assert_eq!(WORK_SCHEMA_VERSION, 3);
        let path = temp_db("fresh_v3");
        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 3);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn read_only_open_reads_current_schema_without_sidecars() {
        let path = temp_db("ro_current");
        {
            let conn = open(&path).unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_READ', 'read', '/tmp/read', 'path:/tmp/read', 'now', 'now')",
                [],
            )
            .unwrap();
        }

        let conn = open_read_only(&path).unwrap();
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
                "SELECT name FROM workspaces WHERE id = 'root_ws_READ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "read");
        let query_only: i64 = conn
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(query_only, 1);
        drop(conn);

        assert!(!path.with_extension("db-wal").exists());
        assert!(!path.with_extension("db-shm").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn read_only_open_rejects_old_schema_without_mutating() {
        let path = temp_db("ro_old");
        seed_v2_db(&path);
        let before = std::fs::read(&path).unwrap();

        let err = open_read_only(&path).unwrap_err().to_string();
        assert!(
            err.contains("requires migration before read-only inspection"),
            "{err}"
        );
        assert!(err.contains("read-write"), "{err}");
        assert!(
            !backup_path(&path, 2).exists(),
            "read-only open must not create a backup"
        );
        assert!(!path.with_extension("db-wal").exists());
        assert!(!path.with_extension("db-shm").exists());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn read_only_open_refuses_newer_schema() {
        let path = temp_db("ro_newer");
        {
            let conn = open(&path).unwrap();
            conn.execute(
                "INSERT INTO work_schema_migrations (version, applied_at) VALUES (999, 'now')",
                [],
            )
            .unwrap();
        }
        let err = open_read_only(&path).unwrap_err().to_string();
        assert!(err.contains("newer than this build supports"), "{err}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn read_only_open_missing_migration_table_is_version_zero() {
        let path = temp_db("ro_empty");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        let err = open_read_only(&path).unwrap_err().to_string();
        assert!(err.contains("schema version 0"), "{err}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrating_v2_creates_valid_backup_with_original_rows() {
        let path = temp_db("backup_v2");
        seed_v2_db(&path);
        let backup = backup_path(&path, 2);
        assert!(!backup.exists());

        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, WORK_SCHEMA_VERSION);
        drop(conn);
        assert!(backup.exists(), "a v2 migration must write .backup-v2");

        let backup_conn = Connection::open(&backup).unwrap();
        let backup_version: i64 = backup_conn
            .query_row(
                "SELECT MAX(version) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            backup_version, 2,
            "backup keeps the original schema version"
        );
        let name: String = backup_conn
            .query_row(
                "SELECT name FROM workspaces WHERE id = 'root_ws_V2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "campfire");
        let workspaces: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(workspaces, 1);
        drop(backup_conn);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn current_schema_is_not_backed_up() {
        let path = temp_db("no_backup");
        {
            let _conn = open(&path).unwrap();
        }
        {
            let _conn = open(&path).unwrap();
        }
        assert!(!backup_path(&path, 2).exists());
        assert!(!backup_path(&path, 3).exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_backup_path_does_not_silently_proceed() {
        use std::os::unix::fs::symlink;
        let path = temp_db("backup_symlink");
        seed_v2_db(&path);
        let target = path.parent().unwrap().join("elsewhere.db");
        std::fs::write(&target, b"not a database").unwrap();
        symlink(&target, backup_path(&path, 2)).unwrap();

        let err = open(&path).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 2, "failed backup must not migrate");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrates_v2_to_v3_without_data_loss() {
        let path = temp_db("v2_to_v3");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute_batch(MIGRATION_V2).unwrap();
            conn.execute_batch(
                "CREATE TABLE work_schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (2, 'now');
                 INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_EXISTING', 'campfire', '/tmp/campfire', 'path:/tmp/campfire', 'now', 'now');
                 INSERT INTO checkpoints (
                     id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                     git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                     profile_reference, environment_status, continuation_summary, snapshot,
                     created_at, provenance_id
                 ) VALUES (
                     'root_cp_EXISTING', 'root_ws_EXISTING', NULL, 'kept', 7, 'headsha', 'main',
                     1, 'fp', 'rf', 'lock', '/profiles/default', 'observed', 'summary', '{\"k\":1}',
                     'now', NULL
                 );",
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

        let has_agent_env_ref: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'agent_env_ref'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_agent_env_ref, 1);

        let row: (
            String,
            Option<String>,
            i64,
            Option<String>,
            bool,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT id, message, work_revision, git_head, git_dirty, root_lock_digest, snapshot
                 FROM checkpoints WHERE id = 'root_cp_EXISTING'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get::<_, i64>(4)? != 0,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "root_cp_EXISTING");
        assert_eq!(row.1.as_deref(), Some("kept"));
        assert_eq!(row.2, 7);
        assert_eq!(row.3.as_deref(), Some("headsha"));
        assert!(row.4);
        assert_eq!(row.5, "lock");
        assert_eq!(row.6, "{\"k\":1}");

        let agent_env_ref: Option<String> = conn
            .query_row(
                "SELECT agent_env_ref FROM checkpoints WHERE id = 'root_cp_EXISTING'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(agent_env_ref, None, "v2 rows migrate with no agent env ref");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn failed_v3_migration_is_retryable_without_loss() {
        let path = temp_db("retry_v3");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute_batch(MIGRATION_V2).unwrap();
            // Pre-create the column MIGRATION_V3 will add so the ALTER fails.
            conn.execute_batch("ALTER TABLE checkpoints ADD COLUMN agent_env_ref TEXT;")
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE work_schema_migrations (
                     version INTEGER PRIMARY KEY,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (1, 'now');
                 INSERT INTO work_schema_migrations (version, applied_at) VALUES (2, 'now');
                 INSERT INTO workspaces (id, name, repo_path, repo_identity, created_at, updated_at)
                 VALUES ('root_ws_EXISTING', 'campfire', '/tmp/campfire', 'path:/tmp/campfire', 'now', 'now');",
            )
            .unwrap();
        }

        let err = open(&path).unwrap_err().to_string();
        assert!(err.contains("version 3"), "{err}");

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 2, "failed migration must not advance the version");
        let name: String = conn
            .query_row(
                "SELECT name FROM workspaces WHERE id = 'root_ws_EXISTING'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "campfire");
        conn.execute_batch("ALTER TABLE checkpoints DROP COLUMN agent_env_ref;")
            .unwrap();
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
        let has_agent_env_ref: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'agent_env_ref'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_agent_env_ref, 1);

        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
