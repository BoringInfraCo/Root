//! Unit tests for the MCP request handler and provenance wiring.

use crate::server::{handle_line, handle_message};
use crate::session::ServerState;
use crate::Policy;
use root_work::{Repository, WorkStore};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::MutexGuard;

fn temp(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_mcp_unit_{tag}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let base = temp(tag);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        let repository = Repository::discover(&repo).unwrap();
        WorkStore::init_at(&root_dir, repository).unwrap();
        Self {
            base,
            repo,
            root_dir,
        }
    }

    fn state(&self, policy: Policy) -> ServerState {
        let repository = Repository::discover(&self.repo).unwrap();
        let store = WorkStore::open_at(&self.root_dir, repository.clone()).unwrap();
        ServerState::new(store, self.root_dir.clone(), repository, policy)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Serialize tests that mutate `ROOT_DIR` (see AGENTS.md testing quirks) and
/// restore the previous value on drop.
struct EnvGuard {
    previous_root: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(root_dir: &Path) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let previous_root = std::env::var_os("ROOT_DIR");
        std::env::set_var("ROOT_DIR", root_dir);
        Self {
            previous_root,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous_root {
            Some(value) => std::env::set_var("ROOT_DIR", value),
            None => std::env::remove_var("ROOT_DIR"),
        }
    }
}

/// List files under `dir`, ignoring SQLite sidecar files that opening a WAL
/// database may touch.
fn list_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with("-journal") {
                continue;
            }
            out.push(path.display().to_string());
        }
    }
    out.sort();
    out
}

fn request(state: &mut ServerState, id: i64, method: &str, params: Value) -> Value {
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    handle_message(state, message).expect("request must produce a response")
}

fn initialize(state: &mut ServerState) {
    let response = request(
        state,
        1,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "codex", "version": "1.0" },
        }),
    );
    assert_eq!(response["result"]["serverInfo"]["name"], "root");
}

fn call(state: &mut ServerState, id: i64, name: &str, arguments: Value) -> Value {
    request(
        state,
        id,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )["result"]
        .clone()
}

#[test]
fn initialize_handshake_reports_capabilities_and_creates_session() {
    let fixture = Fixture::new("init");
    let mut state = fixture.state(Policy::default_allow());

    let response = request(
        &mut state,
        1,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "codex", "version": "1.0" },
        }),
    );

    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(response["result"]["serverInfo"]["name"], "root");
    assert_eq!(
        response["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert!(state.session_id().is_some());
    assert_eq!(state.agent(), Some("codex"));
}

#[test]
fn unknown_protocol_version_falls_back_to_supported() {
    let fixture = Fixture::new("version");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(
        &mut state,
        1,
        "initialize",
        json!({ "protocolVersion": "2099-01-01" }),
    );
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
}

#[test]
fn notification_produces_no_response() {
    let fixture = Fixture::new("notification");
    let mut state = fixture.state(Policy::default_allow());
    let response = handle_message(
        &mut state,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert!(response.is_none());
}

#[test]
fn ping_returns_empty_object() {
    let fixture = Fixture::new("ping");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(&mut state, 1, "ping", json!({}));
    assert_eq!(response["result"], json!({}));
}

#[test]
fn tools_list_includes_the_documented_surface() {
    let fixture = Fixture::new("list");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(&mut state, 1, "tools/list", json!({}));
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for expected in [
        "workspace.get",
        "workspace.status",
        "work.get_goal",
        "work.list_decisions",
        "work.record_decision",
        "work.list_findings",
        "work.record_finding",
        "work.list_artifacts",
        "continuity.checkpoint",
        "continuity.resume",
        "continuity.handoff",
        "environment.status",
        "environment.verify",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
}

#[test]
fn workspace_status_returns_structured_content() {
    let fixture = Fixture::new("status");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let result = call(&mut state, 2, "workspace.status", json!({}));
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["success"], true);
    assert_eq!(result["structuredContent"]["workspace"]["name"], "campfire");
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("success"));
}

#[test]
fn record_decision_then_list_decisions() {
    let fixture = Fixture::new("decision");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let recorded = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": "Invitations expire after 24h", "rationale": "security" }),
    );
    assert_eq!(recorded["isError"], false);
    assert_eq!(
        recorded["structuredContent"]["decision"]["statement"],
        "Invitations expire after 24h"
    );
    assert!(recorded["structuredContent"]["decision"]["provenance_id"]
        .as_str()
        .is_some());

    let listed = call(&mut state, 3, "work.list_decisions", json!({}));
    assert_eq!(listed["isError"], false);
    let decisions = listed["structuredContent"]["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["statement"], "Invitations expire after 24h");
}

#[test]
fn record_finding_requires_statement() {
    let fixture = Fixture::new("finding");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let missing = call(&mut state, 2, "work.record_finding", json!({}));
    assert_eq!(missing["isError"], true);
    assert!(missing["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("statement"));

    let recorded = call(
        &mut state,
        3,
        "work.record_finding",
        json!({ "statement": "Consumption fails", "evidence_ref": "test output" }),
    );
    assert_eq!(recorded["isError"], false);
    assert_eq!(
        recorded["structuredContent"]["finding"]["evidence_ref"],
        "test output"
    );
}

#[test]
fn malformed_json_is_a_parse_error() {
    let fixture = Fixture::new("parse");
    let mut state = fixture.state(Policy::default_allow());
    let response = handle_line(&mut state, "{ this is not json").unwrap();
    assert_eq!(response["error"]["code"], -32700);
}

#[test]
fn unknown_method_is_method_not_found() {
    let fixture = Fixture::new("method");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(&mut state, 1, "workspace.bogus", json!({}));
    assert_eq!(response["error"]["code"], -32601);
}

#[test]
fn unknown_tool_is_invalid_params() {
    let fixture = Fixture::new("unknowntool");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(
        &mut state,
        1,
        "tools/call",
        json!({ "name": "workspace.nope", "arguments": {} }),
    );
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn non_object_arguments_are_invalid_params() {
    let fixture = Fixture::new("badargs");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(
        &mut state,
        1,
        "tools/call",
        json!({ "name": "workspace.get", "arguments": 42 }),
    );
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn denied_record_capability_returns_tool_error() {
    let fixture = Fixture::new("denied");
    std::fs::write(
        fixture.root_dir.join("mcp.toml"),
        "[capabilities]\nrecord = \"deny\"\n",
    )
    .unwrap();
    let policy = Policy::load_at(&fixture.root_dir);
    let mut state = fixture.state(policy);
    initialize(&mut state);

    let result = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": "Should not be recorded" }),
    );
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["content"][0]["text"],
        "Unauthorized: record capability is denied by policy."
    );

    let listed = call(&mut state, 3, "work.list_decisions", json!({}));
    assert_eq!(
        listed["structuredContent"]["decisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn mutations_record_agent_provenance() {
    let fixture = Fixture::new("provenance");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let result = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": "Adopt durable provenance" }),
    );
    let provenance_id = result["structuredContent"]["decision"]["provenance_id"]
        .as_str()
        .unwrap()
        .to_string();

    let record = state.store().provenance(&provenance_id).unwrap();
    assert_eq!(record.source_type, "agent");
    assert_eq!(record.agent.as_deref(), Some("codex"));
    assert_eq!(record.harness.as_deref(), Some("codex"));
    assert_eq!(record.session_id.as_deref(), state.session_id());
    assert!(record.session_id.is_some());
}

#[test]
fn checkpoint_records_agent_provenance() {
    let fixture = Fixture::new("checkpoint_provenance");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let result = call(
        &mut state,
        2,
        "continuity.checkpoint",
        json!({ "message": "agent checkpoint" }),
    );
    assert_eq!(result["isError"], false);
    let provenance_id = result["structuredContent"]["checkpoint"]["provenance_id"]
        .as_str()
        .unwrap()
        .to_string();

    let record = state.store().provenance(&provenance_id).unwrap();
    assert_eq!(record.source_type, "agent");
    assert_eq!(record.agent.as_deref(), Some("codex"));
    assert_eq!(record.session_id.as_deref(), state.session_id());
}

#[test]
fn handoff_tool_projects_state_and_rejects_unknown_target() {
    let fixture = Fixture::new("handoff");
    let repository = Repository::discover(&fixture.repo).unwrap();
    let mut store = WorkStore::open_at(&fixture.root_dir, repository.clone()).unwrap();
    store.set_goal("Implement workspace invitations").unwrap();
    let mut state = ServerState::new(
        store,
        fixture.root_dir.clone(),
        repository,
        Policy::default_allow(),
    );
    initialize(&mut state);
    let _guard = EnvGuard::set(&fixture.root_dir);

    let decision = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": "Tokens expire after 24h" }),
    );
    assert_eq!(decision["isError"], false);

    let checkpoint = call(
        &mut state,
        3,
        "continuity.checkpoint",
        json!({ "message": "agent checkpoint" }),
    );
    assert_eq!(checkpoint["isError"], false);

    let result = call(
        &mut state,
        4,
        "continuity.handoff",
        json!({ "to": "claude" }),
    );

    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["handoff"]["to"], "claude");
    assert_eq!(result["structuredContent"]["handoff"]["from"], "codex");
    assert_eq!(
        result["structuredContent"]["handoff"]["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert_eq!(
        result["structuredContent"]["handoff"]["checkpoint"]["message"],
        "agent checkpoint"
    );
    assert!(
        result["structuredContent"]["handoff"]["suggested_continuation"][0]
            .as_str()
            .unwrap()
            .starts_with("Suggestion (not verified):")
    );
    assert!(result["structuredContent"]["rendered"]
        .as_str()
        .unwrap()
        .contains("Handoff"));

    let invalid = call(
        &mut state,
        5,
        "continuity.handoff",
        json!({ "to": "gemini" }),
    );
    assert_eq!(invalid["isError"], true);
    assert!(invalid["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Supported adapters"));
}

#[test]
fn oversized_statement_is_rejected() {
    let fixture = Fixture::new("oversized");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let statement = "x".repeat(10_001);
    let result = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": statement }),
    );
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("10000"));
}

#[test]
fn secret_statement_is_refused_by_mcp() {
    let fixture = Fixture::new("secret");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);

    let result = call(
        &mut state,
        2,
        "work.record_decision",
        json!({ "statement": "password = hunter2-secret" }),
    );
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Refusing to persist what looks like a secret"));

    let listed = call(&mut state, 3, "work.list_decisions", json!({}));
    assert_eq!(
        listed["structuredContent"]["decisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn tools_call_params_must_be_an_object() {
    let fixture = Fixture::new("paramsobject");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(&mut state, 1, "tools/call", json!("not-an-object"));
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn tools_call_requires_string_name() {
    let fixture = Fixture::new("paramname");
    let mut state = fixture.state(Policy::default_allow());
    let response = request(
        &mut state,
        1,
        "tools/call",
        json!({ "name": 42, "arguments": {} }),
    );
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn continuity_resume_with_target_returns_steps_without_mutation() {
    let fixture = Fixture::new("resume_with");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);
    let _guard = EnvGuard::set(&fixture.root_dir);

    let checkpoint = call(
        &mut state,
        2,
        "continuity.checkpoint",
        json!({ "message": "agent checkpoint" }),
    );
    assert_eq!(checkpoint["isError"], false);

    let files_before = list_files(&fixture.root_dir);
    let events_before = state.store().events().unwrap().len();

    let result = call(
        &mut state,
        3,
        "continuity.resume",
        json!({ "with": "claude" }),
    );

    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["target"], "claude");
    let steps = result["structuredContent"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    assert!(steps.iter().all(|step| step["ok"].is_boolean()));

    assert_eq!(list_files(&fixture.root_dir), files_before);
    assert_eq!(state.store().events().unwrap().len(), events_before);
}

#[test]
fn continuity_resume_with_unknown_target_fails_closed() {
    let fixture = Fixture::new("resume_bad_target");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);
    let _guard = EnvGuard::set(&fixture.root_dir);

    let checkpoint = call(
        &mut state,
        2,
        "continuity.checkpoint",
        json!({ "message": "agent checkpoint" }),
    );
    assert_eq!(checkpoint["isError"], false);

    let result = call(
        &mut state,
        3,
        "continuity.resume",
        json!({ "with": "gemini" }),
    );

    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Unsupported --with target"), "{text}");
    assert!(text.contains("codex, opencode, claude"), "{text}");
}

#[test]
fn continuity_resume_with_non_string_target_is_invalid() {
    let fixture = Fixture::new("resume_bad_with");
    let mut state = fixture.state(Policy::default_allow());
    initialize(&mut state);
    let _guard = EnvGuard::set(&fixture.root_dir);

    let result = call(&mut state, 2, "continuity.resume", json!({ "with": 42 }));

    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("with"));
}

#[test]
fn continuity_resume_without_target_is_unchanged() {
    let fixture = Fixture::new("resume_plain");
    let repository = Repository::discover(&fixture.repo).unwrap();
    let mut store = WorkStore::open_at(&fixture.root_dir, repository.clone()).unwrap();
    store.set_goal("Bound the package").unwrap();
    for index in 0..12 {
        store
            .add_decision(&format!("Decision {index:02}"), None)
            .unwrap();
    }
    for index in 0..12 {
        store
            .add_finding(&format!("Finding {index:02}"), None)
            .unwrap();
    }
    let mut state = ServerState::new(
        store,
        fixture.root_dir.clone(),
        repository,
        Policy::default_allow(),
    );
    initialize(&mut state);
    let _guard = EnvGuard::set(&fixture.root_dir);

    let checkpoint = call(
        &mut state,
        2,
        "continuity.checkpoint",
        json!({ "message": "agent checkpoint" }),
    );
    assert_eq!(checkpoint["isError"], false);

    let result = call(&mut state, 3, "continuity.resume", json!({}));
    assert_eq!(result["isError"], false);
    let resume = &result["structuredContent"]["resume"];
    assert_eq!(resume["decisions"].as_array().unwrap().len(), 10);
    assert_eq!(resume["decisions_omitted"], 2);
    assert_eq!(resume["findings"].as_array().unwrap().len(), 10);
    assert_eq!(resume["findings_omitted"], 2);
    let rendered = result["structuredContent"]["rendered"].as_str().unwrap();
    assert!(rendered.contains("Root Resume"));
    assert!(rendered.contains("Suggestion (not verified):"));
    assert!(resume["suggested_continuation"][0]
        .as_str()
        .unwrap()
        .starts_with("Investigate:"));
}
