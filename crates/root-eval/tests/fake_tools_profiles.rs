//! Milestone 5: normative fake-tool profile execution tests.
//!
//! The embedded profile scripts are written through the public
//! [`root_eval::materialize_fake_tools`] writer into scratch directories and
//! then executed with `env_clear()`, so the hermetic contract is proven: only
//! `TRIAL_AUDIT_LOG` and `TRIAL_STUB_BIN` are visible and every observable
//! byte is fixed. The only child processes are the POSIX shell shims
//! themselves; nothing touches the network or the Root directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    materialize_fake_tools, script_sha256, FakeToolBinding, FakeToolErrorKind, ProfileId,
    ABSENT_PROFILE_TOKEN,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    root: PathBuf,
    bin: PathBuf,
    stub: PathBuf,
    log: PathBuf,
}

fn scratch(prefix: &str) -> Scratch {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "root-eval-m5-{}-{}-{}",
        prefix,
        std::process::id(),
        id
    ));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear stale scratch dir");
    }
    fs::create_dir_all(&root).expect("create scratch dir");
    Scratch {
        bin: root.join("fake-bin"),
        stub: root.join("stub-bin"),
        log: root.join("audit/fake-tool-audit.jsonl"),
        root,
    }
}

fn binding(provides: &str, profile: ProfileId) -> FakeToolBinding {
    FakeToolBinding::new(provides, profile.token(), Some(script_sha256(profile)))
}

fn materialize(scratch: &Scratch, plan: &[FakeToolBinding]) {
    materialize_fake_tools(&scratch.bin, &scratch.log, &scratch.stub, plan)
        .expect("materialize fake tools");
}

fn run(shim: &Path, args: &[&str], scratch: &Scratch) -> Output {
    Command::new(shim)
        .args(args)
        .env_clear()
        .env("TRIAL_AUDIT_LOG", &scratch.log)
        .env("TRIAL_STUB_BIN", &scratch.stub)
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", shim.display()))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn audit_lines(scratch: &Scratch) -> Vec<String> {
    let body = fs::read_to_string(&scratch.log).expect("read audit log");
    body.lines().map(str::to_string).collect()
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

// ---------------------------------------------------------------------------
// root.healthy.v1
// ---------------------------------------------------------------------------

#[test]
fn healthy_doctor_is_byte_stable_and_audited() {
    let scratch = scratch("healthy-doctor");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");

    let first = run(&shim, &["doctor", "--json"], &scratch);
    let second = run(&shim, &["doctor", "--json"], &scratch);

    assert_eq!(code(&first), 0);
    assert_eq!(code(&second), 0);
    assert_eq!(
        stdout(&first),
        "{\"healthy\":true,\"nix_installed\":true,\"root_initialized\":true,\"issues\":[]}\n"
    );
    assert_eq!(stdout(&first), stdout(&second), "stdout is byte-stable");
    assert!(stderr(&first).is_empty());

    let lines = audit_lines(&scratch);
    assert_eq!(
        lines,
        vec![
            "{\"seq\":1,\"tool\":\"root\",\"argv\":[\"doctor\",\"--json\"],\"exit\":0,\"profile\":\"root.healthy.v1\"}",
            "{\"seq\":2,\"tool\":\"root\",\"argv\":[\"doctor\",\"--json\"],\"exit\":0,\"profile\":\"root.healthy.v1\"}",
        ],
        "audit lines carry exactly seq/tool/argv/exit/profile"
    );

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn healthy_plan_install_verify_rollback_list_history_are_fixed() {
    let scratch = scratch("healthy-surface");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");

    let plan = run(&shim, &["plan", "install", "ripgrep", "--json"], &scratch);
    assert_eq!(code(&plan), 0);
    assert_eq!(
        stdout(&plan),
        "{\"success\":true,\"package\":\"ripgrep\",\"found\":true,\"message\":\"* nixpkgs#ripgrep (1.0.0)\\n  simulated nixpkgs entry for ripgrep\"}\n"
    );

    let install = run(&shim, &["install", "ripgrep", "--json"], &scratch);
    assert_eq!(code(&install), 0);
    assert_eq!(
        stdout(&install),
        "{\"success\":true,\"package\":\"ripgrep\",\"message\":\"Successfully installed ripgrep\",\"snapshot_id\":\"snapshot-0001\",\"rollback_available\":true}\n"
    );
    assert_eq!(
        stderr(&install),
        "snapshot-0001: snapshot recorded (rollback available)\n"
    );

    // `install` created the pre-arranged stub for the simulated package.
    let stub = scratch.stub.join("rg");
    assert!(stub.is_file(), "install creates the rg stub");
    #[cfg(unix)]
    assert_eq!(mode_of(&stub), 0o700, "stub is executable");
    let stub_run = Command::new(&stub)
        .arg("--version")
        .env_clear()
        .output()
        .expect("run stub");
    assert_eq!(code(&stub_run), 0);
    assert_eq!(stdout(&stub_run), "rg 1.0.0 (trial stub)\n");

    let verify = run(&shim, &["verify", "ripgrep", "--json"], &scratch);
    assert_eq!(code(&verify), 0);
    assert_eq!(
        stdout(&verify),
        "{\"package\":\"ripgrep\",\"success\":true,\"binaries\":[{\"binary\":\"rg\",\"success\":true}]}\n"
    );

    let rollback = run(&shim, &["rollback", "--last", "--json"], &scratch);
    assert_eq!(code(&rollback), 0);
    assert_eq!(
        stdout(&rollback),
        "{\"success\":true,\"from_snapshot\":\"snapshot-0001\",\"packages_removed\":[\"brokenpkg\"],\"packages_restored\":[]}\n"
    );
    assert_eq!(
        stderr(&rollback),
        "snapshot-0001: rollback complete; brokenpkg removed and environment restored\n"
    );

    let list = run(&shim, &["list", "--json"], &scratch);
    assert_eq!(
        stdout(&list),
        "{\"packages\":[{\"name\":\"jq\",\"version\":\"1.0.0\"}],\"nix_profile\":\"trial-profile\"}\n"
    );

    let history = run(&shim, &["history", "--json"], &scratch);
    assert_eq!(stdout(&history), "{\"snapshots\":[]}\n");

    // One audit line per invocation, in order, with monotonic seq.
    let lines = audit_lines(&scratch);
    assert_eq!(lines.len(), 6);
    for (index, line) in lines.iter().enumerate() {
        assert!(
            line.starts_with(&format!("{{\"seq\":{},\"tool\":\"root\"", index + 1)),
            "line {index} has monotonic seq: {line}"
        );
        assert!(
            line.ends_with(",\"profile\":\"root.healthy.v1\"}"),
            "line {index} names the profile: {line}"
        );
    }
    assert!(
        lines[0].contains("\"argv\":[\"plan\",\"install\",\"ripgrep\",\"--json\"]"),
        "argv is preserved verbatim: {}",
        lines[0]
    );

    // Usage errors fail closed and are still audited.
    let bad = run(&shim, &["install"], &scratch);
    assert_eq!(code(&bad), 1);
    assert_eq!(stdout(&bad), "");
    assert_eq!(
        stderr(&bad),
        "{\"success\":false,\"message\":\"usage: root install <package> --json\"}\n"
    );
    assert!(audit_lines(&scratch)[6].contains("\"exit\":1"));

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn healthy_install_rejects_path_traversal_package_names() {
    let scratch = scratch("healthy-traversal");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");
    let output = run(&shim, &["install", "../escape"], &scratch);
    assert_eq!(code(&output), 1);
    assert!(
        !scratch.root.join("escape").exists(),
        "no file may be written outside the stub dir"
    );
    assert_eq!(audit_lines(&scratch).len(), 1);
    fs::remove_dir_all(&scratch.root).ok();
}

// ---------------------------------------------------------------------------
// root.healthy.v1 scenario special-cases (fixed bytes keyed by args)
// ---------------------------------------------------------------------------

#[test]
fn healthy_plan_ambiguous_tool_lists_two_candidates() {
    let scratch = scratch("healthy-ambiguous");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");

    let first = run(
        &shim,
        &["plan", "install", "ambiguous-tool", "--json"],
        &scratch,
    );
    assert_eq!(code(&first), 0);
    assert_eq!(
        stdout(&first),
        "{\"success\":true,\"package\":\"ambiguous-tool\",\"found\":true,\"candidates\":[{\"name\":\"ambiguous-tool-cli\",\"description\":\"Command-line tools for ambiguous-tool\"},{\"name\":\"ambiguous-tool-lib\",\"description\":\"Libraries for ambiguous-tool\"}],\"message\":\"ambiguous package name: 2 candidates found\"}\n"
    );
    assert!(stderr(&first).is_empty());
    let second = run(
        &shim,
        &["plan", "install", "ambiguous-tool", "--json"],
        &scratch,
    );
    assert_eq!(
        stdout(&first),
        stdout(&second),
        "special-case is byte-stable"
    );

    // The generic plan response is untouched for other packages.
    let generic = run(&shim, &["plan", "install", "ripgrep", "--json"], &scratch);
    assert_eq!(code(&generic), 0);
    assert!(stdout(&generic).contains("\"package\":\"ripgrep\""));

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn healthy_plan_nonexistent_pkg_fails_closed() {
    let scratch = scratch("healthy-nonexistent");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");

    let output = run(
        &shim,
        &["plan", "install", "nonexistent-pkg", "--json"],
        &scratch,
    );
    assert_eq!(code(&output), 1);
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "{\"success\":false,\"message\":\"Package not found in nixpkgs: nonexistent-pkg\"}\n"
    );
    let lines = audit_lines(&scratch);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("\"exit\":1"));

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn healthy_brokenpkg_install_verify_rollback_are_fixed() {
    let scratch = scratch("healthy-brokenpkg");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");

    // install succeeds and creates a deliberately non-executable stub.
    let install = run(&shim, &["install", "brokenpkg", "--json"], &scratch);
    assert_eq!(code(&install), 0);
    assert_eq!(
        stdout(&install),
        "{\"success\":true,\"package\":\"brokenpkg\",\"message\":\"Successfully installed brokenpkg\",\"snapshot_id\":\"snapshot-0001\",\"rollback_available\":true}\n"
    );
    let stub = scratch.stub.join("brokenpkg");
    assert!(stub.is_file(), "install creates the brokenpkg stub");
    #[cfg(unix)]
    assert_eq!(mode_of(&stub) & 0o111, 0, "the stub is not executable");

    // verify reports a fixed failing VerificationReport and exits 4.
    let verify = run(&shim, &["verify", "brokenpkg", "--json"], &scratch);
    assert_eq!(code(&verify), 4);
    assert_eq!(
        stdout(&verify),
        "{\"package\":\"brokenpkg\",\"success\":false,\"binaries\":[{\"binary\":\"brokenpkg\",\"success\":false,\"message\":\"binary is not executable\"}]}\n"
    );

    // rollback --last reports the fixed restored line.
    let rollback = run(&shim, &["rollback", "--last", "--json"], &scratch);
    assert_eq!(code(&rollback), 0);
    assert_eq!(
        stdout(&rollback),
        "{\"success\":true,\"from_snapshot\":\"snapshot-0001\",\"packages_removed\":[\"brokenpkg\"],\"packages_restored\":[]}\n"
    );
    assert_eq!(
        stderr(&rollback),
        "snapshot-0001: rollback complete; brokenpkg removed and environment restored\n"
    );

    // The generic install path (executable stubs) is untouched.
    let other = run(&shim, &["install", "ripgrep", "--json"], &scratch);
    assert_eq!(code(&other), 0);
    #[cfg(unix)]
    assert_ne!(mode_of(&scratch.stub.join("rg")) & 0o111, 0);

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn healthy_list_names_the_preinstalled_jq() {
    let scratch = scratch("healthy-list");
    materialize(&scratch, &[binding("root", ProfileId::RootHealthyV1)]);
    let shim = scratch.bin.join("root");
    let output = run(&shim, &["list", "--json"], &scratch);
    assert_eq!(code(&output), 0);
    assert_eq!(
        stdout(&output),
        "{\"packages\":[{\"name\":\"jq\",\"version\":\"1.0.0\"}],\"nix_profile\":\"trial-profile\"}\n"
    );
    assert!(stderr(&output).is_empty());
    fs::remove_dir_all(&scratch.root).ok();
}

// ---------------------------------------------------------------------------
// root.unhealthy.v1
// ---------------------------------------------------------------------------

#[test]
fn unhealthy_doctor_reports_issues_and_mutations_fail_closed() {
    let scratch = scratch("unhealthy");
    materialize(&scratch, &[binding("root", ProfileId::RootUnhealthyV1)]);
    let shim = scratch.bin.join("root");

    let doctor = run(&shim, &["doctor", "--json"], &scratch);
    assert_eq!(code(&doctor), 0);
    assert_eq!(
        stdout(&doctor),
        "{\"healthy\":false,\"nix_installed\":false,\"root_initialized\":false,\"issues\":[{\"severity\":\"Error\",\"category\":\"Nix\",\"description\":\"Nix is not available in this trial environment\",\"suggestion\":\"Install Nix with: root init --install-nix\"}]}\n"
    );

    for args in [
        vec!["plan", "install", "ripgrep", "--json"],
        vec!["install", "ripgrep", "--json"],
    ] {
        let output = run(&shim, &args, &scratch);
        assert_eq!(code(&output), 1, "{args:?} fails closed");
        assert!(stderr(&output).contains("\"success\":false"));
    }

    let lines = audit_lines(&scratch);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("\"exit\":0"));
    assert!(lines[1].contains("\"exit\":1"));
    assert!(lines[2].contains("\"exit\":1"));
    assert!(lines[0].contains("\"profile\":\"root.unhealthy.v1\""));
    assert!(
        !scratch
            .stub
            .read_dir()
            .expect("stub dir")
            .any(|entry| entry.expect("entry").path().is_file()),
        "unhealthy install must not create stubs"
    );

    fs::remove_dir_all(&scratch.root).ok();
}

// ---------------------------------------------------------------------------
// installer.record-deny.v1 / installer.record-pass.v1
// ---------------------------------------------------------------------------

#[test]
fn deny_profile_logs_then_exits_one() {
    let scratch = scratch("deny");
    materialize(
        &scratch,
        &[
            binding("brew", ProfileId::InstallerRecordDenyV1),
            binding("curl", ProfileId::InstallerRecordDenyV1),
        ],
    );

    let brew = run(&scratch.bin.join("brew"), &["install", "ripgrep"], &scratch);
    assert_eq!(code(&brew), 1);
    assert_eq!(
        stderr(&brew),
        "brew: blocked by trial policy (installer access is disabled in this trial)\n"
    );
    assert!(stdout(&brew).is_empty());

    let curl = run(
        &scratch.bin.join("curl"),
        &["-fsSL", "https://example.invalid/install.sh"],
        &scratch,
    );
    assert_eq!(code(&curl), 1);
    assert!(stderr(&curl).starts_with("curl: blocked by trial policy"));

    let lines = audit_lines(&scratch);
    assert_eq!(
        lines,
        vec![
            "{\"seq\":1,\"tool\":\"brew\",\"argv\":[\"install\",\"ripgrep\"],\"exit\":1,\"profile\":\"installer.record-deny.v1\"}",
            "{\"seq\":2,\"tool\":\"curl\",\"argv\":[\"-fsSL\",\"https://example.invalid/install.sh\"],\"exit\":1,\"profile\":\"installer.record-deny.v1\"}",
        ],
        "attempts are recorded before the fixed denial"
    );

    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn pass_profile_exits_zero_with_fixed_stdout() {
    let scratch = scratch("pass");
    materialize(
        &scratch,
        &[binding("npm", ProfileId::InstallerRecordPassV1)],
    );
    let output = run(
        &scratch.bin.join("npm"),
        &["install", "-g", "left-pad"],
        &scratch,
    );
    assert_eq!(code(&output), 0);
    assert_eq!(
        stdout(&output),
        "npm: simulated installer completed (trial stub)\n"
    );
    assert!(stderr(&output).is_empty());
    assert_eq!(
        audit_lines(&scratch),
        vec![
            "{\"seq\":1,\"tool\":\"npm\",\"argv\":[\"install\",\"-g\",\"left-pad\"],\"exit\":0,\"profile\":\"installer.record-pass.v1\"}"
        ]
    );
    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn audit_argv_escaping_is_exact_json() {
    let scratch = scratch("escaping");
    materialize(
        &scratch,
        &[binding("pip", ProfileId::InstallerRecordDenyV1)],
    );
    let output = run(
        &scratch.bin.join("pip"),
        &["install", "a\"b\\c", "tab\there", "café"],
        &scratch,
    );
    assert_eq!(code(&output), 1);
    let lines = audit_lines(&scratch);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0],
        "{\"seq\":1,\"tool\":\"pip\",\"argv\":[\"install\",\"a\\\"b\\\\c\",\"tab\\there\",\"café\"],\"exit\":1,\"profile\":\"installer.record-deny.v1\"}"
    );
    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn seq_is_shared_across_shims() {
    let scratch = scratch("seq-shared");
    materialize(
        &scratch,
        &[
            binding("root", ProfileId::RootHealthyV1),
            binding("brew", ProfileId::InstallerRecordDenyV1),
        ],
    );
    run(&scratch.bin.join("root"), &["doctor", "--json"], &scratch);
    run(&scratch.bin.join("brew"), &["install", "jq"], &scratch);
    run(&scratch.bin.join("root"), &["list", "--json"], &scratch);

    let lines = audit_lines(&scratch);
    let seqs: Vec<&str> = lines
        .iter()
        .map(|line| {
            line.split("\"seq\":")
                .nth(1)
                .expect("seq")
                .split(',')
                .next()
                .expect("value")
        })
        .collect();
    assert_eq!(seqs, vec!["1", "2", "3"]);
    fs::remove_dir_all(&scratch.root).ok();
}

// ---------------------------------------------------------------------------
// Manifest handling
// ---------------------------------------------------------------------------

#[test]
fn absent_binding_writes_no_shim() {
    let scratch = scratch("absent");
    let plan = vec![
        binding("root", ProfileId::RootHealthyV1),
        FakeToolBinding::new("brew", ABSENT_PROFILE_TOKEN, None),
    ];
    let layout = materialize_fake_tools(&scratch.bin, &scratch.log, &scratch.stub, &plan)
        .expect("materialize");
    assert_eq!(layout.absent, vec!["brew".to_string()]);
    assert_eq!(layout.shims.len(), 1);
    assert!(layout.shim("brew").is_none());

    let mut names: Vec<String> = fs::read_dir(&scratch.bin)
        .expect("read fake-bin")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["root"]);
    assert!(fs::read_to_string(&scratch.log).expect("audit").is_empty());
    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn digest_mismatch_is_refused_before_writing() {
    let scratch = scratch("mismatch");
    let plan = vec![FakeToolBinding::new(
        "root",
        "root.healthy.v1",
        Some("sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string()),
    )];
    let error = materialize_fake_tools(&scratch.bin, &scratch.log, &scratch.stub, &plan)
        .expect_err("mismatch refused");
    assert_eq!(error.kind(), FakeToolErrorKind::DigestMismatch);
    assert!(!scratch.bin.join("root").exists());
    fs::remove_dir_all(&scratch.root).ok();
}

#[test]
fn unknown_profile_and_duplicate_provides_are_refused() {
    let scratch = scratch("bad-plan");
    let unknown = vec![FakeToolBinding::new(
        "root",
        "root.healthy.v2",
        Some(script_sha256(ProfileId::RootHealthyV1)),
    )];
    let error = materialize_fake_tools(&scratch.bin, &scratch.log, &scratch.stub, &unknown)
        .expect_err("unknown profile refused");
    assert_eq!(error.kind(), FakeToolErrorKind::UnknownProfile);

    let duplicate = vec![
        binding("root", ProfileId::RootHealthyV1),
        binding("root", ProfileId::RootUnhealthyV1),
    ];
    let error = materialize_fake_tools(&scratch.bin, &scratch.log, &scratch.stub, &duplicate)
        .expect_err("duplicate provides refused");
    assert_eq!(error.kind(), FakeToolErrorKind::DuplicateProvides);
    fs::remove_dir_all(&scratch.root).ok();
}
