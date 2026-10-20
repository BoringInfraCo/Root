# Milestone 6 — Discovery Conformance (OpenCode `1.18.27`, live probe)

Date: 2026-10-19 · Prober: milestone-6 discovery subagent · Scope: **probe only**
(no `src` edits, no commits, no cargo).

Subject under test: `crates/root-eval/src/context.rs` (the M1 effective-context
resolver) versus the pinned upstream CLI it models.

---

## 0. Binary identity and hermeticity

| Item | Value |
|---|---|
| Binary | `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/bin/opencode` |
| sha256 | `f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` ✅ matches `Docs/Eval/milestone-0/HOST-PROBE.md` §1 |
| Size / type | 144,074,210 B, Mach-O 64-bit arm64 |
| Re-acquisition | **not needed** (hash matched the pinned value; `HOST-PROBE.md` §1 procedure not triggered) |
| `--version` | `1.18.27` |

Scratch root: `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery/`
(trials, raw stdout/stderr, extracted bundle strings).

Every run was hermetic:

```sh
env -i HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
       XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
       PATH=<scratch>/syspath OPENCODE_DISABLE_AUTOUPDATE=1 npm_config_cache=<scratch>/shared/npmcache \
  opencode run --format json --model opencode/big-pickle --pure "<prompt>"
```

Model: built-in free `opencode/big-pickle` ($0, no credentials, `cost=0` in every
`step_finish`). Host isolation verified: **no file under `/Users/sergio/.opencode`
or `/Users/sergio/.config/opencode` was modified** (`find -newermt "-3 hours"` → empty).

Evidence layout (this directory):

| Path | Contents |
|---|---|
| `probes/discovery/task-a/` | TASK A raw NDJSON (model-driven `run`) + `debug skill` JSON (model-free) |
| `probes/discovery/task-b/` | TASK B flag trials (b0 control … b4) |
| `probes/discovery/task-b2/` | instruction-attachment matrix (m1–m5) |
| `probes/discovery/task-b3/` | 30-sample race sets, m5/m6/m7 follow-ups, flag matrix JSON |
| `probes/discovery/binary-evidence.txt` | upstream source excerpts extracted from the pinned binary |
| `probes/discovery/scripts/` | `lib.sh`, `d1.sh`…`d5.sh` (probe scripts), `summarize.py` |
| `probes/discovery/task-*/summary.txt` | machine-readable tallies (regenerate with `summarize.py`) |

### Observables used

1. **Skill winner** — the native `skill` tool's `tool_use` event,
   `state.metadata.dir` (per upstream contract §4.2). Also the `MARKER:` line
   inside `state.output`, which identifies *which* file won.
2. **Skill inventory** — `opencode debug skill` (model-free; prints
   `name`, `description`, `location`, `content`).
3. **Instruction attachment** — two independent methods:
   * *path*: the model is asked to list verbatim the lines starting with
     `Instructions from:` (upstream injects `Instructions from: <path>\n<body>`);
   * *content (hallucination-proof)*: each planted instruction file contains a
     unique magic passphrase (`MAGIC-…-NNNN`); the model is asked for the
     passphrase. A passphrase can only be produced if the file was really
     injected. Control run with **no** instruction files answered `NONE`
     (`task-b/b0-control.model.run-1.ndjson`).
4. **Upstream implementation** — the Bun single-file bundle keeps the module
   sources verbatim; `strings -a <binary>` yields the decompiled
   `skill/discovery.ts`, `session/instruction.ts`, `config/config.ts` and
   `core/flag/flag.ts` code (excerpts in `binary-evidence.txt`). This is the
   tie-breaker for anything the model cannot prove.

---

## 1. Assumption scoreboard

| # | Assumption | Source | Verdict | Evidence |
|---|---|---|---|---|
| A1 | Project `.opencode/skill/dup` beats `${XDG_CONFIG_HOME}/opencode/skills/dup` (config dirs scanned XDG-first, so project `.opencode` is later) | `context.rs` §"Documented scan order" step 5 | **CONFIRMED (modal) / determinism REFUTED** | `task-a/a1.*`: project 20/30 model-free, 4/6 model-driven |
| A2 | Project `.opencode/skill/dup` beats `~/.claude/skills/dup` | `context.rs` step 2 vs 5 | **CONFIRMED (modal) / determinism REFUTED** | `task-a/a2.*`: project 24/30 model-free, 3/6 model-driven |
| A3 | With two `.opencode` dirs at different depths, the **outermost** wins (chain is innermost-first) | `context.rs` step 5 | **CONFIRMED (modal) / determinism REFUTED** | `task-a/a3.*`: far 25/30 model-free; model-driven 4/6 near (bias flips) |
| A4 | Cross-agent walk-up is scanned **per level** (`.claude` then `.agents` at each level) | `context.rs` step 4 | **CONFIRMED** (model-free n=30; model-driven n=4 too small to add weight) | `task-b3/a4.[0-9]*.json` (FAR-CLAUDE 24/30) + upstream `up()` generator |
| A5 | Duplicate skill names resolve by a **deterministic** last-scanned-wins order | `context.rs` lines 12–37, 522–523 | **REFUTED** — upstream is racy | 6/30, 5/30, 10/30, 6/30 flip-flops; `duplicate skill name` WARN pairs swap between runs |
| B1 | `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` also skips `.agents` | M1 assumption | **REFUTED** | `task-b3/m7.*.json`: `.agents` skills stay visible |
| B2 | `OPENCODE_DISABLE_PROJECT_CONFIG` leaves the project instruction walk-up active | M1 assumption | **REFUTED** | `task-b2/m1-*`: project `AGENTS.md` not attached under the flag |
| B3 | `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` suppresses the global `~/.claude/CLAUDE.md` fallback | `context.rs` lines 752–775 | **CONFIRMED** | `task-b2/m2-*`: attached without flag, absent with flag |
| B4 | `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` leaves the project walk-up unaffected | `context.rs` line 138 | **REFUTED for project-level `CLAUDE.md`** | `task-b2/m3-*`: project `CLAUDE.md` absent under the flag |
| B5 | Global instruction = first match wins (`${XDG_CONFIG_HOME}/opencode/AGENTS.md` beats `~/.claude/CLAUDE.md`) | `context.rs` lines 752–775 | **CONFIRMED** | `task-b3/m6-*`: only the XDG file attached |
| B6 | One project instruction file is attached per message | `context.rs` lines 46–48, 777–791 | **REFUTED** | `task-b3/m5b-paths.model.run-1.ndjson`: **two** `Instructions from:` blocks (both levels) |
| B7 | `OPENCODE_DISABLE_PROJECT_CONFIG` also removes project `.opencode` skills and `skills.paths` | `context.rs` lines 819–831, 1072–1123 | **CONFIRMED** | `task-b/b2-on.*`: `projskill`/`explicitskill` → `Skill "…" not found` |
| B8 | Config-dir order = XDG `opencode`, every `.opencode` (instance→worktree), `~/.opencode`, `OPENCODE_CONFIG_DIR` | `context.rs` lines 819–831 | **CONFIRMED (code-level; not separately exercised live)** | upstream `ConfigPaths.directories` in `binary-evidence.txt` §4 |

---

## 2. TASK A — skill scan order

### 2.1 Predicted order (from `crates/root-eval/src/context.rs`)

`skill_scan_roots()` (line 843) pushes, in order:

1. `~/.claude/skills`
2. `~/.agents/skills`
3. per level of the walk chain (instance dir → worktree root, innermost first):
   `<level>/.claude/skills`, then `<level>/.agents/skills`
4. per config dir (`config_dirs()`, line 819) = `${XDG_CONFIG_HOME}/opencode`,
   then every `<level>/.opencode` (innermost first), then `~/.opencode`, then
   `OPENCODE_CONFIG_DIR`: `<dir>/skill/**` then `<dir>/skills/**`
5. explicit `skills.paths`

`winning_skills()` (line 1195) keeps the **last** candidate for a name →
"later scans override earlier ones by skill name".

### 2.2 Observed

| Case | Layout (both files have frontmatter `name: dup`) | Predicted winner | Observed winner (`metadata.dir` / `debug skill` location) | Verdict |
|---|---|---|---|---|
| A1 | `cwd/.opencode/skill/dup/SKILL.md` (PROJECT) vs `config/opencode/skills/dup/SKILL.md` (XDG) | PROJECT | model-free **PROJECT 20 / XDG 10** (n=30); model-driven PROJECT 4 / XDG 2 (n=6) | modal ✓, non-deterministic ✗ |
| A2 | `cwd/.opencode/skill/dup/SKILL.md` (PROJECT) vs `home/.claude/skills/dup/SKILL.md` (HOME-CLAUDE) | PROJECT | model-free **PROJECT 24 / HOME-CLAUDE 6** (n=30); model-driven 3 / 3 | modal ✓, non-deterministic ✗ |
| A3 | `worktree/app/.opencode/skill/dup` (NEAR) vs `worktree/.opencode/skill/dup` (FAR), git root = `worktree`, instance = `worktree/app` | FAR (outermost scanned last) | model-free **FAR 25 / NEAR 5** (n=30); model-driven NEAR 4 / FAR 2 (n=6) | modal ✓ (model-free), runner-dependent ✗ |
| A4 | `worktree/app/.agents/skills/dup` (NEAR-AGENTS) vs `worktree/.claude/skills/dup` (FAR-CLAUDE) — discriminates per-level vs per-target ordering | FAR-CLAUDE | model-free **FAR-CLAUDE 24 / NEAR-AGENTS 6** (n=30); model-driven NEAR 3 / FAR 1 (n=4) | modal ✓, non-deterministic ✗ |

A4 is the discriminator for the *structure* of the order: the resolver scans
per level (`app/.claude`, `app/.agents`, `worktree/.claude`, `worktree/.agents`),
while a per-target reading would end with `app/.agents`. The upstream
`Filesystem.up()` generator (`binary-evidence.txt` §5) confirms the per-level
reading:

```js
async function* qq({targets, start, stop}) { let H = start;
  while (true) { for (const J of targets) { const K = join(H, J); if (exists(K)) yield K }
    if (stop === H) break; const I = dirname(H); if (I === H) break; H = I } }
```

⇒ **the resolver's scan-order structure matches upstream** (also confirmed
against `ConfigPaths.directories`, `binary-evidence.txt` §4).

### 2.3 The duplicate-name resolution is a race (upstream bug, resolver over-claims)

Upstream registers discovered skills with unbounded concurrency:

```js
pA = fnUntraced(function*(state, matches){ await forEach(matches, (f) => uA(state, f, K), {concurrency:"unbounded"}) })
uA = … if (state.skills[name]) logWarning("duplicate skill name", {name, existing, duplicate});
     state.skills[name] = {…}          // last REGISTRATION wins
```

The `matches` set is ordered (deterministic scan order) but the *registration*
order is the async completion order of the file reads. The `duplicate skill
name` WARN pair therefore swaps between runs — e.g. for A2 (raw log:
`task-a/a2.duplicate-skill-warnings.log`, 6 model-driven runs):

```
existing=…/cwd/.opencode/skill/dup/SKILL.md  duplicate=…/home/.claude/skills/dup/SKILL.md
existing=…/home/.claude/skills/dup/SKILL.md  duplicate=…/cwd/.opencode/skill/dup/SKILL.md
```

Registration-order tally over the 22 model-driven runs
(`task-a/{a1,a2,a3,a4}.duplicate-skill-warnings.log`):

| Case | first-registered file (count) |
|---|---|
| A1 | XDG `skills/dup` ×4, project `.opencode/skill/dup` ×2 |
| A2 | project `.opencode/skill/dup` ×3, `~/.claude/skills/dup` ×3 |
| A3 | far `worktree/.opencode/skill/dup` ×4, near `worktree/app/...` ×2 |
| A4 | far `worktree/.claude/skills/dup` ×3, near `worktree/app/.agents/...` ×1 |

(The model-free `opencode debug skill` path does not emit these WARN lines —
its log file stays empty — so the WARN evidence comes from the model-driven
runs only.)

Consequence for Root: **the winner of a duplicate skill name is not a
function of the layout**, so `EffectiveContextManifest::sha256()` (which
encodes the winning path) is not reproducible for such layouts. See **BUG-5**.

---

## 3. TASK B — flag semantics

### 3.1 Skills under the `OPENCODE_DISABLE_*` gates

Layout: `~/.claude/skills/claudeskill`, `~/.agents/skills/agentsskill`,
`cwd/.claude/skills/projclaudeskill`, `cwd/.agents/skills/projagentsskill`,
`cwd/.opencode/skill/ocskill`. Observable: `opencode debug skill` (model-free),
cross-checked with a model-driven run (`task-b/b1-off|b1-on.*`).

| Env | `~/.claude/skills` | `~/.agents/skills` | walk-up `.claude/skills` | walk-up `.agents/skills` | `.opencode` config skills |
|---|:--:|:--:|:--:|:--:|:--:|
| *(none)* | ✅ | ✅ | ✅ | ✅ | ✅ |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` | ❌ | **✅** | ❌ | **✅** | ✅ |
| `OPENCODE_DISABLE_EXTERNAL_SKILLS=1` | ❌ | ❌ | ❌ | ❌ | ✅ |
| `OPENCODE_DISABLE_CLAUDE_CODE=1` (broad) | ❌ | **✅** | ❌ | **✅** | ✅ |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` + `OPENCODE_DISABLE_EXTERNAL_SKILLS=1` | ❌ | ❌ | ❌ | ❌ | ✅ |
| `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1` | ✅ | ✅ | ✅ | ✅ | ✅ |

Upstream flag definitions (`binary-evidence.txt` §1):

```js
disableExternalSkills:    k("OPENCODE_DISABLE_EXTERNAL_SKILLS"),
disableClaudeCodePrompt:  broad(OPENCODE_DISABLE_CLAUDE_CODE) || direct(OPENCODE_DISABLE_CLAUDE_CODE_PROMPT),
disableClaudeCodeSkills:  broad(OPENCODE_DISABLE_CLAUDE_CODE) || direct(OPENCODE_DISABLE_CLAUDE_CODE_SKILLS),
```

and the discovery gate (`binary-evidence.txt` §2):

```js
if (!disableExternalSkills) { if (!disableClaudeCodeSkills) Z.push(".claude"); Z.push(".agents"); … }
```

⇒ `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` removes **only the `.claude` roots**;
the `.agents` roots belong to `OPENCODE_DISABLE_EXTERNAL_SKILLS`.
**M1 assumption B1 is REFUTED.**

### 3.2 Instructions under the gates (magic-passphrase method)

One planted instruction file per trial, each carrying a unique passphrase.

| Planted file | no flag | `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` | `OPENCODE_DISABLE_PROJECT_CONFIG` | `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` | `OPENCODE_DISABLE_CLAUDE_CODE` (broad) |
|---|---|---|---|---|---|
| `cwd/AGENTS.md` (project) | ATTACHED | ATTACHED | **NOT attached** | ATTACHED | ATTACHED |
| `home/.claude/CLAUDE.md` (global) | ATTACHED | ATTACHED | ATTACHED | **NOT attached** | **NOT attached** |
| `cwd/CLAUDE.md` (project) | ATTACHED | ATTACHED | **NOT attached** | **NOT attached** | **NOT attached** |
| `config/opencode/AGENTS.md` (global XDG) | ATTACHED | ATTACHED | ATTACHED | ATTACHED | ATTACHED |

Upstream implementation (`binary-evidence.txt` §3):

```js
n = [ join(config, "AGENTS.md"), ...(!disableClaudeCodePrompt ? [join(home, ".claude", "CLAUDE.md")] : []) ]   // global, first match wins
y = ["AGENTS.md", ...(!disableClaudeCodePrompt ? ["CLAUDE.md"] : []), "CONTEXT.md"]                              // project priority names
if (!OPENCODE_DISABLE_PROJECT_CONFIG) for (const O of y) { const U = await findUp(O, directory, worktree); … }
```

⇒ **M1 assumption B2 is REFUTED**: `OPENCODE_DISABLE_PROJECT_CONFIG` switches
the project instruction walk-up **off entirely** (the upstream code only
re-points the *relative* `instructions` glob base at the global config dir).

Two more facts measured:

* **Multi-level attachment** (`task-b3/m5b-paths.model.run-1.ndjson`): with `AGENTS.md` at both
  `worktree/app/` and `worktree/`, upstream attaches **both**:
  ```
  Instructions from: …/worktree/app/AGENTS.md
  Instructions from: …/worktree/AGENTS.md
  ```
  `findUp` returns *every* match of the first-priority name; the resolver keeps
  only the innermost one.
* **Global precedence** (`task-b3/m6-*`): with `${XDG_CONFIG_HOME}/opencode/AGENTS.md`
  **and** `~/.claude/CLAUDE.md` both present, only the XDG file is attached
  (first-match-wins) — matches the resolver.

---

## 4. Bug reports (all in `crates/root-eval/src/context.rs`)

### BUG-1 — `disable_claude_code_skills` wrongly disables the `.agents` skill roots

* **File**: `crates/root-eval/src/context.rs`
* **Where**: doc comment lines 134–136; implementation lines 856–883
  (`if !flags.disable_claude_code_skills { … push(home/.claude/skills) … push(home/.agents/skills) … }`,
  plus the walk-up `.claude`/`.agents` pushes at lines 869–882).
* **Expected** (upstream 1.18.27): `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` removes
  only `~/.claude/skills/**` and the walk-up `<level>/.claude/skills/**`;
  `~/.agents/skills/**` and `<level>/.agents/skills/**` remain discoverable.
* **Actual** (live): the resolver removes all four roots; the harness still
  exposes `agentsskill` and `projagentsskill` (`task-b3/m7.OPENCODE_DISABLE_CLAUDE_CODE_SKILLS_1.json`).
* **Impact**: the resolver reports *fewer* skills than the harness will find →
  manifest/skill-availability drift, and Root would classify a discoverable
  skill as missing. Also `DiscoveryFlags` has **no** field for
  `OPENCODE_DISABLE_EXTERNAL_SKILLS` (the gate that actually removes the
  `.agents` roots) nor for the broad `OPENCODE_DISABLE_CLAUDE_CODE`
  (= prompt **and** skills), so the caller cannot express upstream behaviour.
  The flag-definition vocabulary in upstream is:
  `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`, `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`,
  `OPENCODE_DISABLE_CLAUDE_CODE` (broad), `OPENCODE_DISABLE_EXTERNAL_SKILLS`.

### BUG-2 — project instruction walk-up is not gated by `disable_project_config`

* **File**: `crates/root-eval/src/context.rs`
* **Where**: line 499 (`if let Some(instruction) = project_instruction(&resolved)?` — called
  unconditionally) and `project_instruction()` lines 777–791.
* **Expected**: with `OPENCODE_DISABLE_PROJECT_CONFIG=1` no project-level
  `AGENTS.md` / `CLAUDE.md` / `CONTEXT.md` is attached.
* **Actual** (live): the resolver attaches the project instruction file; the
  harness attaches nothing (`task-b2/m1-none-skills-projectcfg.model.run-1.ndjson` → `NONE`,
  `task-b3/m5b-projectcfg.model.run-1.ndjson` → `NONE`).
* **Impact**: the manifest lists an instruction file that was never injected into
  the harness prompt → hash mismatch and a false "effective context" claim.

### BUG-3 — project-level `CLAUDE.md` is not gated by `disable_claude_code_prompt`

* **File**: `crates/root-eval/src/context.rs`
* **Where**: `project_instruction()` line 778
  (`for file_name in ["AGENTS.md", "CLAUDE.md", "CONTEXT.md"]`) and the flag doc
  at lines 137–139 ("the project walk-up is unaffected").
* **Expected**: upstream's project priority list is
  `["AGENTS.md", ...(prompt ? ["CLAUDE.md"] : []), "CONTEXT.md"]` — under
  `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1` a project-level `CLAUDE.md` is **not**
  a candidate.
* **Actual** (live): the resolver still resolves `cwd/CLAUDE.md`
  (`task-b2/m3-none-skills-projectcfg-prompt.model.run-1.ndjson` → `NONE` upstream).

### BUG-4 — project instruction attachment is multi-file, not single-file

* **File**: `crates/root-eval/src/context.rs`
* **Where**: module docs lines 46–48 ("one file per message") and
  `project_instruction()` lines 777–791 (returns `Option<ResolvedInstruction>`).
* **Expected**: upstream attaches **every** file with the first-priority name
  found along the walk-up (`findUp` returns all levels).
* **Actual** (live): the resolver keeps only the innermost match.
* **Impact**: the manifest under-counts instruction files (and the canonical
  projection/hash differs from the harness prompt).

### BUG-5 — duplicate skill names are treated as deterministic upstream

* **File**: `crates/root-eval/src/context.rs`
* **Where**: module docs lines 12–37 ("duplicate names resolve 'last-scanned
  wins'"), `duplicate_warnings()` lines 1172–1193, `winning_skills()` lines 1195–1204.
* **Expected**: upstream registers skills with `{concurrency:"unbounded"}`, so the
  winner is decided by async completion order and **flips between runs**
  (observed: 10/30, 6/30, 5/30, 6/30 flip-flops across A1–A4; the
  `duplicate skill name` WARN `existing`/`duplicate` pair swaps accordingly).
* **Actual**: the resolver always returns the last-scanned candidate
  deterministically.
* **Impact**: `EffectiveContextManifest::sha256()` is not reproducible for any
  layout where two `SKILL.md` files share a frontmatter name; a harness run can
  legitimately load a different file than the manifest predicts. The resolver
  should either (a) treat duplicate names as a nondeterministic outcome
  (record both candidates, hash the *set*), or (b) the fixture corpus must
  forbid duplicate names.

---

## 5. Secondary documentation defects (not code)

* `Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md` §4.1 — "Global cross-agent dirs
  (skipped entirely under `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`/external-skills
  disable flags)" — the `.agents` roots are **not** skipped by
  `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` (see §3.1).
* `Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md` §4.3 — "one file per message" for
  project instructions is wrong (all levels attach); the
  `OPENCODE_DISABLE_PROJECT_CONFIG` effect on the walk-up is not documented.
* `crates/root-eval/tests/fixtures/opencode-discovery/09-cross-agent-roots/notes.md`
  — the note already records the `.agents` behaviour as “an interpretation”; it is
  now **refuted** by live measurement and should be corrected together with
  `expected.json` (scenario `disable-claude-skills` expects zero skills; upstream
  still finds the two `.agents` skills).

---

## 6. OPEN items (not settled by this probe)

| # | Item | Why open |
|---|---|---|
| O1 | Intra-root candidate ordering (“lexicographic by path”, `context.rs` line 36) | Upstream globs files and registers them concurrently; the scan order is not observable at 1.18.27 because the race hides it. |
| O2 | A3/A4 winner bias differs between `opencode run` and `opencode debug skill` (model-driven favours the *near* file, model-free the *far* one) | Two entry points appear to schedule the scan differently; not characterised further. |
| O3 | `skills.urls` (remote skill pull into `${XDG_CACHE_HOME}/opencode/skills/<name>`) | Not exercised (network + cache side effects). |
| O4 | `OPENCODE_CONFIG_DIR` position in the config-dir order | Confirmed only from the extracted upstream code, not exercised live. |
| O5 | `~/.opencode` position in the config-dir order | Confirmed only from the extracted upstream code, not exercised live. |
| O6 | `opencode debug skill` vs `opencode run` — whether the *reported* skill list can differ for the same layout | Suggested by O2; not quantified. |

---

## 7. Reproduction

```sh
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
sh $D/d1.sh   # TASK A: duplicate skill-name winners (a1,a2,a3)
sh $D/d2.sh   # TASK B: flag trials b0..b4
sh $D/d3.sh   # TASK B2: instruction attachment matrix (m1..m5)
sh $D/d4.sh   # 30-sample race sets, m5b/m6, m7 flag matrix
sh $D/d5.sh   # TASK A case 4 (per-level vs per-target discriminator)
python3 Docs/Eval/milestone-6/probes/discovery/scripts/summarize.py
```

`d1.sh`–`d5.sh` are copied verbatim into
`Docs/Eval/milestone-6/probes/discovery/scripts/`; `lib.sh` holds the trial
factory (`mktrial`), the hermetic env (`trienv`) and the fixture writers
(`skillmd`, `instruct`).
