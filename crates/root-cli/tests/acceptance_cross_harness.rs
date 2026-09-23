//! Sprint 014 §2: 12-step cross-harness acceptance, encoded as an automated
//! test (Mac A / Claude → Mac B / Codex).
//!
//! This is the deterministic CI proxy for the manual smoke doc
//! `Docs/Release/V0_6_WORKSPACE_SMOKE_TEST.md`. It never invokes a real harness
//! binary and never touches the network: two isolated `ROOT_DIR`s, two project
//! dirs (simulated Mac A / Mac B clones), and fake `claude`/`codex` shims on a
//! private child `PATH`. Only the workspace transfer document + checkpoint id
//! travel from A to B.
//!
//! Steps 1–12 mirror §2. `root mcp serve` carries the harness identity for the
//! provenance-bearing checkpoint (step 9); the CLI defaults a checkpoint's
//! provenance to `human`, which the manual smoke doc records for the
//! `root checkpoint create` form.
//!
//! Hermetic child-env isolation only (no process-env mutation); a file-local
//! mutex serializes tests in this binary.

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
        "root-acceptance-cross-harness-{name}-{}-{id}",
        std::process::id()
    ))
}

// ---------------------------------------------------------------------------
// One isolated machine: repo + ROOT_DIR + harness homes + shims.
// ---------------------------------------------------------------------------

struct Machine {
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

impl Machine {
    /// Scaffold an isolated machine. `clone_from` models Mac B's `git clone` of
    /// Mac A (same history/HEAD); `None` creates Mac A's fresh repository.
    fn new(base: &Path, name: &str, bin: &Path, clone_from: Option<&Path>) -> Self {
        let repo = base.join(name).join("repo");
        let root_dir = base.join(name).join("root");
        let home = base.join(name).join("home");
        let codex_home = base.join(name).join("codex");
        let xdg = base.join(name).join("xdg");
        let oc = base.join(name).join("oc");
        let cl = base.join(name).join("cl");
        let tmpdir = base.join(name).join("tmp");
        for dir in [
            &repo,
            &root_dir,
            &home,
            &codex_home,
            &xdg,
            &oc,
            &cl,
            &tmpdir,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        match clone_from {
            Some(source) => {
                let status = Command::new("git")
                    .args(["clone", "-q"])
                    .arg(source)
                    .arg(&repo)
                    .status()
                    .expect("git must be available for acceptance tests");
                assert!(status.success(), "git clone failed");
            }
            None => {
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
            }
        }
        Self {
            repo,
            root_dir,
            home,
            codex_home,
            xdg,
            oc,
            cl,
            tmpdir,
            bin: bin.to_path_buf(),
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
}

struct Env {
    base: PathBuf,
    a: Machine,
    b: Machine,
    transfer: PathBuf,
}

impl Env {
    fn new(name: &str) -> Self {
        let base = unique_base(name);
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_shim(&bin, "codex", "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n");
        write_shim(&bin, "claude", "#!/bin/sh\nprintf '2.1.260\\n'\n");
        // Fake `nix` satisfies only the availability/experimental-feature probes
        // (as in restore_bind.rs). The lock has zero packages, so restore is a
        // genuine no-op: no package is installed and no real Nix is invoked.
        write_shim(&bin, "nix", "#!/bin/sh\nexit 0\n");
        let a = Machine::new(&base, "mac-a", &bin, None);
        // Mac B is a clone of Mac A (same Git history/HEAD).
        let b = Machine::new(&base, "mac-b", &bin, Some(&a.repo));
        let transfer = base.join("transfer.rootws.json");
        Self {
            base,
            a,
            b,
            transfer,
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_shim(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for acceptance tests");
    assert!(status.success(), "git {args:?} failed");
}

// ---------------------------------------------------------------------------
// MCP stdio client: synthetic harness identity over the real surface.
// ---------------------------------------------------------------------------

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Mcp {
    fn start(machine: &Machine, client_name: &str) -> Self {
        let mut cmd = Command::new(root_bin());
        cmd.args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        machine.child_env(&mut cmd);
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
        assert_eq!(init["result"]["serverInfo"]["name"], "root", "{init}");
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// In-process store helpers (same library the CLI/MCP surfaces are built on).
// ---------------------------------------------------------------------------

fn provenance_agent(root_dir: &Path, repo: &Path, provenance_id: &str) -> Option<String> {
    let repository = root_work::Repository::discover(repo).ok()?;
    let store = root_work::WorkStore::open_at(root_dir, repository).ok()?;
    store.provenance(provenance_id).ok()?.agent
}

fn event_count(root_dir: &Path, repo: &Path) -> usize {
    let repository = root_work::Repository::discover(repo).unwrap();
    let store = root_work::WorkStore::open_at(root_dir, repository).unwrap();
    store.events().unwrap().len()
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_no_secret_under(dir: &Path, secret: &str) {
    for path in walk_files(dir) {
        if let Ok(bytes) = std::fs::read(&path) {
            assert!(
                !contains_bytes(&bytes, secret.as_bytes()),
                "secret value leaked into {}",
                path.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The 12-step scenario.
// ---------------------------------------------------------------------------

#[test]
fn claude_to_codex_twelve_step_acceptance() {
    let _guard = lock_tests();
    let env = Env::new("claude-codex");
    let goal = "Implement the PKCE login slice";
    let key_decision = "Tokens live in the keychain, never in work state";
    let checkpoint_message = "PKCE login slice implemented";
    let secret = "AKIAIOSFODNN7EXAMPLE";

    // ---- Step 1: Mac A restore (dry-run sane + no mutation), then restore ----
    std::fs::write(env.a.root_dir.join("Rootfile"), b"[packages]\n").unwrap();
    std::fs::write(
        env.a.root_dir.join("root.lock"),
        b"{\"version\":2,\"platform\":\"\",\"packages\":[]}\n",
    )
    .unwrap();
    let lock_before = std::fs::read(env.a.root_dir.join("root.lock")).unwrap();
    let profile = env.a.home.join(".root/profiles/default");

    let plan = env.a.json(&["restore", "--dry-run", "--json"]);
    assert_eq!(plan["total_packages"], 0);
    assert!(plan["will_install"].as_array().unwrap().is_empty());
    assert!(plan["will_remove"].as_array().unwrap().is_empty());
    assert!(!plan["work_bind"]["available"].as_bool().unwrap());
    assert_eq!(
        std::fs::read(env.a.root_dir.join("root.lock")).unwrap(),
        lock_before,
        "--dry-run must not mutate root.lock"
    );
    assert!(!profile.exists(), "--dry-run must not create a Nix profile");

    let restored = env.a.json(&["restore", "--json"]);
    assert!(restored["lock_path"]
        .as_str()
        .unwrap()
        .ends_with("root.lock"));
    assert!(restored["installed"].as_array().unwrap().is_empty());
    assert!(!restored["work_bind"]["available"].as_bool().unwrap());

    // ---- Step 2: workspace init; goal set; status baseline ----
    let init = env.a.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    assert!(workspace_id.starts_with("root_ws_"));
    env.a.json(&["goal", "set", goal, "--json"]);
    let status = env.a.json(&["workspace", "status", "--json"]);
    assert_eq!(status["goal"]["statement"], goal);
    assert_eq!(status["counts"]["decisions"], 0);
    assert_eq!(status["counts"]["findings"], 0);
    assert_eq!(status["counts"]["artifacts"], 0);
    let baseline_revision = status["work_revision"].as_i64().unwrap();

    // ---- Step 3: Claude records via MCP (>=2 decisions + 1 finding/evidence) ----
    let claude_decisions = [key_decision, "PKCE uses the S256 code challenge"];
    let claude_evidence = "cargo test refresh_token";
    let mut claude_provenance = Vec::new();
    {
        let mut mcp = Mcp::start(&env.a, "claude");
        for statement in claude_decisions {
            let result = mcp.call("work.record_decision", json!({ "statement": statement }));
            let provenance = result
                .pointer("/structuredContent/decision/provenance_id")
                .and_then(Value::as_str)
                .expect("decision provenance id");
            claude_provenance.push(provenance.to_string());
        }
        let finding = mcp.call(
            "work.record_finding",
            json!({ "statement": "Refresh-token test is green", "evidence_ref": claude_evidence }),
        );
        assert_eq!(
            finding.pointer("/structuredContent/finding/evidence_ref"),
            Some(&json!(claude_evidence))
        );
        // list_* round-trip over the same surface.
        let list = mcp.call("work.list_decisions", json!({}));
        assert_eq!(
            list["structuredContent"]["decisions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    // Provenance agent=claude is visible on the recorded rows.
    for provenance in &claude_provenance {
        assert_eq!(
            provenance_agent(&env.a.root_dir, &env.a.repo, provenance).as_deref(),
            Some("claude"),
            "provenance agent=claude must be recorded"
        );
    }

    // ---- Step 4: implement the slice; record artifacts ----
    env.a
        .write_repo_file("src/auth/pkce.ts", b"export const pkce = 1;\n");
    env.a
        .write_repo_file("src/auth/routes.ts", b"export const routes = 1;\n");
    for uri in ["src/auth/pkce.ts", "src/auth/routes.ts"] {
        let artifact = env.a.json(&["artifact", "add", uri, "--json"]);
        assert_eq!(artifact["artifact"]["uri"], uri);
        assert!(
            artifact["artifact"]["fingerprint"].as_str().is_some(),
            "artifact must carry a fingerprint: {artifact}"
        );
    }

    // ---- Step 5: capture Claude's environment FIRST, then checkpoint ----
    // Ordering matters (smoke test §4): `checkpoint create` embeds the
    // resolved canonical environment into the immutable snapshot
    // (`agent_env` + `agent_env_sha256`). Capturing after the checkpoint
    // would leave the snapshot without an agent mapping, so Mac B's
    // `resume --with` could never map skills/credentials.
    std::fs::create_dir_all(env.a.repo.join(".root")).unwrap();
    let capture = env.a.json(&[
        "agent",
        "capture",
        "--from",
        "claude",
        "--apply",
        "--out",
        ".root/agent.toml",
        "--json",
    ]);
    assert_eq!(capture["source_agent"], "claude");
    assert_eq!(capture["source_agent_version"], "2.1.260");
    assert!(env.a.repo.join(".root/agent.toml").exists());

    let checkpoint = env.a.json(&[
        "checkpoint",
        "create",
        "--message",
        checkpoint_message,
        "--json",
    ]);
    let checkpoint_one = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();
    assert!(checkpoint_one.starts_with("root_cp_"));
    assert!(
        checkpoint["checkpoint"]["work_revision"].as_i64().unwrap() > baseline_revision,
        "checkpoint must advance the work revision"
    );
    assert_eq!(
        checkpoint["checkpoint"]["git_head"].as_str().unwrap().len(),
        40,
        "checkpoint must record a Git HEAD"
    );
    assert_eq!(
        checkpoint["checkpoint"]["environment_status"], "observed",
        "checkpoint must record the observed environment"
    );
    // The snapshot now carries the captured claude environment: that is what
    // `resume --with` maps on Mac B (asserted in step 8).
    assert!(
        checkpoint["checkpoint"]["agent_env_ref"].is_string(),
        "capture-before-checkpoint must embed agent_env_ref: {checkpoint}"
    );

    // ---- Step 6: export ----
    let export = env.a.json(&[
        "workspace",
        "export",
        "-o",
        env.transfer.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(export["workspace_id"], workspace_id);
    assert_eq!(export["goals"], 1);
    assert_eq!(export["decisions"], 2);
    assert_eq!(export["findings"], 1);
    assert_eq!(export["checkpoints"], 1);
    assert_eq!(
        export["payload_sha256"].as_str().unwrap().len(),
        64,
        "transfer document must carry a payload digest"
    );

    // ---- Step 7: Mac B import into a fresh ROOT_DIR + different project path ----
    let import = env.b.json(&[
        "workspace",
        "import",
        env.transfer.to_str().unwrap(),
        "--project",
        env.b.repo.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(import["workspace_id"], workspace_id);
    assert_eq!(import["latest_checkpoint_id"], checkpoint_one);
    assert!(!env.b.repo.join(".root/agent.toml").exists());
    // Mac B carries the same Root environment declaration (Rootfile +
    // root.lock) as Mac A: that is machine configuration, not work state, so
    // it travels outside the transfer document. Without it the fresh
    // ROOT_DIR has no environment and resume step 4 (`restore workspace`)
    // could never verify the checkpoint digests on a genuine second machine.
    std::fs::copy(
        env.a.root_dir.join("Rootfile"),
        env.b.root_dir.join("Rootfile"),
    )
    .expect("Mac B must carry the same Rootfile");
    std::fs::copy(
        env.a.root_dir.join("root.lock"),
        env.b.root_dir.join("root.lock"),
    )
    .expect("Mac B must carry the same root.lock");

    // ---- Step 8: resume --with codex: 7 steps + the four questions ----
    let resume = env.b.json(&["resume", "--with", "codex", "--json"]);
    assert_eq!(resume["target"], "codex");
    // The checkpoint snapshot carries Mac A's captured claude environment
    // (capture ran before checkpoint creation in step 5), so the cross-harness
    // mapping is available and grounded in that environment.
    assert_eq!(
        resume["mapping_available"], true,
        "capture-before-checkpoint must yield a mappable checkpoint: {resume}"
    );
    assert_eq!(
        resume["translation"]["from"], "claude",
        "translation must be grounded in the checkpointed claude env: {resume}"
    );
    let steps = resume["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7, "resume --with must execute 7 steps");
    let step_names: Vec<&str> = steps
        .iter()
        .map(|step| step["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        step_names,
        vec![
            "prepare config",
            "map skills/instructions",
            "credential refs",
            "restore workspace",
            "repo drift check",
            "assemble work state",
            "launch continuation",
        ],
        "the seven steps must be present in order: {resume}"
    );
    for step in steps {
        assert_eq!(
            step["ok"],
            true,
            "step `{}` must succeed on Mac B: {resume}",
            step["name"].as_str().unwrap()
        );
    }
    let package = &resume["package"];
    // (a) what is the goal?
    assert_eq!(package["goal"]["statement"], goal);
    // (b) what was done? (decisions + checkpoint message)
    assert_eq!(package["checkpoint"]["message"], checkpoint_message);
    let statements: Vec<&str> = package["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["statement"].as_str())
        .collect();
    assert!(statements.contains(&claude_decisions[0]));
    assert!(statements.contains(&claude_decisions[1]));
    // (c) what constraints hold? (key decision verbatim)
    assert!(
        statements.contains(&key_decision),
        "key decision must appear verbatim: {statements:?}"
    );
    // (d) what is next? (suggested continuation, labelled not-verified)
    let suggestions = package["suggested_continuation"].as_array().unwrap();
    assert!(!suggestions.is_empty(), "next step must be present");
    assert!(
        suggestions[0]
            .as_str()
            .unwrap()
            .starts_with("Suggestion (not verified):"),
        "suggestions must stay labelled not-verified: {suggestions:?}"
    );
    // The rendered text carries the same package.
    let rendered_out = env.b.run(&["resume", "--with", "codex"]);
    assert!(rendered_out.status.success());
    let rendered = String::from_utf8_lossy(&rendered_out.stdout);
    assert!(rendered.contains("Root Resume --with Codex"), "{rendered}");
    assert!(rendered.contains(goal), "{rendered}");

    // ---- Step 9: Codex continues; second checkpoint carries codex provenance ----
    {
        let mut mcp = Mcp::start(&env.b, "codex");
        mcp.call(
            "work.record_decision",
            json!({ "statement": "Codex added the refresh handler to the PKCE route" }),
        );
        mcp.call(
            "work.record_finding",
            json!({
                "statement": "Refresh handler passes the integration test",
                "evidence_ref": "cargo test integration",
            }),
        );
        let checkpoint = mcp.call(
            "continuity.checkpoint",
            json!({ "message": "Codex continuation checkpoint" }),
        );
        let checkpoint_two = checkpoint
            .pointer("/structuredContent/checkpoint/id")
            .and_then(Value::as_str)
            .unwrap();
        assert!(checkpoint_two.starts_with("root_cp_"));
        assert_ne!(checkpoint_two, checkpoint_one);
    }
    let checkpoints = env.b.json(&["checkpoint", "list", "--json"]);
    assert_eq!(
        checkpoints["checkpoints"].as_array().unwrap().len(),
        2,
        "both checkpoints must be listed: {checkpoints}"
    );
    assert_eq!(
        checkpoints["checkpoints"][0]["provenance_agent"],
        "codex",
        "checkpoint list must distinguish the creating agent from the captured environment: {checkpoints}"
    );
    let resume_codex = env.b.json(&["resume", "--with", "claude", "--json"]);
    assert_eq!(
        resume_codex["source"], "codex",
        "the second checkpoint must carry codex provenance: {resume_codex}"
    );
    assert_eq!(
        resume_codex["package"]["checkpoint"]["message"],
        "Codex continuation checkpoint"
    );

    // ---- Step 10: drift + recover ----
    env.b
        .write_repo_file("src/followup.ts", b"export const followup = 1;\n");
    env.b.git_commit_all("codex follow-up");
    let resume_drift = env.b.json(&["resume", "--json"]);
    assert_eq!(resume_drift["drift"]["level"], "warning");
    assert!(
        resume_drift["drift"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "repository.head"),
        "resume must surface repository.head drift: {resume_drift}"
    );
    let recover = env.b.json(&["recover", "--json"]);
    assert_eq!(
        recover["recommended_action"],
        "Inspect drift before resuming."
    );
    let mut not_recoverable: Vec<String> = recover["not_recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect();
    let mut expected = vec![
        "unrecorded agent conversation".to_string(),
        "unsaved editor state".to_string(),
        "commands not observed by Root".to_string(),
    ];
    not_recoverable.sort();
    expected.sort();
    assert_eq!(
        not_recoverable, expected,
        "the not_recoverable triple is fixed: {recover}"
    );

    // ---- Step 11: secret probe refused, no row and no event ----
    let decisions_before = env.b.json(&["decision", "list", "--json"]);
    let decisions_before = decisions_before["decisions"].as_array().unwrap().len();
    let findings_before = env.b.json(&["finding", "list", "--json"]);
    let findings_before = findings_before["findings"].as_array().unwrap().len();
    let events_before = event_count(&env.b.root_dir, &env.b.repo);

    let secret_decision = env.b.run(&[
        "decision",
        "add",
        &format!("AWS access key {secret}"),
        "--json",
    ]);
    assert!(
        !secret_decision.status.success(),
        "secret-shaped decision must be refused"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&secret_decision.stdout),
        String::from_utf8_lossy(&secret_decision.stderr)
    );
    assert!(
        combined.contains("Refusing to persist"),
        "refusal must name the guard: {combined}"
    );
    assert!(
        !combined.contains(secret),
        "refusal must not echo the secret value: {combined}"
    );
    let secret_finding = env
        .b
        .run(&["finding", "add", "token = abcdef123456", "--json"]);
    assert!(
        !secret_finding.status.success(),
        "secret-shaped finding must be refused"
    );

    assert_eq!(
        env.b.json(&["decision", "list", "--json"])["decisions"]
            .as_array()
            .unwrap()
            .len(),
        decisions_before,
        "refused decision must not create a row"
    );
    assert_eq!(
        env.b.json(&["finding", "list", "--json"])["findings"]
            .as_array()
            .unwrap()
            .len(),
        findings_before,
        "refused finding must not create a row"
    );
    assert_eq!(
        event_count(&env.b.root_dir, &env.b.repo),
        events_before,
        "refused secret must not append an event"
    );

    // ---- Step 12: no manual copies; no secret values anywhere ----
    // The only agent environment lives on Mac A; B received none.
    assert!(
        env.a.repo.join(".root/agent.toml").exists(),
        "Mac A captured its agent environment"
    );
    assert!(
        !env.b.repo.join(".root/agent.toml").exists(),
        "Mac B must not receive a copied agent environment"
    );
    assert!(
        !env.b.codex_home.join("config.toml").exists(),
        "no toolchain config copied to Mac B"
    );
    assert!(
        !env.b.cl.join("CLAUDE.md").exists(),
        "no briefing copied to Mac B's Claude home"
    );
    // Every produced file lives under the isolated base.
    for path in walk_files(&env.base) {
        assert!(
            path.starts_with(&env.base),
            "test wrote outside the isolated dirs: {}",
            path.display()
        );
    }
    // No secret value anywhere in the isolated world or the transfer document.
    assert_no_secret_under(&env.base, secret);
    let transfer_text = std::fs::read_to_string(&env.transfer).unwrap();
    assert!(!transfer_text.contains(secret));
}
