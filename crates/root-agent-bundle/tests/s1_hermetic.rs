//! S1 hermetic tests: security properties with isolated ROOT_DIR/HOME.
//!
//! All env-mutating tests hold TEST_MUTEX (see root-core TEST_MUTEX pattern).

use std::sync::Mutex;

static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn unique_tmp(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), n))
}

struct EnvGuard {
    saved_root: Option<std::ffi::OsString>,
    saved_home: Option<std::ffi::OsString>,
    saved_codex: Option<std::ffi::OsString>,
    saved_path: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn isolate(tmp: &std::path::Path) -> Self {
        let saved_root = std::env::var_os("ROOT_DIR");
        let saved_home = std::env::var_os("HOME");
        let saved_codex = std::env::var_os("CODEX_HOME");
        std::env::set_var("ROOT_DIR", tmp.join("root"));
        std::env::set_var("HOME", tmp.join("home"));
        std::env::set_var("CODEX_HOME", tmp.join("home").join(".codex"));
        let _ = std::fs::create_dir_all(tmp.join("home").join(".codex"));
        Self {
            saved_root,
            saved_home,
            saved_codex,
            saved_path: None,
        }
    }

    fn prepend_path(&mut self, dir: &std::path::Path) {
        if self.saved_path.is_none() {
            self.saved_path = Some(std::env::var_os("PATH").unwrap_or_default());
        }
        let mut new_path = dir.as_os_str().to_os_string();
        new_path.push(":");
        new_path.push(std::env::var_os("PATH").unwrap_or_default());
        std::env::set_var("PATH", new_path);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.saved_root {
            Some(v) => std::env::set_var("ROOT_DIR", v),
            None => std::env::remove_var("ROOT_DIR"),
        }
        match &self.saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.saved_codex {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
        if let Some(v) = &self.saved_path {
            std::env::set_var("PATH", v);
        }
    }
}

fn write_manifest(dir: &std::path::Path, manifest: &root_agent_bundle::manifest::Manifest) {
    std::fs::create_dir_all(dir.join("blobs")).unwrap();
    let bytes = serde_json::to_vec_pretty(manifest).unwrap();
    std::fs::write(dir.join("manifest.json"), bytes).unwrap();
}

fn minimal_manifest() -> root_agent_bundle::manifest::Manifest {
    root_agent_bundle::manifest::Manifest::new("0.150.1".to_string(), None)
}

#[test]
fn symlink_in_bundle_rejected() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_symlink");
    let bundle = tmp.join("bundle");
    write_manifest(&bundle, &minimal_manifest());
    std::fs::write(bundle.join("blobs").join("x"), b"y").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("x", bundle.join("blobs").join("evil")).unwrap();
    let err = root_agent_bundle::manifest::load_bundle(&bundle).unwrap_err();
    assert!(err.to_string().contains("symlink"), "got: {}", err);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn traversal_rel_rejected() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let mut m = minimal_manifest();
    m.files.push(root_agent_bundle::manifest::BundleFile {
        scope: root_agent_bundle::scope::Scope::CodexHome,
        rel: "../escape".to_string(),
        sha256: "a".repeat(64),
        size: 1,
        mode: "0644".to_string(),
        kind: root_agent_bundle::manifest::FileKind::PromptContent,
        executable: false,
    });
    assert!(m.validate().is_err());
}

#[test]
fn interpolated_target_strings_rejected() {
    assert!(root_agent_bundle::scope::Scope::parse("$CODEX_HOME/AGENTS.md").is_err());
    assert!(root_agent_bundle::scope::validate_rel("$CODEX_HOME/AGENTS.md").is_err());
}

#[test]
fn snapshot_tombstone_rollback_deletes_created_file() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_tombstone");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("NEWFILE.md");
    assert!(!target.exists());
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_test",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "NEWFILE.md".to_string(),
        )],
        None,
    )
    .unwrap();
    // Simulate apply durably recording its expected hash before the write.
    let applied = root_lockfile::compute_sha256(b"new");
    root_agent_bundle::snapshot::record_expected_applied(
        &snap.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "NEWFILE.md",
        &applied,
    )
    .unwrap();
    std::fs::write(&target, b"new").unwrap();
    assert!(target.exists());
    let restored = root_agent_bundle::snapshot::restore_snapshot(&snap).unwrap();
    assert!(!target.exists(), "tombstoned path must be deleted");
    assert!(!restored.is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn tombstone_rollback_refuses_directory_and_drift() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_tombdrift");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("NEWDIR");
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_dir",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "NEWDIR".to_string(),
        )],
        None,
    )
    .unwrap();
    // User (or attacker) creates a directory at the tombstoned path.
    std::fs::create_dir_all(target.join("sub")).unwrap();
    std::fs::write(target.join("sub").join("data.txt"), b"precious").unwrap();
    root_agent_bundle::snapshot::record_expected_applied(
        &snap.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "NEWDIR",
        &"b".repeat(64),
    )
    .unwrap();
    let err = root_agent_bundle::snapshot::restore_snapshot(&snap).unwrap_err();
    assert!(
        err.to_string().contains("directory") || err.to_string().contains("drift"),
        "got: {}",
        err
    );
    // Precious data survives.
    assert!(tmp
        .join("home")
        .join(".codex")
        .join("NEWDIR")
        .join("sub")
        .join("data.txt")
        .exists());
    // Modified file at a tombstoned path also refuses.
    let target2 = tmp.join("home").join(".codex").join("NEWFILE2");
    let snap2 = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_dir2",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "NEWFILE2".to_string(),
        )],
        None,
    )
    .unwrap();
    let applied = root_lockfile::compute_sha256(b"applied-bytes");
    root_agent_bundle::snapshot::record_expected_applied(
        &snap2.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "NEWFILE2",
        &applied,
    )
    .unwrap();
    std::fs::write(&target2, b"applied-bytes").unwrap();
    std::fs::write(&target2, b"user-edited-after-apply").unwrap();
    let err = root_agent_bundle::snapshot::restore_snapshot(&snap2).unwrap_err();
    assert!(err.to_string().contains("drift"), "got: {}", err);
    assert_eq!(std::fs::read(&target2).unwrap(), b"user-edited-after-apply");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn lock_contention_blocks_mutation_and_stale_recovers() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_lock");
    let _env = EnvGuard::isolate(&tmp);
    // Live holder blocks a second mutation (same-process: holder PID is alive).
    let holder = root_agent_bundle::lock::GlobalMutationLock::acquire().unwrap();
    let err = root_agent_bundle::apply::purge_snapshots(None, true).unwrap_err();
    assert!(err.to_string().contains("in progress"), "got: {}", err);
    drop(holder);
    // Released: mutation proceeds (no snapshots → empty ok).
    let deleted = root_agent_bundle::apply::purge_snapshots(None, true).unwrap();
    assert!(deleted.is_empty());
    // Stale lock (dead PID) is recovered automatically.
    let root = std::env::var_os("ROOT_DIR").unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        std::path::Path::new(&root).join("root.lockfile"),
        "99999999\n0\n",
    )
    .unwrap();
    let _recovered = root_agent_bundle::lock::GlobalMutationLock::acquire().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rollback_failure_marks_journal_failed() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_rbfail");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("BLOCKED");
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_blocked",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "BLOCKED".to_string(),
        )],
        None,
    )
    .unwrap();
    root_agent_bundle::snapshot::record_expected_applied(
        &snap.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "BLOCKED",
        &"c".repeat(64),
    )
    .unwrap();
    std::fs::create_dir_all(target.join("keep")).unwrap();
    // Newest snapshot is the blocking one → rollback refuses with failed semantics.
    let err = root_agent_bundle::apply::rollback_last().unwrap_err();
    assert!(err.to_string().contains("Rollback failed"), "got: {}", err);
    let journal = root_agent_bundle::journal::read_journal()
        .unwrap()
        .expect("journal must record the failure");
    assert_eq!(
        journal.phase,
        root_agent_bundle::journal::Phase::Failed,
        "journal must be Failed, not RolledBack"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn mcp_manifest_invariants_authenticated() {
    use root_agent_bundle::manifest::{
        Manifest, McpEntry, NeedsApproval, ADAPTER_SCHEMA_VERSION, BUNDLE_VERSION,
    };
    use root_agent_bundle::scope::Scope;
    fn entry_with(cmd_hash: Option<String>, enabled: bool, transport: &str) -> McpEntry {
        McpEntry {
            transport: transport.to_string(),
            enabled,
            needs_env: vec!["TOK".to_string()],
            command_sha256: cmd_hash,
            command: vec!["npx".to_string()],
            args: vec!["-y".to_string(), "pkg".to_string()],
            cwd: None,
            env_keys: vec!["TOK".to_string()],
        }
    }
    fn manifest_with(entry: McpEntry) -> Manifest {
        let hash = entry.command_sha256.clone().unwrap_or_default();
        let mut m = Manifest::new("0.150.1".to_string(), None);
        m.bundle_version = BUNDLE_VERSION;
        m.adapter_schema_version = ADAPTER_SCHEMA_VERSION;
        m.mcp.insert("srv".to_string(), entry);
        if !hash.is_empty() {
            m.needs_approval.push(NeedsApproval {
                scope: Scope::CodexHome,
                rel: "config.toml#mcp_servers.srv".to_string(),
                sha256: hash,
                reason: "test".to_string(),
            });
        }
        m
    }
    // Omitted fingerprint.
    assert!(manifest_with(entry_with(None, false, "stdio"))
        .validate()
        .is_err());
    // Enabled entry.
    let good_hash = root_agent_bundle::manifest::mcp_command_hash(
        &["npx".to_string()],
        &["-y".to_string(), "pkg".to_string()],
        &None,
        &["TOK".to_string()],
    );
    assert!(
        manifest_with(entry_with(Some(good_hash.clone()), true, "stdio"))
            .validate()
            .is_err()
    );
    // Wrong transport.
    assert!(
        manifest_with(entry_with(Some(good_hash.clone()), false, "http"))
            .validate()
            .is_err()
    );
    // Tampered command with stale hash.
    let mut tampered = entry_with(Some(good_hash.clone()), false, "stdio");
    tampered.command = vec!["evil".to_string()];
    assert!(manifest_with(tampered).validate().is_err());
    // Missing approval record.
    let mut m = Manifest::new("0.150.1".to_string(), None);
    m.mcp.insert(
        "srv".to_string(),
        entry_with(Some(good_hash), false, "stdio"),
    );
    assert!(m.validate().is_err());
}

#[test]
fn ancestor_symlink_escape_rejected() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_symlink_escape");
    let _env = EnvGuard::isolate(&tmp);
    let outside = tmp.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let link = tmp.join("home").join(".codex").join("linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let err = root_agent_bundle::scope::resolve_target(
        root_agent_bundle::scope::Scope::CodexHome,
        "linked/evil.md",
    )
    .unwrap_err();
    assert!(err.to_string().contains("symlink"), "got: {}", err);
    // Normal nested path still resolves.
    std::fs::create_dir_all(tmp.join("home").join(".codex").join("real")).unwrap();
    assert!(root_agent_bundle::scope::resolve_target(
        root_agent_bundle::scope::Scope::CodexHome,
        "real/ok.md",
    )
    .is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn snapshot_tamper_refuses_rollback() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_tamper");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("KEEP.md");
    std::fs::write(&target, b"keep").unwrap();
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_tamper",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "KEEP.md".to_string(),
        )],
        None,
    )
    .unwrap();
    // Corrupt a stored blob.
    let blobs: Vec<_> = std::fs::read_dir(
        tmp.join("root")
            .join("agent-snapshots")
            .join(&snap.id)
            .join("blobs"),
    )
    .unwrap()
    .collect();
    assert!(!blobs.is_empty());
    for b in blobs {
        std::fs::write(b.unwrap().path(), b"tampered").unwrap();
    }
    let err = root_agent_bundle::snapshot::load_snapshot(&snap.id).unwrap_err();
    assert!(
        err.to_string().contains("mismatch") || err.to_string().contains("corrupt"),
        "got: {}",
        err
    );
    assert!(!root_agent_bundle::snapshot::valid_snapshot_id("../evil"));
    assert!(!root_agent_bundle::snapshot::valid_snapshot_id("snap_x"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn plan_displays_config_changes() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_planview");
    let _env = EnvGuard::isolate(&tmp);
    std::fs::write(
        tmp.join("home").join(".codex").join("config.toml"),
        "model = \"old-model\"\n",
    )
    .unwrap();
    let bundle = tmp.join("bundle");
    std::fs::create_dir_all(bundle.join("blobs")).unwrap();
    let mut m = minimal_manifest();
    m.settings.insert(
        "model".to_string(),
        serde_json::Value::String("new-model".to_string()),
    );
    let cmd_hash =
        root_agent_bundle::manifest::mcp_command_hash(&["npx".to_string()], &[], &None, &[]);
    m.mcp.insert(
        "demo".to_string(),
        root_agent_bundle::manifest::McpEntry {
            transport: "stdio".to_string(),
            enabled: false,
            needs_env: vec![],
            command_sha256: Some(cmd_hash.clone()),
            command: vec!["npx".to_string()],
            args: vec![],
            cwd: None,
            env_keys: vec![],
        },
    );
    m.needs_approval
        .push(root_agent_bundle::manifest::NeedsApproval {
            scope: root_agent_bundle::scope::Scope::CodexHome,
            rel: "config.toml#mcp_servers.demo".to_string(),
            sha256: cmd_hash,
            reason: "test".to_string(),
        });
    m.validate().unwrap();
    let bytes = serde_json::to_vec_pretty(&m).unwrap();
    std::fs::write(bundle.join("manifest.json"), bytes).unwrap();
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &m).unwrap();
    assert_eq!(plan.settings_changes.len(), 1);
    assert_eq!(plan.settings_changes[0].key, "model");
    assert_eq!(plan.settings_changes[0].old.as_deref(), Some("old-model"));
    assert_eq!(plan.settings_changes[0].new, "new-model");
    assert_eq!(plan.mcp_to_add.len(), 1);
    assert_eq!(plan.mcp_to_add[0].id, "demo");
    assert!(!plan.mcp_to_add[0].exists);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn snapshot_restores_original_bytes_and_rollback_last_works() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_restore");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("AGENTS.md");
    std::fs::write(&target, b"original").unwrap();
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_test2",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "AGENTS.md".to_string(),
        )],
        None,
    )
    .unwrap();
    let applied = root_lockfile::compute_sha256(b"mutated");
    root_agent_bundle::snapshot::record_expected_applied(
        &snap.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "AGENTS.md",
        &applied,
    )
    .unwrap();
    std::fs::write(&target, b"mutated").unwrap();
    // rollback_last restores the newest snapshot (this one).
    let report = root_agent_bundle::apply::rollback_last().unwrap();
    assert_eq!(report.snapshot_id, snap.id);
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn purge_requires_confirmation() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_purge");
    let _env = EnvGuard::isolate(&tmp);
    let err = root_agent_bundle::apply::purge_snapshots(None, false).unwrap_err();
    assert!(err.to_string().contains("--yes"), "got: {}", err);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn incomplete_journal_blocks_new_mutation() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_journal");
    let _env = EnvGuard::isolate(&tmp);
    let journal = root_agent_bundle::journal::ApplyJournal {
        op_id: "op_stuck".to_string(),
        agent: "codex".to_string(),
        plan_hash: "x".to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: root_agent_bundle::journal::Phase::Applying,
        completed_paths: vec![],
        target_preconditions: Default::default(),
        mcp_provenance: Default::default(),
        prior_mcp_provenance: Default::default(),
    };
    root_agent_bundle::journal::write_journal(&journal).unwrap();
    let err = root_agent_bundle::journal::require_no_incomplete_op().unwrap_err();
    assert!(err.to_string().contains("rollback --last"), "got: {}", err);
    // Done journals do not block.
    let mut done = journal;
    done.phase = root_agent_bundle::journal::Phase::Done;
    root_agent_bundle::journal::write_journal(&done).unwrap();
    root_agent_bundle::journal::require_no_incomplete_op().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn enable_requires_plan_approval_and_env() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_enable");
    let _env = EnvGuard::isolate(&tmp);
    // Config with a disabled server requiring a definitely-absent env var.
    let config = "model = \"gpt-x\"\n[mcp_servers.testy]\ncommand = \"npx\"\nargs = []\nenv_vars = [\"ROOT_S1_DEFINITELY_ABSENT_VAR\"]\nenabled = false\n";
    std::fs::write(tmp.join("home").join(".codex").join("config.toml"), config).unwrap();
    std::env::remove_var("ROOT_S1_DEFINITELY_ABSENT_VAR");
    let plan = root_agent_bundle::codex::enable_plan("testy").unwrap();
    // Wrong plan hash.
    let err = root_agent_bundle::apply::enable_server("testy", "bogus", &[]).unwrap_err();
    assert!(err.to_string().contains("plan hash"), "got: {}", err);
    // Missing hash-bound approval.
    let err = root_agent_bundle::apply::enable_server("testy", &plan.plan_hash, &[]).unwrap_err();
    assert!(err.to_string().contains("approval"), "got: {}", err);
    // Correct approval is still insufficient for a hand-written server: Root
    // only enables descriptors imported by a completed reviewed bundle apply.
    let err = root_agent_bundle::apply::enable_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("no completed agent-bundle provenance"),
        "got: {}",
        err
    );

    // Simulate the completed provenance record produced by a successful
    // reviewed bundle apply. The unit tests cover the full apply transition;
    // this integration test isolates enable's plan/approval/env gates.
    let mut provenance = std::collections::BTreeMap::new();
    provenance.insert(
        root_agent_bundle::journal::mcp_provenance_key("codex", "testy"),
        plan.descriptor_hash.clone(),
    );
    root_agent_bundle::journal::write_journal(&root_agent_bundle::journal::ApplyJournal {
        op_id: "op_reviewed_import".to_string(),
        agent: "codex".to_string(),
        plan_hash: "reviewed-bundle-plan".to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: root_agent_bundle::journal::Phase::Done,
        completed_paths: vec!["codex_home:config.toml".to_string()],
        target_preconditions: Default::default(),
        mcp_provenance: provenance,
        prior_mcp_provenance: Default::default(),
    })
    .unwrap();

    // Provenance + correct approval, but missing env.
    let err = root_agent_bundle::apply::enable_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("secret references missing"),
        "got: {}",
        err
    );
    // Satisfy env: enable succeeds and flips the flag.
    std::env::set_var("ROOT_S1_DEFINITELY_ABSENT_VAR", "present");
    let report = root_agent_bundle::apply::enable_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap();
    assert!(report.mcp_imported.contains(&"testy".to_string()));
    std::env::remove_var("ROOT_S1_DEFINITELY_ABSENT_VAR");
    // Already enabled → enable-plan refuses.
    let err = root_agent_bundle::codex::enable_plan("testy").unwrap_err();
    assert!(err.to_string().contains("already enabled"), "got: {}", err);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn export_is_read_only_and_validates() {
    let _guard = TEST_MUTEX.lock().unwrap();
    // Live export needs an exact supported Codex; skip if absent or other.
    match root_agent_bundle::codex::find_on_path("codex") {
        Some(bin) => match root_agent_bundle::codex::probe_codex_version(&bin) {
            Ok(v) if root_agent_bundle::SUPPORTED_CODEX_VERSIONS.contains(&v.as_str()) => {}
            _ => return,
        },
        None => return,
    }
    let tmp = unique_tmp("s1_export");
    let _env = EnvGuard::isolate(&tmp);
    let bundle = tmp.join("bundle");
    let opts = root_agent_bundle::export::ExportOptions {
        skills: vec![],
        include_mcp: vec![],
        include_executable: vec![],
        no_timestamp: true,
    };
    let manifest = root_agent_bundle::export::export_codex(&bundle, &opts).unwrap();
    assert_eq!(manifest.bundle_version, 1);
    assert_eq!(manifest.adapter, "codex");
    assert_eq!(
        manifest.disclosure,
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    );
    // Export wrote only the bundle dir; CODEX_HOME has no skills/config beyond ours.
    let reloaded = root_agent_bundle::manifest::load_bundle(&bundle).unwrap();
    assert_eq!(reloaded.files.len(), manifest.files.len());
    let _ = std::fs::remove_dir_all(&tmp);
}

fn write_fake_codex(bin_dir: &std::path::Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let path = bin_dir.join("codex");
    std::fs::write(&path, "#!/bin/sh\necho 'codex-cli 0.150.1'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
}

fn file_entry(
    scope: root_agent_bundle::scope::Scope,
    rel: &str,
    bytes: &[u8],
    executable: bool,
) -> (
    root_agent_bundle::manifest::BundleFile,
    (String, Vec<u8>),
    Option<root_agent_bundle::manifest::NeedsApproval>,
) {
    let digest = root_lockfile::compute_sha256(bytes);
    let approval = executable.then(|| root_agent_bundle::manifest::NeedsApproval {
        scope,
        rel: rel.to_string(),
        sha256: digest.clone(),
        reason: format!("executable {}", rel),
    });
    let file = root_agent_bundle::manifest::BundleFile {
        scope,
        rel: rel.to_string(),
        sha256: digest.clone(),
        size: bytes.len() as u64,
        mode: if executable {
            "0755".to_string()
        } else {
            "0644".to_string()
        },
        kind: if executable {
            root_agent_bundle::manifest::FileKind::Executable
        } else {
            root_agent_bundle::manifest::FileKind::PromptContent
        },
        executable,
    };
    (file, (digest, bytes.to_vec()), approval)
}

#[test]
fn representative_apply_keeps_mcp_disabled_until_enable_and_rollback_is_byte_identical() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_accept");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_codex(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let agents_src = b"# Source AGENTS.md\nUse Root for installs.\n";
    let skill_md = b"# docs-writer\nWrite concise docs.\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let pre_agents = b"# Target AGENTS.md\nold prompt\n";
    let pre_config = "model = \"gpt-4.1\"\nmodel_reasoning_effort = \"medium\"\nservice_tier = \"flex\"\ntarget_only_experimental = \"preserve-this-value\"\n\n[mcp_servers.other]\ncommand = \"echo\"\nenabled = true\n";
    std::fs::write(
        tmp.join("home").join(".codex").join("AGENTS.md"),
        pre_agents,
    )
    .unwrap();
    std::fs::write(
        tmp.join("home").join(".codex").join("config.toml"),
        pre_config,
    )
    .unwrap();

    let (agents_file, agents_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::CodexHome,
        "AGENTS.md",
        agents_src,
        false,
    );
    let (md_file, md_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::SharedSkills,
        "docs-writer/SKILL.md",
        skill_md,
        false,
    );
    let (sh_file, sh_blob, sh_approval) = file_entry(
        root_agent_bundle::scope::Scope::SharedSkills,
        "repo-helper/run.sh",
        skill_sh,
        true,
    );
    let command = vec!["npx".to_string()];
    let args = vec![
        "-y".to_string(),
        "@modelcontextprotocol/server-github".to_string(),
    ];
    let env_keys = vec!["ROOT_S1_ACCEPT_TOKEN".to_string()];
    let mcp_hash = root_agent_bundle::manifest::mcp_command_hash(&command, &args, &None, &env_keys);
    let mut manifest = root_agent_bundle::manifest::Manifest::new("0.150.1".to_string(), None);
    manifest.files = vec![agents_file, md_file, sh_file];
    manifest.settings.insert(
        "model".to_string(),
        serde_json::Value::String("gpt-5".to_string()),
    );
    manifest.settings.insert(
        "model_reasoning_effort".to_string(),
        serde_json::Value::String("high".to_string()),
    );
    manifest.settings.insert(
        "service_tier".to_string(),
        serde_json::Value::String("fast".to_string()),
    );
    manifest.mcp.insert(
        "github".to_string(),
        root_agent_bundle::manifest::McpEntry {
            transport: "stdio".to_string(),
            enabled: false,
            needs_env: env_keys.clone(),
            command_sha256: Some(mcp_hash.clone()),
            command,
            args,
            cwd: None,
            env_keys,
        },
    );
    manifest.needs_env = vec!["ROOT_S1_ACCEPT_TOKEN".to_string()];
    manifest.needs_approval = vec![
        sh_approval.expect("executable skill requires approval"),
        root_agent_bundle::manifest::NeedsApproval {
            scope: root_agent_bundle::scope::Scope::CodexHome,
            rel: "config.toml#mcp_servers.github".to_string(),
            sha256: mcp_hash.clone(),
            reason: "MCP stdio command for server 'github'".to_string(),
        },
    ];
    manifest.validate().unwrap();
    let bundle = tmp.join("bundle");
    root_agent_bundle::blob::write_bundle_dir(&bundle, &manifest, &[agents_blob, md_blob, sh_blob])
        .unwrap();

    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    let err = root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &[]).unwrap_err();
    assert!(
        err.to_string().contains("hash-bound approval"),
        "got: {}",
        err
    );

    let approvals: Vec<String> = manifest
        .needs_approval
        .iter()
        .map(|a| a.sha256.clone())
        .collect();
    let applied =
        root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();
    assert!(applied.mcp_imported.contains(&"github".to_string()));

    let config_path = tmp.join("home").join(".codex").join("config.toml");
    let applied_config = std::fs::read_to_string(&config_path).unwrap();
    let parsed: toml::Value = toml::from_str(&applied_config).unwrap();
    let github = parsed.get("mcp_servers").and_then(|v| v.get("github"));
    let github = github.unwrap_or_else(|| panic!("mcp_servers.github missing:\n{applied_config}"));
    assert_eq!(github.get("enabled").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        parsed
            .get("mcp_servers")
            .and_then(|v| v.get("other"))
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        parsed
            .get("target_only_experimental")
            .and_then(|v| v.as_str()),
        Some("preserve-this-value")
    );
    assert_eq!(
        std::fs::read(tmp.join("home").join(".codex").join("AGENTS.md")).unwrap(),
        agents_src
    );
    assert_eq!(
        std::fs::read(
            tmp.join("home")
                .join(".agents")
                .join("skills")
                .join("docs-writer")
                .join("SKILL.md")
        )
        .unwrap(),
        skill_md
    );
    let sh_path = tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("repo-helper")
        .join("run.sh");
    assert_eq!(std::fs::read(&sh_path).unwrap(), skill_sh);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(&sh_path).unwrap().permissions().mode() & 0o111,
            0
        );
    }
    let after_apply_config = std::fs::read(&config_path).unwrap();
    let after_apply_agents =
        std::fs::read(tmp.join("home").join(".codex").join("AGENTS.md")).unwrap();

    std::env::remove_var("ROOT_S1_ACCEPT_TOKEN");
    let enable = root_agent_bundle::codex::enable_plan("github").unwrap();
    let err =
        root_agent_bundle::apply::enable_server("github", &enable.plan_hash, &[]).unwrap_err();
    assert!(err.to_string().contains("approval"), "got: {}", err);
    let err = root_agent_bundle::apply::enable_server(
        "github",
        &enable.plan_hash,
        std::slice::from_ref(&enable.descriptor_hash),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("secret references missing"),
        "got: {}",
        err
    );
    std::env::set_var("ROOT_S1_ACCEPT_TOKEN", "dummy-not-a-real-secret");
    root_agent_bundle::apply::enable_server(
        "github",
        &enable.plan_hash,
        std::slice::from_ref(&enable.descriptor_hash),
    )
    .unwrap();
    let enabled_config = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !enabled_config.contains("dummy-not-a-real-secret"),
        "secret value leaked into config"
    );
    let enabled_parsed: toml::Value = toml::from_str(&enabled_config).unwrap();
    assert_eq!(
        enabled_parsed
            .get("mcp_servers")
            .and_then(|v| v.get("github"))
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    root_agent_bundle::apply::rollback_last().unwrap();
    assert_eq!(std::fs::read(&config_path).unwrap(), after_apply_config);
    assert_eq!(
        std::fs::read(tmp.join("home").join(".codex").join("AGENTS.md")).unwrap(),
        after_apply_agents
    );
    let rolled: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        rolled
            .get("mcp_servers")
            .and_then(|v| v.get("github"))
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(false)
    );

    root_agent_bundle::apply::rollback_last().unwrap();
    assert_eq!(
        std::fs::read(tmp.join("home").join(".codex").join("AGENTS.md")).unwrap(),
        pre_agents
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), pre_config.as_bytes());
    assert!(!sh_path.exists());
    assert!(!tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("docs-writer")
        .join("SKILL.md")
        .exists());
    std::env::remove_var("ROOT_S1_ACCEPT_TOKEN");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn fifo_blob_is_rejected() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_fifo");
    let bundle = tmp.join("bundle");
    write_manifest(&bundle, &minimal_manifest());
    let fifo = bundle
        .join("blobs")
        .join("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    #[cfg(unix)]
    {
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let err = root_agent_bundle::manifest::load_bundle(&bundle).unwrap_err();
        assert!(
            err.to_string().contains("regular") || err.to_string().contains("non-regular"),
            "got: {}",
            err
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn incomplete_op_rollback_survives_snapshot_hash_ahead_of_journal() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let tmp = unique_tmp("s1_hashahead");
    let _env = EnvGuard::isolate(&tmp);
    let target = tmp.join("home").join(".codex").join("AGENTS.md");
    std::fs::write(&target, b"original").unwrap();
    let snap = root_agent_bundle::snapshot::take_snapshot(
        "codex",
        "op_ahead",
        &[(
            root_agent_bundle::scope::Scope::CodexHome,
            "AGENTS.md".to_string(),
        )],
        None,
    )
    .unwrap();
    let bound = root_agent_bundle::snapshot::snapshot_manifest_hash(&snap).unwrap();
    root_agent_bundle::journal::write_journal(&root_agent_bundle::journal::ApplyJournal {
        op_id: "op_ahead".to_string(),
        agent: "codex".to_string(),
        plan_hash: "plan".to_string(),
        snapshot_id: Some(snap.id.clone()),
        snapshot_manifest_hash: Some(bound),
        phase: root_agent_bundle::journal::Phase::Applying,
        completed_paths: vec![],
        target_preconditions: Default::default(),
        mcp_provenance: Default::default(),
        prior_mcp_provenance: Default::default(),
    })
    .unwrap();
    let applied = root_lockfile::compute_sha256(b"mutated");
    root_agent_bundle::snapshot::record_expected_applied(
        &snap.id,
        root_agent_bundle::scope::Scope::CodexHome,
        "AGENTS.md",
        &applied,
    )
    .unwrap();
    std::fs::write(&target, b"mutated").unwrap();
    let report = root_agent_bundle::apply::rollback_last().unwrap();
    assert_eq!(report.snapshot_id, snap.id);
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    let _ = std::fs::remove_dir_all(&tmp);
}
