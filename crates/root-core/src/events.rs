use anyhow::{Context, Result};
use root_lockfile::get_root_dir;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RootEventType {
    Doctor,
    Install,
    Update,
    Remove,
    Verification,
    VerificationFailed,
    Rollback,
    Restore,
    RestorePlanned,
    RestoreRecovered,
    Execution,
    Policy,
    Sandbox,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RootEventStatus {
    Started,
    Planned,
    Completed,
    Failed,
    Verified,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: RootEventType,
    pub command: String,
    pub status: RootEventStatus,
    pub package: Option<String>,
    pub snapshot_id: Option<String>,
    pub restored_snapshot_id: Option<String>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryOutput {
    pub events: Vec<RootEvent>,
}

fn generate_event_id() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = now.as_secs();
    let micros = now.subsec_micros();
    let datetime = chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.format("%Y%m%d_%H%M%S").to_string())
        .unwrap_or_else(|| format!("{}", secs));
    format!("evt_{}_{:06}", datetime, micros)
}

pub fn now_iso_for_event() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = now.as_secs();
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{}", secs))
}

fn events_path() -> Result<std::path::PathBuf> {
    Ok(get_root_dir()?.join("events.jsonl"))
}

pub fn append_event(event: &RootEvent) -> Result<()> {
    let path = events_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open events file at {:?}", path))?;
    let line = serde_json::to_string(event)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn read_events() -> Result<Vec<RootEvent>> {
    read_events_with_limit(None)
}

pub fn read_events_with_limit(limit: Option<usize>) -> Result<Vec<RootEvent>> {
    let path = events_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)
        .with_context(|| format!("Failed to open events file at {:?}", path))?;
    let reader = BufReader::new(file);
    let mut events: std::collections::VecDeque<RootEvent> = std::collections::VecDeque::new();
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<RootEvent>(line) {
            if limit.map(|l| events.len() >= l).unwrap_or(false) {
                events.pop_front();
            }
            events.push_back(event);
        }
    }
    Ok(events.into_iter().rev().collect())
}

pub fn create_event(
    event_type: RootEventType,
    status: RootEventStatus,
    command: &str,
    package: Option<String>,
    snapshot_id: Option<String>,
    restored_snapshot_id: Option<String>,
    message: Option<String>,
) -> RootEvent {
    RootEvent {
        id: generate_event_id(),
        timestamp: now_iso_for_event(),
        event_type,
        command: command.to_string(),
        status,
        package,
        snapshot_id,
        restored_snapshot_id,
        message,
        task_name: None,
        exit_code: None,
        started_at: None,
        finished_at: None,
        duration_ms: None,
        policy_decision: None,
        sandbox_id: None,
        failure_phase: None,
        installed_count: None,
        removed_count: None,
        kept_count: None,
    }
}

pub struct ExecutionEventDetails {
    pub task_name: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub message: Option<String>,
}

pub fn record_execution_event(
    command: &str,
    status: RootEventStatus,
    details: ExecutionEventDetails,
) -> Result<RootEvent> {
    let mut event = create_event(
        RootEventType::Execution,
        status,
        command,
        None,
        None,
        None,
        details.message,
    );
    event.task_name = details.task_name;
    event.exit_code = details.exit_code;
    event.started_at = Some(details.started_at);
    event.finished_at = Some(details.finished_at);
    event.duration_ms = Some(details.duration_ms);
    append_event(&event)?;
    Ok(event)
}

pub fn record_policy_event(
    command: &str,
    status: RootEventStatus,
    decision: &str,
    message: String,
) -> Result<RootEvent> {
    let mut event = create_event(
        RootEventType::Policy,
        status,
        command,
        None,
        None,
        None,
        Some(message),
    );
    event.policy_decision = Some(decision.to_string());
    append_event(&event)?;
    Ok(event)
}

pub fn record_event(
    event_type: RootEventType,
    status: RootEventStatus,
    command: &str,
    package: Option<String>,
    snapshot_id: Option<String>,
    restored_snapshot_id: Option<String>,
    message: Option<String>,
) -> Result<RootEvent> {
    let event = create_event(
        event_type,
        status,
        command,
        package,
        snapshot_id,
        restored_snapshot_id,
        message,
    );
    append_event(&event)?;
    Ok(event)
}
