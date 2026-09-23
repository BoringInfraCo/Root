//! Read-only finance: fixture transactions, receipts, and reconcile.
//! There is no transfer tool and no card number.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_finance_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

fn package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../connectors/finance.local")
}

struct Fixture {
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = tmp("books");
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

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("ROOT_DIR", &self.root_dir)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.run(args);
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

    fn call(&mut self, name: &str) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":name,"arguments":{}}
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

fn content(response: &serde_json::Value) -> serde_json::Value {
    let text = response["result"]["structuredContent"]["content"]
        .as_str()
        .unwrap_or("");
    serde_json::from_str(text).unwrap_or_else(|_| panic!("{response}"))
}

#[test]
fn fixture_reconciles_without_a_transfer() {
    let refused = Fixture::new();
    refused.json(&["workspace", "init", "--json"]);
    let bad_manifest = tmp("bad_manifest");
    std::fs::create_dir_all(&bad_manifest).unwrap();
    std::fs::write(
        bad_manifest.join("manifest.json"),
        r#"{
  "schema": 1,
  "id": "finance.local",
  "version": "0.1.0",
  "publisher": "boring-infra",
  "executable": "finance-connector",
  "executable_sha256": "476b6e5ab25f990235365daab4d92eb485173bc69d4dc48d52446b8e83db7908",
  "tools": [{
    "name": "finance.local.transfer",
    "description": "nope",
    "risk": "destructive",
    "input_schema": { "type": "object", "properties": {} },
    "idempotent": false
  }],
  "events": [],
  "credentials": [],
  "network": ["none"],
  "filesystem": ["none"],
  "limits": {"timeout_ms": 2000, "max_output_bytes": 4096, "max_calls": 30, "window_seconds": 60}
}"#,
    )
    .unwrap();
    let denied = refused.run(&[
        "connector",
        "install",
        bad_manifest.join("manifest.json").to_str().unwrap(),
        "--package",
        bad_manifest.to_str().unwrap(),
        "--json",
    ]);
    assert!(!denied.status.success(), "{denied:?}");
    let denied_text = format!(
        "{}{}",
        String::from_utf8_lossy(&denied.stdout),
        String::from_utf8_lossy(&denied.stderr)
    );
    assert!(denied_text.contains("finance"), "{denied_text}");
    assert!(!refused.root_dir.join("connectors").exists());
    assert!(!refused.root_dir.join("finance").exists());

    let pan = Fixture::new();
    pan.json(&["workspace", "init", "--json"]);
    let pan_dir = tmp("pan_package");
    copy_dir(&package_dir(), &pan_dir);
    std::fs::write(
        pan_dir.join("transactions.json"),
        r#"{"currency":"USD","transactions":[{"id":"t1","posted_on":"2026-01-15","payee":"4111111111111111","amount_cents":1840,"category":"groceries"}]}"#,
    )
    .unwrap();
    let pan_install = pan.run(&[
        "connector",
        "install",
        pan_dir.join("manifest.json").to_str().unwrap(),
        "--package",
        pan_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(!pan_install.status.success(), "{pan_install:?}");
    let pan_text = format!(
        "{}{}",
        String::from_utf8_lossy(&pan_install.stdout),
        String::from_utf8_lossy(&pan_install.stderr)
    );
    assert!(pan_text.contains("card or bank"), "{pan_text}");
    assert!(!pan_text.contains("4111111111111111"), "{pan_text}");
    assert!(!pan.root_dir.join("connectors").exists());
    assert!(!pan.root_dir.join("finance").exists());

    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let package = package_dir();
    fixture.json(&[
        "connector",
        "install",
        package.join("manifest.json").to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    fixture.json(&["connector", "enable", "finance.local", "--json"]);
    assert!(!fixture.root_dir.join("finance").exists());

    let mut server = Server::start(&fixture);
    let transactions = server.call("finance.local.transactions");
    assert_eq!(transactions["result"]["isError"], false, "{transactions}");
    let transactions = content(&transactions);
    assert_eq!(transactions["currency"], "USD");
    assert_eq!(transactions["transactions"].as_array().unwrap().len(), 3);
    assert_eq!(transactions["transactions"][0]["category"], "groceries");
    assert_eq!(transactions["transactions"][1]["payee"], "grace transit");
    assert_eq!(transactions["transactions"][2]["amount_cents"], 990);

    let receipts = content(&server.call("finance.local.receipts"));
    assert_eq!(receipts["receipts"].as_array().unwrap().len(), 2);

    let report = server.call("finance.local.reconcile");
    assert!(!report.to_string().contains("root_appr_"), "{report}");
    assert!(!report.to_string().contains("transfer"), "{report}");
    let report = content(&report);
    assert_eq!(report["matched"].as_array().unwrap().len(), 1);
    assert_eq!(report["matched"][0]["transaction_id"], "t1");
    assert_eq!(report["matched"][0]["receipt_id"], "r1");
    let unmatched_tx: Vec<_> = report["unmatched_transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(unmatched_tx, ["t2", "t3"]);
    assert_eq!(report["unmatched_receipts"][0], "r2");
    assert!(!fixture.root_dir.join("finance").exists());
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}
