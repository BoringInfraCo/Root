# Milestone 2 — Executive Summary & Verdict: Isolated Supervised Runner

Date: 2026-10-19 · Repo checkout: `4563cf4` + M2 working tree
Scope basis: audit's Milestone 2 (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*) under the recorded **feasibility-spike scope decision**
Implementation: two parallel subagent tracks sharing one working tree, merged cleanly, independently CI-gated by the lead agent

## Verdict

**MILESTONE 2 COMPLETE — all five audit exit criteria pass, workspace CI green (`build` / `fmt --check` / `clippy -D warnings` / `test --all`: 1161 passed, 0 failed).**

## Deliverables

### Track A — trial materializer + isolation (`trial.rs`, `materializer.rs`, `isolation.rs`)
- `TrialSpec` / `TrialLayout`: fresh `<base>/<trial_id>/` roots (0700 dirs, 0600 files), detached `git clone --no-checkout` + `checkout --detach <commit>` with fail-closed HEAD-clean verification, treatment-only `.opencode/skill/root/SKILL.md` materialization, hermetic `opencode.json` (ask-mode permissions per the M0 deny≠ask correction), fake `root`/brew/curl/npm/pip shims (deterministic JSON + audit-log side channel, no network), and an env allowlist built from the layout only.
- Secret hygiene: library never reads the parent env; `scrub_env` fail-closes on any blocked pattern (`AWS_*/GITHUB_*/OPENAI_*/ANTHROPIC_*`, `*_TOKEN/*_SECRET/*_KEY`); tested with a live parent secret asserting absence.
- Isolation: `verify_isolation` (canonicalized-path disjointness, symlink-escape detection), `residue_scan` (orphan dirs, stale locks/heartbeats, pids), `cleanup_trial` (remove + verify, idempotent), `check_no_leak`.
- 3 unit + 8 integration tests; design doc `Docs/Eval/milestone-2/MATERIALIZER.md`.
- Sole documented exception to the M1 no-spawn rule: bounded `git` invocations (`clone/checkout/rev-parse/status`, `env_clear`, PATH-only, captured stderr) inside the materializer.

### Track B — supervised runner (`events.rs`, `supervisor.rs`, `runner.rs`)
- `parse_ndjson_line`: full M0 stream contract (envelope, `step_start`/`step_finish` with tokens+cost, `tool_use` terminal states, ask-denial shape, unknown types → typed `UnknownEventType` never skipped, torn lines → `DroppedLine` → `missing_telemetry`). All six golden fixtures replay from real capture NDJSON, including the byte-equal skill-content hash on capture 03.
- `run_supervised`: explicit-env `env_clear` spawn, Unix process-group leadership (`process_group(0)`, TERM→grace→KILL via pgid, `kill -0` probe + second KILL pass), output-cap kill, wall-timeout kill, `CancelFlag` kill path, wall-time recording, `ResidueReport`. Termination taxonomy (7 kinds) derives kill paths from process state only — never from abort-shaped content — and natural exits by error-event → nonzero-exit → ask-denial → Completed precedence.
- `run_trial`: requested-model verbatim, `observed = None` + recorded limitation (M0 identity policy), session-ID harvest, `step_finish`-only usage totals, missing-not-zeroed telemetry list.
- Fake-harness test double proves EOF settle, timeout kill incl. children, output-cap kill, cancel kill, and partial-stream classification. Design doc `Docs/Eval/milestone-2/RUNNER.md`.
- Platform note: Windows = direct-child kill only (documented); concurrent-spawn pre-exec stalls observed on the build host are classified correctly regardless.

## Exit-criteria scorecard (audit M2)

| Criterion | Result | Evidence |
|---|---|---|
| Two trials cannot observe each other's files, sessions, caches, or condition config | **PASS** | disjoint roots/HOME/XDG/fake-bins asserted; cross-trial file invisibility test; delta check on materialized trees |
| Timeout/cancel kills descendants and records terminal state | **PASS** | group-kill + reap with `TimeoutKilled`/`CancelledKilled` classification; fake-harness sleeper+child test; no reliance on abort events |
| Crash recovery finds and cleans orphaned trial resources | **PASS** | `residue_scan` finds stale locks/orphan dirs; `cleanup_trial` removes + verifies; rescan-clean test |
| Secrets from the parent environment are absent from a child fixture | **PASS** | empty-base env construction; live-secret parent test asserts absence from map + file |
| No external network/tool side effect in the benchmark | **PASS** | fake tools carry no network references (grep-tested); materializer writes nothing outside the trial root |

## Recorded assumptions & deviations

Track A: skill bytes sourced from manifest paths (fail-closed on symlink/size/digest mismatch); non-git sources copied verbatim, tree hash recorded-not-verified; no kernel/network namespace (offline-by-construction). Track B: additive `DroppedLine` field; `ToolCall.error: Option<String>`; `truncated=true` on any kill. Scan-order/flags/config-subset ambiguities from M1 (`TEST-PLAN.md` §7) carry over unchanged and remain live-confirmation items, not M2 blockers.

## Deliberately NOT in M2

Real-harness end-to-end runs (fake harness + golden replay only), verifier execution, scheduler/aggregation, CLI surface — Milestones 3–5 per the scope decision. M2 runs processes locally and deterministically; nothing phones home.
