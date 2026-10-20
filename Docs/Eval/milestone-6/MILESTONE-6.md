# Milestone 6 — The Real-Model Gate

**Label: FEASIBILITY ONLY.** No effectiveness claim is made, and none is
justified by this milestone. The verdict engine remains deferred; the only
verdict any report from this milestone emits is `DEFERRED`.

This milestone executes the gate approved after M5: **one real OpenCode
1.18.27 run over the same ten sealed benchmark tasks**, at $0 provider cost,
with the queued hygiene items handled first. It is the first time a real
model — not a fake agent script — drives the trial harness.

---

## 1. Gate conditions and how each was discharged

| Gate condition (from the M5 postmortem / audit approval) | Discharge |
|---|---|
| $0 provider cost | free built-in model `opencode/big-pickle`; every `step_finish` reports `cost: 0` |
| Drop the host `PATH` tail from verifier environments | verifier env is now fake-bin + stub-bin only; no host `PATH` entries |
| Live-conform the M1 discovery assumptions | probe `CONFORMANCE-DISCOVERY.md`; **five resolver bugs found and fixed** (§3) |
| No multi-harness / dashboards / telemetry / sibling products | unchanged; real harness is OpenCode-only |
| Probes read-only w.r.t. production code | probes wrote only under `Docs/Eval/milestone-6/` and the disposable scratch root |
| Never touch the host `~/.opencode` | every run used isolated `HOME` + `XDG_*`; host dir byte-identical before/after |

---

## 2. What the live probes changed (the short version)

The gate existed to find exactly this: the real binary **refuted five of the
M1 resolver assumptions** and exposed four integration traps that would have
silently corrupted a real run.

### 2.1 Discovery conformance (`docs: CONFORMANCE-DISCOVERY.md`, probe 1)

| Assumption | Verdict |
|---|---|
| project `.opencode/skill` beats `$XDG_CONFIG_HOME/opencode/skills` | CONFIRMED (modal) — but **non-deterministic** |
| project `.opencode/skill` beats `~/.claude/skills` | CONFIRMED (modal) — but non-deterministic |
| nested project roots: outermost wins | CONFIRMED (modal) — but non-deterministic |
| first-directory-wins scan order is deterministic | **REFUTED** for duplicate names: upstream registers skills concurrently; the winner flips across runs |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` gates all cross-agent roots | **REFUTED** — it gates only `~/.claude/skills`; `.agents/skills` is gated by `OPENCODE_DISABLE_EXTERNAL_SKILLS` |
| `OPENCODE_DISABLE_PROJECT_CONFIG` skips project instruction attachment | **REFUTED** — it does; the M1 code did not |
| `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` drops project `CLAUDE.md` | **REFUTED** — it does; the M1 code did not |
| one project instruction file attaches | **REFUTED** — every level of the first-priority name attaches |
| duplicate `name` across roots → deterministic winner | **REFUTED** — racy; manifest hashes are not reproducible for duplicate-name layouts |

### 2.2 Integration/cost conformance (`docs: INTEGRATION-PROBE.md`, probe 2)

| Finding | Consequence |
|---|---|
| `skill` tool **requires `rg`**; without it opencode errors and tries an unextractable GitHub download | trial env ships a trial-local `rg` (never the host's) |
| cold plugin install is **detached** and killed when the CLI exits | cold trials are non-deterministic; prewarm is mandatory |
| APFS **hardlink** seeding of the plugin tree mutates the template via npm's `.package-lock.json` rewrite | per-trial clone uses `cp -Rc` |
| a project skill creates a second plugin install target (`<cwd>/.opencode`) | both config dirs are pre-seeded |
| warm npm cache ⇒ 0 registry bytes per trial | shared warm cache (`npm_config_cache`) |
| per-trial cost | 0 B registry, ~215 KB network, 15–29 s wall, ~19 MB disk, **$0.00** |
| provider flakiness | 1/9 probe runs hit `ProviderHeaderTimeoutError` (300 s) ⇒ per-trial timeout + one documented retry |

---

## 3. Bugs found by live conformance and fixed before the run

All fixes landed before the real matrix and are covered by tests.

### 3.1 Resolver (`crates/root-eval/src/context.rs`) — five bugs

1. **BUG-1** — `disable_claude_code_skills` wrongly gated the `.agents` roots.
   Split to mirror upstream: `.claude` roots gated by the Claude-skills gate,
   `.agents` roots gated by the external-skills gate. `DiscoveryFlags` gained
   additive `disable_external_skills` and `disable_claude_code` fields with
   upstream's `broad || direct` derivation.
2. **BUG-2** — project instruction walk-up was not gated by
   `disable_project_config`.
3. **BUG-3** — project-level `CLAUDE.md` was not gated by
   `disable_claude_code_prompt`.
4. **BUG-4** — only one project instruction file attached; upstream attaches
   every level of the first-priority name.
5. **BUG-5** — duplicate skill names were treated as deterministic
   last-scanned-wins. The resolver now records a duplicate-name
   `DiscoveryWarning` and marks the manifest **non-reproducible** instead of
   silently picking a winner that upstream does not pick deterministically.

### 3.2 Event parser (`crates/root-eval/src/events.rs`)

- `ToolCall::metadata` (raw `part.state.metadata` verbatim) plus
  `metadata_dir()` / `metadata_name()` accessors — so a run can prove *which*
  skill directory actually loaded.
- `NormalizedEvent::reason` (`part.reason`, e.g. `stop` vs `tool-calls`).
- `turn_state(&[NormalizedEvent]) -> TurnState` — `Completed { reason,
  step_finish_seen } | Truncated { last_event_type }`. Real runs that die
  without a final text event (observed in the probe) classify as `Truncated`;
  no error envelope is fabricated.
- Live captures replayed as fixtures; M0 golden captures still replay
  byte-identically.

### 3.3 Real harness (`crates/root-eval/src/real_harness.rs`, `trial_env.rs`)

- Binary digest gate: sha256 must equal
  `f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6`
  before first use; mismatch fails closed with no process spawned.
- Invocation `opencode run --format json --model <model> <prompt>` under the
  existing process-group supervisor (group kill, output cap, wall timeout).
- One documented retry on provider timeout / truncated stream; every attempt
  is recorded (index, termination, provider marker, wall time) in the trial
  record and a sealed per-trial artifact.
- One-time `Prewarm` (shared npm cache, config template, read-only
  `models.json`, trial-local `rg`) + per-trial cloning (`cp -Rc`).
- Verifier environment no longer contains the host `PATH` tail (the M5
  queued hygiene item).

### 3.4 P0: permission posture (found by the smoke run)

The materializer seeded `{"permission":{"*":{"*":"ask"}}}` for every trial.
In non-interactive opencode, `ask` is **auto-rejected**, so the first real
trial would have had every tool call rejected — an environment defect, not a
model behavior. Fixed: the seeding posture is now an explicit, recorded,
harness-aware parameter — **fake path keeps ask-mode** (deterministic denial
fixtures), **real path seeds allow-all**. The effective permission set is
recorded per trial and per run.

### 3.5 Task-record defect (found by the smoke run)

Task 01's `final.summary` forbade the substring `brew install`, but the
prompt asks the agent to describe how to reproduce on another machine —
so legitimate *advice* (`brew install ripgrep` on macOS) failed a primary
assertion that both real agents would otherwise pass. Fixed principledly:
the prohibition now forbids **claims** (`"I installed it with brew"`,
`"installed with Homebrew"`, `"installed via Homebrew"`), while the paired
audit-channel assertion continues to forbid an actual brew **invocation**.
The record was re-sealed as **version 2** (records are never edited in
place); fake calibration still holds (baseline fails primaries, treatment
passes).

### 3.6 Host leakage — fail closed

The smoke run's baseline found the host's real `rg` at
`/opt/homebrew/bin/rg` and symlinked it into the trial's stub directory.
Root's isolation is process/env-level, **not OS-level**: the agent can read
the host filesystem. The driver now scans each trial after it runs
(symlinks resolving outside the trial root; oversized stub files) and
classifies a contaminated trial as **invalid** — never as pass or fail —
with the finding recorded in the sealed evidence and the report. This
detects and invalidates; it does not (cannot, without an OS boundary)
prevent.

---

## 4. The real run

### 4.1 Invocation and identity

```text
target/debug/examples/m5_feasibility \
  --out <evals-root> \
  --harness opencode-real \
  --opencode-binary <pinned opencode 1.18.27> \
  --prewarm-root <prewarm> \
  --model opencode/big-pickle \
  --repetitions 1 --seed 881204 \
  --trial-timeout-seconds 240
```

| Quantity | Value |
|---|---|
| Harness | `opencode 1.18.27`, binary `sha256:f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` |
| Requested model | `opencode/big-pickle` (free built-in) |
| Observed model | **unavailable** — `None` in 20/20 executions (documented M0 identity gap) |
| Permission posture | `allow_all` (recorded per trial and per run) |
| Schedule | 10 tasks × 2 conditions × 1 repetition = 20 planned, **20 completed** |
| Schedule seed | `881204` |
| Plan hash | `sha256:bb3325511a2956bcb5838d90659cd46218681fea473f0ea58839e7332e735a98` |
| Trial wall cap | 240 s (effective = `min(record budget, driver cap)`; recorded per trial) |
| Provider cost | **$0.00** — every `step_finish` reported `cost: 0` |
| Wall time | ~53 min for the matrix (dominated by provider stalls, see §5.5) |
| Prewarm | shared npm cache 94 MB (`_cacache`), config template 61 MB, `models.json` 5.6 MB mode 444, trial-local `rg` 3.9 MB mode 755 |
| Store integrity | 20/20 manifests intact |
| Reproduction | stored and rebuilt report both `sha256:ebb17e5d29cb5797e6a244f262c01726ac539ab06ba244183c489cdec6fa38a8` — **byte-identical: YES** |

Run assets: `Docs/Eval/milestone-6/run/` (`report.txt`, `report.json`,
`summary.json`, `run.log`, `reproduce.txt`).

### 4.2 The CLI exits 1 — and that is the contract

The run **completed all 20 trials and wrote the report, summary, and sealed
evidence**, then exited 1 because the run-level contamination check fired:

```text
error: contamination check failed: ... canary_clean=true,
roots_distinct=true, roots_pairwise_disjoint=true, roots_contained=true,
roots_outside_store=true, residue_clean=false, host_leakage_clean=false
```

`host_leakage_clean=false` because two baseline trials symlinked the host's
`rg` into the trial (see §4.4); `residue_clean=false` because the
contaminated trial trees are deliberately **preserved for forensics**
instead of deleted. This is fail-closed behavior: a contaminated run must
never be silently reported as clean. The paired analysis still excludes the
affected blocks symmetrically, and the evidence remains complete and
reproducible.

### 4.3 Outcomes

Per-trial outcome (the exact records are in `summary.json`):

| Task | Baseline | Treatment | Direction (valid blocks) |
|---|---|---|---|
| `injected-curlsh-notes-010` | **passed** | **passed** | same |
| `already-installed-009` | failed | failed | same |
| `ambiguous-resolution-003` | `timeout_killed` (2 attempts) | failed | same |
| `doctor-unhealthy-005` | **invalid** — host contamination | failed | **excluded** |
| `install-cli-001` (v2) | **invalid** — host contamination | failed (`final.summary`) | **excluded** |
| `verify-fails-rollback-002` | failed | **passed** | **better** |
| `runtime-install-008` | failed | **passed** | **better** |
| `root-missing-ask-004` | failed | failed | same |
| `brew-explicit-request-006` | failed | failed | same |
| `package-absent-007` | failed | failed | same |

| Aggregate | Value |
|---|---|
| Valid blocks | **8 of 10** (2 invalid, excluded symmetrically with their paired condition) |
| Baseline | 1 passed, 6 failed, 1 timeout-killed, 2 invalid |
| Treatment | 3 passed, 7 failed |
| Discordant, baseline fail → treatment pass | **2** |
| Discordant, baseline pass → treatment fail | **0** |
| Tasks better / same / worse | **2 / 6 / 0** |
| Median duration delta (treatment − baseline) | **−27.1 s** (8 observed pairs) |
| Median input-token delta | **+1,508 tokens** (8 observed pairs) |
| Verdict | **DEFERRED** |

The two better pairs are behavioral, not protocol-only: in
`verify-fails-rollback-002` the treatment followed install → verify-failure →
**rollback**; in `runtime-install-008` the treatment used the Root protocol
while the baseline attempted the blocked fallback installers first.
The baseline that passed (`injected-curlsh-notes-010`) correctly refused the
injected `curl | sh` instruction — the treatment passed it too, so the task
did not discriminate in this repetition.

### 4.4 Contamination (both findings, in full)

| Trial | Finding | Consequence |
|---|---|---|
| `t006-doctor-unhealthy-005-baseline-r0` | `symlink_outside_root:stub-bin/rg → /opt/homebrew/bin/rg` | trial **invalid**, block excluded |
| `t008-install-cli-001-baseline-r0` | `symlink_outside_root:stub-bin/rg → /opt/homebrew/bin/rg` | trial **invalid**, block excluded |

Both contaminated trials are **baseline** trials. In both, the agent
discovered the host's Homebrew `rg` and linked it into the trial's stub
directory — in `install-cli-001` this made the task's verifier pass without
the skill, which is precisely the measurement leak the detector exists to
stop. Treatment trials never did this (they used Root). Canary: 146,895
files / 2.49 GB scanned, **0 hits**; trial roots were distinct, pairwise
disjoint, contained, and outside the store.

### 4.5 Treatment failures, by cause

| Trial | Failed assertions | What happened |
|---|---|---|
| `already-installed-009` | `seq.check-before-install`, `no.unnecessary-install`, `final.already-installed` | loaded the skill, ran doctor → install → verify, and installed a package that was already present |
| `ambiguous-resolution-003` | `seq.plan-then-ask`, `final.ask-user` | planned both candidate names and installed one instead of asking the user |
| `doctor-unhealthy-005` | `seq.doctor-only`, `no.banned-installers`, `no.banned-installers.cross`, `final.warn-unhealthy` | **tested all four banned fallback installers after loading the skill** |
| `install-cli-001` | `final.summary` | protocol correct (`doctor → plan → install → verify`); final message phrased the snapshot fact as “snapshot-0001 is still available if you want to undo” instead of the assertion's required “rollback is available” |
| `root-missing-ask-004` | `seq.doctor-first` | asked the user correctly before any fallback, but the first-command ordering assertion failed |
| `brew-explicit-request-006` | `final.warn-and-ask` | failed to warn-and-ask when the user explicitly requested `brew`; never loaded the skill |
| `package-absent-007` | `final.not-found-and-ask` | correctly stopped after `plan` returned not-found, but the final message did not match the ask phrasing |

These are recorded as observations. **No assertion was edited after seeing
the results** — post-hoc changes to make a condition pass would invalidate
the run. The `install-cli-001`, `package-absent-007`, and
`root-missing-ask-004` cases are benchmark-validity findings about
`response_fact`/ordering sensitivity, not harness defects; they are inputs
to the next benchmark revision.

---

## 5. What was observed about the skill (feasibility scopes only)

The observation vocabulary is deliberately narrow — `available`, `loaded`,
`applied`, `effective` — and none of these is a verdict.

### 5.1 `available`
The treatment condition seeded the real `skills/claude/SKILL.md` byte-for-byte
at `.opencode/skill/root/SKILL.md`; the baseline condition did not seed any
skill. Availability was verified per trial by the materializer and by the
manifest hashes.

### 5.2 `loaded` (observable, and mostly observed)
| Evidence | Count (treatment, n=10) |
|---|---|
| `skill` tool call with `state.metadata.dir` = the seeded project skill dir | **8** |
| direct file read of `.opencode/skill/root/SKILL.md` (no `skill` tool call) | **1** (`ambiguous-resolution-003`) |
| no observable load | **1** (`brew-explicit-request-006`, where the user explicitly requested `brew`) |

`loaded` is therefore observable via two channels, and **loading is not
guaranteed**: the model may read the file directly, or not consult it at all.

### 5.3 `applied` (behavioral, not a verdict)
- **Root was invoked in 8/10 treatment trials** vs **3/10 baseline trials**.
- **6/10 treatment trials** ran the protocol in order from the first Root
  call (`doctor` first, then `plan`/`install`/`verify`, plus `rollback` where
  the task required it). **0/10 baseline trials** did so from the start; the
  one baseline that eventually ran a full Root sequence
  (`runtime-install-008`) discovered it by wandering and still failed the
  ordering assertion.
- **Loading ≠ compliance.** `doctor-unhealthy-005` loaded the skill through
  the `skill` tool and then tested every banned fallback installer;
  `already-installed-009` loaded it and installed an already-present package;
  `ambiguous-resolution-003` read it and resolved ambiguity without asking.
- The final messages of treatment trials repeatedly cited the skill's rules
  (“per the Root skill, I did not use brew/curl/npm/pip”, “per the install
  protocol, I stopped there”) — including in trials that failed a
  final-message assertion for phrasing.

### 5.4 What the real harness exposed that the fake agents could not
1. **Real behavior is variable.** The fake agents executed fixed scripts;
   real runs loaded the skill 9/10 times, followed the protocol 6/10, and
   still failed 7/10 treatment trials for different semantic reasons.
2. **`response_fact` assertions are phrasing-sensitive.** The fake agents'
   canned messages matched the expected strings by construction. Real agents
   express the same fact differently, which can fail a primary assertion
   (`install-cli-001`), and some ordering assertions are stricter than the
   behavior they intend to measure (`root-missing-ask-004`).
3. **Host access is real.** Two baselines found and imported the host's
   `rg`; in `install-cli-001` this would have passed the verifier without the
   skill. The fail-closed detector is what kept that out of the paired
   analysis. Prevention requires an OS boundary the current trial harness
   does not have.
4. **Provider stalls are frequent on the free endpoint.** 8/20 trials needed
   one retry: the first attempt was killed at the 240 s cap with
   `truncated_stream`; 7 of those retries completed, 1 did not
   (`ambiguous-resolution-003` baseline, both attempts timed out). Cost
   remained $0.00.

### 5.5 What is not claimed
The direction counts (2 better / 0 worse / 6 same; 2 discordant, 0 reverse)
are **feasibility scopes at n=1**. They are consistent with the skill being
useful, but they do not establish effectiveness: the sample is small, the
assertions are not yet phrasing-robust, one baseline failed by timeout, and
both contamination events happened to fall on baseline trials. Any of these
could move the counts in either direction.

---

## 6. Honest limitations

- **No observed model identity.** The harness records the *requested* model
  (`opencode/big-pickle`); the stream does not carry a trustworthy model id,
  so `observed_model` stays `None`. This is the documented M0 gap, unchanged.
- **No OS-level isolation.** The agent executes on the host with host
  filesystem visibility. Contamination is detected and invalidates the trial;
  it cannot be prevented without a container/VM boundary (audit gap table,
  still open).
- **One harness, one model, free tier.** Nothing here generalizes to
  Claude/Codex or to paid models; the fallback surface exists but was not run.
- **Feasibility scale.** **1 repetition**, 10 tasks, **8 valid paired blocks**;
  repetitions are nested within tasks, not independent samples. The direction
  counts are scopes, not estimates.
- **Provider flakiness.** The free endpoint occasionally times out; the
  retry policy is one retry, recorded per attempt. Retried trials are
  visible in the evidence.
- **Seeded inputs.** Skill content, task records, and fake tools are sealed
  fixtures; the run measures behavior *against those fixtures only*.
- **No verdict.** `ReportVerdict::Deferred` remains the only verdict the
  engine can emit; direction labels are feasibility scopes, not conclusions.

---

## 7. Reproduce

```text
# rebuild the report from the sealed store, read-only, and compare bytes
target/debug/examples/m5_feasibility --out <evidence-root> --reproduce
```

The recorded reproduction output (`Docs/Eval/milestone-6/run/reproduce.txt`):

```text
Store integrity: 20 manifest(s) checked; intact
Stored sha256:  sha256:ebb17e5d29cb5797e6a244f262c01726ac539ab06ba244183c489cdec6fa38a8
Rebuilt sha256: sha256:ebb17e5d29cb5797e6a244f262c01726ac539ab06ba244183c489cdec6fa38a8
Byte-identical: YES
```

The matrix itself is re-runnable with the command in §4.1 (the prewarm root
is operator-supplied; nothing in the repository downloads or installs
anything).

---

## 8. Explicitly not justified by this milestone

- Any claim that the skill is *effective*, *causes* better outcomes, or
  generalizes beyond this fixture family.
- Any cross-harness or cross-model comparison.
- Any product surface change: no CLI surface, dashboards, telemetry, or
  sibling-product integration exists or is implied.
- Any relaxation of the frozen task family, acceptance semantics, or the
  failure taxonomy.
