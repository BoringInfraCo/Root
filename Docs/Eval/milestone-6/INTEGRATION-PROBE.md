# Milestone 6 — Integration Probe: cheapest hermetic real-trial setup for OpenCode 1.18.27

Date: 2026-10-19 · Platform: macOS darwin/arm64 · Model: `opencode/big-pickle` (built-in free default, $0) · Auth: none
Scratch: `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/` (disposable)
Raw captures: `Docs/Eval/milestone-6/probes/integration/`

**Scope.** Probe only. No `src/` edits, no commits, no `cargo`. The repo was read-only except for this
document and the raw captures under `Docs/Eval/milestone-6/probes/integration/`.
`/Users/sergio/.opencode` was never touched (the trial env is built with `env -i` and its own
`HOME`/`XDG_*`; the host's own `opencode serve` process was excluded from network accounting by PID).

---

## 0. Binary identity (re-verified, not re-acquired)

Per `Docs/Eval/milestone-0/HOST-PROBE.md` §1 the pinned 1.18.27 CLI was located in the `m0` scratch
tree and its sha256 re-verified. **No re-acquisition was needed**; all four available copies of the
darwin/arm64 binary agree, and equal the frozen expectation.

```
sha256 of pinned 1.18.27 CLI:
f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6  .../integration/bin/opencode
.../m0/install/pkg/node_modules/opencode-darwin-arm64/bin/opencode   -> same sha256
.../m0/install/pkg/node_modules/opencode-ai/bin/opencode.exe         -> same sha256
.../m0/gh-extract/opencode                                           -> same sha256
binary size: 144074210 bytes
version: 1.18.27
```

Raw: `probes/integration/00-binary-identity.txt`.

---

## TASK A — COST

### A.1 Cost table (all measurements from this probe)

| # | Scenario | Wall time | Bytes fetched from registry | Network bytes (this trial's PID only) | Trial disk (apparent) | Notes |
|---|---|---|---|---|---|---|
| A1 | **Cold**: empty npm cache, no config dir, no models.json | **3 961 ms** (run) | **41 766 629 B** npm cache | *not sampled* | 11 579 766 B | Plugin install is **detached**; it does **not** finish before the CLI exits (see A.6) |
| A1b | Cold, observed 120 s after exit | 6 001 ms (run) | **98 525 434 B** npm cache | — | 66 495 578 B | Install landed *during* the run when the cache dir was the shared one |
| A1c | Cold + nettop, 120 s observation | 5 273 ms (run) | 41 330 060 B npm cache | 3 809 906 in / 13 641 576 out (host-wide sample) | 11 579 765 B | `models.json` = 5 825 141 B; install still not landed after 120 s |
| A1d | Cold, install-lifecycle probe | 5 s (run) | 62 712 995 B npm cache | — | — | `config/opencode/node_modules` = **0 B** — install never landed |
| A2 | **npm cache env var** — lowercase `npm_config_cache` | 4 617 ms | 41 750 245 B into the dedicated dir | — | — | `home/.npm` = **0 B** → honoured |
| A2 | **npm cache env var** — uppercase `NPM_CONFIG_CACHE` | 9 914 ms | 98 525 434 B into the dedicated dir | — | — | `home/.npm` = **0 B** → honoured |
| A2 | **npm cache env var** — neither set | 6 906 ms | 0 B dedicated, **93 095 685 B** in `home/.npm` | — | — | default is `$HOME/.npm` |
| A3 | **Pre-seeded config dir** (global `node_modules` present) | 3 614 ms | **0 B** | — | 66 397 103 B | Install skipped entirely; `node_modules` untouched (54 901 253 B) |
| A4 | **Shared pre-warmed npm cache**, trial 1 | 6 259 ms | **0 B** | — | 66 495 970 B | No breakage |
| A4 | **Shared pre-warmed npm cache**, trial 2 | 4 398 ms | **0 B** | — | 66 458 889 B | No breakage; cache reuse is safe |
| A5 | **models.json shared read-only** (hardlink, mode 444), trial 1 | 4 507 ms | 0 B | — | — | File mode still `-r--r--r--` after the run; inode shared |
| A5 | **models.json shared read-only**, trial 2 | 4 775 ms | 0 B | — | — | Same; **no rewrite, no failure** |
| A6-1 | Cold shared cache, **no** skill (global target only) | 4 749 ms | 39 462 284 B | — | 15 629 198 B | Only the global install target |
| A6-2 | Cold shared cache, **with** skill (global **+** project targets) | 4 049 ms | 245 760 B (cache already warm) | — | 15 533 677 B | A project skill creates a **second** install target |
| A6-3 | Warm cache, global seeded, project **not** seeded | 6 509 ms | 48 272 959 B | — | 70 548 871 B | Project target still pulls from the registry |
| A6-4 | Warm cache, global **and** project seeded | 5 109 ms | **12 685 B** | — | 125 414 652 B | Both targets pre-seeded ⇒ registry traffic ≈ 0 |
| A7 | **Recommended setup**, *cold* shared cache | 76 408 ms | **41 762 930 B** | — | 125 457 079 B | Pre-seeding alone is **not** enough — the cache must be warm too |
| A8 | **Recommended setup**, warm shared cache, trial 1 | 119 336 ms | **0 B** | — | 127 261 192 B | — |
| A8 | **Recommended setup**, warm shared cache, trial 2 | 22 836 ms | 9 144 B | — | 127 750 512 B | 9 144 B = npm `_logs` churn, not a package fetch |
| **A9** | **Recommended setup**, 3 consecutive trials | **18 492 / 15 030 / 16 996 ms** | **9 144 / −9 144 / 0 B** (i.e. **0 net**) | *host-wide sample — includes the host's own long-lived `opencode serve`; **not** per-trial. Use A10 for the per-trial figure.* | 127 626 914 B | 3/3 trials ran the exact 4-step protocol |
| **A10** | **Recommended setup**, **PID-accurate** nettop, 2 trials | 19 221 / 28 739 ms | **0 B** | **56 938 in / 157 629 out** and **35 060 in / 160 387 out** — *this trial's PID only* | 127 444 453 B | `du -sk` of a trial = 143 052 KB (APFS clone) |
| A11 | Disk: 60 trials, APFS clone (`cp -Rc`) | — | — | — | 8 108 880 KB batch | `du` apparent |
| A13 | **Disk ground truth** (`df` free-space delta), 60 trials | — | — | — | clone **19 374 KB/trial**; hardlink **11 629 KB/trial**; full copy ≈ 125 496 KB/trial | `df`-measured |
| A14 | **Contamination**: template mutated by a trial run? | — | — | — | clone → **NO**; hardlink → **YES** (`.package-lock.json` 12 161 → 6 889 B) | see A.7 |

Raw: `probes/integration/cost/`.

### A.2 Which npm cache env var is honoured?

**Both `npm_config_cache` (lowercase) and `NPM_CONFIG_CACHE` (uppercase) are honoured** by the npm
client that OpenCode 1.18.27 embeds (it bundles `@npmcli/arborist` + `pacote` in-process — no host
`npm` binary is needed or used).

Decisive evidence, each variant with **exactly one** cache variable set and its own dedicated cache
directory (A2, `probes/integration/cost/a2-cache-env-decisive.txt`):

```
--- variant=lowercase
    dedicated_cache_bytes=41750245   home_dot_npm_bytes=0
--- variant=uppercase
    dedicated_cache_bytes=98525434   home_dot_npm_bytes=0
--- variant=none
    dedicated_cache_bytes=0          home_dot_npm_bytes=93095685
```

With neither set, the cache lands in `$HOME/.npm` (93 MB) — i.e. the default. With either variable
set, `$HOME/.npm` stays at 0 B and the dedicated directory takes the traffic.

Cross-check with the *host* `npm` (not used by the trial, but the same client family) gives the same
answer, and shows the two spellings are aliases of one npm config key:

```
host-npm variant=lowercase -> npm config get cache = .../out/a2d-lowercase/cache
host-npm variant=uppercase -> npm config get cache = .../out/a2d-uppercase/cache
```

**Recommendation: use lowercase `npm_config_cache`.** It is npm's canonical spelling, it is what the
M0 log recorded, and it avoids the aliasing ambiguity (setting *both* to different values makes npm
emit a malformed concatenated cache path).

### A.3 Does a pre-seeded config dir skip the `@opencode-ai/plugin` npm install?

**Yes.** With `$XDG_CONFIG_HOME/opencode/node_modules` already populated, the registry fetch is skipped
completely:

```
############ A3 pre-seeded config dir (node_modules already present)
wall_ms=3614
npm_cache_before=0
npm_cache_after=0
npm_cache_delta=0
config_node_modules_bytes=54901253
```

But there is a second, easily-missed install target (A.6):

> OpenCode installs the plugin into **every `.opencode` config directory it discovers**. Seeding the
> *global* dir is not enough. A project skill at `<cwd>/.opencode/skill/root/SKILL.md` **creates
> `<cwd>/.opencode`**, which becomes a second install target. A6-3 (global seeded, project not seeded)
> still pulled **48 272 959 B**; A6-4 (both seeded) pulled **12 685 B**.

### A.4 Shared pre-warmed npm cache reused across two sequential runs

**No breakage.** A4 ran two trials back-to-back against one shared cache; both succeeded, both pulled
**0 B**, and the shared cache was byte-identical before and after each run. A8 repeated this with the
full recommended setup: trial 1 pulled 0 B, trial 2 pulled 9 144 B (npm `_logs` churn only, not a
package fetch). A9 ran three more: net delta 0 B across the batch.

The shared cache is **read-write** from opencode's point of view (npm writes index entries into it),
so it must be writable — but it is safe to share sequentially. It is *not* safe to share
**concurrently** without a lock; the probe did not test that, and this report does not claim it.

### A.5 `models.json` catalog — size and read-only sharing

- Size: **5 825 527 B** (5.6 MB) — the provider/model catalog snapshot written to
  `$XDG_CACHE_HOME/opencode/models.json`.
- **Yes, it can be shared read-only.** A5 hardlinked one shared copy (mode `444`) into two trials.
  After both runs the file was still `-r--r--r--`, still 5 825 527 B, still the same inode, and both
  trials answered correctly (`text = "ok"`). OpenCode did not attempt to rewrite it.

### A.6 The cold-run trap: the plugin install is detached and races the CLI exit

The single largest cost in a naive setup is a fetch that **may never complete**. A1d watched a cold
run to completion:

```
cli exited
immediately after exit: cache=62712995 config_nm=0
...
final: cache=62712995 config_nm=0
config tree:
$T/config
$T/config/opencode
$T/config/opencode/.gitignore
$T/config/opencode/opencode.jsonc
```

The install is a *background* task (opencode's own log: `background dependency install failed` is
logged at `forkDetach` scope). The CLI process exits when the model turn finishes; the npm install is
killed with it. Depending on timing you land anywhere between "no `node_modules` at all" (A1d) and
"fully installed" (A1b). **A cold trial is therefore non-deterministic and may leave the trial env
without the plugin.** The pre-seeded configuration removes this race entirely.

### A.7 Disk strategy: APFS clone, not hardlink

Measured with `df` free-space delta over a 60-trial batch (A13, ground truth — not `du`, which
over-reports clones):

| Seeding strategy | Real disk / trial | Template mutated by a trial run? |
|---|---|---|
| `cp -R` (full copy) | ≈ 125 496 KB | n/a (no shared state) |
| `cp -Rc` (APFS copy-on-write clone) | **19 374 KB** | **NO** — template bytes and file count identical before/after |
| `cp -al` (hardlink) | **11 629 KB** | **YES** — `.package-lock.json` in the shared template was rewritten in place, 12 161 B → 6 889 B |

The hardlink strategy is cheaper but **unsafe**: npm rewrites `node_modules/.package-lock.json`
through the shared inode, so every trial mutates the shared template and trials contaminate each
other. **Use `cp -Rc` (APFS clone).** Note `cp -c` needs `-R` for directories — `cp -c <dir>` alone
fails with "is a directory (not copied)".

### A.8 RECOMMENDED per-trial setup (for a 20–60 trial run)

**One-time prewarm (do once, reuse for the whole batch):**

| Asset | Size | How to obtain |
|---|---|---|
| Shared warm npm cache | 86 092 KB (84 MB) | one run with `npm_config_cache=$SHARED/cache`; keep writable |
| Config template (global `node_modules`) | 62 776 KB (61 MB) | from that prewarm run's `$XDG_CONFIG_HOME/opencode` |
| `models.json` | 5 692 KB (5.6 MB) | from that run's `$XDG_CACHE_HOME/opencode/models.json`; `chmod 444` |
| `rg` (ripgrep 15.1.0 darwin-arm64) | 4 352 KB (4.2 MB) | `XDG_CACHE_HOME/opencode/bin/rg` — see §B.4 |
| **Total one-time** | **158 912 KB (155 MB)** | |

**Per trial (exact env + prewarm steps):**

```sh
T=$SCRATCH/trials/<name>
mkdir -p "$T"/{home,config,data,cache,state,tmp,cwd,bin,out}

# --- fake shims: PATH contains ONLY these two ---
cat > "$T/bin/root" <<'SH'
#!/bin/sh
printf '%s\n' "root $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' '{"success":true}'
exit 0
SH
cat > "$T/bin/brew" <<'SH'
#!/bin/sh
printf '%s\n' "brew $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' 'brew: fake shim: refusing to install (trial sandbox)' >&2
exit 1
SH
chmod +x "$T/bin/root" "$T/bin/brew"
: > "$T/audit.log"

# --- pre-seed the plugin install targets (APFS clone, NOT hardlink) ---
mkdir -p "$T/config/opencode" "$T/cwd/.opencode" "$T/cache/opencode/bin"
cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/config/opencode/node_modules"
cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"

# --- pre-seed models.json (read-only) and rg (needed by the `skill` tool) ---
cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"; chmod 444 "$T/cache/opencode/models.json"
cp "$SHARED/rg/rg" "$T/cache/opencode/bin/rg"; chmod 755 "$T/cache/opencode/bin/rg"

# --- seed the project skill (treatment) ---
mkdir -p "$T/cwd/.opencode/skill/root"
cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill/root/SKILL.md"

# --- run ---
cd "$T/cwd" && /usr/bin/env -i \
  HOME=$T/home \
  XDG_CONFIG_HOME=$T/config \
  XDG_DATA_HOME=$T/data \
  XDG_CACHE_HOME=$T/cache \
  XDG_STATE_HOME=$T/state \
  TMPDIR=$T/tmp \
  PATH=$T/bin \
  TRIAL_AUDIT_LOG=$T/audit.log \
  OPENCODE_DISABLE_AUTOUPDATE=1 \
  npm_config_cache=$SHARED/npmcache \
  "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root" \
  > "$OUT/<label>.ndjson" 2> "$OUT/<label>.stderr.txt"
```

**Per-trial cost with this setup (measured, A9 / A10):**

| Quantity | Value |
|---|---|
| Registry bytes fetched | **0 B** (net; 0–9 144 B of npm `_logs` churn) |
| Network bytes (trial's own PID, A10) | **35–57 KB in, 158–160 KB out** (≈ 215 KB total) |
| Wall time | **15–19 s** typical (A9/A10); 50–57 s in the longer C-run trials (more verification calls) |
| Trial disk | 19 374 KB real (APFS clone); 127 MB apparent |
| One-time shared assets | 158 912 KB (155 MB) |
| **60-trial batch disk** | 155 MB shared + 60 × 19 374 KB ≈ **1.3 GB** |
| Tokens per trial | ≈ 9 100 in / 190–204 out / 90–104 reasoning |
| **Cost** | **$0** (`part.cost == 0` on every `step_finish` in every run) |

**Throughput:** ≈ 15–29 s wall per trial for a short (4-command) trial, ≈ 50–57 s for a trial where
the model did extra verification; 60 trials ≈ 15–30 min of wall clock, sequential (plus allowance for
the provider stall below).

> **Flakiness note (measured, not hypothetical).** One baseline run hit
> `ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms` and took
> **314 923 ms** instead of ~25 s. This is a provider-side stall, not a harness bug. A 20–60 trial
> batch must budget for it: either a per-trial wall-clock timeout with retry, or a large enough
> timeout that a single stall does not fail the batch.

---

## TASK B — INTEGRATION

### B.1 Trial environment

`B1_treat` (treatment) and `B2_base` / `B3_base_retry` (baseline) used the §A.8 setup. Prompt for
every run: `install ripgrep using Root`.

PATH isolation, measured from inside the trial env
(`probes/integration/integration/B1_treat.pathsanity.txt`):

```
root -> .../trials/B1_treat/bin/root
brew -> .../trials/B1_treat/bin/brew
npm -> NOTFOUND
node -> NOTFOUND
git -> NOTFOUND
curl -> NOTFOUND
nix -> NOTFOUND
cargo -> NOTFOUND
rg -> NOTFOUND
python3 -> NOTFOUND
sed -> NOTFOUND
awk -> NOTFOUND
```

**No host PATH leakage.** The only two resolvable executables are the fake shims. The seeded skill
was byte-identical to the repo skill:
`seeded_skill_sha256=51297a76e3afb5949596f9c3a605c4f9dc80cb3a48bcc3a78735f0615e581a46` ==
`repo_skill_sha256=51297a76e3afb5949596f9c3a605c4f9dc80cb3a48bcc3a78735f0615e581a46`.

### B.2 NDJSON event types emitted vs `crates/root-eval/src/events.rs`

Every line of every real capture was checked against a Python mirror of
`parse_ndjson_line` that was first validated against the M0 golden fixture `03-skill/stdout.ndjson`
(6/6 accepted, 0 rejected).

**17 captures, 390 NDJSON lines, 0 rejected.** The Rust parser accepts every line the real harness
produces. Full report: `probes/integration/integration/ndjson-vs-parser.txt`.

| Envelope type | `events.rs` expects | Emitted? | Result |
|---|---|---|---|
| `step_start` | `part` must be an object | ✅ every run | accepted |
| `text` | `part.text` must be a string | ⚠️ **not always** — absent in 3 of the 9 Task-B/C captures (`B2_base`, `B3_base_retry`, `C_run2`) | accepted when present; **absent** in those three |
| `step_finish` | `part.tokens.{input,output,reasoning,cache.read,cache.write}` ints, `part.cost` number | ✅ every run | accepted |
| `tool_use` | `part.tool` str, `part.callID` str, `part.state` object | ✅ every run | accepted |
| `error` | `error.name` str, `error.data` any | ❌ **never emitted** in any capture | n/a |

**Field-by-field incompatibilities (all are silent drops, none are parse errors):**

| Field | Present in real stream | `events.rs` handling | Impact |
|---|---|---|---|
| `part.state.metadata` | ✅ on every `tool_use` | **not read at all** — `ToolCall` has no metadata field | **The `skill` tool's `metadata.dir` is invisible to the parser.** The M0 golden test `golden_03_skill_replays_with_byte_equal_body` asserts on `tool.output_str`, not on `metadata.dir`, so the golden still passes — but a harness that wants to assert "the skill loaded *from this directory*" cannot read it out of `NormalizedEvent` today. `ToolCall` would need a `metadata` field. |
| `part.state.metadata.name` | ✅ (`"root"`) | not read | Same as above; the skill name is only recoverable from `state.input.name` (which *is* read, as part of `input_json`). |
| `part.state.time` | ✅ on every `tool_use` | not read | No impact. |
| `part.state.title` | ✅ on most `tool_use` | not read | No impact. |
| `part.time` | ✅ on `text` events | not read | No impact. |
| `part.reason` | ✅ on every `step_finish` (`"tool-calls"` / `"stop"`) | not read | **Meaningful gap:** `reason` distinguishes "the model asked for another tool call" from "the model finished". Without it a trial cannot tell a completed turn from a truncated one except by looking for a trailing `text` event — and `text` is not always emitted (§B.2 above). |
| `part.tokens.total` | ✅ | not read | No impact (the five fields the parser wants are all present). |
| `text` event absence | — | parser only handles `text` *if* it appears | **The parser has no "turn ended" signal.** A stream that ends with `step_finish{reason:"tool-calls"}` and no `text` is indistinguishable from a truncated stream. `B2_base`, `B3_base_retry` and `C_run2` are real examples of runs that ended with **no final assistant text at all**. |
| `state.status` = `"error"` with `state.error` = `"The user rejected permission to use this specific tool call."` | ✅ observed in `B2_base`, `B3_base_retry`, `C_run1`, `C_run2` | `ASK_REJECT_MESSAGE` → `is_ask_denial()` | ✅ **matches exactly.** No incompatibility. |
| `state.status` = `"error"` with `state.error` = `"ripgrep execution failed"` | ✅ observed (see §B.4) | not the ask-denial message ⇒ `is_ask_denial()` = false | ✅ correct behaviour; a genuine tool failure is not mistaken for a denial. |
| envelope `type` values outside the five frozen shapes | ❌ never observed | typed error `UnknownEventType` | No live incompatibility. |

**Summary of parser incompatibilities: zero hard incompatibilities (0 rejected lines).** The
substantive gaps are *missing reads*, not wrong reads: `part.state.metadata` (needed for
`metadata.dir`), `part.reason`, and the absence of any "the turn ended" signal when a run ends
without a `text` event.

### B.3 Were the fake shims actually invoked? (does the trial PATH reach tool executions?)

**Yes.** The audit log written by the shims proves the trial PATH reaches tool executions — the
harness's `bash` tool resolves `root` to the fake shim.

`B1_treat` audit log (verbatim):

```
root doctor --json
root plan install ripgrep --json
root install ripgrep --json
root verify ripgrep --json
```

The corresponding `tool_use` events carry the shim's fixed output:

```json
{"type":"tool_use","timestamp":1792445979240,"sessionID":"ses_ea9e524e7ffeXj9X3tXTMbIKnK",
 "part":{"type":"tool","tool":"bash","callID":"call_4c9c727fedc04c608349b16d",
  "state":{"status":"completed",
   "input":{"command":"root doctor --json","timeout":60000},
   "output":"{\"success\":true}\n",
   "metadata":{"output":"{\"success\":true}\n","exit":0,"truncated":false},
   "title":"root doctor --json",
   "time":{"start":1792445978876,"end":1792445979238}}}}}
```

`B2_base` (baseline) audit log: `brew --version` — the fake `brew` shim was reached too.

### B.4 Did the skill load?

**Yes — after `rg` was pre-seeded.** The `skill` tool event in `B1_treat`:

```json
{"type":"tool_use","timestamp":1792445968335,"sessionID":"ses_ea9e524e7ffeXj9X3tXTMbIKnK",
 "part":{"type":"tool","tool":"skill","callID":"call_b53df694c7234b9ab7435b18",
  "state":{"status":"completed",
   "input":{"name":"root"},
   "metadata":{"name":"root",
     "dir":"/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/trials/B1_treat/cwd/.opencode/skill/root",
     "truncated":false}}}}
```

`part.tool == "skill"` ✅ and `part.state.metadata.dir` points at the seeded project skill ✅.

**But this required a discovery that is easy to miss.** The `skill` tool is implemented on top of a
ripgrep service. OpenCode's `RipgrepBinary` resolves ripgrep in this order: `which rg` → else
`$XDG_CACHE_HOME/opencode/bin/rg` → else **download and extract** ripgrep 15.1.0 from GitHub. In a
shims-only PATH, `which rg` fails and the download path needs `tar` — which is also absent. Result:

```
  1 tool_use tool=skill  status=error
      error='ripgrep execution failed'
```

with opencode's own log line:

```
message="downloading ripgrep" url=https://github.com/BurntSushi/ripgrep/releases/download/15.1.0/ripgrep-15.1.0-aarch64-apple-darwin.tar.gz
```

The 1 777 930 B tarball landed in the cache dir but was never extracted (no `tar`), so the skill tool
failed. A/B measured three ways (`probes/integration/cost/b-rg-skill.txt`):

| Variant | `skill` tool | `metadata.dir` | ripgrep downloaded? |
|---|---|---|---|
| `rg` **not** seeded | `status=error`, `'ripgrep execution failed'` | absent | **YES** (1 777 930 B tarball, never extracted) |
| `rg` seeded at `$XDG_CACHE_HOME/opencode/bin/rg` | `status=completed` | ✅ present | **NO** |
| `rg` on PATH as a symlink in the trial `bin` | `status=completed` | ✅ present | **NO** |

**Recommendation: pre-seed `$XDG_CACHE_HOME/opencode/bin/rg`** (4 048 816 B, chmod 755). It makes the
skill tool hermetic, avoids a 1.78 MB GitHub fetch per trial, and removes a `tar` dependency that a
minimal PATH cannot satisfy. Raw evidence: `probes/integration/cost/rg-download-evidence.txt`.

Interesting side-observation: even when the `skill` tool *failed*, the model recovered by calling
`read` on the SKILL.md path directly and then ran the correct protocol. So a missing `rg` degrades
gracefully rather than failing the trial — but it does add a tool call and changes the event stream.

### B.5 Model behaviour

**Treatment (`B1_treat`, with skill):**

- Did it run `root doctor --json` first? **Yes.** The very first bash call after the skill load was
  `root doctor --json`. The full call order was exactly the skill's protocol:
  `doctor → plan → install → verify`.
- Any prohibited installer attempt? **No.** The fake `brew` shim was **never invoked** in any
  treatment run (verified by extracting every `bash` `state.input.command` and filtering for `brew`:
  zero hits across `B1_treat`, `b_treat`, `brg-not-seeded`, `brg-bin-seeded`, `brg-path-shim`,
  `C_run1`, `C_run2`). No `curl … | sh`, no `npm install -g`, no `pip install --user`.
- Final message, verbatim (from `B1_treat.ndjson`, `text` event):

  > `Installed ripgrep via Root — deterministic and snapshot-protected. All binaries verified executable. Pre-install snapshot is available via `root rollback --last --json` if needed.`

- Intermediate text, verbatim: `Environment healthy and ripgrep found in nixpkgs. Installing now.`
- Wall time: **27 050 ms**. Tokens/cost in `step_finish`: input 7 842–1 204, output 18–69,
  reasoning 7–238, **cost 0** on all 5 `step_finish` events.

**The baseline (no skill) — the model never reached `root` at all.** The only shim it reached was the
fake `brew` (audit log: `brew --version`), and only via a probing command
`type brew rg sudo root Root 2>&1; echo ---; brew --version 2>&1`. It then explored the filesystem
(`command -v rg; command -v brew; command -v sudo; uname -m`, `ls -la; echo "PATH=$PATH"; ls bin`,
`glob *`, `glob bin/*`) and the run **ended with no final assistant message** — the last NDJSON line is
a `step_finish` with `reason: "tool-calls"`, i.e. the model was still mid-exploration when the turn
ended. A `read` on the trial's `bin/` directory was auto-rejected (`external_directory` permission ask
→ `The user rejected permission to use this specific tool call.`), and opencode logged
`permission requested: external_directory (...); auto-rejecting` on stderr. Wall time: 25 101 ms.

**Baseline retry (`B3_base_retry`):** the provider stalled for 299 s
(`ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms`, logged twice — once
for the `title` agent, once for the `build` agent), giving a 314 923 ms wall time. The run then
produced 10 NDJSON lines, **no final assistant text**, and an **empty audit log** — the fake shims
were never reached. (Its bash commands were `which rg; which brew; which sudo` and
`ls -la "$(dirname "$(which brew)")"; ...` — again only *probes* for `brew`, never an install.)

### B.6 With-skill vs without-skill — behavioural difference (observation, not conclusion)

> **Observation only.** The following describes what was seen in the runs captured here. It is not a
> claim about the model, the skill, or Root.

| Dimension | With skill (`B1_treat`, `C_run1`, `C_run2`, `b_treat`, `brg-*`) | Without skill (`B2_base`, `B3_base_retry`) |
|---|---|---|
| Skill tool | invoked, `status=completed`, `metadata.dir` = project skill dir | never invoked |
| First shim call | `root doctor --json` | `brew --version` (brew shim) or none at all |
| Protocol order | **doctor → plan → install → verify** in every treatment run | `root` never reached the protocol |
| `root` shim calls | 4 (plus extra verification/exploration calls in the longer runs) | 0 |
| `brew` shim calls | **0** | 1 (`brew --version`, reached via a `type … ; brew --version` probe) |
| Final assistant text | present in most treatment runs | **absent in both baseline runs** |
| Tool-call count | 5–15 | 4–10 (exploration only) |
| Wall time | 15–57 s (plus one provider stall) | 25 s / 315 s (stall) |
| Prohibited installer | none | `brew` invoked (a `brew --version` probe, not an install) |

In plain language: **with the skill, the model loaded the Root skill, ran Root's four-step protocol
in order through the fake `root` shim, never touched `brew`, and produced a closing summary. Without
the skill, the model did not find the protocol at all** — it went looking for whatever package
manager was available, landed on `brew`, wandered through the filesystem, and in both captured runs
ended without a final answer.

Caveat stated plainly: the two baseline runs are n=2, one of which was provider-stalled. The
difference is consistent across the four independent treatment runs and both baseline runs, but the
sample is small.

### B.7 Provider flakiness

**One real failure in 9 runs.** `B3_base_retry` (baseline, no skill):

```
timestamp=2026-10-19T21:57:54.856Z level=ERROR run=ad0687e6 message="stream error" providerID=opencode modelID=big-pickle session.id=ses_ea9d8c3ffffezYU2sKDTAvxjD4 small=true agent=title mode=primary error.error="ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms"
timestamp=2026-10-19T21:57:55.821Z level=ERROR run=ad0687e6 message="stream error" providerID=opencode modelID=big-pickle session.id=ses_ea9d8c3ffffezYU2sKDTAvxjD4 small=false agent=build mode=primary error.error="ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms"
```

- **Run 1 (`B3_base_retry`)**: stalled 299 s, wall 314 923 ms, 10 NDJSON lines, empty audit log.
- **Run 2 (the retry, reported above as `B2_base`/`B3_base_retry` pair)**: the earlier baseline run
  `B2_base` completed normally in 25 101 ms with 14 NDJSON lines.

The other 7 runs (`B1_treat`, `b_treat`, `brg-not-seeded`, `brg-bin-seeded`, `brg-path-shim`,
`C_run1`, `C_run2`, `a9trial1-3`, `a10trial1-2`) were all clean.

**Neither run produced an `error` envelope in the NDJSON.** The provider failure appears only in
`$XDG_DATA_HOME/opencode/log/opencode.log` and as a **truncated stream** (the NDJSON simply stops
mid-turn). A harness that only watches the NDJSON stream would see a truncated run, not an error
event — consistent with the M0 "no terminal event" contract in `events.rs`.

---

## TASK C — VARIANCE

Two treatment trials in two freshly-materialised envs, using the §A.8 recommended setup. Prompt
identical; only the trial directory differs.

- `C_run1`: 39 NDJSON lines, wall 57 407 ms, registry bytes pulled **0**
- `C_run2`: 30 NDJSON lines, wall 50 783 ms, registry bytes pulled **0**

Raw: `probes/integration/variance/` (including `c-variance.txt`, the full field-by-field diff).

### Fields that VARY

| Field | C_run1 | C_run2 |
|---|---|---|
| `sessionID` | `ses_ea9b3…` (distinct) | `ses_ea9b37bb1ffeATtS03fkv6MiP6` (distinct) |
| `timestamp` (envelope, ms) | 1 792 449 152 849 … 1 792 449 206 350 | 1 792 449 220 374 … 1 792 449 267 350 |
| envelope type sequence | 39 lines, 15 `tool_use` | 30 lines, 10 `tool_use` (different interleaving) |
| `tool` names + `status` sequence | `skill×1, bash×12, read×2(error)` | `skill×1, bash×8, read×1(error)` |
| commands issued | 8 distinct bash commands | 8 distinct bash commands (different set) |
| `text` events | `["Verification claimed success but \`rg\` isn't on PATH. Checking further.", "Root has replaced the system PATH entirely. Investigating the damage."]` | `[]` — **no assistant text at all** |
| `step_finish` token counts | 9 distinct token tuples | 10 distinct token tuples |
| `callID` values | 15 distinct ids | 10 distinct ids |
| audit log (exact and path-normalised) | 8 lines incl. `root install ripgrep --json --verbose`, `root --version` | 7 lines incl. `root --help`, `root plan install ripgrep` (no `--json`) |

### Fields that are STABLE

| Field | Value in both runs |
|---|---|
| skill-load event present | **yes** |
| `skill` `state.status` | `completed` |
| `skill` `metadata.dir` (path-normalised) | `…/trials/<TRIAL>/cwd/.opencode/skill/root` — identical modulo the trial dir |
| `skill` `state.input.name` | `root` |
| Root protocol order | **`doctor → plan → install → verify`** — identical |
| protocol == exactly doctor/plan/install/verify | **yes** (both) |
| `part.cost` | `0` on every `step_finish` in both runs |
| first tool call | `skill` (the model loads the skill before doing anything else) |
| first bash command | `root doctor --json` |
| `brew` shim | **never invoked** in either run |
| registry bytes pulled | 0 in both runs |

### What this means for a trial harness (observation)

The **behavioural spine is stable**: the skill loads, the four Root protocol commands are issued in
the same order, the shims are reached, and cost is $0. The **peripheral detail is not**: how many
extra verification calls the model makes, whether it emits assistant text, the token counts, the
call ids, the session id, and the timestamps all move between runs. A trial harness that asserts on
the *exact* command list or on a *final text event* will be flaky. A harness that asserts on
**protocol order, skill load, and the absence of prohibited installers** will be stable.

Notably, `C_run2` ended with **no assistant text at all** despite successfully running the whole
protocol — see §B.2, `text` event absence.

---

## Reproducing

Scripts are under `probes/integration/scripts/` (verbatim copies of what was run):

| Script | Purpose |
|---|---|
| `lib.sh` | trial construction, env, seeding helpers |
| `a_cost2.sh`, `a2_cache_env_decisive.sh` | Task A: cold/warm/cache-env-var |
| `a4_a5_shared.sh` | Task A: shared cache reuse, models.json read-only sharing |
| `a6_install_targets.sh` | Task A: global vs project plugin install targets |
| `a9_final_cost.sh`, `a10_final_pid_cost.sh` | Task A: final per-trial cost, PID-accurate nettop |
| `a11_disk_scaling.sh`, `a13_disk_groundtruth.sh`, `a14_contamination.sh` | Task A: disk strategy + contamination |
| `b_rg_skill.sh` | Task B: the `rg` dependency of the `skill` tool |
| `b_final.sh` | Task B: treatment + baseline trials |
| `c_final.sh` | Task C: two treatment runs + variance diff |
| `ndjson_vs_parser.py` | NDJSON vs `crates/root-eval/src/events.rs` compatibility checker |
| `c_variance.py` | field-by-field variance comparator |
| `nettop_sampler.py`, `nettop_pid.py` | network byte accounting |

---

## Findings at a glance

1. **Cheapest hermetic setup**: pre-seed the *global* **and** *project* `.opencode` plugin
   `node_modules` (APFS clone, not hardlink), pre-seed `models.json` read-only, pre-seed `rg`, and
   point `npm_config_cache` at one shared warm cache. Result: **0 registry bytes per trial**,
   ≈ 215 KB network, **15–19 s wall**, **$0**, 19.4 MB real disk per trial, ≈ 155 MB one-time.
2. **Both** `npm_config_cache` and `NPM_CONFIG_CACHE` are honoured; use lowercase.
3. **A pre-seeded config dir does skip the plugin install — but only if you seed the *project*
   `.opencode` dir too.** A project skill creates that dir, hence a second install target.
4. **The `skill` tool needs `rg`.** Without it the tool errors with `'ripgrep execution failed'`
   and opencode tries to download ripgrep (1.78 MB) — which then fails to extract without `tar`.
   Pre-seeding `$XDG_CACHE_HOME/opencode/bin/rg` fixes it.
5. **The real harness does drive the fake tools** and **does load the skill** — proven by the
   audit log and the `skill` `tool_use` with `metadata.dir`.
6. **Zero parser incompatibilities** (0 rejected lines over 390 real NDJSON lines across 17 captures).
   Three missing *reads* in `events.rs`: `part.state.metadata` (needed for `metadata.dir`),
   `part.reason`, and there is no "turn ended" signal when a run ends without a `text` event.
7. **Hardlink seeding is unsafe** — npm rewrites `.package-lock.json` through the shared inode and
   mutates the shared template. Use APFS clones.
8. **Provider flakiness is real**: one `ProviderHeaderTimeoutError` (300 s) in 9 runs, which
   truncated the NDJSON stream with no `error` envelope.
