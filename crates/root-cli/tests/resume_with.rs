//! Sprint 013 group A: `resume --with` harness-aware continuation.
//!
//! Hermetic child-env isolation (isolated HOME/ROOT_DIR/TMPDIR + fake
//! codex/opencode/claude shims on PATH). No process-env mutation in this
//! binary; in-process work-state seeding uses explicit `open_at`/`init_at`
//! Root directories rather than `ROOT_DIR`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_resume_with_{name}_{}_{}",
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
    home: PathBuf,
    tmpdir: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        let home = base.join("home");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for dir in [&repo, &root_dir, &home, &tmpdir, &bin] {
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
        let fixture = Self {
            base,
            repo,
            root_dir,
            home,
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

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(dirs).unwrap();
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path)
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

    fn repository(&self) -> root_work::Repository {
        root_work::Repository::discover(&self.repo).unwrap()
    }

    fn init_workspace(&self) {
        root_work::WorkStore::init_at(&self.root_dir, self.repository()).unwrap();
    }

    fn store(&self) -> root_work::WorkStore {
        root_work::WorkStore::open_at(&self.root_dir, self.repository()).unwrap()
    }

    fn seed_checkpoint(&self) {
        self.json(&["workspace", "init", "--json"]);
        self.json(&["goal", "set", "Ship harness-aware resume", "--json"]);
        self.json(&["decision", "add", "Project only, never mutate", "--json"]);
        self.json(&["finding", "add", "Seven steps are observable", "--json"]);
        self.json(&["checkpoint", "create", "--json"]);
    }

    fn write_codex_agent_toml(&self) {
        std::fs::create_dir_all(self.repo.join(".root")).unwrap();
        std::fs::write(
            self.repo.join(".root/agent.toml"),
            root_agent_bundle::project::emit_agent_toml(&codex_canonical()).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn codex_canonical() -> root_agent_bundle::canonical::CanonicalEnv {
    use root_agent_bundle::canonical::{
        CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalMcp,
        CanonicalProvenance, CanonicalSkill, CANONICAL_SCHEMA_VERSION,
    };
    use root_agent_bundle::manifest::SECRET_DISCLOSURE;
    use std::collections::BTreeMap;

    let mut mcp_servers = BTreeMap::new();
    mcp_servers.insert(
        "github".to_string(),
        CanonicalMcp {
            id: "github".to_string(),
            transport: "stdio".to_string(),
            command: vec!["npx".to_string()],
            args: vec!["-y".to_string(), "pkg".to_string()],
            cwd: None,
            credential_refs: vec!["GITHUB_TOKEN".to_string()],
            environment: vec!["GITHUB_TOKEN".to_string()],
            enabled: false,
        },
    );
    CanonicalEnv {
        schema_version: CANONICAL_SCHEMA_VERSION,
        source_agent: "codex".to_string(),
        source_agent_version: "0.150.1".to_string(),
        instructions: CanonicalInstructions {
            main: "AGENTS.md".to_string(),
        },
        skills: vec![CanonicalSkill {
            name: "docs-writer".to_string(),
            files: vec!["SKILL.md".to_string()],
        }],
        tools: Vec::new(),
        mcp_servers,
        policies: Vec::new(),
        environment: CanonicalEnvironment {
            needs_env: vec!["GITHUB_TOKEN".to_string()],
        },
        provenance: CanonicalProvenance {
            captured_at: "1234567890".to_string(),
            bundle_hash: String::new(),
            disclosure: SECRET_DISCLOSURE.to_string(),
        },
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for resume --with tests");
    assert!(status.success(), "git {:?} failed", args);
}

fn list_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.display().to_string());
            }
        }
    }
    out.sort();
    out
}

fn timestamp(index: usize) -> String {
    format!("2026-01-01T00:{:02}:{:02}Z", index / 60, index % 60)
}

const STEP_NAMES: [&str; 7] = [
    "prepare config",
    "map skills/instructions",
    "credential refs",
    "restore workspace",
    "repo drift check",
    "assemble work state",
    "launch continuation",
];

#[test]
fn resume_with_explicit_target_has_seven_steps() {
    let fixture = Fixture::new("explicit");
    fixture.seed_checkpoint();

    let report = fixture.json(&["resume", "--with", "codex", "--json"]);
    assert_eq!(report["target"], "codex");
    assert_eq!(report["steps"].as_array().unwrap().len(), 7);
    assert_eq!(report["steps"][0]["name"], "prepare config");
    assert_eq!(report["steps"][6]["name"], "launch continuation");
    assert!(!report["instructions"].as_array().unwrap().is_empty());
    // Bounded projection is preserved inside the package.
    assert_eq!(report["package"]["decisions"].as_array().unwrap().len(), 1);

    let output = fixture.run(&["resume", "--with", "codex"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Root Resume --with Codex"), "{stdout}");
    assert!(stdout.contains("Steps"), "{stdout}");
    assert!(stdout.contains("Instructions (Codex)"), "{stdout}");
    assert!(stdout.contains("Next"), "{stdout}");
}

#[test]
fn resume_with_all_seven_steps_and_sections() {
    let fixture = Fixture::new("sections");
    fixture.write_codex_agent_toml();
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Ship harness-aware resume", "--json"]);
    fixture.json(&["decision", "add", "Project only, never mutate", "--json"]);
    fixture.json(&["finding", "add", "Seven steps are observable", "--json"]);
    std::fs::write(fixture.repo.join("src.ts"), b"export {}\n").unwrap();
    fixture.json(&["artifact", "add", "src.ts", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let report = fixture.json(&["resume", "--with", "claude", "--json"]);
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    let names: Vec<&str> = steps.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, STEP_NAMES.to_vec());
    assert!(report["package"].is_object());
    assert!(!report["instructions"].as_array().unwrap().is_empty());
    assert!(report["warnings"].is_array());

    let next = report["next"].as_array().unwrap();
    assert_eq!(
        next[0],
        "root agent plan --env .root/agent.toml --to claude"
    );
    let apply = next[1].as_str().unwrap();
    assert!(
        apply.starts_with("root agent apply --env .root/agent.toml --apply --plan-hash "),
        "{apply}"
    );
    assert!(apply.ends_with(" --approve <sha>..."), "{apply}");

    let output = fixture.run(&["resume", "--with", "claude"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for section in [
        "Root Resume --with Claude Code",
        "Steps",
        "Instructions (Claude Code)",
        "Next",
    ] {
        assert!(stdout.contains(section), "missing {section}:\n{stdout}");
    }
}

#[test]
fn resume_with_deterministic() {
    let fixture = Fixture::new("deterministic");
    fixture.seed_checkpoint();

    let first = fixture.run(&["resume", "--with", "claude", "--json"]);
    let second = fixture.run(&["resume", "--with", "claude", "--json"]);
    assert!(first.status.success() && second.status.success());
    assert_eq!(
        first.stdout, second.stdout,
        "resume --with JSON must be deterministic"
    );
}

#[test]
fn resume_with_unknown_target_fails_closed() {
    let fixture = Fixture::new("unknown");
    fixture.seed_checkpoint();

    let output = fixture.run(&["resume", "--with", "gemini"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unsupported --with target"), "{stderr}");
    assert!(stderr.contains("codex, opencode, claude"), "{stderr}");
}

#[test]
fn resume_with_caps_and_omitted() {
    use root_work::model::{ENV_MISSING, STATUS_ACTIVE, STATUS_SUPERSEDED};
    use root_work::{NewArtifact, NewCheckpoint, ProvenanceContext};

    let fixture = Fixture::new("caps");
    fixture.init_workspace();
    let workspace_id;
    let revision;
    {
        let mut store = fixture.store();
        workspace_id = store.workspace().id.clone();
        store.set_goal("Bound the harness-aware package").unwrap();
        for index in 0..25 {
            store
                .add_decision(&format!("Decision {index:02}"), None)
                .unwrap();
        }
        for index in 0..50 {
            store
                .add_finding(&format!("Finding {index:02}"), None)
                .unwrap();
        }
        for index in 0..100 {
            let uri = format!("reports/{index:03}.md");
            store
                .add_artifact(NewArtifact {
                    kind: "report",
                    uri: &uri,
                    fingerprint: None,
                    evidence_ref: None,
                })
                .unwrap();
        }
        revision = store.work_revision().unwrap();
    }

    let mut decisions = Vec::new();
    for index in 0..25 {
        decisions.push(root_work::DecisionRecord {
            id: format!("root_dec_{index:02}"),
            workspace_id: workspace_id.clone(),
            goal_id: None,
            statement: format!("Decision {index:02}"),
            rationale: None,
            status: STATUS_ACTIVE.to_string(),
            created_at: timestamp(index),
            provenance_id: None,
        });
    }
    for index in 0..3 {
        decisions.push(root_work::DecisionRecord {
            id: format!("root_dec_super_{index}"),
            workspace_id: workspace_id.clone(),
            goal_id: None,
            statement: format!("Superseded decision {index}"),
            rationale: None,
            status: STATUS_SUPERSEDED.to_string(),
            created_at: timestamp(100 + index),
            provenance_id: None,
        });
    }
    let mut findings = Vec::new();
    for index in 0..50 {
        findings.push(root_work::FindingRecord {
            id: format!("root_find_{index:02}"),
            workspace_id: workspace_id.clone(),
            goal_id: None,
            statement: format!("Finding {index:02}"),
            evidence_ref: None,
            status: STATUS_ACTIVE.to_string(),
            created_at: timestamp(index),
            provenance_id: None,
        });
    }
    for index in 0..2 {
        findings.push(root_work::FindingRecord {
            id: format!("root_find_super_{index}"),
            workspace_id: workspace_id.clone(),
            goal_id: None,
            statement: format!("Superseded finding {index}"),
            evidence_ref: None,
            status: STATUS_SUPERSEDED.to_string(),
            created_at: timestamp(100 + index),
            provenance_id: None,
        });
    }
    let mut artifacts = Vec::new();
    for index in 0..100 {
        artifacts.push(root_work::ArtifactRecord {
            id: format!("root_art_{index:03}"),
            workspace_id: workspace_id.clone(),
            kind: "report".to_string(),
            uri: format!("reports/{index:03}.md"),
            fingerprint: None,
            created_at: timestamp(index),
            provenance_id: None,
        });
    }
    let snapshot = root_continuity::snapshot::CheckpointSnapshot {
        goal: Some(root_work::GoalRecord {
            id: "root_goal_caps".to_string(),
            workspace_id: workspace_id.clone(),
            statement: "Bound the harness-aware package".to_string(),
            status: STATUS_ACTIVE.to_string(),
            created_at: timestamp(0),
            completed_at: None,
            provenance_id: None,
        }),
        decisions,
        findings,
        artifacts,
        work_revision: revision,
        agent_env: None,
        agent_env_sha256: None,
    };
    let snapshot_json = serde_json::to_string(&snapshot).unwrap();

    {
        let mut store = fixture.store();
        let empty = serde_json::json!({
            "goal": null,
            "decisions": [],
            "findings": [],
            "artifacts": [],
            "work_revision": revision,
        })
        .to_string();
        for index in 0..19 {
            store
                .create_checkpoint_with(
                    NewCheckpoint {
                        message: Some(&format!("filler {index:02}")),
                        work_revision: revision,
                        git_head: None,
                        git_branch: None,
                        git_dirty: false,
                        git_dirty_fingerprint: None,
                        rootfile_digest: None,
                        root_lock_digest: None,
                        profile_reference: None,
                        environment_status: ENV_MISSING,
                        continuation_summary: "filler",
                        snapshot: &empty,
                        agent_env_ref: None,
                    },
                    ProvenanceContext::default(),
                )
                .unwrap();
        }
        store
            .create_checkpoint_with(
                NewCheckpoint {
                    message: Some("bounded"),
                    work_revision: revision,
                    git_head: None,
                    git_branch: None,
                    git_dirty: false,
                    git_dirty_fingerprint: None,
                    rootfile_digest: None,
                    root_lock_digest: None,
                    profile_reference: None,
                    environment_status: ENV_MISSING,
                    continuation_summary: "bounded",
                    snapshot: &snapshot_json,
                    agent_env_ref: None,
                },
                ProvenanceContext::default(),
            )
            .unwrap();
    }

    let report = fixture.json(&["resume", "--with", "codex", "--json"]);
    let package = &report["package"];
    assert_eq!(package["decisions"].as_array().unwrap().len(), 10);
    assert_eq!(package["decisions_omitted"], 15);
    assert_eq!(package["findings"].as_array().unwrap().len(), 10);
    assert_eq!(package["findings_omitted"], 40);
    assert_eq!(package["artifacts"].as_array().unwrap().len(), 20);
    assert_eq!(package["artifacts_omitted"], 80);
    assert_eq!(package["current_state"]["active_decisions"], 25);
    assert_eq!(package["current_state"]["active_findings"], 50);
    assert_eq!(package["current_state"]["artifacts"], 100);

    assert_eq!(package["decisions"][0]["id"], "root_dec_24");
    assert_eq!(package["findings"][0]["id"], "root_find_49");
    assert_eq!(package["artifacts"][0]["id"], "root_art_099");
    for record in package["decisions"].as_array().unwrap() {
        assert!(
            !record["id"].as_str().unwrap().contains("super"),
            "superseded decision leaked: {record}"
        );
    }
    for record in package["findings"].as_array().unwrap() {
        assert!(
            !record["id"].as_str().unwrap().contains("super"),
            "superseded finding leaked: {record}"
        );
    }
}

#[test]
fn resume_with_from_provenance() {
    let fixture = Fixture::new("provenance");
    fixture.init_workspace();
    {
        let repository = fixture.repository();
        let mut store = fixture.store();
        let context = root_work::ProvenanceContext {
            source_type: root_work::model::SOURCE_AGENT,
            agent: Some("codex"),
            harness: None,
            session_id: None,
            evidence_ref: None,
        };
        root_continuity::create_on_store(
            &mut store,
            &repository,
            &fixture.root_dir,
            Some("agent checkpoint"),
            context,
        )
        .unwrap();
    }

    let report = fixture.json(&["resume", "--with", "claude", "--json"]);
    assert_eq!(report["source"], "codex");
    assert_eq!(report["target"], "claude");
}

#[test]
fn resume_without_flag_is_legacy() {
    let fixture = Fixture::new("legacy");
    fixture.seed_checkpoint();

    let report = fixture.json(&["resume", "--json"]);
    assert!(report.get("checkpoint").is_some());
    assert!(report.get("steps").is_none(), "legacy resume gained steps");
    assert!(report.get("package").is_none());
}

#[test]
fn resume_with_does_not_mutate() {
    let fixture = Fixture::new("no_mutation");
    fixture.write_codex_agent_toml();
    fixture.seed_checkpoint();

    let files_before = list_files(&fixture.repo);
    let (events_before, checkpoints_before) = {
        let store = fixture.store();
        (
            store.events().unwrap().len(),
            store.list_checkpoints().unwrap().len(),
        )
    };

    let output = fixture.run(&["resume", "--with", "codex", "--json"]);
    assert!(output.status.success());

    assert_eq!(list_files(&fixture.repo), files_before);
    let store = fixture.store();
    assert_eq!(store.events().unwrap().len(), events_before);
    assert_eq!(store.list_checkpoints().unwrap().len(), checkpoints_before);
}

#[test]
fn resume_with_no_value_uses_default_target() {
    let fixture = Fixture::new("default");
    fixture.seed_checkpoint();
    std::fs::write(
        fixture.repo.join("Rootfile"),
        b"[agents]\ndefault_target = \"opencode\"\n",
    )
    .unwrap();

    let report = fixture.json(&["resume", "--with", "--json"]);
    assert_eq!(report["target"], "opencode");
    assert_eq!(report["steps"].as_array().unwrap().len(), 7);
}

#[test]
fn resume_with_no_value_without_default_exits_2() {
    let fixture = Fixture::new("nodefault");
    fixture.seed_checkpoint();

    let output = fixture.run(&["resume", "--with"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no --with target available"), "{stderr}");
}

#[test]
fn resume_with_unknown_target_exits_2() {
    let fixture = Fixture::new("unknown_legacy");
    fixture.seed_checkpoint();

    let output = fixture.run(&["resume", "--with", "gemini"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unsupported --with target"), "{stderr}");
}

#[test]
fn workspace_init_write_pointer_mentions_pointer() {
    let fixture = Fixture::new("pointer");
    let output = fixture.run(&["workspace", "init", "--write-pointer"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pointer"), "{stdout}");
    assert!(fixture.repo.join(".root/workspace.json").exists());
}

#[test]
fn resume_with_uses_checkpointed_agent_env() {
    let fixture = Fixture::new("checkpointed_env");
    fixture.write_codex_agent_toml();
    fixture.seed_checkpoint();

    let first = fixture.run(&["resume", "--with", "codex", "--json"]);
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );

    // Overwrite the live environment after the checkpoint. Resume must read the
    // immutable checkpoint, so output is byte-identical to the pre-edit run.
    let mut hostile = codex_canonical();
    hostile.skills.clear();
    std::fs::write(
        fixture.repo.join(".root/agent.toml"),
        root_agent_bundle::project::emit_agent_toml(&hostile).unwrap(),
    )
    .unwrap();

    let second = fixture.run(&["resume", "--with", "codex", "--json"]);
    assert!(
        second.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        first.stdout, second.stdout,
        "resume --with must ignore live edits to .root/agent.toml"
    );
}

#[test]
fn resume_with_without_captured_agent_env_is_honest() {
    let fixture = Fixture::new("honest_unmapped");
    fixture.seed_checkpoint();

    let report = fixture.json(&["resume", "--with", "codex", "--json"]);
    assert_eq!(
        report["mapping_available"], false,
        "no captured env must report mapping unavailable: {report}"
    );

    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    assert_eq!(
        steps[1]["ok"], false,
        "map skills/instructions must honestly fail: {report}"
    );
    assert_eq!(
        steps[2]["ok"], false,
        "credential refs must honestly fail: {report}"
    );
    for (index, step) in steps.iter().enumerate() {
        if index == 1 || index == 2 {
            continue;
        }
        assert_eq!(step["ok"], true, "step {index} must be ok: {report}");
    }

    let output = fixture.run(&["resume", "--with", "codex"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent mapping: unavailable"),
        "human output must not imply success: {stdout}"
    );
}

/// Blocker 5: `resume --with` verifies `agent_env_sha256` before mapping.
/// A structurally valid but modified stored environment (digest unchanged)
/// must fail closed end to end without echoing the value.
#[test]
fn resume_with_rejects_tampered_checkpoint_agent_env() {
    use root_work::model::ENV_OBSERVED;
    use root_work::{NewCheckpoint, ProvenanceContext};

    let fixture = Fixture::new("tampered_env");
    fixture.write_codex_agent_toml();
    fixture.seed_checkpoint();

    // Happy path first: the real pipeline's checkpoint passes verification.
    let before = fixture.json(&["resume", "--with", "codex", "--json"]);
    assert_eq!(
        before["mapping_available"], true,
        "untampered checkpoint must map: {before}"
    );

    // Tamper the stored value only (structurally valid, secret-shaped);
    // leave agent_env_sha256 untouched.
    let secret = "sk-tampered-checkpoint-0123456789";
    {
        let mut store = fixture.store();
        let latest = store
            .latest_checkpoint()
            .unwrap()
            .expect("seed_checkpoint must create a checkpoint");
        let mut snapshot =
            root_continuity::CheckpointSnapshot::from_json(&latest.snapshot).unwrap();
        let value = snapshot.agent_env.as_mut().expect("env captured");
        value["source_agent_version"] = serde_json::json!(secret);
        assert!(
            snapshot.agent_env_sha256.is_some(),
            "real-pipeline snapshot must keep its digest"
        );
        store
            .create_checkpoint_with(
                NewCheckpoint {
                    message: Some("tampered"),
                    work_revision: snapshot.work_revision,
                    git_head: None,
                    git_branch: None,
                    git_dirty: false,
                    git_dirty_fingerprint: None,
                    rootfile_digest: None,
                    root_lock_digest: None,
                    profile_reference: None,
                    environment_status: ENV_OBSERVED,
                    continuation_summary: "tampered",
                    snapshot: &snapshot.to_json().unwrap(),
                    agent_env_ref: None,
                },
                ProvenanceContext::default(),
            )
            .unwrap();
    }

    let output = fixture.run(&["resume", "--with", "codex"]);
    assert!(
        !output.status.success(),
        "tampered agent env must fail closed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("integrity check"),
        "failure must name the integrity check: {combined}"
    );
    assert!(
        combined.contains("agent_env_sha256"),
        "failure must name the digest field: {combined}"
    );
    assert!(
        !combined.contains(secret),
        "tampered value leaked: {combined}"
    );
    assert!(
        !combined.contains("mapping_available"),
        "no report (and no mapping) may be emitted: {combined}"
    );
}
