# Milestone 5 — Executive Summary & Verdict: Ten-Task Feasibility Matrix + Postmortem

Date: 2026-10-19 · Repo checkout: `7795341` + M5 working tree
Scope basis: audit's Milestone 5 (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*) under the recorded **feasibility-spike scope decision**
Implementation: three subagent waves (infrastructure; ten frozen tasks split 5/5; driver + run), independently CI-gated by the lead agent

## Verdict

**MILESTONE 5 COMPLETE — all five audit exit criteria pass. The full feasibility matrix ran end-to-end: 60/60 trials, calibrated outcomes, sealed evidence, byte-identical reproduction, zero contamination. Workspace CI green (`fmt --check` / `clippy -D warnings` / `test --all`: 1354 passed, 0 failed).**

## The run (fake agents, $0 provider cost, zero network)

| Metric | Result |
|---|---|
| Trials | **60/60 completed** (10 tasks × 2 conditions × 3 reps), 0 invalid, 0 missing |
| Treatment | 30/30 `completed_passed` (all primaries) |
| Baseline | 30/30 `completed_failed` (all primaries fail — calibrated naive failure modes) |
| Valid blocks | 30 (0 invalid), independent tasks 10 |
| Discordant | baseline fail → treatment pass: **30**; reverse: 0 |
| Per-task direction | all ten: 3 better / 0 same / 0 worse |
| Label / verdict | **FEASIBILITY ONLY — no effectiveness verdict** / **DEFERRED** |
| Plan hash | `sha256:7afd88ea907e467a2ebe8417e95290f603d599c328df97ec774ca8247112ea8e` |
| Secondary | median duration Δ −79 ms; median input-token Δ 0; missing: `observed_model` 60/60 (the documented M0 identity gap) |

Evidence: `Docs/Eval/milestone-5/run/` — `report.txt`, `report.json`, `summary.json`, `reproduce.txt` (byte-for-byte from the run).

## Exit-criteria scorecard (audit M5)

| Criterion | Result | Evidence |
|---|---|---|
| 60 planned trials finish or have classified retained failures | **PASS** | 60/60 completed; schedule (seed 881204, balanced) fully executed |
| No shared-state/secret/network contamination | **PASS** | per-trial canary secret: 0 hits across 2,565 files (~3.0 MB) incl. stored artifacts; 60 disjoint trial roots; residue scans clean; fake tools offline by construction |
| Report reproducible from sealed evidence without rerunning agents | **PASS** | `--reproduce`: 60 manifests re-verified intact; rebuilt report **byte-identical** (`sha256:07fb3bbc…33ad`); no agents/verifiers ran |
| Result labeled feasibility-only | **PASS** | `FEASIBILITY ONLY — no effectiveness verdict`; verdict `DEFERRED`; no beneficial/harmful language anywhere |
| Postmortem decides whether more tasks/harnesses are justified | **PASS** | see below |

## Postmortem — the decision gate

**What M5 proves:** the complete local pipeline works end-to-end on real processes with real isolation, sealing, storage, and reproduction — materialize → supervise (group-kill/reap) → verify (mutation-guarded) → accept (six assertion kinds over five evidence channels) → seal (CAS + tamper-evident) → store (queryable, integrity-checked) → report (symmetric exclusion, deferred verdict) → reproduce (byte-identical from sealed evidence).

**What M5 does not prove:** anything about real-model skill effectiveness. The agents were calibrated deterministic doubles; the 30/0 discordance is a pipeline validation, not evidence about `skills/claude/SKILL.md`. The verdict engine stays deferred until real runs produce data.

**Decision — next gate is a real-model run, not more infrastructure:**
1. **Justified now:** one real-model feasibility run on OpenCode 1.18.27 with the same ten tasks — the only remaining unknown is real-agent behavior. Cost: $0 provider at 1.18.27's free default model; requires explicit user approval (real model calls + network) and a decision on model choice.
2. **Required before that run (small, queued):**
   - Verifier env hygiene: drop the host `PATH` tail from verifier environments (observed during this run: task 01's external verifier could resolve `rg` from the host; primaries still discriminated, but the stricter env is the correct contract).
   - Live conformance of the two M1 assumptions against the real binary (scan-order totalization; flag semantics) — first real run doubles as that probe.
   - Credential handling for the real harness: auth via the harness mechanism only; the canary pattern generalizes.
3. **Explicitly NOT justified yet:** verdict engine (needs real-run variance data); more tasks (10 sufficed for feasibility); second harness (Claude fallback is pre-probed, Codex pin-blocked); parallel execution (sequential is fine at this scale); CLI surface (the example driver suffices until a real run is accepted).

## Deliverables

- `crates/root-eval/src/feasibility.rs` — matrix driver (library): schedule → per-trial materialize/run/verify/accept/seal/store → report; outcome mapping documented (acceptance folds into the stored class; raw layers remain queryable as verification rows, incl. `builtin:acceptance`).
- `examples/m5_feasibility.rs` — CLI wrapper (`--out`, `--reproduce`, seed/reps/task-dir overrides).
- `tests/m5_matrix_smoke.rs` — reduced-matrix test incl. reproduction.
- Ten sealed tasks + prompts + fake agents: `tests/fixtures/m5/` (calibrated both directions per task; profile special-cases documented in `fake_tools.rs`).
- Minimal documented relaxations: `m5-fake-agent` harness name accepted alongside `opencode`; `generated:<sha256>` repository-commit marker for generated-fixture experiments.

## Recorded deviations
Verifier `PATH` tail (item above); task 01 fixture `rg` stub; task prompts use inline variant for the first five records, file variant for the last five (both valid; unification is a cleanup item); zero redactions occurred in this deterministic run (mechanism exercised by unit tests, not by run data).
