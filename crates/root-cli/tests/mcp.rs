//! Sprint 008 integration: the local MCP stdio server.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_mcp_{name}_{}_{}",
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
        .expect("git must be available for mcp tests");
    assert!(status.success(), "git {:?} failed", args);
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
        Self {
            child,
            stdin,
            stdout,
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

    fn notify(&mut self, method: &str) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        }));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_stdio_handshake_and_tools() {
    let fixture = Fixture::new("serve");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement MCP", "--json"]);

    let mut server = Server::start(&fixture);

    let init = server.request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "codex", "version": "1.0" },
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init["result"]["serverInfo"]["name"], "root");
    assert_eq!(
        init["result"]["capabilities"]["tools"],
        serde_json::json!({})
    );

    server.notify("notifications/initialized");
    let pong = server.request(2, "ping", serde_json::json!({}));
    assert_eq!(pong["result"], serde_json::json!({}));

    let list = server.request(3, "tools/list", serde_json::json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"workspace.status"));
    assert!(names.contains(&"work.record_decision"));

    let record = server.request(
        4,
        "tools/call",
        serde_json::json!({
            "name": "work.record_decision",
            "arguments": { "statement": "MCP records agent decisions" },
        }),
    );
    assert_eq!(record["result"]["isError"], false);
    assert!(
        record["result"]["structuredContent"]["decision"]["provenance_id"]
            .as_str()
            .is_some()
    );

    let status = server.request(
        5,
        "tools/call",
        serde_json::json!({ "name": "workspace.status", "arguments": {} }),
    );
    assert_eq!(status["result"]["isError"], false);
    assert_eq!(
        status["result"]["structuredContent"]["workspace"]["name"],
        "campfire"
    );
    assert_eq!(
        status["result"]["structuredContent"]["counts"]["decisions"],
        1
    );
}

#[test]
fn mcp_status_reports_workspace_capabilities_and_tools() {
    let fixture = Fixture::new("status");
    fixture.json(&["workspace", "init", "--json"]);

    let status = fixture.json(&["mcp", "status", "--json"]);
    assert_eq!(status["success"], true);
    assert_eq!(status["workspace"]["name"], "campfire");
    assert_eq!(status["policy_source"], "default");
    assert_eq!(status["protocol_version"], "2024-11-05");
    assert_eq!(status["capabilities"]["read"], "allow");
    assert_eq!(status["capabilities"]["record"], "allow");
    assert_eq!(status["capabilities"]["checkpoint"], "allow");
    assert_eq!(status["capabilities"]["environment_verify"], "allow");
    let tools = status["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool == "workspace.status"));
    assert!(tools.iter().any(|tool| tool == "continuity.resume"));

    std::fs::write(
        fixture.root_dir.join("mcp.toml"),
        "[capabilities]\nrecord = \"deny\"\n",
    )
    .unwrap();
    let configured = fixture.json(&["mcp", "status", "--json"]);
    assert_eq!(configured["policy_source"], "configured");
    assert_eq!(configured["capabilities"]["record"], "deny");
    assert_eq!(configured["capabilities"]["read"], "allow");
}

#[test]
fn mcp_status_without_workspace_is_null() {
    let fixture = Fixture::new("noworkspace");
    let status = fixture.json(&["mcp", "status", "--json"]);
    assert_eq!(status["success"], true);
    assert!(status["workspace"].is_null());
    assert_eq!(status["policy_source"], "default");
}
