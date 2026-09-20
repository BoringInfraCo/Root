//! Sprint 009 integration: cross-agent continuity (Codex -> Root -> Claude).
//!
//! This exercises the root `mcp serve` stdio surface with two synthetic
//! harness identities. It never invokes the real Codex or Claude binaries.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_cross_agent_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("README.md"), b"# campfire\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &[
                "-c",
                "user.email=root@example.com",
                "-c",
                "user.name=Root",
                "commit",
                "-q",
                "-m",
                "initial",
            ],
        );
        Self {
            base,
            repo,
            root_dir,
        }
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
            "command {:?} failed: stderr={} stdout={}",
            args,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON for {:?}: {}\nstdout={}",
                args,
                e,
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn git(repo: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for cross-agent tests");
    assert!(status.success(), "git {:?} failed", args);
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    client_name: &'static str,
}

impl Server {
    fn start(fixture: &Fixture, client_name: &'static str) -> Self {
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
        Self {
            child,
            stdin,
            stdout,
            client_name,
        }
    }

    fn send(&mut self, message: &serde_json::Value) {
        writeln!(self.stdin, "{}", message).unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).unwrap();
        assert!(read > 0, "mcp server closed stdout unexpectedly");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("invalid MCP response: {e}\nline={line}"))
    }

    fn request(&mut self, id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.read()
    }

    fn initialize(&mut self) -> serde_json::Value {
        let response = self.request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": self.client_name, "version": "1.0" },
            }),
        );
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }));
        response
    }

    fn call(&mut self, id: i64, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        self.request(
            id,
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )["result"]
            .clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn codex_state_reaches_claude_without_copying_a_transcript() {
    let fixture = Fixture::new("handoff");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement workspace invitations", "--json"]);

    // Session 1: Codex records decisions, findings, and a checkpoint.
    let checkpoint_id = {
        let mut codex = Server::start(&fixture, "codex");
        let init = codex.initialize();
        assert_eq!(init["result"]["serverInfo"]["name"], "root");

        let decision = codex.call(
            2,
            "work.record_decision",
            serde_json::json!({
                "statement": "Invitations expire after 24h",
                "rationale": "security",
            }),
        );
        assert_eq!(decision["isError"], false);

        let finding = codex.call(
            3,
            "work.record_finding",
            serde_json::json!({
                "statement": "Invite consumption fails inside the membership transaction",
                "evidence_ref": "integration test output",
            }),
        );
        assert_eq!(finding["isError"], false);

        let checkpoint = codex.call(
            4,
            "continuity.checkpoint",
            serde_json::json!({ "message": "Endpoint implemented; transaction test failing" }),
        );
        assert_eq!(checkpoint["isError"], false);
        checkpoint["structuredContent"]["checkpoint"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // Session 2: Claude resumes and produces a handoff to itself.
    let mut claude = Server::start(&fixture, "claude");
    let init = claude.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "root");

    let resume = claude.call(2, "continuity.resume", serde_json::json!({}));
    assert_eq!(resume["isError"], false);
    let resume = &resume["structuredContent"]["resume"];
    assert_eq!(
        resume["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert_eq!(resume["checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(resume["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(
        resume["decisions"][0]["statement"],
        "Invitations expire after 24h"
    );
    assert_eq!(resume["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        resume["findings"][0]["statement"],
        "Invite consumption fails inside the membership transaction"
    );

    let handoff = claude.call(
        3,
        "continuity.handoff",
        serde_json::json!({ "to": "claude" }),
    );
    assert_eq!(handoff["isError"], false);
    let rendered = handoff["structuredContent"]["rendered"]
        .as_str()
        .unwrap()
        .to_string();
    let handoff = &handoff["structuredContent"]["handoff"];
    assert_eq!(handoff["from"], "codex");
    assert_eq!(handoff["to"], "claude");
    assert_eq!(handoff["checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(
        handoff["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert!(handoff["instructions"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("checkpoint"));
    assert!(handoff["suggested_continuation"][0]
        .as_str()
        .unwrap()
        .starts_with("Suggestion (not verified):"));
    assert!(rendered.contains("Handoff"));
    assert!(rendered.contains("Invitations expire after 24h"));
}
