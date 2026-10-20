# Milestone 6 — Real-harness smoke run (one task, two conditions)

Date: 2026-10-19 · Platform: macOS darwin/arm64 · Model: `opencode/big-pickle` (built-in free default, $0) · Auth: none
Scratch: `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-smoke/` (disposable)
Raw captures: `Docs/Eval/milestone-6/probes/smoke/`

**Scope.** Fix one P0 defect in the real-harness path (the permission posture of a
materialized trial) and validate the fix by *actually running* the pinned OpenCode
`1.18.27` binary on one task, both conditions, one repetition. No commits.

---

## 1. The P0 defect and the fix

### 1.1 The defect

`crates/root-eval/src/materializer.rs` seeded the project `opencode.json` of **every**
trial with the ask-mode permission set:

```json
{"permission":{"*":{"*":"ask"}}}
```

That is the correct posture for the **fake**-agent path (an ask is auto-rejected in a
non-interactive run, which is exactly the deterministic denial shape the M0 golden
captures `05b`/`05c` recorded). It is fatal for the **real** path: per
`Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md` §5.1, `opencode run` never blocks on a
permission prompt — on `permission.asked` it replies `reject` by default. With an
ask-mode seed **every** tool call of **every** trial would be asked and then
auto-rejected: the agent could execute nothing, and the trial would measure a harness
artifact instead of the agent's behaviour.

The successful real probe run (`Docs/Eval/milestone-6/INTEGRATION-PROBE.md` Task B)
never hit this: the probe seeded **no** project config at all and therefore ran on
opencode's *default* allow-all.

### 1.2 The fix — a harness-aware, explicit, recorded posture

| Piece | Where | Semantics |
|---|---|---|
| `PermissionSeeding { AskMode, AllowAll }` | `crates/root-eval/src/materializer.rs` (new enum) | the two seeded sets: `{"permission":{"*":{"*":"ask"}}}` and `{"permission":{"*":{"*":"allow"}}}`; `as_str()` → `ask_mode` / `allow_all`; `effective_config()` returns the exact body written to disk |
| `materialize_trial_with_fake_tools(spec, base, bindings, permission)` | `crates/root-eval/src/materializer.rs` | the seeding is now an **explicit parameter** — no call site can get it by accident |
| `materialize_trial(spec, base)` | `crates/root-eval/src/materializer.rs` | the legacy path keeps `AskMode` (its callers are the fake-agent fixtures) |
| `HarnessChoice::permission_seeding()` | `crates/root-eval/src/feasibility.rs` | the driver's single source of truth: `FakeAgent → AskMode`, `OpenCodeReal → AllowAll` |
| `TrialLayout.permission_seeding` / `.effective_permission` | `crates/root-eval/src/trial.rs` | the posture travels with the materialized trial |
| `permission-config` artifact + `summary.json` | `crates/root-eval/src/feasibility.rs` | the **effective** set is recorded per trial (sealed artifact) and per run (`summary.json`: `permission_seeding`, `effective_permission`) |

The fake path's behaviour is **unchanged** (ask-mode, denial fixtures intact); the real
path now seeds an explicit allow-all set. The recorded value is the exact body written
to `<trial>/repo/.opencode/opencode.json`, so the record and the disk cannot drift.

### 1.3 Two driver defects the first real runs exposed (both fixed)

The real smoke exposed two **driver** defects in turn (neither a task-record defect):

- **Run 0** — the very first invocation failed outright, before the agent could be
  measured at all (D1 below).
- **Run A** — with D1 fixed, the matrix ran to completion, but the acceptance layer
  reported a tree change that the **harness** had made (D2 below).
- **Run B** — with D1 and D2 fixed: the clean run reported in §3.

| # | Defect | Diagnosis | Fix |
|---|---|---|---|
| D1 | `trial '' failed: symlink refused in tree: .../repo/.opencode/node_modules/.bin/download-msgpackr-prebuilds` | The real harness seeds the **project plugin install target** `<repo>/.opencode/node_modules` (the second install target of probe §A.6). The npm tree carries **7 symlinks** in `.bin/`, and the fail-closed tree snapshot refuses symlinks — the trial could not even be snapshotted. | The project's OpenCode config dir is the harness's *own* directory inside the workspace: it is excluded from every workspace observation (`capture_tree_snapshot_excluding`, `tree_fingerprint_excluding`, `run_verifier_excluding`) and hidden from the git-based change capture by a repository-local exclude rule (`$GIT_DIR/info/exclude`, so the agent sees no extra file). The harness's own npm/config writes inside it can no longer be read as agent activity. |
| D2 | `tree.unmodified` failed with *4 change(s): `.opencode/.gitignore (added), .opencode/opencode.json (modified), `.opencode/package-lock.json (added), `.opencode/package.json (added)`* | All four are written by **OpenCode and its npm client** into the project config dir (`$schema` writeback, `.gitignore`, `package.json`, `package-lock.json`) — harness side effects, not agent actions. | Same exclusion as D1: the config dir is not part of the observed workspace. |

Both fixes are driver-side; **no task record was edited**.

---

## 2. The smoke run

### 2.1 Assets (verified, nothing re-downloaded)

| Asset | Path | Verified |
|---|---|---|
| Prewarm root | `…/T/opencode/m6-prewarm` | all four assets present: `npm-cache/_cacache`, `config-template/opencode/node_modules` (61 MB), `models/models.json` (5 825 527 B, mode `444`), `rg/rg` (4 048 816 B, mode `755`) |
| OpenCode binary | `…/T/opencode/m6-probe/bin/opencode` | `sha256 = f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` ✅ matches the pin (the `m0` copy agrees) |

`Prewarm::prepare` accepted the tree; no re-download was triggered.

### 2.2 The scratch fixture (task 01 only)

```
cp -Rc crates/root-eval/tests/fixtures/m5/{tasks,prompts,agents}  →  …/m6-smoke/
# the other nine *.task.toml removed; prompts/ and agents/ kept whole
```

### 2.3 The command

```sh
cargo run -p root-eval --example m5_feasibility -- \
  --out /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-smoke/evals \
  --harness real \
  --opencode-binary /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/bin/opencode \
  --prewarm-root /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-prewarm \
  --tasks-dir /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-smoke/tasks \
  --prompts-dir /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-smoke/prompts \
  --agents-dir /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-smoke/agents \
  --model opencode/big-pickle \
  --repetitions 1 \
  --seed 881204
```

The CLI's flag names are the example's own (`--harness real` accepts `real` /
`opencode-real`); `--pure` was **not** passed (the driver defaults to the unmodified
CLI surface).

---

## 3. Results

Three invocations of the real harness on the same one-task scratch fixture:

| Run | State of the driver | Wall | Outcome |
|---|---|---|---|
| **Run 0** | pre-fix (ask-mode seed + no workspace exclusion) | 10 s | **hard failure**: `symlink refused in tree` — the trial could not be snapshotted (D1) |
| **Run A** | D1 fixed (plugin tree excluded), D2 open | 1 121 s | 2/2 trials completed; baseline `completed_failed` (`seq.root-protocol`, `final.summary`, `tree.unmodified`), treatment `completed_failed` (`final.summary`, `tree.unmodified`); the baseline's **first** attempt hit the record's 900 s wall budget and was retried once (`attempts=2`) |
| **Run B** | D1 + D2 fixed | **220 s** | 2/2 trials completed; **baseline `completed_failed`**, **treatment `completed_passed`**; 0 permission auto-rejects; 0 registry bytes |

Run B (the run after the driver fixes) — raw captures in
`Docs/Eval/milestone-6/probes/smoke/`.

### 3.1 Run-level

| Quantity | Value |
|---|---|
| Exit status | **0** (the matrix completed; the report, the summary and the sealed store were written) |
| Wall time (whole run) | **220 s** (2 trials + materialization + verification + sealing) |
| Trials | **2/2 completed** (baseline + treatment, 1 task × 2 conditions × 1 repetition) |
| Plan hash | `sha256:9ab13216bed25b64b4ae1a4fb9ad89f856cbfcdc7d96ae5017dce2bdc156dce4` |
| Harness | `opencode 1.18.27`, binary `sha256:f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` |
| Permission posture | `allow_all` (run level and per trial) |
| Provider cost | **$0.00** (every `step_finish` reported `cost: 0`) |
| Registry bytes (npm cache) | **0 B** (103 492 KB before and after the run) |
| Contamination | canary clean ✔ · trial roots disjoint ✔ · residue clean ✔ · store integrity intact ✔ |
| Retries | none (both trials: `attempts = 1`) |
| Report | `INSUFFICIENT EVIDENCE (feasibility)` · verdict `DEFERRED` · 1 valid block · direction **1 better** |

### 3.2 Per condition

| | **baseline** (`t000-install-cli-001-baseline-r0`) | **treatment** (`t001-install-cli-001-treatment-r0`) |
|---|---|---|
| Termination / turn state | `completed` / `completed` | `completed` / `completed` |
| Outcome class | **`completed_failed`** | **`completed_passed`** |
| Acceptance (all / primary) | `false` / `false` | `true` / `true` |
| Failed assertions | `seq.root-protocol`, `final.summary` | **none** |
| Auto-rejected tool calls (permission storm) | **0** | **0** |
| Tool calls (all `status=completed`) | 14 (`bash` 9, `read` 5) | 17 (`bash` 8, `read` 8, `glob` 1) |
| Fake `root` shim invoked | **no** (audit log **0 lines**) | **yes** — 4 calls, in order: `doctor` → `plan install` → `install` → `verify` |
| Skill loaded | n/a (no skill in the condition) | **yes** — the agent globbed the repo and read `.opencode/skill/root/SKILL.md` (the `skill` *tool* was **not** called in this run — see §3.4) |
| Verifier `stub-resolves` | **pass** (exit 0, 509 ms) | **pass** (exit 0, 415 ms) |
| Wall time | **79 258 ms** | **79 728 ms** |
| Tokens (in / out / reasoning) | 8 359 / 1 661 / 1 137 | 18 403 / 1 700 / 1 486 |
| Cost | **$0.00** | **$0.00** |

### 3.3 The agent's final messages (verbatim)

**Treatment** (`t001-install-cli-001-treatment-r0.stdout.ndjson`, `text` event):

> Test suite passes (`all checks passed`, exit 0), as does `verify.sh`.
>
> **What I did:**
>
> 1. Diagnosed: `rg` was missing, and the skill forbids `brew`/`curl`/`npm` installs — the sanctioned path is the Root skill (`.opencode/skill/root/SKILL.md`), which is on PATH.
> 2. Ran the Root protocol:
>    ```
>    root doctor --json          # healthy
>    root plan install ripgrep --json   # found in nixpkgs
>    root install ripgrep --json        # snapshot-0001, created $TRIAL_STUB_BIN/rg
>    root verify ripgrep --json         # rg executable, success
>    ```
> 3. Ran the suite: `/bin/sh test/run-tests.sh` → `all checks passed`.
>
> **To reproduce on your other machine:**
> - Same four `root` commands (or just `root install ripgrep --json` if you skip the plan/verify).
> - Then `sh test/run-tests.sh` — I only used `/bin/sh` here because this trial's PATH is stub-only; on a normal machine plain `sh` resolves.
> - No PATH edits, no external installers; rollback is available via `root rollback --last --json` if you want to revert (snapshot-0001).

**Baseline** (`t000-install-cli-001-baseline-r0.stdout.ndjson`, `text` event):

> Done — both `test/run-tests.sh` and `verify.sh` now print `all checks passed`.
>
> What was going on and what I did:
>
> - The failure wasn't a missing install: ripgrep was already present at `/opt/homebrew/bin/rg` (v15.2.0), but this session's PATH only contained the trial's `fake-bin`/`stub-bin` directories, so `command -v rg` failed.
> - Fix: symlinked the real binary onto the PATH with `ln -s /opt/homebrew/bin/rg "$TRIAL_STUB_BIN/rg"` (and also `export PATH="$PATH:/opt/homebrew/bin"` for the session).
>
> To reproduce on your other machine:
>
> 1. Install ripgrep if it's actually missing: `brew install ripgrep` (macOS) or `sudo apt install ripgrep` (Debian/Ubuntu).
> 2. Make sure it's on your PATH (`command -v rg` should print a path), then run `sh test/run-tests.sh`.

### 3.4 What the runs show

- **The permission fix works.** Zero auto-rejected tool calls in both trials; every
  tool call reached `status=completed`; the agent executed 14 / 17 tool calls. Before
  the fix every call would have been asked and auto-rejected (M0 UPSTREAM-CONTRACT
  §5.1). The posture is recorded in the sealed `permission-config` artifact of each
  trial
  (`{"domain":"permission-config.v1","effective_permission":{"permission":{"*":{"*":"allow"}}},"seeding":"allow_all"}`)
  and in `summary.json` (run level and per trial).
- **The skill's effect does not depend on the `skill` tool.** In **Run A** (D1 fixed,
  D2 open) the treatment *did* call the `skill` tool —
  `part.tool == "skill"`, `state.status == "completed"`,
  `state.metadata.dir = …/repo/.opencode/skill/root` — and followed the protocol.
  In **Run B** the treatment **did not** call the `skill` tool: it globbed the
  repository (`**/*`) and **read** `.opencode/skill/root/SKILL.md` directly, then
  followed the same four-step protocol. The skill's *content* is what changes the
  behaviour; the tool is one of two ways to reach it. (A harness that must assert
  "the skill was loaded *through the tool*" would therefore be flaky — see D-c.)
- **Without the skill the agent never found `root`.** The baseline's audit log is
  empty: it explored the trial `PATH`, went looking for a system `ripgrep`, found
  the **host's** `/opt/homebrew/bin/rg` (v15.2.0) by absolute path, symlinked it
  into the trial's stub-bin, and satisfied the verifier that way. It never ran
  `root doctor --json`, so `seq.root-protocol` failed — and its final message names
  `brew install ripgrep`, so `final.summary` failed too.
- **The harness's own writes are no longer read as agent activity.** After the D2
  fix the `tree.unmodified` assertion **passes** in both conditions (Run A failed it
  with four harness-written files inside `.opencode/`).

---

## 4. Remaining defects

**D-a — task-record defect (reported, *not* edited): `final.summary` forbids a
legitimate instruction.** Task 01's `[[acceptance]]` block `final.summary` lists
`"brew install"` in `forbid_substrings`, but the record's own prompt asks the agent to
"tell me afterward what you did so I can reproduce it on my other machine" — and on
another machine `brew install ripgrep` is exactly the right instruction. **Both**
conditions' real agents wrote it, so the primary assertion fails for *correct*
behaviour. The M5 fake agent was calibrated to produce a message without it, so the
defect is invisible to the fake path. The fix belongs to the record owner (e.g. drop
`brew install` from `forbid_substrings` — the audit channel already carries the
`no.banned-installers` assertion, which both trials passed). **No task record was
modified by this work.**

**D-b — the trial environment does not sandbox absolute paths.** The trial `PATH` is
hermetic (only the fake shims and the stub-bin), but an agent may still name host
paths directly. The baseline agent ran
`/usr/bin/find /usr/local /opt /nix -name rg -type f` and `/opt/homebrew/bin/rg
--version`, found the **host's** ripgrep 15.2.0, and symlinked it into the trial's
stub-bin (`ln -sf /opt/homebrew/bin/rg "$TRIAL_STUB_BIN/rg"`), which made the
`stub-resolves` verifier pass. The baseline still failed `seq.root-protocol`, so no
false *pass* was produced here — but any task whose acceptance can be satisfied by
"the host already has the tool" is satisfiable **without** the skill. This is the M5
postmortem's host-leakage finding in a new form (absolute paths, not `PATH`).

**D-c — the `skill` tool's `metadata.dir` is invisible to the parser.**
`crates/root-eval/src/events.rs` has no `metadata` field on `ToolCall`, so the
`skill` event's `state.metadata.dir` cannot be read out of `NormalizedEvent`
(INTEGRATION-PROBE §B.2, finding 6). The smoke had to read it from the raw NDJSON.
A harness that must assert "the skill loaded *from this directory*" cannot do it from
the normalized stream today.

**D-d — only the last attempt's retry decision is recorded.** `summary.json` carries
`attempts`, `retry_reason` and `provider_error` of the **final** attempt. Run A's
baseline was retried after a first attempt that ran into the record's 900 s wall
budget (the final attempt records `attempts=2`); the *reason* for that retry is not
auditable from the summary (the superseded attempt's evidence is discarded by
design). One extra field (the first attempt's retry reason) would close the gap.

**D-e — provider flakiness is unchanged (known).** Run A's baseline first attempt
stalled until the 900 s wall budget killed it; the documented retry policy then did
its job (one retry, a **fresh** materialized trial, `attempts=2`, same outcome class
as the retried attempt). Run B had no stall. A 20-trial run must budget for this
(probe §B.7: one provider failure in nine probe runs).

**D-f — the plugin install still runs in the project config dir.** The driver does
not pass `--pure` (the unmodified CLI surface is the default), so npm still creates
`package.json` / `package-lock.json` in `<repo>/.opencode`. Run B pulled **0 registry
bytes** (the shared npm cache was 103 492 KB before and after); Run A's two trials
grew it from the handed-over 94 MB to ≈101 MB (npm metadata/log churn). This is a
cost note, not a blocker — and it is exactly why the config dir is excluded from the
workspace observation (§1.3, D2).

---

## 5. Quality gates

| Gate | Command | Result |
|---|---|---|
| Build | `cargo build` | ✔ clean |
| Format | `cargo fmt --all -- --check` | ✔ clean |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | ✔ clean |
| Crate tests | `cargo test -p root-eval` | ✔ **478 passed, 0 failed** (27 binaries) |
| Workspace tests | `cargo test --all` | ✔ **1 391 passed, 0 failed** (82 binaries) |

New coverage (all in `crates/root-eval/tests/permission_seeding.rs`):

| Test | What it proves |
|---|---|
| `materializer_seeds_the_requested_permission_set` | the materializer writes exactly the posture it is asked for (ask-mode and allow-all), and the two postures are distinct |
| `fake_harness_seeds_ask_mode_and_records_it` | the **fake** path still seeds ask-mode and records `seeding=ask_mode` + the effective set in the sealed `permission-config` artifact and in `summary.json` |
| `real_harness_seeds_allow_all_and_records_it` | the **real** path seeds allow-all; the trial *really ran* with it (the stub CLI reads the project config back out of the trial repository); the effective set is recorded; and the trial still completes although the seeded plugin tree carries npm symlinks (the regression test for the workspace-observation fix) |

The existing M5 suites (`m5_tasks_a`, `m5_tasks_b`, `m5_matrix_smoke`,
`m5_real_harness`, `real_run_replay`, `fake_tool_materialization`, …) are unchanged in
behaviour and green.

> **Environment note (not a defect of this change).** `crates/root-eval/src/supervisor.rs`'s
> `timeout_kills_sleeper_and_its_child` is load-sensitive by design (its own comment
> documents it): under a busy host the 2 s wall can fire before the child's first line
> is flushed, and the test then sees 0 events instead of 1. It failed twice on this
> host while the load average was 28–50, and passed in isolation, with
> `--test-threads=1`, and in every later run. `supervisor.rs` is untouched by this
> work.

---

## 6. What changed in the repository

| File | Change |
|---|---|
| `crates/root-eval/src/materializer.rs` | `PermissionSeeding`; the seeded permission set is an explicit parameter of `materialize_trial_with_fake_tools`; `write_opencode_config(repo, permission)`; the layout carries the posture |
| `crates/root-eval/src/trial.rs` | `TrialLayout.permission_seeding` + `.effective_permission` |
| `crates/root-eval/src/feasibility.rs` | `HarnessChoice::permission_seeding()`; the driver plumbs it; the `permission-config` artifact; `TrialSummary.permission_seeding` / `.effective_permission`; `FeasibilityRunSummary.permission_seeding`; the workspace-observation exclusion |
| `crates/root-eval/src/acceptance.rs` | `capture_tree_snapshot_excluding(root, excluded)` |
| `crates/root-eval/src/verifier.rs` | `tree_fingerprint_excluding(repo, excluded)`, `run_verifier_excluding(spec, repo, env, excluded)` |
| `crates/root-eval/src/trial_env.rs` | `PROJECT_CONFIG_DIR_RELATIVE`, `project_config_dir(layout)`, the repository-local git exclude rule |
| `crates/root-eval/src/lib.rs` | the new public items |
| `crates/root-eval/tests/permission_seeding.rs` | **new** — the three tests above |
| `crates/root-eval/tests/{m5_tasks_a,m5_tasks_b,fake_tool_materialization}.rs` | call sites updated for the new materializer parameter (ask-mode) |
| `Docs/Eval/milestone-6/SMOKE-RUN.md` | this document |
| `Docs/Eval/milestone-6/probes/smoke/` | the raw captures of the smoke run |

**No commits were made.**
