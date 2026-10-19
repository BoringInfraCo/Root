# Milestone 2, Track B — Supervised Harness Runner

Date: 2026-10-18 · Scope: `crates/root-eval` only (`events.rs`, `supervisor.rs`,
`runner.rs`, lib re-exports). No CLI, no `root-cli` changes, no Docs edits
beyond this file. `root-eval` stays std-only (+ `serde`/`serde_json`); in
particular it does **not** depend on `root-core` (the `execution.rs` run-result
shape was reference only).

## What it does

`run_trial(&TrialRunRequest)` runs one harness invocation under supervision
and folds the outcome into a sealed `TrialRunRequest` → `TrialRunRecord`
mapping:

1. `supervisor::run_supervised` spawns the child (piped stdout/stderr, null
   stdin, cleared environment rebuilt from the explicit map, caller cwd),
   drains stdout line-by-line under an output cap and a wall timeout, kills
   the process *group* on timeout/cancel/cap-breach, reaps, and classifies
   termination from process state — never from stream content after a kill.
2. `events::parse_ndjson_line` normalizes each stdout line against the frozen
   OpenCode `1.18.27` NDJSON envelope (`{type, timestamp, sessionID,
   part|error}`) into `NormalizedEvent` records.
3. `runner::run_trial` harvests session IDs, sums usage **only** from
   `step_finish` parts, records the requested model verbatim, leaves the
   observed model `None` with the M0 identity-policy limitation, and names
   every unavailable field in `missing_telemetry` instead of zeroing it.

## Event schema coverage vs golden fixtures

`NormalizedEvent` carries: `sequence`, `source_event_type` (`step_start` |
`step_finish` | `text` | `tool_use` | `error`), `session_id`, `timestamp_ms`,
`model_requested` (always `None` at parse time — filled verbatim at the trial
layer), `model_observed` (always `None`; see `MODEL_OBSERVED_LIMITATION`),
`tool {name, call_id, input_json, output_str, status, error}`,
`tokens {input, output, reasoning, cache_read, cache_write}`, `cost`, `error
{name, data_json}`, `raw_line_hash` (`sha256:` of the raw line bytes).

Golden replay tests (`events.rs`, fixture paths `CARGO_MANIFEST_DIR`-relative
into `Docs/Eval/milestone-0/fixtures/golden/`) cover all six shapes:

| Fixture | Assertions |
|---|---|
| `01-no-tool` | 3 events, `text == "ok"`, step_finish tokens `{6544, 8, 175, 98, 0}`, cost 0, single `sessionID` on all lines |
| `02-tool` | `tool == "read"`, `callID`, `state.input` JSON (`filePath`, `limit`), output contains seeded first line, second step_finish `cache.read == 7744` |
| `03-skill` | `tool == "skill"`, input `name == "root"`, output embeds the seeded `SKILL.md` post-frontmatter body **byte-for-byte** (hash `sha256:36e66cf5…b910`, the M0-recorded constant) |
| `04-provider-error` | single `error` line, `UnknownError {message, ref: err_354fccf3}`, no preceding events |
| `05-denial/05c-ask-reject-shape` | `tool == "bash"`, `status == "error"`, `error == "The user rejected permission to use this specific tool call."` (`ASK_REJECT_MESSAGE`); `is_ask_denial()` true only there |
| `06-timeout` | single `step_start`, no terminal event — EOF-settle tolerance (no event required at end of stream) |

Plus parser-contract tests: unknown envelope `type` → typed
`UnknownEventType` error (never skipped); invalid JSON / missing `type` /
`sessionID` / `timestamp` / malformed `part` → typed errors. Unparseable lines
(any reason, e.g. a torn trailing write after a kill) are reported as
`DroppedLine {sequence, reason}` by the supervisor and surfaced as
`dropped_line[<seq>]` entries in trial `missing_telemetry` — never silently
dropped.

Live corrections encoded: deny-rules remove tools (no event — nothing to
parse); ask-mode denial is the only error-status tool shape; `tokens.reasoning`
is provider-reported even with thinking off (recorded, never interpreted);
`metadata`/`title`/`time` ride in `part.state`, not at `part` level.

## Termination taxonomy

`TerminationKind`: `Completed` | `TimeoutKilled` | `CancelledKilled` |
`ProviderError` | `Denied` | `SpawnFailed` | `OutputCapExceeded`.

- Kill paths (`TimeoutKilled`, `CancelledKilled`, `OutputCapExceeded`) are
  decided **from process supervision alone**. After a kill the supervisor never
  consults abort-shaped stream content (there is none — M0 06: SIGINT ignored,
  TERM yields exit 143 with only `step_start` persisted, no
  `MessageAbortedError` flush).
- Natural exits are classified from stream content + exit code, in precedence
  order: `error` event → `ProviderError`; nonzero exit → `ProviderError`;
  ask-denial tool event (`is_ask_denial`) with zero exit → `Denied`; otherwise
  `Completed`. Exit code is always recorded separately.
- No-abort-reliance test: a kill with zero trailing events still classifies
  `TimeoutKilled` with no error event present.

## Kill / reap design + platform notes

- Unix: the child is spawned as a process-group leader (`process_group(0)`,
  i.e. pre-exec `setpgid`), and every kill targets the **negative pgid** via
  `/bin/kill` (no extra dependencies — `root-eval` stays std-only): `TERM`,
  `kill_grace_ms` wait, `KILL`, recheck. This reaps tool children (M0 06b:
  orphaned `bash`/`sleep` survived bare SIGTERM). After reaping, `kill -0
  -<pgid>` probes the group; survivors get a second `KILL` pass and are
  reported in `ResidueReport {cleanup_ok, leftover_killed, note}`.
- stderr is drained on a helper thread and discarded (only stdout feeds
  `raw_bytes_sha256` and event parsing); the stdout reader thread forwards
  lines over a channel so the drain loop can poll child exit, wall deadline,
  output cap, and the `CancelFlag` (`Arc<AtomicBool>`, same group-kill path as
  timeout → `CancelledKilled`) every 5 ms.
- Natural-exit drain is time-boxed (5 s) so a surviving pipe-holding
  grandchild cannot hang the supervisor; post-kill channel drain is bounded
  (2 s) so already-written bytes still reach the parser.
- **Windows limitation:** no process groups — the supervisor falls back to
  killing the direct child only (`Child::kill`). Tool grandchildren may
  survive there; `ResidueReport::cleanup_ok` is authoritative and callers must
  not assume group semantics off Unix.

## Security properties (inherited + enforced here)

Explicit env only (`env_clear` + provided map — the `explicit_env` test proves
a parent secret is not inherited), null stdin, caller-set cwd (expected: a
fresh Track A trial checkout), bounded wall/output, no network policy of its
own (network control belongs to the trial materialization + verifier layers),
no secret logging (stderr discarded, raw stdout hashed not printed).

## Deviations

1. `DroppedLine` on `SupervisedResult` (additive, not in the brief's struct
   list): unparseable lines must surface somewhere or the no-silent-skip rule
   is unenforceable end-to-end. The runner copies them into
   `missing_telemetry`.
2. `ToolCall` carries an extra `error: Option<String>` (needed to recognize
   the ask-denial family and to classify `Denied`); `SupervisedResult` marks
   `stdout_truncated = true` on every kill (a killed stream is definitionally
   incomplete).
3. `kill_grace_ms` defaults to 2000 ms (`DEFAULT_KILL_GRACE_MS`); the
   timeout fake-harness test uses a 2000 ms wall with a comment — under
   parallel test load this host delays some spawns past process creation
   (child alive but not yet executing) for hundreds of milliseconds, so a
   tight wall can fire before the first line flushes. The supervisor is
   correct either way (kill + `TimeoutKilled` + clear probe); the generous
   wall only lets the event assertion observe exec.
4. Sibling-track lint fixes: three trivial clippy lints in
   `tests/trial_materializer.rs` (`contains_key`, two `needless_borrow`) were
   repaired to keep the workspace `-D warnings` gate green; no semantic change.

## Test inventory

- `events.rs`: 6 golden replays + byte-equal skill hash + unknown-type /
  malformed typed-error tests (13 tests).
- `supervisor.rs`: normal EOF settle, timeout kill of sleeper + child
  (residue probe clear), `ps`-sweep proving no surviving sentinel sleeper,
  output-cap kill + truncated, cancel-flag kill, taxonomy
  (completed/timeout/cancelled/provider-error/output-cap/denied/spawn-failed),
  no-abort-reliance, spawn-failure, explicit-env isolation (9 tests).
- `runner.rs`: harvest + step_finish-only totals, missing-telemetry listing
  (no zeroing), golden-01 end-to-end via `cat` (3 tests).
