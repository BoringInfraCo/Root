//! Milestone 5 feasibility-matrix CLI wrapper.
//!
//! ```text
//! cargo run -p root-eval --example m5_feasibility -- --out <evals-root>
//! cargo run -p root-eval --example m5_feasibility -- --out <evals-root> --reproduce
//! cargo run -p root-eval --example m5_feasibility -- --out <evals-root> \
//!     --harness opencode-real \
//!     --opencode-binary /path/to/opencode-1.18.27 \
//!     --prewarm-root /path/to/prewarm \
//!     --model opencode/big-pickle
//! ```
//!
//! The first form runs the full ten-task × two-condition × R fake-agent
//! matrix ([`root_eval::run_feasibility_matrix`]) and writes `report.txt`,
//! `report.json`, and `summary.json` under `--out`. The second form rebuilds
//! the report from the sealed store read-only and checks it byte-for-byte
//! ([`root_eval::reproduce_feasibility`]); it exits non-zero when the report
//! is not identical.
//!
//! The third form runs the **real** OpenCode harness: the binary is
//! digest-verified against the pinned `1.18.27` expectation before the first
//! use (the run fails closed on a mismatch), and every trial runs in the
//! hermetic, prewarmed environment of [`root_eval::trial_env`].
//!
//! `--trial-timeout-seconds <n>` caps the wall time of one trial from above.
//! The **effective** cap is `min(record budget, driver cap)`: the record's
//! own `budgets.wall_time_seconds_max` always binds. The cap changes trial
//! semantics, so it is hashed into the plan hash and recorded in
//! `summary.json` and in every trial's sealed `trial-policy` artifact.
//!
//! Defaults: fixtures from this crate's `tests/fixtures/m5/`, seed `881204`,
//! `R = 3`, experiment id `m5-feasibility`, the fake harness, and the
//! repository's real `skills/claude/SKILL.md` as the treatment skill when it
//! exists.

use std::path::PathBuf;
use std::process::ExitCode;

use root_eval::{
    reproduce_feasibility, run_feasibility_matrix, FeasibilityConfig, HarnessChoice,
    RealHarnessConfig,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut out: Option<PathBuf> = None;
    let mut reproduce = false;
    let mut experiment_id = "m5-feasibility".to_string();
    let mut seed: u64 = 881_204;
    let mut repetitions: u32 = 3;
    let mut tasks_dir = fixture_dir("tests/fixtures/m5/tasks");
    let mut prompts_dir = fixture_dir("tests/fixtures/m5/prompts");
    let mut agents_dir = fixture_dir("tests/fixtures/m5/agents");
    let mut treatment_skill = default_treatment_skill();
    let mut harness = HarnessChoice::FakeAgent;
    let mut opencode_binary: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut opencode_args: Vec<String> = Vec::new();
    let mut prewarm_root: Option<PathBuf> = None;
    let mut trial_timeout_seconds: Option<u64> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("`{name}` requires a value"))
        };
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--reproduce" => reproduce = true,
            "--experiment-id" => experiment_id = value("--experiment-id")?,
            "--seed" => {
                seed = value("--seed")?
                    .parse()
                    .map_err(|error| format!("invalid --seed: {error}"))?
            }
            "--repetitions" => {
                repetitions = value("--repetitions")?
                    .parse()
                    .map_err(|error| format!("invalid --repetitions: {error}"))?
            }
            "--tasks-dir" => tasks_dir = PathBuf::from(value("--tasks-dir")?),
            "--prompts-dir" => prompts_dir = PathBuf::from(value("--prompts-dir")?),
            "--agents-dir" => agents_dir = PathBuf::from(value("--agents-dir")?),
            "--skill" => treatment_skill = Some(PathBuf::from(value("--skill")?)),
            "--no-skill" => treatment_skill = None,
            "--harness" => {
                harness = match value("--harness")?.as_str() {
                    "fake" | "fake-agent" => HarnessChoice::FakeAgent,
                    "real" | "opencode-real" => HarnessChoice::OpenCodeReal,
                    other => {
                        return Err(format!(
                            "unknown --harness `{other}` (expected `fake` or `real`)"
                        ))
                    }
                };
            }
            "--opencode-binary" => {
                opencode_binary = Some(PathBuf::from(value("--opencode-binary")?))
            }
            "--model" => model = Some(value("--model")?),
            "--opencode-arg" => opencode_args.push(value("--opencode-arg")?),
            "--prewarm-root" => prewarm_root = Some(PathBuf::from(value("--prewarm-root")?)),
            "--trial-timeout-seconds" => {
                trial_timeout_seconds = Some(
                    value("--trial-timeout-seconds")?
                        .parse()
                        .map_err(|error| format!("invalid --trial-timeout-seconds: {error}"))?,
                )
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let out = out.ok_or_else(|| "--out <evals-root> is required".to_string())?;
    std::fs::create_dir_all(&out).map_err(|error| format!("create {}: {error}", out.display()))?;

    if reproduce {
        let outcome =
            reproduce_feasibility(&out, &experiment_id).map_err(|error| error.to_string())?;
        print!("{}", outcome.render_text());
        if !outcome.report_byte_identical {
            return Err(format!(
                "report is not byte-identical: stored {}, rebuilt {}",
                outcome.stored_report_sha256, outcome.rebuilt_report_sha256
            ));
        }
        return Ok(());
    }

    let mut config =
        FeasibilityConfig::new(&tasks_dir, &prompts_dir, &agents_dir, &out, &experiment_id);
    config.seed = seed;
    config.repetitions = repetitions;
    config.treatment_skill = treatment_skill;
    config.harness = harness;
    config.trial_timeout_seconds = trial_timeout_seconds;
    if harness == HarnessChoice::OpenCodeReal {
        let binary = opencode_binary.ok_or_else(|| {
            "--harness opencode-real requires --opencode-binary <path>".to_string()
        })?;
        let mut real = RealHarnessConfig::new(binary);
        if let Some(model) = model {
            real.model = model;
        }
        real.extra_args = opencode_args;
        config.real = Some(real);
        config.prewarm_root =
            Some(prewarm_root.ok_or_else(|| {
                "--harness opencode-real requires --prewarm-root <dir>".to_string()
            })?);
    }

    let summary = run_feasibility_matrix(&config).map_err(|error| error.to_string())?;
    for trial in &summary.trials {
        println!(
            "{:>3}  {:<38} {:<9} r{}  {:<18} primary_passed={} attempts={}",
            trial.order_index,
            trial.trial_id,
            trial.condition_id,
            trial.repetition,
            trial.outcome_class,
            trial.acceptance_all_primary_passed,
            trial.attempts,
        );
    }
    let report_text = std::fs::read_to_string(out.join("report.txt"))
        .map_err(|error| format!("read report.txt: {error}"))?;
    print!("\n{report_text}");
    println!(
        "Plan hash: {}\nSchedule seed: {} | trials: {} planned, {} completed\n\
         Harness: {} {} (binary sha256 {})\n\
         Trial wall cap: {} (effective = min(record budget, driver cap))\n\
         Canary clean: {} | roots disjoint: {} | residue clean: {} | store intact: {} | \
         host leakage clean: {}",
        summary.plan_hash,
        summary.schedule_seed,
        summary.planned_trials,
        summary.completed_trials,
        summary.harness_name,
        summary.harness_version,
        summary.harness_binary_sha256,
        match summary.trial_timeout_seconds {
            Some(seconds) => format!("{seconds}s (driver cap)"),
            None => "record budget (no driver cap)".to_string(),
        },
        summary.contamination.canary_clean,
        summary.contamination.roots_pairwise_disjoint,
        summary.contamination.residue_clean,
        summary.store_integrity_intact,
        summary.contamination.host_leakage_clean,
    );
    if !summary.store_integrity_intact {
        return Err(format!(
            "store integrity failures: {:#?}",
            summary.store_integrity_failures
        ));
    }
    Ok(())
}

fn fixture_dir(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// The repository's real treatment skill, when this checkout carries it.
fn default_treatment_skill() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/claude/SKILL.md");
    path.is_file().then_some(path)
}
