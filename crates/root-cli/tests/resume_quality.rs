//! Sprint 014 §3: deterministic resume-quality eval loop (LOCAL, deterministic).
//!
//! This is *not* a product and *not* an LLM judge. It measures whether a
//! successor agent that receives ONLY `root resume --with <target> --json` (plus
//! the rendered text and the repo at the recorded HEAD) can continue correctly.
//!
//! Protocol (§3.1): Agent A captures the ACTUAL source harness environment
//! (`root agent capture --from <from>`), records goal/decisions/findings/
//! artifacts through the isolated `root mcp serve` stdio surface under the
//! real source identity (codex or claude — never a synthetic name; real
//! harness binaries are never invoked), checkpoints (embedding the captured
//! environment), the session is killed (new process / no shared memory), and
//! Agent B resumes from the package alone. Each pair therefore carries a
//! different provenance agent and a different checkpointed canonical
//! environment; `mapping_available` and every mapping step must succeed in
//! every run.
//!
//! B's structured continuation attempt is derived *deterministically* from
//! the package alone via an independent parse (`parse_package_view`), so the
//! eval is reproducible and JSON-assertable. Ground truth lives ONLY in the
//! fixture brief: the scorer compares the attempt against the brief (M1–M5)
//! and the attempt against package reality + the fixture filesystem (M6) —
//! never a field echoed from the package by the same derivation that built
//! the attempt. A sibling control test (`negative_controls_flip_each_metric`)
//! corrupts the attempt one metric at a time and asserts exactly that metric
//! flips, so a tautological/unfailable metric breaks the gate. Metrics M1–M6
//! and their pass conditions follow SPRINT_014 §3.2 exactly:
//!   M1 goal understood        attempt goal == fixture goal
//!   M2 no repeated work       next_action not in fixture completed set
//!   M3 decisions respected    attempt constraints cover the fixture's active
//!                             decisions; no contradiction marker
//!   M4 artifacts found        attempt artifact refs cover the fixture's
//!                             required artifacts (within the 20-cap)
//!   M5 knew next              next_action matches the fixture-accepted set
//!   M6 no hallucination       claimed ids exist in the package B received;
//!                             claimed paths/HEADs exist in the fixture
//!
//! Matrix: 3 starter briefs (§3.3) × harness pairs (codex→claude, claude→codex)
//! × drift variants (clean, head-changed, artifact-missing, env-changed).
//!
//! Hermetic child-env isolation only (HOME/CODEX_HOME/XDG_*/OPENCODE_CONFIG_DIR/
//! CLAUDE_CONFIG_DIR/ROOT_DIR/TMPDIR + fake agent shims on PATH). No process-env
//! mutation; a file-local mutex serializes the tests in this binary.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static TEST_MUTEX: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

fn unique_base(name: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "root-resume-quality-{name}-{}-{id}",
        std::process::id()
    ))
}

// ---------------------------------------------------------------------------
// Fixture: isolated repo + ROOT_DIR + shims.
// ---------------------------------------------------------------------------

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    root_dir: PathBuf,
    home: PathBuf,
    codex_home: PathBuf,
    xdg: PathBuf,
    oc: PathBuf,
    cl: PathBuf,
    tmpdir: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = unique_base(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        let home = base.join("home");
        let codex_home = base.join("codex");
        let xdg = base.join("xdg");
        let oc = base.join("oc");
        let cl = base.join("cl");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for dir in [
            &base,
            &repo,
            &root_dir,
            &home,
            &codex_home,
            &xdg,
            &oc,
            &cl,
            &tmpdir,
            &bin,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
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
        // A complete, observed environment: Rootfile + root.lock digests are
        // recorded by the checkpoint and compared by drift.
        std::fs::write(root_dir.join("Rootfile"), b"[packages]\n").unwrap();
        std::fs::write(root_dir.join("root.lock"), b"lock-one\n").unwrap();

        let fixture = Self {
            base,
            repo,
            root_dir,
            home,
            codex_home,
            xdg,
            oc,
            cl,
            tmpdir,
            bin,
        };
        fixture.write_shim("codex", "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n");
        fixture.write_shim("opencode", "#!/bin/sh\nprintf '1.18.27\\n'\n");
        fixture.write_shim("claude", "#!/bin/sh\nprintf '2.1.260\\n'\n");
        fixture
    }

    fn write_shim(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn child_env(&self, cmd: &mut Command) {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(dirs).unwrap();
        cmd.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("XDG_DATA_HOME", &self.xdg)
            .env("OPENCODE_CONFIG_DIR", &self.oc)
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("CLAUDE_CONFIG_DIR", &self.cl)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path);
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(root_bin());
        cmd.args(args);
        self.child_env(&mut cmd);
        cmd.output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "command {args:?} failed: status={:?} stderr={} stdout={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON for {args:?}: {e}\nstdout={}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    fn write_repo_file(&self, uri: &str, body: &[u8]) {
        let path = self.repo.join(uri);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn remove_repo_file(&self, uri: &str) {
        let path = self.repo.join(uri);
        std::fs::remove_file(path).unwrap();
    }

    fn git_commit_all(&self, message: &str) {
        git(&self.repo, &["add", "-A"]);
        git(
            &self.repo,
            &[
                "-c",
                "user.email=root@example.com",
                "-c",
                "user.name=Root",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
    }

    /// True when `sha` names a commit that exists in the fixture repository.
    fn commit_exists(&self, sha: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for resume-quality tests");
    assert!(status.success(), "git {args:?} failed");
}

// ---------------------------------------------------------------------------
// Minimal MCP stdio client: real source-harness identity (never a real binary).
// ---------------------------------------------------------------------------

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Mcp {
    fn start(fixture: &Fixture, client_name: &str) -> Self {
        let mut cmd = Command::new(root_bin());
        cmd.args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        fixture.child_env(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": client_name, "version": "1.0" },
            },
        }));
        let init = server.read();
        assert_eq!(init["result"]["serverInfo"]["name"], "root");
        server.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }));
        server.next_id = 2;
        server
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).unwrap();
        assert!(read > 0, "mcp server closed stdout unexpectedly");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("invalid MCP response: {e}\n{line}"))
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }));
        let response = self.read();
        assert!(
            response.get("error").is_none(),
            "MCP call {name} errored: {response}"
        );
        let result = response["result"].clone();
        assert_ne!(
            result["isError"],
            json!(true),
            "MCP {name} isError: {result}"
        );
        result
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        // "terminate": kill the MCP child; Agent B runs in a fresh process and
        // receives only the package. No shared memory survives this.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Task briefs (§3.3) and the deterministic attempt/score model.
// ---------------------------------------------------------------------------

struct Brief {
    id: &'static str,
    goal: &'static str,
    decisions: &'static [&'static str],
    findings: &'static [&'static str],
    evidence: &'static [&'static str],
    artifacts: &'static [&'static str],
    /// Substrings that mark work already recorded done (M2 must not redo them).
    completed: &'static [&'static str],
    /// Substrings accepted as the correct next action (M5).
    accepted_next: &'static [&'static str],
    /// Pre-registered strings that would contradict an active decision (M3).
    contradictions: &'static [&'static str],
    checkpoint_message: &'static str,
}

fn briefs() -> Vec<Brief> {
    vec![
        Brief {
            id: "pkce-login-slice",
            goal: "Implement the PKCE login slice",
            decisions: &[
                "PKCE uses the S256 code challenge",
                "Tokens live in the keychain, never in work state",
            ],
            findings: &["Refresh-token test is green"],
            evidence: &["cargo test refresh_token"],
            artifacts: &["src/auth/pkce.ts", "src/auth/routes.ts"],
            completed: &["slice implemented"],
            accepted_next: &["Refresh-token test is green"],
            contradictions: &[
                "tokens in work state",
                "store tokens in state",
                "plaintext token in state",
                "skip PKCE",
            ],
            checkpoint_message: "pkce-login-slice slice implemented",
        },
        Brief {
            id: "invitation-expiry",
            goal: "Implement workspace invitations",
            decisions: &["Invitations expire after 24h"],
            findings: &["Invite consumption fails inside the membership transaction"],
            evidence: &["integration test output"],
            artifacts: &["src/invite.ts"],
            completed: &["endpoint implemented"],
            accepted_next: &["Invite consumption fails inside the membership transaction"],
            contradictions: &[
                "invitations never expire",
                "expiry removed",
                "expire after 48h",
            ],
            checkpoint_message: "invitation-expiry endpoint implemented",
        },
        Brief {
            id: "drifted-workspace",
            goal: "Implement workspace invitations",
            decisions: &[
                "Invitations expire after 24h",
                "Invitation tests run under the continuity fixture",
            ],
            findings: &["Invite consumption fails inside the membership transaction"],
            evidence: &["integration test output"],
            artifacts: &["src/invite.ts", "src/expiry.ts"],
            completed: &["endpoint implemented"],
            accepted_next: &["Invite consumption fails inside the membership transaction"],
            contradictions: &["invitations never expire", "delete the expiry test"],
            checkpoint_message: "drifted-workspace endpoint implemented",
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Variant {
    Clean,
    HeadChanged,
    ArtifactMissing,
    EnvChanged,
}

impl Variant {
    fn label(self) -> &'static str {
        match self {
            Variant::Clean => "clean",
            Variant::HeadChanged => "head-changed",
            Variant::ArtifactMissing => "artifact-missing",
            Variant::EnvChanged => "env-changed",
        }
    }

    fn expected_drift_level(self) -> &'static str {
        match self {
            Variant::Clean => "none",
            _ => "warning",
        }
    }

    fn expected_drift_kind(self) -> Option<&'static str> {
        match self {
            Variant::Clean => None,
            Variant::HeadChanged => Some("repository.head"),
            Variant::ArtifactMissing => Some("artifact.missing"),
            Variant::EnvChanged => Some("environment.lock"),
        }
    }

    fn all() -> [Variant; 4] {
        [
            Variant::Clean,
            Variant::HeadChanged,
            Variant::ArtifactMissing,
            Variant::EnvChanged,
        ]
    }
}

/// Structured continuation attempt emitted by Agent B: a deterministic
/// reading of the package ONLY (B never sees the brief). Cloned when the
/// control test corrupts one field at a time.
#[derive(Debug, Clone)]
struct Attempt {
    goal_restated: String,
    constraints_list: Vec<String>,
    next_action: String,
    artifact_refs: Vec<String>,
    claimed_decision_ids: Vec<String>,
    claimed_artifact_ids: Vec<String>,
    claimed_heads: Vec<String>,
}

fn strip_not_verified(text: &str) -> String {
    text.strip_prefix("Suggestion (not verified): ")
        .unwrap_or(text)
        .to_string()
}

/// What Agent B can SEE: an independent parse of the continuation package.
/// Kept separate from [`score_attempt`], which re-reads the package (and the
/// fixture ground truth) through its own code path, so the scorer never
/// trusts fields copied out of the package by this derivation.
#[derive(Debug, Default)]
struct PackageView {
    goal: Option<String>,
    decision_statements: Vec<String>,
    decision_ids: Vec<String>,
    artifact_uris: Vec<String>,
    artifact_ids: Vec<String>,
    suggestion: Option<String>,
    unresolved_first: Option<String>,
    head: Option<String>,
    drift_heads: Vec<String>,
}

fn parse_package_view(package: &Value) -> PackageView {
    let mut view = PackageView {
        goal: package
            .pointer("/goal/statement")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Default::default()
    };
    if let Some(items) = package.get("decisions").and_then(Value::as_array) {
        for decision in items {
            if let Some(statement) = decision.get("statement").and_then(Value::as_str) {
                view.decision_statements.push(statement.to_string());
            }
            if let Some(id) = decision.get("id").and_then(Value::as_str) {
                view.decision_ids.push(id.to_string());
            }
        }
    }
    if let Some(items) = package.get("artifacts").and_then(Value::as_array) {
        for artifact in items {
            if let Some(uri) = artifact.get("uri").and_then(Value::as_str) {
                view.artifact_uris.push(uri.to_string());
            }
            if let Some(id) = artifact.get("id").and_then(Value::as_str) {
                view.artifact_ids.push(id.to_string());
            }
        }
    }
    view.suggestion = package
        .get("suggested_continuation")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .map(str::to_string);
    view.unresolved_first = package
        .get("unresolved_work")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .map(str::to_string);
    view.head = package
        .pointer("/repository_state/head")
        .and_then(Value::as_str)
        .map(str::to_string);
    // Under head drift the package quotes both shas ("HEAD changed: a -> b").
    // A real successor repeats those shas; M6 verifies they exist.
    if let Some(items) = package.pointer("/drift/items").and_then(Value::as_array) {
        for item in items {
            if item.get("kind").and_then(Value::as_str) == Some("repository.head") {
                if let Some(detail) = item.get("detail").and_then(Value::as_str) {
                    for token in detail.split_whitespace() {
                        let candidate = token.trim_end_matches(['(', ')', ',']);
                        if candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit())
                        {
                            view.drift_heads.push(candidate.to_string());
                        }
                    }
                }
            }
        }
    }
    view
}

/// Build B's attempt from the independent package parse. The brief models
/// WHICH dimensions an attempt must carry (restated goal, honored constraints,
/// next action, artifact refs, claimed ids/heads) but supplies none of the
/// content: content comes solely from what B could see in the package, so
/// scoring attempt-vs-brief genuinely diverges whenever the package diverges
/// from fixture ground truth — and corrupted attempts (control test) fail.
fn derive_attempt(package: &Value) -> Attempt {
    let view = parse_package_view(package);
    let goal_restated = view.goal.unwrap_or_default();
    let constraints_list = view.decision_statements;
    let next_action = view
        .suggestion
        .as_deref()
        .map(strip_not_verified)
        .or(view.unresolved_first)
        .unwrap_or_default();
    let artifact_refs = view.artifact_uris;
    let claimed_decision_ids = view.decision_ids;
    let claimed_artifact_ids = view.artifact_ids;
    let mut claimed_heads = Vec::new();
    if let Some(head) = view.head {
        claimed_heads.push(head);
    }
    claimed_heads.extend(view.drift_heads);

    Attempt {
        goal_restated,
        constraints_list,
        next_action,
        artifact_refs,
        claimed_decision_ids,
        claimed_artifact_ids,
        claimed_heads,
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Debug, Clone, Copy)]
struct Score {
    m1: bool,
    m2: bool,
    m3: bool,
    m4: bool,
    m5: bool,
    m6: bool,
}

impl Score {
    fn all(&self) -> [bool; 6] {
        [self.m1, self.m2, self.m3, self.m4, self.m5, self.m6]
    }
}

fn score_attempt(brief: &Brief, fixture: &Fixture, package: &Value, attempt: &Attempt) -> Score {
    // M1: goal understood — normalized exact match.
    let m1 = normalize(&attempt.goal_restated) == normalize(brief.goal);

    // M2 (inverted): no repeated work.
    let next = normalize(&attempt.next_action);
    let m2 = !brief
        .completed
        .iter()
        .any(|done| next.contains(&normalize(done)));

    // M3: every fixture decision must be honored verbatim in B's constraints
    // list, and no fixture contradiction marker may appear. Compared against
    // `attempt` (B's reading) rather than a package field echoed verbatim, so
    // a corrupted/partial constraints list genuinely fails. No other metric
    // flips with it: the contradiction is checked only against next+constraints.
    let attempt_constraints: Vec<String> = attempt
        .constraints_list
        .iter()
        .map(|statement| normalize(statement))
        .collect();
    let all_present = brief.decisions.iter().all(|required| {
        let required = normalize(required);
        attempt_constraints.iter().any(|found| found == &required)
    });
    let combined = normalize(&format!(
        "{} {}",
        attempt.next_action,
        attempt.constraints_list.join(" ")
    ));
    let contradicts = brief
        .contradictions
        .iter()
        .any(|marker| combined.contains(&normalize(marker)));
    let m3 = all_present && !contradicts;

    // M4: every fixture-required artifact URI must be referenced by B's
    // attempt (within the package's 20-cap). Checked against `attempt`, not a
    // re-parse of the package, so dropping a reference from the attempt fails
    // exactly M4.
    let m4 = brief
        .artifacts
        .iter()
        .all(|required| attempt.artifact_refs.iter().any(|uri| uri == required));

    // M5: next_action matches the fixture-accepted set (label already stripped).
    let m5 = !next.is_empty()
        && brief.accepted_next.iter().any(|accepted| {
            let accepted = normalize(accepted);
            !accepted.is_empty() && (next.contains(&accepted) || accepted.contains(&next))
        });

    // M6 (inverted): no invented ids, paths, or commits.
    let mut m6 = true;
    for path in &attempt.artifact_refs {
        let known = brief.artifacts.iter().any(|uri| uri == path);
        if !known && !fixture.repo.join(path).exists() {
            m6 = false;
        }
    }
    for head in &attempt.claimed_heads {
        if !fixture.commit_exists(head) {
            m6 = false;
        }
    }
    // A claimed decision/artifact id must exist in the package (or fixture).
    for id in &attempt.claimed_decision_ids {
        let in_package = package
            .get("decisions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .any(|d| d.get("id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .unwrap_or(false);
        if !in_package {
            m6 = false;
        }
    }
    for id in &attempt.claimed_artifact_ids {
        let in_package = package
            .get("artifacts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .any(|a| a.get("id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .unwrap_or(false);
        if !in_package {
            m6 = false;
        }
    }

    Score {
        m1,
        m2,
        m3,
        m4,
        m5,
        m6,
    }
}

// ---------------------------------------------------------------------------
// Agent A: record the briefing through the real stdio MCP surface.
// ---------------------------------------------------------------------------

fn record_task_a(fixture: &Fixture, brief: &Brief, from: &str) -> String {
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", brief.goal, "--json"]);
    for uri in brief.artifacts {
        fixture.write_repo_file(uri, format!("export const {uri} = 1;\n").as_bytes());
        fixture.json(&["artifact", "add", uri, "--json"]);
    }
    // Capture the ACTUAL source harness environment FIRST, then checkpoint
    // (smoke test §4): the checkpoint snapshot embeds the captured canonical
    // environment, which is what `resume --with` maps on Agent B. Capturing
    // after would leave the snapshot without an agent mapping.
    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    let capture = fixture.json(&[
        "agent",
        "capture",
        "--from",
        from,
        "--apply",
        "--out",
        ".root/agent.toml",
        "--json",
    ]);
    assert_eq!(
        capture["source_agent"], from,
        "capture must record the real source identity: {capture}"
    );
    assert!(
        fixture.repo.join(".root/agent.toml").exists(),
        "capture --apply must write .root/agent.toml"
    );
    // Provenance comes from the MCP clientInfo name: the session speaks as the
    // real source harness (codex or claude), never a synthetic "agent-a".
    let mut mcp = Mcp::start(fixture, from);
    for decision in brief.decisions {
        mcp.call("work.record_decision", json!({ "statement": decision }));
    }
    for (index, finding) in brief.findings.iter().enumerate() {
        mcp.call(
            "work.record_finding",
            json!({ "statement": finding, "evidence_ref": brief.evidence[index] }),
        );
    }
    let checkpoint = mcp.call(
        "continuity.checkpoint",
        json!({ "message": brief.checkpoint_message }),
    );
    checkpoint
        .pointer("/structuredContent/checkpoint/id")
        .and_then(Value::as_str)
        .expect("checkpoint id")
        .to_string()
}

// ---------------------------------------------------------------------------
// Cross-harness provenance + 7-step assertions for every matrix run.
// ---------------------------------------------------------------------------

const STEP_NAMES: [&str; 7] = [
    "prepare config",
    "map skills/instructions",
    "credential refs",
    "restore workspace",
    "repo drift check",
    "assemble work state",
    "launch continuation",
];

/// The checkpointed provenance/environment and the resume report must all
/// agree on the real `from` identity, and the mapping must be available with
/// a translation grounded in that environment — in EVERY matrix run.
fn assert_cross_harness_signals(
    fixture: &Fixture,
    report: &Value,
    checkpoint_id: &str,
    from: &str,
    to: &str,
) {
    let repository = root_work::Repository::discover(&fixture.repo).unwrap();
    let store = root_work::WorkStore::open_at(&fixture.root_dir, repository).unwrap();

    let checkpoint = store
        .show_checkpoint(checkpoint_id)
        .expect("checkpoint must be readable from the store");
    let provenance_id = checkpoint
        .provenance_id
        .as_deref()
        .expect("checkpoint must carry provenance");
    let provenance = store.provenance(provenance_id).expect("provenance row");
    assert_eq!(
        provenance.agent.as_deref(),
        Some(from),
        "checkpoint provenance must name the real source agent"
    );
    let raw_env_ref = checkpoint
        .agent_env_ref
        .as_deref()
        .expect("capture-before-checkpoint must embed agent_env_ref");
    let summary: Value = serde_json::from_str(raw_env_ref).expect("agent_env_ref is a JSON string");
    assert_eq!(
        summary["adapter"], from,
        "checkpointed environment must be the captured {from} env: {summary}"
    );

    assert_eq!(
        report["source"], from,
        "resume report source must come from checkpoint provenance: {report}"
    );
    assert_eq!(
        report["mapping_available"], true,
        "capture-before-checkpoint must yield a mappable checkpoint: {report}"
    );
    assert_eq!(report["target"], to, "resume target: {report}");
    assert_eq!(
        report["translation"]["from"], from,
        "translation must be grounded in the checkpointed {from} env: {report}"
    );
}

/// All 7 steps must be present in order. Every step must succeed in every
/// variant EXCEPT step 4 (`restore workspace`) under `EnvChanged`, which
/// deliberately rewrites root.lock after the checkpoint so the honest digest
/// comparison fails — that failure is the point of the variant, so it is
/// asserted explicitly rather than excluded from the metric.
fn assert_resume_steps(report: &Value, variant: Variant) {
    let steps = report["steps"].as_array().expect("steps array");
    assert_eq!(
        steps.len(),
        7,
        "resume --with must execute 7 steps: {report}"
    );
    let names: Vec<&str> = steps
        .iter()
        .map(|step| step["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, STEP_NAMES, "seven steps must be ordered: {report}");
    for step in steps {
        let name = step["name"].as_str().unwrap();
        let ok = step["ok"].as_bool().unwrap();
        if name == "restore workspace" && variant == Variant::EnvChanged {
            // Justified exception: the variant rewrote root.lock after the
            // checkpoint, so verify-only restore MUST report the mismatch.
            assert!(
                !ok,
                "env-changed must fail `restore workspace` honestly: {report}"
            );
        } else {
            assert!(ok, "step `{name}` must succeed ({variant:?}): {report}");
        }
    }
}

fn apply_variant(fixture: &Fixture, brief: &Brief, variant: Variant) {
    match variant {
        Variant::Clean => {}
        Variant::HeadChanged => {
            fixture.write_repo_file("drift-note.txt", b"head advanced after the checkpoint\n");
            fixture.git_commit_all("advance head");
        }
        Variant::ArtifactMissing => {
            fixture.remove_repo_file(brief.artifacts[0]);
        }
        Variant::EnvChanged => {
            std::fs::write(fixture.root_dir.join("root.lock"), b"lock-two\n").unwrap();
        }
    }
}

#[test]
fn resume_quality_matrix_meets_the_gate() {
    let _guard = lock_tests();

    let pairs = [("codex", "claude"), ("claude", "codex")];
    let variants = Variant::all();

    let mut runs: Vec<Value> = Vec::new();
    let mut metric_counts = [0usize; 6];
    let mut drift_failures: Vec<String> = Vec::new();
    let mut m6_drift_failures: Vec<String> = Vec::new();

    for brief in briefs() {
        for (from, to) in pairs {
            for variant in variants {
                let name = format!("{}-{from}-{to}-{}", brief.id, variant.label());
                let fixture = Fixture::new(&name);
                let checkpoint_id = record_task_a(&fixture, &brief, from);
                apply_variant(&fixture, &brief, variant);

                // Agent B: fresh process, package-only.
                let report = fixture.json(&["resume", "--with", to, "--json"]);
                assert_cross_harness_signals(&fixture, &report, &checkpoint_id, from, to);
                assert_resume_steps(&report, variant);
                let package = report.get("package").cloned().unwrap_or(Value::Null);
                let rendered_out = fixture.run(&["resume", "--with", to]);
                assert!(
                    rendered_out.status.success(),
                    "rendered resume failed: {}",
                    String::from_utf8_lossy(&rendered_out.stderr)
                );
                let rendered = String::from_utf8_lossy(&rendered_out.stdout).to_string();

                // B receives only the package (+ rendered text + repo HEAD).
                assert!(
                    rendered.contains(brief.goal),
                    "rendered package must carry the goal: {rendered}"
                );
                assert!(
                    rendered.contains("Suggestion (not verified):"),
                    "rendered package must label suggestions not-verified: {rendered}"
                );

                let attempt = derive_attempt(&package);
                let score = score_attempt(&brief, &fixture, &package, &attempt);

                // Drift honesty (§3.1/§3.2): the variant must surface the right
                // level and kind; M6 must still hold under drift.
                let drift_level = package
                    .pointer("/drift/level")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let drift_kinds: Vec<String> = package
                    .pointer("/drift/items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.get("kind").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if drift_level != variant.expected_drift_level() {
                    drift_failures.push(format!(
                        "{name}: expected drift {} got {drift_level}",
                        variant.expected_drift_level()
                    ));
                }
                if let Some(kind) = variant.expected_drift_kind() {
                    if !drift_kinds.iter().any(|k| k == kind) {
                        drift_failures.push(format!("{name}: missing drift kind {kind}"));
                    }
                    if !score.m6 {
                        m6_drift_failures.push(name.clone());
                    }
                }
                if variant == Variant::Clean && !drift_kinds.is_empty() {
                    drift_failures
                        .push(format!("{name}: clean run reported drift {drift_kinds:?}"));
                }

                let flags = score.all();
                for (index, passed) in flags.iter().enumerate() {
                    if *passed {
                        metric_counts[index] += 1;
                    }
                }

                runs.push(json!({
                    "task": brief.id,
                    "pair": format!("{from}->{to}"),
                    "variant": variant.label(),
                    "source": report["source"],
                    "mapping_available": report["mapping_available"],
                    "M1": score.m1,
                    "M2": score.m2,
                    "M3": score.m3,
                    "M4": score.m4,
                    "M5": score.m5,
                    "M6": score.m6,
                    "drift_level": drift_level,
                    "package_ids": {
                        "checkpoint_id": checkpoint_id,
                        "decision_ids": attempt.claimed_decision_ids,
                        "artifact_ids": attempt.claimed_artifact_ids,
                        "next_action": attempt.next_action,
                    },
                }));
            }
        }
    }

    let total = runs.len();
    assert_eq!(total, 3 * 2 * 4, "matrix must be 3x2x4");

    let rate = |index: usize| metric_counts[index] as f64 / total as f64;
    let aggregate = json!({
        "runs": total,
        "M1": rate(0),
        "M2": rate(1),
        "M3": rate(2),
        "M4": rate(3),
        "M5": rate(4),
        "M6": rate(5),
    });
    let report = json!({
        "schema": "root.resume_quality.v1",
        "matrix": { "tasks": 3, "pairs": 2, "variants": 4 },
        "aggregate": aggregate,
        "drift_failures": drift_failures,
        "m6_drift_failures": m6_drift_failures,
        "runs": runs,
    });

    // Emit the aggregate JSON report to the test's temp dir and assert on it.
    let report_dir = unique_base("report");
    std::fs::create_dir_all(&report_dir).unwrap();
    let report_path = report_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let readback: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap())
        .expect("report is valid JSON");

    assert!(
        readback["drift_failures"].as_array().unwrap().is_empty(),
        "drift variant expectations failed: {}",
        readback["drift_failures"]
    );
    assert!(
        readback["m6_drift_failures"].as_array().unwrap().is_empty(),
        "M6 (no hallucination) failed under drift: {}",
        readback["m6_drift_failures"]
    );

    let agg_rate = |key: &str| readback["aggregate"][key].as_f64().unwrap();

    // Gate (SPRINT_014 §3.2): honesty-critical metrics at 100%, the rest ≥ 80%.
    for honesty in ["M1", "M3", "M6"] {
        assert!(
            (agg_rate(honesty) - 1.0).abs() < f64::EPSILON,
            "gate: {honesty} must be 100% (got {}) :: {}",
            agg_rate(honesty),
            readback["runs"]
        );
    }
    for soft in ["M2", "M4", "M5"] {
        assert!(
            agg_rate(soft) >= 0.8,
            "gate: {soft} must be >= 80% (got {}) :: {}",
            agg_rate(soft),
            readback["runs"]
        );
    }

    println!(
        "resume-quality gate (n={total}): M1={:.3} M2={:.3} M3={:.3} M4={:.3} M5={:.3} M6={:.3}",
        agg_rate("M1"),
        agg_rate("M2"),
        agg_rate("M3"),
        agg_rate("M4"),
        agg_rate("M5"),
        agg_rate("M6"),
    );

    let _ = std::fs::remove_dir_all(&report_dir);
}

/// One field of B's attempt is corrupted per control; exactly the targeted
/// metric must flip false, every other metric must stay true (except the one
/// documented M2→M5 collateral). A tautological/unfappable metric cannot
/// satisfy both the passing baseline and its own failing control, so this
/// test breaks whenever a metric degenerates into "always true".
#[test]
fn negative_controls_flip_each_metric() {
    let _guard = lock_tests();

    let brief = briefs()
        .into_iter()
        .find(|b| b.id == "pkce-login-slice")
        .expect("pkce-login-slice brief");
    let fixture = Fixture::new("controls");
    let _checkpoint_id = record_task_a(&fixture, &brief, "claude");
    let report = fixture.json(&["resume", "--with", "codex", "--json"]);
    let package = report.get("package").cloned().unwrap_or(Value::Null);

    let baseline_attempt = derive_attempt(&package);
    let baseline = score_attempt(&brief, &fixture, &package, &baseline_attempt);
    assert!(
        baseline.all().iter().all(|passed| *passed),
        "baseline attempt must pass every metric: {baseline:?}"
    );

    let flags_of = |score: &Score| [score.m1, score.m2, score.m3, score.m4, score.m5, score.m6];
    let assert_flipped = |score: Score, flipped: usize, label: &str, keep: &[bool]| {
        let flags = flags_of(&score);
        assert!(
            !flags[flipped],
            "{label}: M{} must flip false, got {flags:?}",
            flipped + 1
        );
        for (index, expected) in keep.iter().enumerate() {
            assert_eq!(
                flags[index],
                *expected,
                "{label}: M{} must stay {}, got {flags:?}",
                index + 1,
                expected
            );
        }
    };

    // M1: wrong restated goal.
    {
        let mut attempt = baseline_attempt.clone();
        attempt.goal_restated = "Implement a different feature entirely".to_string();
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert_flipped(
            score,
            0,
            "M1 control",
            &[false, true, true, true, true, true],
        );
    }

    // M2: next action redoes completed work. Expected collateral: M5 also
    // fails because the corrupted next no longer matches the accepted set —
    // M5 is "knew next", so a wrong next failing both M2 and M5 is correct.
    {
        let mut attempt = baseline_attempt.clone();
        attempt.next_action = format!(
            "Continue with {} (already recorded done)",
            brief.completed[0]
        );
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert!(
            !score.m2,
            "M2 control: redoing completed work must fail M2: {score:?}"
        );
        assert!(
            !score.m5,
            "M2 control: a next that redoes work cannot also be the accepted next"
        );
        assert!(
            score.m1 && score.m3 && score.m4 && score.m6,
            "M2 control: only M2 (and its M5 collateral) may fail: {score:?}"
        );
    }

    // M3: constraints contradict an active decision.
    {
        let mut attempt = baseline_attempt.clone();
        attempt
            .constraints_list
            .push(brief.contradictions[0].to_string());
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert_flipped(
            score,
            2,
            "M3 control",
            &[true, true, false, true, true, true],
        );
    }

    // M4: a required artifact reference is dropped from the attempt.
    {
        let mut attempt = baseline_attempt.clone();
        let dropped = brief.artifacts[0];
        attempt.artifact_refs.retain(|uri| uri != dropped);
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert_flipped(
            score,
            3,
            "M4 control",
            &[true, true, true, false, true, true],
        );
    }

    // M5: next action is neither in the completed set nor the accepted set.
    {
        let mut attempt = baseline_attempt.clone();
        attempt.next_action =
            "Refactor the settings page and clarify the contract before touching the endpoint"
                .to_string();
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert_flipped(
            score,
            4,
            "M5 control",
            &[true, true, true, true, false, true],
        );
    }

    // M6: fabricated ids, path, and HEAD.
    {
        let mut attempt = baseline_attempt.clone();
        attempt
            .claimed_decision_ids
            .push("root_dec_fabricated".to_string());
        attempt
            .claimed_artifact_ids
            .push("root_art_fabricated".to_string());
        attempt.artifact_refs.push("src/ghost.ts".to_string());
        attempt.claimed_heads.push("f".repeat(40));
        let score = score_attempt(&brief, &fixture, &package, &attempt);
        assert_flipped(
            score,
            5,
            "M6 control",
            &[true, true, true, true, true, false],
        );
    }
}
