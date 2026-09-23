//! Milestone 5: browser observe and act are separate grants. Downloads wait
//! for elevated approval. The fixture does not attach to a desktop.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_browser_{name}_{}_{}",
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
        let base = tmp("page");
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

    fn call(&mut self, name: &str, target: &str) -> serde_json::Value {
        self.send(&serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":name,"arguments":{"target":target}}
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
fn observe_and_act_grants_expire_and_downloads_wait() {
    let fixture = Fixture::new();
    fixture.json(&["workspace", "init", "--json"]);
    let package =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../connectors/computer.browser");
    fixture.json(&[
        "connector",
        "install",
        package.join("manifest.json").to_str().unwrap(),
        "--package",
        package.to_str().unwrap(),
        "--json",
    ]);
    fixture.json(&["connector", "enable", "computer.browser", "--json"]);

    let mut server = Server::start(&fixture);
    let denied = server.call("computer.browser.observe", "https://example.com/inbox");
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    assert!(denied.to_string().contains("grant required"), "{denied}");
    assert!(!denied.to_string().contains("observed"));

    fixture.json(&[
        "computer",
        "grant",
        "observe",
        "--target",
        "https://example.com",
        "--minutes",
        "15",
        "--json",
    ]);
    let observed = server.call("computer.browser.observe", "https://example.com/inbox");
    assert_eq!(observed["result"]["isError"], false, "{observed}");
    assert!(observed.to_string().contains("observed"), "{observed}");
    let shot = server.call("computer.browser.screenshot", "https://example.com/inbox");
    assert!(shot.to_string().contains("screenshot"), "{shot}");
    let other = server.call("computer.browser.observe", "https://evil.test/inbox");
    assert!(other.to_string().contains("grant required"), "{other}");
    let click = server.call("computer.browser.click", "https://example.com/inbox");
    assert!(click.to_string().contains("grant required"), "{click}");
    assert!(!click.to_string().contains("clicked"));
    let early_download = server.call("computer.browser.download", "https://example.com/file");
    assert_eq!(
        early_download["result"]["isError"], true,
        "{early_download}"
    );
    assert!(
        early_download.to_string().contains("grant required"),
        "{early_download}"
    );
    assert!(
        !early_download.to_string().contains("download recorded"),
        "{early_download}"
    );
    assert!(
        !early_download.to_string().contains("root_appr_"),
        "{early_download}"
    );

    fixture.json(&[
        "computer",
        "grant",
        "act",
        "--target",
        "https://example.com",
        "--minutes",
        "15",
        "--json",
    ]);
    let clicked = server.call("computer.browser.click", "https://example.com/inbox");
    assert_eq!(clicked["result"]["isError"], false, "{clicked}");
    assert!(clicked.to_string().contains("clicked"), "{clicked}");

    let download = server.call("computer.browser.download", "https://example.com/file");
    assert_eq!(download["result"]["isError"], true, "{download}");
    let text = download["result"]["content"][0]["text"].as_str().unwrap();
    let approval = text.split_whitespace().last().unwrap();
    assert!(approval.starts_with("root_appr_"), "{text}");
    drop(server);

    fixture.json(&["approval", "approve", approval, "--json"]);
    let mut server = Server::start(&fixture);
    let allowed = server.call("computer.browser.download", "https://example.com/file");
    assert_eq!(allowed["result"]["isError"], false, "{allowed}");
    assert!(
        allowed.to_string().contains("download recorded"),
        "{allowed}"
    );
    assert!(!allowed.to_string().contains("http://"));

    let events = fixture.json(&["event", "list", "--json"]);
    let selectors: Vec<_> = events
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["selector"].as_str())
        .collect();
    assert!(
        selectors.contains(&"computer.browser.observed"),
        "{selectors:?}"
    );
    assert!(
        selectors.contains(&"computer.browser.acted"),
        "{selectors:?}"
    );
    assert!(
        selectors.contains(&"computer.browser.elevated"),
        "{selectors:?}"
    );

    let grants = std::fs::read_to_string(fixture.root_dir.join("computer/grants.json")).unwrap();
    let mut grants: serde_json::Value = serde_json::from_str(&grants).unwrap();
    for grant in grants["grants"].as_array_mut().unwrap() {
        grant["expires_at"] = serde_json::json!("2000-01-01T00:00:00Z");
    }
    std::fs::write(
        fixture.root_dir.join("computer/grants.json"),
        serde_json::to_vec_pretty(&grants).unwrap(),
    )
    .unwrap();
    // Both grants covered this URL. After expiry, observe must stop.
    let expired = server.call("computer.browser.observe", "https://example.com/inbox");
    assert_eq!(expired["result"]["isError"], true, "{expired}");
    assert!(expired.to_string().contains("grant required"), "{expired}");
    assert!(!expired.to_string().contains("observed"), "{expired}");
}
