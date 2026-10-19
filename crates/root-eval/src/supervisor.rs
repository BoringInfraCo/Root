//! Supervised harness execution (Milestone 2, Track B).
//!
//! [`run_supervised`] spawns one harness child with a piped stdout/stderr,
//! an explicit environment, and a working directory, then drains the stream
//! under an output cap and a wall timeout. Termination is classified from
//! process state, never from stream content after a kill: a killed run is
//! `TimeoutKilled` / `CancelledKilled` / `OutputCapExceeded` even when the
//! truncated stream ends with no terminal event (M0 `06` finding — SIGINT is
//! ignored, watchdog TERM yields exit 143 with only `step_start` persisted,
//! and no abort-shaped event is ever flushed).
//!
//! # Process-group kill and reaping
//!
//! On Unix the child is spawned as a process-group leader
//! (`process_group(0)`, i.e. `setpgid(0)` pre-exec), and every kill targets
//! the *negative* group id via `/bin/kill` so tool children (M0 `06b`: orphaned
//! `bash`/`sleep` descendants surviving SIGTERM) die with the parent. After
//! reaping, the supervisor probes the group with `kill -0 -<pgid>`; survivors
//! trigger one more `KILL` pass and are reported in [`ResidueReport`].
//!
//! # Platform limitation
//!
//! Windows has no Unix process groups: [`run_supervised`] falls back to
//! killing the direct child only (`Child::kill`, equivalent to
//! `TerminateProcess`). Tool grandchildren may survive there; cross-platform
//! callers must treat [`ResidueReport::cleanup_ok`] as authoritative and must
//! not assume group semantics outside Unix.
//!
//! # Security notes
//!
//! The child inherits **nothing** unless listed: the environment is cleared
//! (`env_clear`) and rebuilt from the explicit map, `stdin` is null, and `cwd`
//! is set by the caller (expected to be a fresh trial checkout).
//!
//! # Raw stream capture (Milestone 3, Phase A)
//!
//! [`SupervisedResult::raw_stdout`] carries the exact stdout bytes consumed
//! (bounded by [`SupervisorConfig::output_cap_bytes`]) and
//! [`SupervisedResult::raw_stderr`] carries a bounded best-effort stderr
//! capture: the helper thread keeps draining after the cap so a chatty child
//! can never block on a full pipe, but only the first cap bytes are retained.
//! Both are raw evidence for the artifact store; [`SupervisedResult::events`]
//! is still the only interpreted view of stdout.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::digest::sha256_digest;
use crate::events::{parse_ndjson_line, NormalizedEvent};

/// Shared cancellation handle polled by the drain loop.
///
/// Set the flag from any thread; [`run_supervised`] performs the same
/// group-kill path as a timeout and reports [`TerminationKind::CancelledKilled`].
pub type CancelFlag = Arc<AtomicBool>;

/// Allocate a fresh, unset [`CancelFlag`].
pub fn cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

/// Default pause between `TERM` and `KILL` when the caller leaves the grace unset.
pub const DEFAULT_KILL_GRACE_MS: u64 = 2000;

/// How often the drain loop polls child exit, deadlines, and the cancel flag.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Upper bound for the post-exit stdout drain. The reader thread normally
/// observes EOF immediately, so this budget only binds when a surviving
/// grandchild holds the pipe open.
const NATURAL_DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// Upper bound for the post-kill channel drain. Bytes already written to the
/// pipe before the kill must reach the parser; the reader thread forwards
/// them promptly, so this budget only binds under extreme scheduling delay.
const POST_KILL_DRAIN_BUDGET: Duration = Duration::from_secs(2);

/// Upper bound for the stderr helper thread to observe EOF after the child was
/// reaped. Normally immediate; the budget only binds when a surviving
/// grandchild still holds the stderr pipe open.
const STDERR_DRAIN_BUDGET: Duration = Duration::from_secs(1);

/// Move every queued line into `raw_bytes`/`lines`, enforcing the output cap.
///
/// Returns true when the cap fired; on cap the partial remainder (possibly
/// empty) is retained and the caller must kill and mark truncated.
/// Configuration for [`run_supervised`].
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Hard wall budget for the whole run (spawn to reap), in milliseconds.
    pub wall_timeout_ms: u64,
    /// Maximum retained stdout bytes. Breaching the cap stops the drain,
    /// kills the group, and marks [`SupervisedResult::stdout_truncated`].
    pub output_cap_bytes: u64,
    /// Pause between group `TERM` and group `KILL`, in milliseconds.
    pub kill_grace_ms: u64,
    /// Optional cooperative cancellation; checked every poll interval.
    pub cancel: Option<CancelFlag>,
}

impl SupervisorConfig {
    /// Build a config with the default kill grace and no cancel flag.
    pub fn new(wall_timeout_ms: u64, output_cap_bytes: u64) -> Self {
        Self {
            wall_timeout_ms,
            output_cap_bytes,
            kill_grace_ms: DEFAULT_KILL_GRACE_MS,
            cancel: None,
        }
    }
}

/// Machine-matchable terminal classification for a supervised run.
///
/// Kill classifications (`TimeoutKilled`, `CancelledKilled`,
/// `OutputCapExceeded`) are decided from process supervision alone and never
/// from abort-shaped stream content. `ProviderError` and `Denied` are decided
/// from stream content on natural (unkilled) exit: an `error` event or a
/// nonzero exit is a provider error, an ask-mode rejection tool event with a
/// zero exit is a denial, anything else is `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationKind {
    /// Natural exit with no error event and (when applicable) a zero status.
    Completed,
    /// Wall timeout fired; the process group was killed.
    TimeoutKilled,
    /// The [`CancelFlag`] was set; the process group was killed.
    CancelledKilled,
    /// An `error` event arrived or the process exited nonzero on its own.
    ProviderError,
    /// An ask-mode tool denial (`state.status == "error"` with the frozen
    /// rejection message) with no error event and a zero exit.
    Denied,
    /// The child could not be spawned at all.
    SpawnFailed,
    /// Stdout breached [`SupervisorConfig::output_cap_bytes`]; the group was
    /// killed and the stream marked truncated.
    OutputCapExceeded,
}

impl TerminationKind {
    /// True for the three kill paths owned by the supervisor.
    pub fn killed_by_supervisor(self) -> bool {
        matches!(
            self,
            Self::TimeoutKilled | Self::CancelledKilled | Self::OutputCapExceeded
        )
    }

    /// Stable snake-case token used by evidence manifests and eval storage.
    ///
    /// Identical to the `serde` representation (`snake_case`), asserted by
    /// test, so persisted records and in-memory projections never drift.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::TimeoutKilled => "timeout_killed",
            Self::CancelledKilled => "cancelled_killed",
            Self::ProviderError => "provider_error",
            Self::Denied => "denied",
            Self::SpawnFailed => "spawn_failed",
            Self::OutputCapExceeded => "output_cap_exceeded",
        }
    }

    /// Parse the stable token produced by [`TerminationKind::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "completed" => Some(Self::Completed),
            "timeout_killed" => Some(Self::TimeoutKilled),
            "cancelled_killed" => Some(Self::CancelledKilled),
            "provider_error" => Some(Self::ProviderError),
            "denied" => Some(Self::Denied),
            "spawn_failed" => Some(Self::SpawnFailed),
            "output_cap_exceeded" => Some(Self::OutputCapExceeded),
            _ => None,
        }
    }
}

/// Post-run hygiene report for the killed or reaped process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidueReport {
    /// True when the group probe (`kill -0 -<pgid>`) found no survivors, or
    /// when no group kill was needed (natural exit, spawn failure, Windows).
    pub cleanup_ok: bool,
    /// Number of group members still alive after the first kill pass (0 when
    /// the first pass sufficed or no kill was needed).
    pub leftover_killed: u32,
    /// Human-readable note (kill path taken, probe outcome, platform fallback).
    pub note: String,
}

/// One stream line the supervisor could not parse.
///
/// Lines are never silently skipped: unparseable bytes (e.g. a torn trailing
/// write after a kill) are reported here with their sequence and reason so the
/// trial layer can surface them as missing telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedLine {
    /// Zero-based line position in stream order.
    pub sequence: u64,
    /// Parser error message.
    pub reason: String,
}

/// The complete supervised-run outcome.
#[derive(Debug, Clone)]
pub struct SupervisedResult {
    /// Terminal classification (kill paths decided from process state alone).
    pub termination: TerminationKind,
    /// Process exit code (`None` when signaled or never spawned).
    pub exit_code: Option<i32>,
    /// True only for [`TerminationKind::TimeoutKilled`].
    pub timed_out: bool,
    /// True when the supervisor sent kill signals to the child/group.
    pub killed: bool,
    /// Wall time from spawn (or spawn attempt) to reap, in milliseconds.
    pub wall_ms: u64,
    /// Parsed stream events in stream order.
    pub events: Vec<NormalizedEvent>,
    /// Lines that failed to parse (never silently dropped).
    pub dropped_lines: Vec<DroppedLine>,
    /// `sha256:<hex>` over the exact stdout bytes consumed.
    pub raw_bytes_sha256: String,
    /// Exact stdout bytes consumed (bounded by the output cap).
    ///
    /// This is the raw execution artifact: callers that persist evidence store
    /// these bytes content-addressed and keep [`Self::raw_bytes_sha256`] as the
    /// reference. [`Self::events`] remains the only interpreted view.
    pub raw_stdout: Vec<u8>,
    /// Bounded best-effort stderr capture (at most the output cap).
    ///
    /// The helper thread keeps draining after the cap so a chatty child cannot
    /// block on a full pipe, but only the first cap bytes are retained; a
    /// stderr writer that outlives the child may leave bytes unread.
    pub raw_stderr: Vec<u8>,
    /// True when the output cap fired (always true with dropped torn tails
    /// after a kill; otherwise false unless the cap fired).
    pub stdout_truncated: bool,
    /// Group-reaping hygiene report.
    pub residue: ResidueReport,
}

/// Spawn `cmd` under supervision and drain it to completion or kill.
///
/// `cmd[0]` is the program; the child runs with `cwd`, a cleared environment
/// rebuilt from `env`, null stdin, and piped stdout/stderr. This function
/// blocks until the child is reaped (naturally or via the two-stage
/// group-kill path) and never returns until the process group is probed.
pub fn run_supervised(
    cmd: &[String],
    env: &[(String, String)],
    cwd: &Path,
    config: &SupervisorConfig,
) -> SupervisedResult {
    let started = Instant::now();
    // Bounded stderr capture shared with the drain thread. `done` starts true
    // so the snapshot helper is safe on the pre-spawn failure paths.
    let stderr_buffer: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_done = Arc::new(AtomicBool::new(true));
    let stderr_now = || {
        let until = Instant::now() + STDERR_DRAIN_BUDGET;
        while !stderr_done.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(POLL_INTERVAL);
        }
        stderr_buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    };
    let finish = |termination: TerminationKind,
                  exit_code: Option<i32>,
                  killed: bool,
                  events: Vec<NormalizedEvent>,
                  dropped_lines: Vec<DroppedLine>,
                  raw_bytes: Vec<u8>,
                  stdout_truncated: bool,
                  residue: ResidueReport| {
        SupervisedResult {
            termination,
            exit_code,
            timed_out: termination == TerminationKind::TimeoutKilled,
            killed,
            wall_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            events,
            dropped_lines,
            raw_bytes_sha256: sha256_digest(&raw_bytes),
            raw_stdout: raw_bytes,
            raw_stderr: stderr_now(),
            stdout_truncated,
            residue,
        }
    };

    let (program, args) = match cmd.split_first() {
        Some((program, args)) => (program, args),
        None => {
            return finish(
                TerminationKind::SpawnFailed,
                None,
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                false,
                ResidueReport {
                    cleanup_ok: true,
                    leftover_killed: 0,
                    note: "empty command: nothing spawned".to_string(),
                },
            );
        }
    };

    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(cwd);
    command.env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Become a process-group leader so group-directed signals reach tool
        // children (M0 06b) as well as the harness itself.
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return finish(
                TerminationKind::SpawnFailed,
                None,
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                false,
                ResidueReport {
                    cleanup_ok: true,
                    leftover_killed: 0,
                    note: format!("spawn failed: {error}"),
                },
            );
        }
    };

    // Group id == child pid on Unix (setpgid leader). Used only for signals.
    #[cfg(unix)]
    let pgid: u32 = child.id();
    #[cfg(not(unix))]
    let pgid: u32 = 0;

    // Drain stderr on a helper thread so a chatty child can never block on a
    // full pipe, retaining at most `output_cap_bytes` bytes. The thread keeps
    // reading after the cap (discarding the overflow) and signals completion
    // through `stderr_done`.
    if let Some(stderr) = child.stderr.take() {
        let buffer = Arc::clone(&stderr_buffer);
        let done = Arc::clone(&stderr_done);
        let cap = config.output_cap_bytes as usize;
        done.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut chunk = [0u8; 8192];
            let mut captured = 0usize;
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if captured < cap {
                            let take = (cap - captured).min(n);
                            buffer
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .extend_from_slice(&chunk[..take]);
                            captured += take;
                        }
                    }
                }
            }
            done.store(true, Ordering::SeqCst);
        });
    }

    let stdout = child.stdout.take().expect("stdout was piped");
    let (line_tx, line_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut buf = Vec::new();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if line_tx.send(buf).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Duration::from_millis(config.wall_timeout_ms);
    let grace = Duration::from_millis(config.kill_grace_ms);
    let mut raw_bytes: Vec<u8> = Vec::new();
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
    let mut kill: Option<KillReason> = None;

    // Returns true when a kill was issued on this pass.
    let poll_once = |pending: &mut VecDeque<Vec<u8>>,
                     raw_bytes: &mut Vec<u8>,
                     lines: &mut Vec<Vec<u8>>,
                     kill: &mut Option<KillReason>| {
        while let Ok(buf) = line_rx.try_recv() {
            pending.push_back(buf);
        }
        while let Some(buf) = pending.pop_front() {
            if raw_bytes.len().saturating_add(buf.len()) > config.output_cap_bytes as usize {
                *kill = Some(KillReason::OutputCap);
                return true;
            }
            raw_bytes.extend_from_slice(&buf);
            lines.push(buf);
        }
        if config
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            *kill = Some(KillReason::Cancelled);
            return true;
        }
        if started.elapsed() >= deadline {
            *kill = Some(KillReason::Timeout);
            return true;
        }
        false
    };

    let mut exit_status = None;
    loop {
        if poll_once(&mut pending, &mut raw_bytes, &mut lines, &mut kill) {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // (killed, exit_code, residue, truncated, kill_reason); events are parsed
    // once, after the match, from the finalized lines.
    let (killed, exit_code, residue, truncated, kill_reason): (
        bool,
        Option<i32>,
        ResidueReport,
        bool,
        Option<KillReason>,
    ) = match kill {
        Some(reason) => {
            let residue = kill_group(pgid, &mut child, grace);
            // Reap; blocking wait also collects zombies of the direct child.
            let exit_code = child.wait().ok().and_then(|status| status.code());
            // The reader thread may not have forwarded pipe bytes to the
            // channel yet; drain it (bounded) before parsing so kill timing
            // cannot lose already-written lines.
            drain_reader(&line_rx, &mut pending, POST_KILL_DRAIN_BUDGET);
            flush_pending(
                &mut pending,
                &mut raw_bytes,
                &mut lines,
                config.output_cap_bytes as usize,
            );
            (true, exit_code, residue, true, Some(reason))
        }
        None => {
            // Natural exit: the reader thread forwards lines until it observes
            // EOF (sender drop). The drain is time-boxed so a surviving
            // grandchild holding the stdout pipe cannot hang the supervisor.
            let drain_until = Instant::now() + NATURAL_DRAIN_BUDGET;
            let mut disconnected = false;
            while !disconnected && Instant::now() < drain_until {
                match line_rx.recv_timeout(POLL_INTERVAL) {
                    Ok(buf) => pending.push_back(buf),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
                }
                if flush_pending(
                    &mut pending,
                    &mut raw_bytes,
                    &mut lines,
                    config.output_cap_bytes as usize,
                ) {
                    let residue = kill_group(pgid, &mut child, grace);
                    let _ = child.wait();
                    drain_reader(&line_rx, &mut pending, POST_KILL_DRAIN_BUDGET);
                    flush_pending(
                        &mut pending,
                        &mut raw_bytes,
                        &mut lines,
                        config.output_cap_bytes as usize,
                    );
                    let (events, dropped) = parse_lines(&lines);
                    return finish(
                        TerminationKind::OutputCapExceeded,
                        None,
                        true,
                        events,
                        dropped,
                        raw_bytes,
                        true,
                        residue,
                    );
                }
            }
            while let Ok(buf) = line_rx.try_recv() {
                pending.push_back(buf);
            }
            if flush_pending(
                &mut pending,
                &mut raw_bytes,
                &mut lines,
                config.output_cap_bytes as usize,
            ) {
                let residue = kill_group(pgid, &mut child, grace);
                let _ = child.wait();
                let (events, dropped) = parse_lines(&lines);
                return finish(
                    TerminationKind::OutputCapExceeded,
                    None,
                    true,
                    events,
                    dropped,
                    raw_bytes,
                    true,
                    residue,
                );
            }
            let status = exit_status
                .or_else(|| child.try_wait().ok().flatten())
                .or_else(|| child.wait().ok());
            let exit_code = status.and_then(|status| status.code());
            let residue = ResidueReport {
                cleanup_ok: true,
                leftover_killed: 0,
                note: "natural exit: no group kill needed".to_string(),
            };
            (false, exit_code, residue, false, None)
        }
    };

    let (events, dropped) = parse_lines(&lines);
    let termination = match kill_reason {
        Some(KillReason::Timeout) => TerminationKind::TimeoutKilled,
        Some(KillReason::Cancelled) => TerminationKind::CancelledKilled,
        Some(KillReason::OutputCap) => TerminationKind::OutputCapExceeded,
        None => classify_natural_exit(&events, exit_code),
    };
    finish(
        termination,
        exit_code,
        killed,
        events,
        dropped,
        raw_bytes,
        truncated,
        residue,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KillReason {
    Timeout,
    Cancelled,
    OutputCap,
}

/// Bounded drain of the reader-thread channel into `pending`.
///
/// Returns after the reader observes EOF (sender drop) or the budget lapses,
/// then sweeps any last queued sends without blocking.
fn drain_reader(
    line_rx: &mpsc::Receiver<Vec<u8>>,
    pending: &mut VecDeque<Vec<u8>>,
    budget: Duration,
) {
    let until = Instant::now() + budget;
    while Instant::now() < until {
        match line_rx.recv_timeout(POLL_INTERVAL) {
            Ok(buf) => pending.push_back(buf),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    while let Ok(buf) = line_rx.try_recv() {
        pending.push_back(buf);
    }
}

fn flush_pending(
    pending: &mut VecDeque<Vec<u8>>,
    raw_bytes: &mut Vec<u8>,
    lines: &mut Vec<Vec<u8>>,
    cap_bytes: usize,
) -> bool {
    while let Some(buf) = pending.pop_front() {
        if raw_bytes.len().saturating_add(buf.len()) > cap_bytes {
            let room = cap_bytes.saturating_sub(raw_bytes.len());
            raw_bytes.extend_from_slice(&buf[..room]);
            let mut partial = buf;
            partial.truncate(room);
            if !partial.is_empty() {
                lines.push(partial);
            }
            return true;
        }
        raw_bytes.extend_from_slice(&buf);
        lines.push(buf);
    }
    false
}

fn parse_lines(lines: &[Vec<u8>]) -> (Vec<NormalizedEvent>, Vec<DroppedLine>) {
    let mut events = Vec::with_capacity(lines.len());
    let mut dropped = Vec::new();
    for (index, raw) in lines.iter().enumerate() {
        let sequence = index as u64;
        // A torn trailing write (kill mid-line) is not valid UTF-8 or not
        // valid JSON; report it instead of silently skipping it.
        let text = match std::str::from_utf8(raw) {
            Ok(text) => text.trim_end_matches(['\n', '\r']),
            Err(error) => {
                dropped.push(DroppedLine {
                    sequence,
                    reason: format!("non-UTF-8 stdout bytes: {error}"),
                });
                continue;
            }
        };
        if text.is_empty() {
            dropped.push(DroppedLine {
                sequence,
                reason: "empty line".to_string(),
            });
            continue;
        }
        match parse_ndjson_line(text, sequence) {
            Ok(event) => events.push(event),
            Err(error) => dropped.push(DroppedLine {
                sequence,
                reason: error.to_string(),
            }),
        }
    }
    (events, dropped)
}

/// Classify a naturally-exited run from stream content and exit code.
///
/// Kill paths never reach this function, so abort-shaped events after a kill
/// cannot influence the verdict. Precedence: `error` event, then nonzero
/// exit, then ask-denial tool event, else completed.
fn classify_natural_exit(events: &[NormalizedEvent], exit_code: Option<i32>) -> TerminationKind {
    if events.iter().any(|event| event.error.is_some()) {
        return TerminationKind::ProviderError;
    }
    if exit_code.is_some_and(|code| code != 0) {
        return TerminationKind::ProviderError;
    }
    if events.iter().any(NormalizedEvent::is_ask_denial) {
        return TerminationKind::Denied;
    }
    TerminationKind::Completed
}

/// Two-stage group kill: `TERM`, grace, `KILL`, then probe for survivors.
///
/// On non-Unix platforms this degrades to `Child::kill` on the direct child.
fn kill_group(pgid: u32, child: &mut std::process::Child, grace: Duration) -> ResidueReport {
    #[cfg(unix)]
    {
        signal_group(pgid, "-TERM");
        let waited = Instant::now();
        while waited.elapsed() < grace {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let exited = child.try_wait().ok().flatten().is_some();
        if !exited {
            signal_group(pgid, "-KILL");
            std::thread::sleep(Duration::from_millis(50));
        }
        // Probe: `kill -0 -<pgid>` succeeds while any group member lives.
        if group_alive(pgid) {
            signal_group(pgid, "-KILL");
            std::thread::sleep(Duration::from_millis(100));
            if group_alive(pgid) {
                return ResidueReport {
                    cleanup_ok: false,
                    leftover_killed: 1,
                    note: format!(
                        "group {pgid} still alive after TERM+KILL; survivors reported, not reaped"
                    ),
                };
            }
            return ResidueReport {
                cleanup_ok: true,
                leftover_killed: 1,
                note: format!("group {pgid} needed a second KILL pass; probe is now clear"),
            };
        }
        ResidueReport {
            cleanup_ok: true,
            leftover_killed: 0,
            note: format!("group {pgid} killed and reaped; probe clear"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        let _ = grace;
        let _ = child.kill();
        ResidueReport {
            cleanup_ok: true,
            leftover_killed: 0,
            note: "non-Unix platform: direct child kill only; group semantics unavailable"
                .to_string(),
        }
    }
}

#[cfg(unix)]
fn signal_group(pgid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, &format!("-{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
fn group_alive(pgid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &format!("-{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// Write a fake-harness shell script into a fresh temp dir and return the
    /// dir. The script emits golden-shaped NDJSON with controllable delays.
    fn fake_harness_dir(name: &str, script: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("root-eval-sup-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fake harness dir");
        let path = dir.join("harness.sh");
        std::fs::write(&path, script).expect("write fake harness");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod +x");
        }
        dir
    }

    fn run_sh(dir: &Path, config: &SupervisorConfig) -> SupervisedResult {
        let script = dir.join("harness.sh").to_string_lossy().into_owned();
        run_supervised(&[script, "session".to_string()], &[], dir, config)
    }

    fn normal_script() -> &'static str {
        r#"#!/bin/sh
echo '{"type":"step_start","timestamp":1,"sessionID":"ses_fake1","part":{"type":"step-start"}}'
sleep 0.05
echo '{"type":"text","timestamp":2,"sessionID":"ses_fake1","part":{"type":"text","text":"ok"}}'
sleep 0.05
echo '{"type":"step_finish","timestamp":3,"sessionID":"ses_fake1","part":{"type":"step-finish","reason":"stop","tokens":{"total":10,"input":8,"output":1,"reasoning":1,"cache":{"write":0,"read":0}},"cost":0}}'
"#
    }

    #[test]
    fn normal_eof_settles_as_completed() {
        let dir = fake_harness_dir("normal", normal_script());
        let config = SupervisorConfig::new(5_000, 1024 * 1024);
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::Completed);
        assert!(!result.killed && !result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.events.len(), 3);
        assert!(result.dropped_lines.is_empty());
        assert!(!result.stdout_truncated);
        assert!(result.residue.cleanup_ok);
        assert!(result.raw_bytes_sha256.starts_with("sha256:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_kills_sleeper_and_its_child() {
        // Parent spawns a `sleep` grandchild, prints its pid, then blocks:
        // the group kill must reap both, and classification must be pure
        // timeout even though the stream has no terminal (and no abort) event.
        //
        // The wall is generous on purpose: under parallel test load this host
        // delays some spawns past process creation (child alive but not yet
        // executing) for hundreds of milliseconds, so a tight wall can fire
        // before the first line flushes. The supervisor classifies correctly
        // either way; the event assertion below needs exec to have happened.
        let dir = fake_harness_dir(
            "timeout",
            r#"#!/bin/sh
echo '{"type":"step_start","timestamp":1,"sessionID":"ses_fakeT","part":{"type":"step-start"}}'
sleep 30 &
echo "GRANDCHILD:$!"
wait
"#,
        );
        let config = SupervisorConfig {
            wall_timeout_ms: 2000,
            output_cap_bytes: 1024 * 1024,
            kill_grace_ms: 100,
            cancel: None,
        };
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::TimeoutKilled);
        assert!(result.killed && result.timed_out);
        assert!(result.events.iter().all(|event| event.error.is_none()));
        assert_eq!(
            result.events.len(),
            1,
            "only step_start flushed before kill"
        );
        assert!(result.stdout_truncated);

        // The group kill must leave no survivors: the residue probe is the
        // authoritative signal (a dedicated ps-sweep test follows).
        assert!(result.residue.cleanup_ok, "{}", result.residue.note);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_kill_leaves_no_surviving_sleep() {
        let sentinel = format!("root-eval-sentinel-{}", std::process::id());
        let script_body = format!("#!/bin/sh\nexec -a \"{sentinel}\" sleep 30 &\nwait\n");
        let dir = fake_harness_dir("timeout-sweep", &script_body);
        let script = dir.join("harness.sh").to_string_lossy().into_owned();
        let config = SupervisorConfig {
            wall_timeout_ms: 400,
            output_cap_bytes: 1024 * 1024,
            kill_grace_ms: 100,
            cancel: None,
        };
        let result = run_supervised(&[script], &[], &dir, &config);
        assert_eq!(result.termination, TerminationKind::TimeoutKilled);
        // PIDs carrying our sentinel argv[0] must be gone from the table.
        let probe = Command::new("sh")
            .args([
                "-c",
                &format!("ps -eo args | grep -F \"{sentinel}\" | grep -v grep || true"),
            ])
            .output()
            .expect("ps probe");
        let out = String::from_utf8_lossy(&probe.stdout);
        assert!(
            out.trim().is_empty(),
            "orphaned sleeper survived the group kill: {out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_cap_kills_and_marks_truncated() {
        let dir = fake_harness_dir(
            "cap",
            r#"#!/bin/sh
i=0
while [ "$i" -lt 5000 ]; do
  echo '{"type":"text","timestamp":1,"sessionID":"ses_cap","part":{"type":"text","text":"padding-padding-padding-padding"}}'
  i=$((i+1))
done
"#,
        );
        let config = SupervisorConfig {
            wall_timeout_ms: 10_000,
            output_cap_bytes: 1024,
            kill_grace_ms: 100,
            cancel: None,
        };
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::OutputCapExceeded);
        assert!(result.killed && result.stdout_truncated);
        assert!(!result.timed_out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_flag_kills_with_cancelled_classification() {
        let dir = fake_harness_dir(
            "cancel",
            r#"#!/bin/sh
echo '{"type":"step_start","timestamp":1,"sessionID":"ses_fakeC","part":{"type":"step-start"}}'
sleep 30
"#,
        );
        let flag = cancel_flag();
        let setter = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            setter.store(true, Ordering::SeqCst);
        });
        let config = SupervisorConfig {
            wall_timeout_ms: 10_000,
            output_cap_bytes: 1024 * 1024,
            kill_grace_ms: 100,
            cancel: Some(flag),
        };
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::CancelledKilled);
        assert!(result.killed && !result.timed_out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn termination_taxonomy_from_stream_content() {
        // Provider error: error event + nonzero exit.
        let dir = fake_harness_dir(
            "provider",
            r#"#!/bin/sh
echo '{"type":"error","timestamp":1,"sessionID":"ses_fakeE","error":{"name":"UnknownError","data":{"message":"boom","ref":"err_1"}}}}'
exit 1
"#,
        );
        let config = SupervisorConfig::new(5_000, 1024 * 1024);
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::ProviderError);
        assert!(!result.killed);
        assert_eq!(result.exit_code, Some(1));

        // Denied: ask-reject tool event, zero exit, no error event.
        let dir = fake_harness_dir(
            "denied",
            r#"#!/bin/sh
echo '{"type":"step_start","timestamp":1,"sessionID":"ses_fakeD","part":{"type":"step-start"}}'
echo '{"type":"tool_use","timestamp":2,"sessionID":"ses_fakeD","part":{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"error","input":{"command":"echo hi"},"error":"The user rejected permission to use this specific tool call."}}}'
exit 0
"#,
        );
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::Denied);

        // Completed: zero exit, no error, no denial.
        let dir = fake_harness_dir("completed", normal_script());
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::Completed);
        for name in ["provider", "denied", "completed"] {
            let _ = std::fs::remove_dir_all(
                std::env::temp_dir().join(format!("root-eval-sup-{}-{name}", std::process::id())),
            );
        }
    }

    #[test]
    fn kill_with_no_trailing_events_still_classifies_timeout() {
        // No-abort-reliance: the stream carries one part, then the wall fires.
        // Classification must be TimeoutKilled regardless of stream content.
        let dir = fake_harness_dir(
            "no-abort",
            r#"#!/bin/sh
echo '{"type":"step_start","timestamp":1,"sessionID":"ses_fakeN","part":{"type":"step-start"}}'
sleep 30
"#,
        );
        let config = SupervisorConfig {
            wall_timeout_ms: 300,
            output_cap_bytes: 1024 * 1024,
            kill_grace_ms: 100,
            cancel: None,
        };
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::TimeoutKilled);
        assert!(result.events.iter().all(|event| event.error.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_failure_reports_spawn_failed() {
        let dir = std::env::temp_dir();
        let config = SupervisorConfig::new(5_000, 1024 * 1024);
        let result = run_supervised(
            &["/nonexistent-root-eval-binary-xyz".to_string()],
            &[],
            &dir,
            &config,
        );
        assert_eq!(result.termination, TerminationKind::SpawnFailed);
        assert_eq!(result.exit_code, None);
        assert!(!result.killed);

        let empty = run_supervised(&[], &[], &dir, &config);
        assert_eq!(empty.termination, TerminationKind::SpawnFailed);
    }

    #[test]
    fn explicit_env_replaces_inheritance() {
        let dir = fake_harness_dir(
            "env",
            r#"#!/bin/sh
if [ -n "$ROOT_EVAL_SECRET_INHERIT_ME" ]; then echo "LEAKED"; else echo "CLEAN"; fi
echo "VAL=$ROOT_EVAL_TRIAL_VAR"
"#,
        );
        unsafe {
            std::env::set_var("ROOT_EVAL_SECRET_INHERIT_ME", "parent-secret");
        }
        let script = dir.join("harness.sh").to_string_lossy().into_owned();
        let config = SupervisorConfig::new(5_000, 1024 * 1024);
        let result = run_supervised(
            &[script],
            &[("ROOT_EVAL_TRIAL_VAR".to_string(), "trial-value".to_string())],
            &dir,
            &config,
        );
        unsafe {
            std::env::remove_var("ROOT_EVAL_SECRET_INHERIT_ME");
        }
        assert_eq!(result.termination, TerminationKind::Completed);
        assert_eq!(result.events.len(), 0);
        // Plain-text stdout lines are not NDJSON: they land in dropped_lines,
        // never silently skipped — but the secret must not appear anywhere.
        let raw_hashes: Vec<_> = result.events.iter().map(|_| ()).collect();
        assert!(raw_hashes.is_empty());
        for dropped in &result.dropped_lines {
            assert!(!dropped.reason.contains("parent-secret"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_stdout_and_stderr_bytes_are_captured() {
        let dir = fake_harness_dir(
            "raw",
            r#"#!/bin/sh
echo 'not-ndjson'
echo 'oops' >&2
"#,
        );
        let config = SupervisorConfig::new(5_000, 1024 * 1024);
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::Completed);
        assert_eq!(result.raw_stdout, b"not-ndjson\n");
        assert_eq!(result.raw_stderr, b"oops\n");
        assert_eq!(result.raw_bytes_sha256, sha256_digest(&result.raw_stdout));
        // Plain-text stdout lines remain reported, never silently skipped.
        assert_eq!(result.dropped_lines.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stderr_capture_is_bounded_by_the_output_cap() {
        let dir = fake_harness_dir(
            "stderr-cap",
            r#"#!/bin/sh
i=0
while [ "$i" -lt 5000 ]; do
  echo 'stderr-padding-padding-padding' >&2
  i=$((i+1))
done
"#,
        );
        let config = SupervisorConfig {
            wall_timeout_ms: 10_000,
            output_cap_bytes: 256,
            kill_grace_ms: 100,
            cancel: None,
        };
        let result = run_sh(&dir, &config);
        assert_eq!(result.termination, TerminationKind::Completed);
        assert_eq!(result.raw_stderr.len(), 256, "stderr must be capped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn termination_tokens_match_the_serde_representation() {
        for kind in [
            TerminationKind::Completed,
            TerminationKind::TimeoutKilled,
            TerminationKind::CancelledKilled,
            TerminationKind::ProviderError,
            TerminationKind::Denied,
            TerminationKind::SpawnFailed,
            TerminationKind::OutputCapExceeded,
        ] {
            let serialized = serde_json::to_value(kind).expect("serializable");
            assert_eq!(
                serialized,
                serde_json::Value::String(kind.as_str().to_string()),
                "as_str and serde must agree for {kind:?}"
            );
            assert_eq!(TerminationKind::from_token(kind.as_str()), Some(kind));
        }
        assert_eq!(TerminationKind::from_token("nope"), None);
    }
}
