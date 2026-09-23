//! Milestone 2: connector install, credential names, and the approval queue.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_connector_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../connectors/example")
}

struct Fixture {
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = tmp("pkg");
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(status.success());
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

    fn run_with(&self, args: &[&str], key: &str, value: &str) -> std::process::Output {
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("ROOT_DIR", &self.root_dir)
            .env(key, value)
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
        let mut server = Self {
            child,
            stdin,
            stdout,
        };
        server.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "clientInfo": { "name": "codex", "version": "1" } }
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
        assert!(
            self.stdout.read_line(&mut line).unwrap() > 0,
            "closed stdout"
        );
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, name: &str) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": name, "arguments": {} }
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

#[test]
fn connector_install_bind_and_approval_are_separate() {
    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let manifest = example_dir().join("manifest.json");
    let package = example_dir();
    let installed = fixture.json(&[
        "connector",
        "install",
        manifest.to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(installed["id"], "example.echo");
    assert_eq!(installed["enabled"], false);

    let secret = fixture.run(&[
        "connector",
        "install",
        &write_manifest(&fixture, "secret", "sk-live-abc123"),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        !secret.status.success(),
        "{}",
        String::from_utf8_lossy(&secret.stderr)
    );

    let inspected = fixture.json(&["connector", "inspect", "example.echo", "--json"]);
    assert_eq!(inspected["intact"], true);
    assert_eq!(inspected["credentials"][0]["bound"], false);
    assert_eq!(inspected["network"], serde_json::json!(["none"]));

    let unbound = fixture.run(&[
        "connector",
        "auth",
        "bind",
        "example.echo",
        "--name",
        "EXAMPLE_TOKEN",
    ]);
    assert!(!unbound.status.success());

    let secret_value = "sk-live-abc123";
    let bound = fixture.run_with(
        &[
            "connector",
            "auth",
            "bind",
            "example.echo",
            "--name",
            "EXAMPLE_TOKEN",
            "--json",
        ],
        "EXAMPLE_TOKEN",
        secret_value,
    );
    assert!(
        bound.status.success(),
        "{}",
        String::from_utf8_lossy(&bound.stderr)
    );
    let index = std::fs::read_to_string(fixture.root_dir.join("connectors/index.json")).unwrap();
    let audit = std::fs::read_to_string(fixture.root_dir.join("connectors/audit.jsonl")).unwrap();
    assert!(!index.contains(secret_value), "{index}");
    assert!(!audit.contains(secret_value), "{audit}");

    fixture.json(&["connector", "enable", "example.echo", "--json"]);
    let caps = fixture.json(&["capability", "list", "--json"]);
    assert!(caps
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "example.echo.say"));

    let mut server = Server::start(&fixture);
    server.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}
    }));
    let listed = server.read();
    let names = listed["result"]["tools"].to_string();
    assert!(names.contains("example.echo.say"), "{names}");
    let said = server.call("example.echo.say");
    assert_eq!(said["result"]["isError"], false, "{said}");
    assert!(said.to_string().contains("echo"), "{said}");

    let gated = server.call("example.echo.store");
    assert_eq!(gated["result"]["isError"], true, "{gated}");
    let text = gated["result"]["content"][0]["text"].as_str().unwrap();
    let approval = text.split_whitespace().last().unwrap();
    assert!(approval.starts_with("root_appr_"), "{text}");
    drop(server);

    fixture.json(&["approval", "approve", approval, "--json"]);
    let mut server = Server::start(&fixture);
    let stored = server.call("example.echo.store");
    assert_eq!(stored["result"]["isError"], false, "{stored}");
    assert!(stored.to_string().contains("echo"), "{stored}");
}

fn write_manifest(fixture: &Fixture, name: &str, description: &str) -> String {
    let path = fixture.root_dir.join(format!("{name}.json"));
    let mut value: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(example_dir().join("manifest.json")).unwrap(),
    )
    .unwrap();
    value["id"] = serde_json::json!(format!("example.{name}"));
    value["tools"][0]["name"] = serde_json::json!(format!("example.{name}.say"));
    value["tools"][0]["description"] = serde_json::json!(description);
    value["tools"][1]["name"] = serde_json::json!(format!("example.{name}.store"));
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    path.display().to_string()
}
