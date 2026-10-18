# Milestone 0 — Executive Summary & Verdict: OpenCode 1.18.27 Contract Freeze

Date: 2026-10-18 · Repo checkout: `d16857f` (uncommitted working tree)
Inputs: `UPSTREAM-CONTRACT.md` (source-level freeze at tag `v1.18.27`), `HOST-PROBE.md` + `fixtures/` (exact-binary probes in a disposable sandbox)
Gate basis: the audit's Milestone 0 exit criteria (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*)

## Verdict

**GATE PASSED — adapter feasibility confirmed. Root's pinned `1.18.27` is real, the exact binary is verified, the contract is frozen from source AND live-captured (6/6 golden fixtures, zero provider cost). Proceed to Milestone 1 (immutable spec + effective config snapshot) per the recorded scope decision.**

Three conformance gaps are documented and must be designed around, none is a blocker; all are addressed in the fixture/runner design below.

## Exit-criteria scorecard

| # | Audit's Milestone 0 exit criterion | Result | Evidence |
|---|---|---|---|
| 1 | A published `1.18.27` release exists (added 2026-10-18 as first criterion) | **PASS** | npm `opencode-ai@1.18.27` (trusted publishing, 2026-09-02) + GitHub release `v1.18.27` at `anomalyco/opencode` (repo moved from `sst`); `latest` is `1.19.3` → pin is ~9 releases back in a live 1.x line. Exact-version install semantics required (no dist-tag points at it) |
| 2 | Exact binary + executable hash identified | **PASS** | darwin/arm64 CLI, sha256 `f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` (144,074,210 B); identical sha256 across npm platform package, npm wrapper assembly, and GitHub release zip |
| 3 | Session / lifecycle / terminal / usage / tool / skill-load fields mapped or explicitly unavailable | **PASS with 3 documented gaps** | NDJSON `{type, timestamp, sessionID, part}` stream: `step_start`, `step_finish` (tokens + cost per step), `text`, `tool_use` (name, `callID`, `state.input`, `state.output`), `error` (`{name, data}` union). Gaps: (a) no terminal event — settle = EOF + exit code; (b) **observed model identity absent** from stream; (c) `reasoning` events only with `--thinking` (default off) — aligns with the audit's exclude-thinking-by-default policy |
| 4 | Noninteractive permission / config / home behavior demonstrated | **PASS** | `run` never blocks: default reply is `reject` (stderr notice); stock ruleset is allow-all (`"*": "allow"`, `read` asks on `*.env`), so determinism requires *seeded* permission config (`opencode.json`, agent frontmatter, or `OPENCODE_PERMISSION` env); complete fs-write map captured; host isolation verified byte-identical |
| 5 | Adapter feasibility decision recorded; fallback if fail | **PASS — buildable** | See "Adapter contract verdict" below; no pin change proposed. Fallback surfaces captured (see `fallback/`): **Claude reachable at exact pin `2.1.260`** (stream-json output, `--max-budget-usd`, `--permission-mode`, `--session-id`, `--allowedTools` all present on the surface — budget/turn caps and stream schema details behaviorally NOT-VALIDATED); **codex is pin-blocked** (host `0.163.0` ≠ pin `0.150.1`, would need exact-version acquisition, same class of problem as the OpenCode pin) |

## Adapter contract verdict (conditions for credible fixtures)

1. **Identity policy (resolves audit's `requested` vs `observed` model rule):** the JSON stream never names the model that produced a message. For MVP the adapter records the **requested** `--model provider/model` and emits `observed = unavailable` with a recorded limitation. Post-run correlation via session DB or `--attach` is a P2 upgrade — do not couple the first experiment to it.
2. **Termination policy:** no terminal event and no error-specific exit codes (0/1 only). The runner settles on **EOF + process exit**, owns its own wall timeout (kill → classify `timeout`, parse partial stream), and tolerates the pending-event race between last part emission and stream end. `step_finish.part.reason` is the only finish signal.
3. **Hermetic policy:** stock 1.18.27 is not hermetic — allow-all permissions, config seeding (`opencode.jsonc` + `$schema` writeback + `.gitignore`), SQLite session DB, models cache (~5.6 MB), background plugin `npm install`, persistent lock heartbeats, and **~83 MB of npm deps fetched at run time into `$HOME/.npm`**. Trials must: scope `HOME`/XDG per trial, pass `--pure` (disables plugin installs), consider `OPENCODE_DB=:memory:`, set `OPENCODE_DISABLE_AUTOUPDATE=1` (already used by Root's inventory), prewarm the npm cache once (content-addressed `_cacache` is safe to copy read-only), and snapshot dirs before/after. Denied-permission fixtures require seeded config — they cannot be produced unconfigured.
4. **Skill-load evidence is strong:** a load is a `tool_use` event, `part.tool === "skill"`, full `<skill_content>` in `state.output`, `metadata {name, dir}`, and a denied load is `state.status = "error"`. This directly supports the audit's `Loaded` state via a native observable event. Discovery covers project `.opencode/{skill,skills}/**/SKILL.md`, `~/.claude/skills`, `~/.agents/skills`, config-dir globs, and `skills.paths` — which validates the audit's trial-local materialization design (project `.opencode/skill/` path for condition B).
5. **Rules loading:** project precedence walks up from cwd to worktree root, first of `AGENTS.md` → `CLAUDE.md` → `CONTEXT.md` (one per message, plus global `~/.config/opencode/AGENTS.md` first-match-wins). Root-authored AGENTS.md lands at `${XDG_CONFIG_HOME}/opencode/AGENTS.md` without touching `.claude` — compatible with the bundle allowlist (`manifest.rs` already permits `OpenCodeHome/AGENTS.md` + `OpenCodeHome/skills/*/…`).

## Economics (material discovery)

The unaudited assumption "golden sessions cost money" is false at this version: **1.18.27 auto-provisions a built-in free default provider** (`opencode`/`big-pickle`) when no auth exists — the host probe's unauth run completed `exit=0`, `cost=0`. Consequently:

- Conformance captures can run at **$0 provider cost**.
- There is **no provider-missing error path by default** — error-shaped fixtures must force selection (`--model <nonexistent>/x` → expected `APIError`/`ProviderAuthError`).
- Residual exposure is network activity (model calls, npm fetch, catalog refresh), not spend. Approval should still be explicit per the audit's discipline.

## Golden captures — executed 2026-10-18 (user-approved, zero provider cost)

All six fixtures plus three bounded variants (`05b` deny-observational, `05c` ask-reject, `06b` mid-tool abort) and an optional `OPENCODE_DB=:memory:` sanity run were executed from the hash-verified binary, sequentially, each in a fresh scratch HOME/XDG/TMPDIR with prewarmed npm cache. Raw captures: `fixtures/golden/<capture>/` (`stdout.ndjson`, `stderr.txt`, `meta.json`, `fs-delta.txt`); full findings in `GOLDEN-CAPTURES.md`. Result: **6/6 captured, 0 reasoning events anywhere (thinking-off default validated), leftover-process sweep clean.**

| Capture | Result | Key evidence |
|---|---|---|
| 1. no-tool success | **PASS** | 3 lines, `ses_…` on all, `step_start`/`text`/`step_finish`, numeric tokens+cost, exit 0 |
| 2. one tool call | **PASS** | `tool:"read"`, `callID`, `state.input`/`state.output`, `state.time.start/end` |
| 3. skill load | **PASS** | `part.tool === "skill"`; `<skill_content name="root">`; returned body **byte-equal** (sha `36e66cf5…`) to the seeded file; `state.metadata {name, dir}` resolves the seeded path; normal finish. **Frontmatter finding: `skills/claude/SKILL.md` carries valid YAML frontmatter, `name: root`** → experiment materialization path is `.opencode/skill/root/SKILL.md` |
| 4. provider error | **PASS** | forced via nonexistent provider: single `error` line, `UnknownError {message, ref}`, exit 1, no preceding events (caveat: log `ref` unresolvable from `run` side) |
| 5. denial | **PASS + semantic correction** | see live corrections below |
| 6. timeout/abort | **CAPTURED, contradicts source-read expectations** | see live corrections below |
| 7. `:memory:` + `--pure` (bonus) | **PASS** | accepted, no db files on disk (attempt-1 transient provider stall disclosed, retry clean) |

### Live contract corrections (captured behavior, refines the source freeze)

1. **Deny ≠ error; ask = error.** Stock source-reading predicted deny rules produce `tool_use` errors with a "rule prevents you" message. Live behavior: a `deny` rule **removes the tool from the model's toolset entirely** — no error event, no stderr line, no observable denial trace (the model then works around with another tool; captured in `05b`). The denial error shape exists only for `ask`-mode + auto-reject: `state.status:"error"`, `state.error:"The user rejected permission to use this specific tool call."`, stderr `permission requested: bash (…); auto-rejecting` (`05c`). **Adapter implication:** seeded `deny` produces tool *absence*, so trials must not interpret missing-tool behavior as denial evidence; the audit's "denied permission" evidence model should use `ask`-mode for denial fixtures.
2. **Kill is its own terminal class; nothing abort-flushes.** SIGINT is ignored in non-interactive runs (capture 6); watchdog TERM yields exit 143 with only `step_start` persisted — **no `MessageAbortedError`, no abort flush** (contradicting the source-level expectation). Worse, `06b` showed **orphaned tool children** (bash subprocesses) surviving SIGTERM — a leak captured and cleaned. **Runner implication (Milestone 2, P0):** the supervisor owns wall timeouts, kills process *groups*, reaps tool children explicitly, and classifies `killed` independently from `aborted`; it cannot rely on abort-shaped events after forceful termination.
3. **`--pure` is not fully hermetic.** The config-dir `npm install @opencode-ai/plugin` (+13 MB, one-time) still happens under `--pure`. Also: lock heartbeat files do not persist after clean exits (differs from the m0 probe note — cleanup runs on graceful exit; stale locks only follow kills). **Trial design:** pre-warm the config-dir plugin install once (shared read-only cache/pre-seeded config dir) or accept a bounded per-trial cost; rese done via try/trial-clean home.
4. **Identity channel refinement.** The model-identity gap was re-confirmed live (no provider/model in the JSON stream), and one auxiliary channel was observed (task-tool metadata carries identity-bearing info in captures). MVP stance unchanged: record requested `--model`, emit `observed = unavailable` with the limitation noted.

### Hermeticity verdict

All golden runs wrote only inside scratch dirs; `/Users/sergio/.opencode` byte-identical before/after; host-side churn was attributed with mtime + blob-hash evidence to the harness process running *this session*, not the probes. Host isolation claims from the m0 probe stand.

## Per-trial isolation requirements carried into Milestone 2

Directly from the fs-write map: every trial needs its own HOME/XDG quartet + TMPDIR override; no shared writable cache except content-addressed npm `_cacache` mounted copied read-only; locks persist after exit → residue scan per trial; project cwd receives no writes from stock `run` (fixture writes come only from the agent's tools — see Milestone 2 leakage tests).

## Open items

1. **Golden captures: DONE** (6/6 + variants, zero provider cost) — see above; the event contract is now frozen from both source and live behavior.
2. **Fallback contingency: DONE** — `fallback/CODEX-CLAUDE-SURFACES.md` + raw captures. Claude is REACHABLE AT PIN — a full Claude 2.1.260 contract probe is available for free if OpenCode ever fails; open items there are turn/time caps and stream-json schema details (behaviorally unvalidated, help-absent). Codex is PIN-BLOCKED — its captured `0.163.0` surface is encouraging (`exec --json`, `--output-schema`, sandbox modes) but inadmissible for freezing the `0.150.1` pin without binary acquisition.
3. **Benchmark task-spec design: DONE** — `Docs/Eval/benchmarks/TASK-SPEC.md` (PROPOSAL, immutable TOML task format, 65-field schema, three structurally-unrunnable draft tasks, six documented deviations from the audit's wording resolved by explicit rules).
4. **Carried into Milestone 2 (P0):** process-group kill + tool-child reaping; `killed` vs `aborted` terminal taxonomy; config-dir npm pre-warm strategy; per-trial residue scan incl. orphaned children.
5. **Repo hygiene, not M0:** `Docs/Audit/` vs `Docs/audits/` path-convention decision; untracked `target 2/` directory of unknown origin (pre-existing, untouched).
6. **P0-adjacent bug backlog (documented, unfixed):** `GitState::capture` comment/code mismatch (`dirty=false` on missing git); `ubuntu:latest` sandbox default.

## Decision log

- **2026-10-18, user:** approved the six zero-cost golden captures (all six) and one M0 commit covering audit corrections, AGENTS.md hygiene, and the `Docs/Eval/` deliverables. Executed accordingly; provider cost $0 for all captures.
