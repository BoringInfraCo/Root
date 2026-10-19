# Milestone 4 — Executive Summary & Verdict: Scheduler + Paired Feasibility Report

Date: 2026-10-19 · Repo checkout: `a5b1f8e` + M4 working tree
Scope basis: audit's Milestone 4 (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*) **as adapted by the recorded feasibility-spike scope decision**: the verdict/uncertainty engine is explicitly deferred; this milestone delivers the deterministic scheduler and the scripted paired feasibility report with honest labels only.
Implementation: two subagent phases (scheduler module; wiring + report), independently CI-gated by the lead agent

## Verdict

**MILESTONE 4 COMPLETE (deferred-verdict variant) — all applicable audit exit criteria pass, workspace CI green (`fmt --check` / `clippy -D warnings` / `test --all`: 1246 passed, 0 failed).**

## Deliverables

### Scheduler (`schedule.rs`)
- `generate_schedule(&ScheduleSpec) -> Vec<PlannedTrial>`: blocks = task × repetition; block execution order via in-module **SplitMix64** Fisher–Yates (no external deps; golden-vector test pins seed-0 outputs so the scheme cannot silently change); within-block condition rotation keyed to slot identity so **per-task balance is a construction guarantee** (C=2, even R → exactly R/2 firsts each; odd R → documented ±1 residue).
- Validation taxonomy (8 error kinds, fail-closed incl. `TooManyConditions`/`TooManyTrials`), dense `order_index`, `#[serde(deny_unknown_fields)]` inputs.
- Audit shape verified: 10 tasks × 3 reps × 2 conditions, seed 881204 → 60 trials, balanced 15/15 first positions, deterministic across calls.

### Feasibility report (`report.rs`)
- `build_feasibility_report(&EvalStore, experiment_id)`: block model = `(task, version, repetition)`; a block is valid only when both conditions exist, carry non-invalid outcome classes, share `plan_hash`, and have **distinct manifests** ("conditions not distinct" otherwise).
- **Symmetric exclusion**: any invalid/missing side excludes both sides and records one `InvalidBlock` with a deterministic reason — the audit's exclusion rule, enforced and counted.
- Counts per condition (passed/failed/invalid/missing + `task_failures_by_class`), discordant pairs, per-task better/same/worse direction, robust secondary medians (wall-time and input-token deltas over complete pairs only; `observed + missing = valid_blocks` invariant), aggregated `missing_metrics` ("field: N of M executions"), and seven deterministic limitation strings (independent-task warning, repetition nesting, symmetric-exclusion count, missing telemetry, observed-model unavailable, harness-reported cost, verdict deferral).
- **`ReportVerdict::Deferred` is the only verdict variant** — no confidence scores, no effect estimates; `FEASIBILITY ONLY — no effectiveness verdict` / `INSUFFICIENT EVIDENCE (feasibility)` labels with documented thresholds (≥2 valid blocks and ≥2 independent tasks). Canonical JSON (`to_json`) + plain-text terminal render (`render_text`).

## Exit-criteria scorecard (audit M4, adapted)

| Criterion | Result | Evidence |
|---|---|---|
| Deterministic schedule from seed | **PASS** | same seed → identical plans; seed sensitivity; golden vectors; integration test on the audit shape |
| Invalid pairs are symmetrically excluded and counted | **PASS** | invalid-class / missing-side / not-distinct / plan-mismatch exclusion tests; one counted `InvalidBlock` per failing block |
| Golden reports cover the fixture classes | **PASS (adapted)** | feasibility fixtures: normal complete, invalid-excluded, insufficient-evidence, empty, missing-telemetry — all exact-asserted. Beneficial/harmful report fixtures are intentionally absent: no verdict engine exists (deferred by scope decision) |
| A 2%/10-run toy difference cannot produce an automatic beneficial verdict | **PASS (by construction + test)** | verdict is always `Deferred`; test scans rendered output for beneficial/harmful language and finds none; limitations name the single independent task |
| Report lists independent task count, repetitions, missing metrics, and limitations | **PASS** | seven deterministic limitation strings + `missing_metrics` aggregation, all test-asserted |

## Recorded deviations
>2 conditions refused (paired design only); 1 condition → all-invalid report; `task_failures_by_class` is the literal "all non-completed classes" reading with a separate invalid aggregate; `repetitions` = distinct recorded indices; secondary pair counts are aggregate (invariant preserved) with per-metric medians over complete subsets. Full detail in module docs and the agent report.

## Deliberately NOT in M4 (deferred by scope decision)
Verdict engine (effect sizes, uncertainty intervals, beneficial/harmful classifications), parallel execution, multi-harness adapters, CLI surface. The post-feasibility gate (M5) decides whether any of these are justified.

## Next
**M5 — ten-task feasibility run**: freeze the ten Root-safe-install fixtures, execute the full matrix with deterministic fake agents through materializer → supervisor → verifier → seal → store → report, then the postmortem gate that decides whether the verdict engine, more tasks, or more harnesses are justified.
