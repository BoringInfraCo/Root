//! Provider-neutral messages: a local fixture, not a carrier.
//! Threads and read run immediately. Send waits for approval.
//! An unknown recipient is labeled on that same queue.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_messages_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Fixture {
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = tmp("sms");
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

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start(fixture: &Fixture) -> Self {
        let mut child = Command::new(root_bin())
            .args(["mcp", "serve"])
            .current_dir(&fixture.repo)
            .env("ROOT_DIR", &fixture.root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Self {
            child,
            stdin,
            stdout,
        };
        server.send(&serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"codex","version":"1"}}
        }));
        let init = server.read();
        assert_eq!(init["result"]["serverInfo"]["name"], "root", "{init}");
        server.send(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        server
    }

    fn send(&mut self, message: &serde_json::Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> serde_json::Value {
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).unwrap() > 0);
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }));
        self.read()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn approval_id(response: &serde_json::Value) -> String {
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    text.split_whitespace()
        .last()
        .unwrap()
        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .to_string()
}

#[test]
fn threads_are_immediate_and_unknown_recipients_wait() {
    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../connectors/messages.local");
    fixture.json(&[
        "connector",
        "install",
        package.join("manifest.json").to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    fixture.json(&["connector", "enable", "messages.local", "--json"]);

    let first = fixture.json(&["event", "ingest", "messages.local", "--json"]);
    assert_eq!(first["created"], 1);
    assert_eq!(first["deliveries"], 0);
    let again = fixture.json(&["event", "ingest", "messages.local", "--json"]);
    assert_eq!(again["created"], 0);
    assert_eq!(again["duplicate"], 1);
    let events = fixture.json(&["event", "list", "--json"]);
    assert_eq!(events[0]["selector"], "message.received");
    assert_eq!(events[0]["status"], "recorded");

    let mut server = Server::start(&fixture);
    let threads = server.call("messages.local.threads", serde_json::json!({}));
    assert_eq!(threads["result"]["isError"], false, "{threads}");
    assert!(threads.to_string().contains("ada"), "{threads}");
    let read = server.call("messages.local.read", serde_json::json!({"thread": "t1"}));
    assert!(read.to_string().contains("Hello from ada"), "{read}");

    let draft = server.call(
        "messages.local.draft",
        serde_json::json!({"recipient": "ada", "body": "hello"}),
    );
    assert_eq!(draft["result"]["isError"], true, "{draft}");
    assert!(draft.to_string().contains("approval required"), "{draft}");
    assert!(!draft.to_string().contains("unknown recipient"), "{draft}");
    assert!(!draft.to_string().contains("draft stored"), "{draft}");

    let unknown_draft = server.call(
        "messages.local.draft",
        serde_json::json!({"recipient": "sam", "body": "hello"}),
    );
    assert!(
        unknown_draft.to_string().contains("unknown recipient"),
        "{unknown_draft}"
    );
    assert!(
        !unknown_draft.to_string().contains("draft stored"),
        "{unknown_draft}"
    );

    let send = server.call(
        "messages.local.send",
        serde_json::json!({"recipient": "ada", "body": "hello"}),
    );
    assert!(send.to_string().contains("approval required"), "{send}");
    assert!(!send.to_string().contains("unknown recipient"), "{send}");
    assert!(!send.to_string().contains("send recorded"), "{send}");
    let known_approval = approval_id(&send);

    let unknown_send = server.call(
        "messages.local.send",
        serde_json::json!({"recipient": "sam", "body": "hello"}),
    );
    assert!(
        unknown_send.to_string().contains("unknown recipient"),
        "{unknown_send}"
    );
    assert!(
        !unknown_send.to_string().contains("send recorded"),
        "{unknown_send}"
    );
    let unknown_approval = approval_id(&unknown_send);

    let phone = server.call(
        "messages.local.send",
        serde_json::json!({"recipient": "+15551212", "body": "hello"}),
    );
    assert_eq!(phone["result"]["isError"], true, "{phone}");
    assert!(phone.to_string().contains("display name"), "{phone}");
    assert!(!phone.to_string().contains("root_appr_"), "{phone}");
    assert!(!phone.to_string().contains("send recorded"), "{phone}");
    drop(server);

    fixture.json(&["approval", "approve", &known_approval, "--json"]);
    fixture.json(&["approval", "approve", &unknown_approval, "--json"]);
    let mut server = Server::start(&fixture);
    let sent = server.call(
        "messages.local.send",
        serde_json::json!({"recipient": "ada", "body": "hello"}),
    );
    assert_eq!(sent["result"]["isError"], false, "{sent}");
    assert!(sent.to_string().contains("send recorded locally"), "{sent}");
    assert!(!sent.to_string().contains("delivered"), "{sent}");
    let recorded = server.call(
        "messages.local.send",
        serde_json::json!({"recipient": "sam", "body": "hello"}),
    );
    assert_eq!(recorded["result"]["isError"], false, "{recorded}");
    assert!(
        recorded.to_string().contains("send recorded locally"),
        "{recorded}"
    );
    assert!(!recorded.to_string().contains('+'), "{recorded}");

    let events = fixture.json(&["event", "list", "--json"]);
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["status"], "recorded");
}
