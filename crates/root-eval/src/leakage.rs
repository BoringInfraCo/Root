//! Host-leakage detection: the fail-closed contamination check for a trial
//! tree (Milestone 6, smoke-run defect D-b).
//!
//! # The problem
//!
//! A trial's isolation is **process- and environment-level**, not
//! OS-level: the trial `PATH` holds only the trial's fake shims and its
//! stub-bin, but the agent can still name a **host** path directly. The M6
//! smoke run's baseline agent did exactly that — it ran
//! `/usr/bin/find /usr/local /opt /nix -name rg -type f`, found the host's
//! `/opt/homebrew/bin/rg` (ripgrep 15.2.0), symlinked it into the trial's
//! `$TRIAL_STUB_BIN`, and satisfied the `stub-resolves` verifier that way.
//!
//! The consequence is a **measurement** defect, not a task defect: a trial
//! that reaches the host's tool can satisfy a task *without* the skill, so
//! the baseline and the treatment are no longer comparable. Such a trial must
//! never be scored as a pass or as a fail — it is **invalid**.
//!
//! # The two rules
//!
//! The detector is deliberately simple, deterministic, and cheap:
//!
//! | Rule | What it catches |
//! |---|---|
//! | [`LeakKind::SymlinkOutsideRoot`] | a symlink anywhere in the trial tree whose target resolves **outside** the trial root — the exact shape of the smoke run's host symlink |
//! | [`LeakKind::OversizedStubFile`] | a file in the trial's fake-bin / stub-bin directory that is larger than [`MAX_STUB_FILE_BYTES`] — the trial's own stubs are a few hundred bytes each, a host binary is not |
//!
//! Rule 1 catches the symlink; rule 2 catches a **copied** host binary (and
//! a symlink whose target is large). Together they cover the two ways an
//! agent can smuggle host state into the trial's resolution path.
//!
//! # What is *not* scanned
//!
//! The harness's **own** directories are excluded, exactly like the
//! workspace-observation exclusion of the Milestone 6 driver fix (D1/D2):
//!
//! - the project's OpenCode config dir (`<trial>/repo/.opencode`) — it holds
//!   the seeded permission config, the project skill, the seeded plugin
//!   install target (npm's `.bin` symlinks are **relative** and resolve
//!   inside the trial), and the files OpenCode and its npm client write
//!   there;
//! - the global OpenCode config clone (`<trial>/xdg-config/opencode`) — the
//!   same template, cloned per trial.
//!
//! Those trees are the harness's plumbing, not the agent's work, and their
//! contents are not agent-controllable in any meaningful sense. Every other
//! path under the trial root is scanned.
//!
//! # Fail-closed
//!
//! The scan never "clears" a suspicious tree: a finding means the trial is
//! **invalid** ([`crate::TrialOutcomeClass::HostContamination`]), the finding
//! is recorded in the trial's sealed evidence, and the trial tree is
//! **preserved** for inspection instead of being cleaned up.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Largest file the trial's stub directories may hold.
///
/// The trial's own stubs are tiny: the fake `root` shim is ~7 KB, the
/// installer shims ~1.5 KB, and the `rg` stub the fake `root install`
/// writes is ~50 bytes. A host binary (ripgrep alone is ~4 MB) is far above
/// this threshold, so the rule never fires on the harness's own files.
pub const MAX_STUB_FILE_BYTES: u64 = 64 * 1024;

/// Machine-matchable classification for every [`LeakageError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeakageErrorKind {
    /// An I/O operation failed.
    Io,
}

/// Errors produced while scanning a trial tree for host leakage.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LeakageError {
    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },
}

impl LeakageError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> LeakageErrorKind {
        match self {
            Self::Io { .. } => LeakageErrorKind::Io,
        }
    }
}

/// The kind of host leakage one finding reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeakKind {
    /// A symlink in the trial tree whose target resolves outside the trial
    /// root.
    SymlinkOutsideRoot,
    /// A file in a trial stub directory that is larger than
    /// [`MAX_STUB_FILE_BYTES`].
    OversizedStubFile,
}

impl LeakKind {
    /// Stable snake-case token (recorded in the trial evidence).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SymlinkOutsideRoot => "symlink_outside_root",
            Self::OversizedStubFile => "oversized_stub_file",
        }
    }
}

/// One detected host leakage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeakFinding {
    /// The finding's kind token ([`LeakKind::as_str`]).
    pub kind: String,
    /// The offending path, relative to the trial root.
    pub path: String,
    /// What was observed (the resolved target, the size, ...).
    pub detail: String,
}

impl LeakFinding {
    /// The one-line, deterministic rendering used in the summary and in the
    /// report.
    pub fn token(&self) -> String {
        format!("{}:{}:{}", self.kind, self.path, self.detail)
    }
}

/// The result of one host-leakage scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeakageReport {
    /// True when no finding was detected.
    pub clean: bool,
    /// Every finding, in scan order (path-sorted).
    pub findings: Vec<LeakFinding>,
    /// Symlinks inspected.
    pub symlinks_scanned: u64,
    /// Files inspected in the trial's stub directories.
    pub stub_files_scanned: u64,
}

impl LeakageReport {
    /// A report with no findings.
    fn empty() -> Self {
        Self {
            clean: true,
            findings: Vec::new(),
            symlinks_scanned: 0,
            stub_files_scanned: 0,
        }
    }
}

/// Scan one trial tree for host leakage.
///
/// `trial_root` is the trial's root directory; `stub_dirs` are the trial's
/// fake-bin / stub-bin directories (rule 2); `excluded` are the harness's own
/// directories, which are never entered (see the [module docs]).
///
/// The scan is read-only and never modifies the tree.
pub fn detect_host_leakage(
    trial_root: &Path,
    stub_dirs: &[PathBuf],
    excluded: &[PathBuf],
) -> Result<LeakageReport, LeakageError> {
    let report = LeakageReport::empty();
    if !trial_root.is_dir() {
        return Ok(report);
    }
    let root = fs::canonicalize(trial_root).unwrap_or_else(|_| trial_root.to_path_buf());
    let root_lexical = normalize_lexical(trial_root);
    let mut scanner = Scanner {
        trial_root: trial_root.to_path_buf(),
        root_real: root,
        root_lexical,
        stub_dirs: stub_dirs.to_vec(),
        excluded: excluded.to_vec(),
        report,
    };
    scanner.walk(trial_root, 0)?;
    scanner.report.findings.sort_by(|left, right| {
        (left.path.as_str(), left.kind.as_str()).cmp(&(right.path.as_str(), right.kind.as_str()))
    });
    scanner.report.clean = scanner.report.findings.is_empty();
    Ok(scanner.report)
}

/// The walk state of one scan.
struct Scanner {
    trial_root: PathBuf,
    root_real: PathBuf,
    root_lexical: PathBuf,
    stub_dirs: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
    report: LeakageReport,
}

impl Scanner {
    fn walk(&mut self, dir: &Path, depth: usize) -> Result<(), LeakageError> {
        if depth > 64 {
            return Ok(());
        }
        let mut entries: Vec<PathBuf> = Vec::new();
        let read = fs::read_dir(dir).map_err(|error| LeakageError::Io {
            path: dir.to_path_buf(),
            message: error.to_string(),
        })?;
        for entry in read {
            let entry = entry.map_err(|error| LeakageError::Io {
                path: dir.to_path_buf(),
                message: error.to_string(),
            })?;
            entries.push(entry.path());
        }
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).map_err(|error| LeakageError::Io {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if metadata.file_type().is_symlink() {
                self.check_symlink(&path);
                continue;
            }
            if metadata.is_dir() {
                if self.is_excluded(&path) {
                    continue;
                }
                self.walk(&path, depth + 1)?;
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            self.check_stub_file(&path);
        }
        Ok(())
    }

    /// True when `path` is one of the harness's own directories.
    fn is_excluded(&self, path: &Path) -> bool {
        self.excluded.iter().any(|dir| dir == path)
    }

    /// Rule 1: a symlink whose target resolves outside the trial root.
    fn check_symlink(&mut self, path: &Path) {
        self.report.symlinks_scanned += 1;
        let Ok(target) = fs::read_link(path) else {
            return;
        };
        let parent = path.parent().unwrap_or(&self.trial_root);
        let joined = if target.is_absolute() {
            target.clone()
        } else {
            parent.join(&target)
        };
        let lexical = normalize_lexical(&joined);
        if !lexical.starts_with(&self.root_lexical) {
            self.report.findings.push(LeakFinding {
                kind: LeakKind::SymlinkOutsideRoot.as_str().to_string(),
                path: self.relative(path),
                detail: format!("target `{}` is outside the trial root", lexical.display()),
            });
            return;
        }
        // The lexical target is inside the trial root; a symlinked directory
        // on the way could still escape, so the resolved target decides too.
        if let Ok(resolved) = fs::canonicalize(&joined) {
            if !resolved.starts_with(&self.root_real) {
                self.report.findings.push(LeakFinding {
                    kind: LeakKind::SymlinkOutsideRoot.as_str().to_string(),
                    path: self.relative(path),
                    detail: format!(
                        "resolved target `{}` is outside the trial root",
                        resolved.display()
                    ),
                });
            }
        }
    }

    /// Rule 2: a stub-directory file that is too large to be a trial stub.
    fn check_stub_file(&mut self, path: &Path) {
        if !self.is_stub_dir(path) {
            return;
        }
        self.report.stub_files_scanned += 1;
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.len() > MAX_STUB_FILE_BYTES {
            self.report.findings.push(LeakFinding {
                kind: LeakKind::OversizedStubFile.as_str().to_string(),
                path: self.relative(path),
                detail: format!(
                    "{} bytes exceed the {MAX_STUB_FILE_BYTES}-byte stub limit",
                    metadata.len()
                ),
            });
        }
    }

    /// True when `path` lives in one of the trial's stub directories.
    fn is_stub_dir(&self, path: &Path) -> bool {
        self.stub_dirs
            .iter()
            .any(|dir| path.parent() == Some(dir.as_path()))
    }

    /// The trial-relative, `/`-separated path of `path`.
    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.trial_root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
    }
}

/// Normalize a path **lexically**: resolve `.` and `..` without touching the
/// filesystem, so a broken symlink is still classified.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-leakage-{prefix}-{}-{id}",
            std::process::id()
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("clear stale temp dir");
        }
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_clean_trial_is_not_flagged() {
        let root = unique_dir("clean");
        let fake_bin = root.join("fake-bin");
        let stub_bin = root.join("stub-bin");
        fs::create_dir_all(&fake_bin).expect("fake bin");
        fs::create_dir_all(&stub_bin).expect("stub bin");
        fs::write(fake_bin.join("root"), b"#!/bin/sh\nexit 0\n").expect("shim");
        fs::write(stub_bin.join("rg"), b"#!/bin/sh\nexit 0\n").expect("stub");

        let report = detect_host_leakage(&root, &[fake_bin, stub_bin], &[]).expect("scan");
        assert!(report.clean, "{report:#?}");
        assert!(report.findings.is_empty());
        assert_eq!(report.stub_files_scanned, 2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_to_a_host_path_is_detected() {
        let root = unique_dir("symlink");
        let stub_bin = root.join("stub-bin");
        fs::create_dir_all(&stub_bin).expect("stub bin");
        let host = Path::new("/opt/homebrew/bin/rg");
        #[cfg(unix)]
        std::os::unix::fs::symlink(host, stub_bin.join("rg")).expect("plant the host symlink");

        let report =
            detect_host_leakage(&root, std::slice::from_ref(&stub_bin), &[]).expect("scan");
        assert!(!report.clean, "{report:#?}");
        assert_eq!(report.findings.len(), 1, "{report:#?}");
        assert_eq!(
            report.findings[0].kind,
            LeakKind::SymlinkOutsideRoot.as_str(),
            "{report:#?}"
        );
        assert_eq!(report.findings[0].path, "stub-bin/rg", "{report:#?}");
        assert!(
            report.findings[0].detail.contains("outside the trial root"),
            "{report:#?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_inside_the_trial_root_is_not_flagged() {
        let root = unique_dir("inside");
        let stub_bin = root.join("stub-bin");
        fs::create_dir_all(&stub_bin).expect("stub bin");
        fs::write(stub_bin.join("real"), b"#!/bin/sh\nexit 0\n").expect("target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(stub_bin.join("real"), stub_bin.join("link")).expect("link");

        let report =
            detect_host_leakage(&root, std::slice::from_ref(&stub_bin), &[]).expect("scan");
        assert!(report.clean, "{report:#?}");
        assert_eq!(report.symlinks_scanned, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_oversized_stub_file_is_detected() {
        let root = unique_dir("oversized");
        let stub_bin = root.join("stub-bin");
        fs::create_dir_all(&stub_bin).expect("stub bin");
        fs::write(
            stub_bin.join("rg"),
            vec![b'x'; usize::try_from(MAX_STUB_FILE_BYTES + 1).expect("size")],
        )
        .expect("host binary stand-in");

        let report =
            detect_host_leakage(&root, std::slice::from_ref(&stub_bin), &[]).expect("scan");
        assert!(!report.clean, "{report:#?}");
        assert_eq!(
            report.findings[0].kind,
            LeakKind::OversizedStubFile.as_str(),
            "{report:#?}"
        );
        assert_eq!(report.findings[0].path, "stub-bin/rg", "{report:#?}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_harness_own_directories_are_excluded() {
        let root = unique_dir("excluded");
        let harness_dir = root.join("repo").join(".opencode");
        fs::create_dir_all(&harness_dir).expect("harness dir");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/opt/homebrew/bin/rg", harness_dir.join("rg")).expect("link");

        let report = detect_host_leakage(&root, &[], &[harness_dir]).expect("scan");
        assert!(report.clean, "{report:#?}");
        assert_eq!(report.symlinks_scanned, 0);

        let _ = fs::remove_dir_all(&root);
    }
}
