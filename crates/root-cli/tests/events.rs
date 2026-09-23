//! Milestone 3: inbound mail becomes an event, and a route records a delivery
//! without spawning an agent.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_events_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

fn email_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../connectors/email.local")
}

struct Fixture {
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = tmp("mail");
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        Self { repo, root_dir }
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("ROOT_DIR", &self.root_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?} stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn inbound_mail_does_not_invoke_without_a_route_and_send_waits() {
    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let package = email_dir();
    fixture.json(&[
        "connector",
        "install",
        package.join("manifest.json").to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    fixture.json(&["connector", "enable", "email.local", "--json"]);

    let first = fixture.json(&["event", "ingest", "email.local", "--json"]);
    assert_eq!(first["created"], 1);
    assert_eq!(first["deliveries"], 0);
    let again = fixture.json(&["event", "ingest", "email.local", "--json"]);
    assert_eq!(again["created"], 0);
    assert_eq!(again["duplicate"], 1);

    let events = fixture.json(&["event", "list", "--json"]);
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["selector"], "email.received");
    assert_eq!(events[0]["status"], "recorded");
    let event_id = events[0]["id"].as_str().unwrap();

    let route = fixture.json(&[
        "event",
        "route",
        "add",
        "--selector",
        "email.received",
        "--workspace",
        "campfire",
        "--harness",
        "codex",
        "--capability",
        "email.local.read",
        "--approve",
        "--json",
    ]);
    assert_eq!(route["approval_required"], true);

    let ledger: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.root_dir.join("events/ledger.json")).unwrap(),
    )
    .unwrap();
    let delivery = &ledger["deliveries"][0];
    assert_eq!(delivery["status"], "awaiting_approval");
    assert_eq!(delivery["event_id"], event_id);
    let approval = delivery["approval_id"].as_str().unwrap();

    let skipped = fixture.json(&["event", "deliver", "--json"]);
    assert_eq!(skipped["handed"], 0);

    fixture.json(&["approval", "approve", approval, "--json"]);
    let handed = fixture.json(&["event", "deliver", "--json"]);
    assert_eq!(handed["handed"], 1);

    let ledger: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.root_dir.join("events/ledger.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["deliveries"][0]["status"], "handed");
    assert_eq!(ledger["deliveries"][0]["attempts"], 1);

    fixture.json(&["event", "ack", event_id, "--json"]);
    let watching = fixture.json(&["event", "watch", "--json"]);
    assert!(watching.as_array().unwrap().is_empty());

    let mut server = Command::new(root_bin())
        .args(["mcp", "serve"])
        .current_dir(&fixture.repo)
        .env("ROOT_DIR", &fixture.root_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(server.stdout.take().unwrap());
    use std::io::{BufRead, Write};
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"codex","version":"1"}}});
    writeln!(stdin, "{init}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    let send = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"email.local.send","arguments":{}}});
    writeln!(stdin, "{send}").unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let response: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        response.to_string().contains("approval required"),
        "{response}"
    );
    let _ = server.kill();
    let _ = server.wait();
}

#[test]
fn pull_returns_a_handed_route_and_does_not_start_an_agent() {
    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let package = email_dir();
    fixture.json(&[
        "connector",
        "install",
        package.join("manifest.json").to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    fixture.json(&["connector", "enable", "email.local", "--json"]);
    fixture.json(&["event", "ingest", "email.local", "--json"]);

    let unrouted = fixture.json(&["event", "pull", "--harness", "codex", "--json"]);
    assert!(unrouted.as_array().unwrap().is_empty(), "{unrouted}");

    fixture.json(&[
        "event",
        "route",
        "add",
        "--selector",
        "email.received",
        "--workspace",
        "campfire",
        "--harness",
        "codex",
        "--capability",
        "email.local.read",
        "--json",
    ]);
    let pending = fixture.json(&["event", "pull", "--harness", "codex", "--json"]);
    assert!(pending.as_array().unwrap().is_empty(), "{pending}");

    fixture.json(&["event", "deliver", "--json"]);
    let wakes = fixture.json(&["event", "pull", "--harness", "codex", "--json"]);
    assert_eq!(wakes.as_array().unwrap().len(), 1, "{wakes}");
    assert_eq!(wakes[0]["selector"], "email.received");
    assert_eq!(wakes[0]["harness"], "codex");
    assert_eq!(wakes[0]["workspace"], "campfire");
    assert_eq!(wakes[0]["idempotency_key"], "m1");
    assert_eq!(wakes[0]["capabilities"][0], "email.local.read");
    let other = fixture.json(&["event", "pull", "--harness", "claude", "--json"]);
    assert!(other.as_array().unwrap().is_empty(), "{other}");
    assert!(!wakes.to_string().contains("spawn"), "{wakes}");

    let audit = std::fs::read_to_string(fixture.root_dir.join("events/audit.jsonl")).unwrap();
    assert!(audit.contains("event.ingest"), "{audit}");
    assert!(audit.contains("handed"), "{audit}");
    assert!(!audit.contains("sk-"));
}
