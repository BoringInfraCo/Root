//! Sprint 010 integration: a long-running workspace must stay usable and
//! resume output must remain concise.
//!
//! This drives the public work/continuity APIs with an isolated `ROOT_DIR`.
//! It does not assert tight timings; durations are measured and printed so
//! reviewers can see the shape without flaky thresholds.

use root_work::{Repository, WorkStore};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct RootDirGuard {
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl RootDirGuard {
    fn set(dir: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let previous = std::env::var_os("ROOT_DIR");
        std::env::set_var("ROOT_DIR", dir);
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for RootDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("ROOT_DIR", value),
            None => std::env::remove_var("ROOT_DIR"),
        }
    }
}

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    root_dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "root_continuity_long_running_{tag}_{}",
            std::process::id()
        ));
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
        Self {
            base,
            repo,
            root_dir,
        }
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.repo).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_artifact(repo: &Path, index: usize) {
    let path = repo.join("src").join(format!("file_{index:03}.ts"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("export const file{index} = {index};\n")).unwrap();
}

#[test]
fn long_running_workspace_stays_usable_and_resume_is_concise() {
    let fixture = Fixture::new("fixture");
    let _guard = RootDirGuard::set(&fixture.root_dir);

    let status_start = Instant::now();
    root_work::workspace_init(&fixture.repo).unwrap();
    root_work::goal_set(&fixture.repo, "Ship engineering continuity").unwrap();
    let status = root_work::workspace_status(&fixture.repo).unwrap();
    assert_eq!(status.counts.decisions, 0);

    for index in 0..25 {
        root_work::decision_add(&fixture.repo, &format!("Decision {index:02}"), None).unwrap();
    }
    for index in 0..50 {
        root_work::finding_add(&fixture.repo, &format!("Finding {index:02}"), None).unwrap();
    }
    for index in 0..100 {
        write_artifact(&fixture.repo, index);
        root_work::artifact_add(&fixture.repo, &format!("src/file_{index:03}.ts")).unwrap();
    }
    for index in 0..20 {
        root_continuity::create(&fixture.repo, Some(&format!("checkpoint {index:02}"))).unwrap();
    }

    {
        let mut store = WorkStore::open_at(&fixture.root_dir, fixture.repository()).unwrap();
        store.start_session(Some("codex"), Some("codex")).unwrap();
        store.start_session(Some("claude"), Some("claude")).unwrap();
    }
    let status_elapsed = status_start.elapsed();

    let resume_start = Instant::now();
    let resume = root_continuity::resume(&fixture.repo, None).unwrap();
    let resume_elapsed = resume_start.elapsed();

    let final_status = root_work::workspace_status(&fixture.repo).unwrap();
    assert_eq!(final_status.counts.goals, 1);
    assert_eq!(final_status.counts.decisions, 25);
    assert_eq!(final_status.counts.findings, 50);
    assert_eq!(final_status.counts.artifacts, 100);
    assert_eq!(final_status.counts.sessions, 2);

    assert_eq!(resume.decisions.len(), 10);
    assert_eq!(resume.decisions_omitted, 15);
    assert_eq!(resume.findings.len(), 10);
    assert_eq!(resume.findings_omitted, 40);
    assert_eq!(resume.artifacts.len(), 20);
    assert_eq!(resume.artifacts_omitted, 80);
    assert!(resume.decisions[0].created_at >= resume.decisions[1].created_at);

    println!("long-running fixture: build+status {status_elapsed:?}, resume {resume_elapsed:?}");
}
