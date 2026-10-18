# Fallback Harness Command Surfaces — Milestone 0 Contingency Probe

**Purpose.** If the pinned OpenCode harness (1.18.27, handled by a separate agent) fails to freeze, Root's
skill-evaluation pipeline needs to know — immediately and at zero cost — how far behind the *fallback*
harnesses already installed on this host are. This document records their **version** and **help-surface
command contract**.

**Date:** 2026-10-18 (Milestone 0 contingency probe)
**Probe kind:** READ-ONLY — `--version`, `--help`, and `-p --help` of installed binaries. No prompt/task was
ever executed (`codex exec "…"` / `claude -p "…"` are forbidden because auth on this host may be live and any
run would incur API cost). No installs, updates, config changes, or git commits. Writes confined to
`Docs/Eval/milestone-0/fallback/`.

**Evidence discipline.** Every conclusion below is extracted **from help text only**. Nothing behavioral has
been validated by running a session. Claims that would require a live run are explicitly labeled
**NOT-VALIDATED** (see the register in §6). Raw, exact output (one `$ <command>` header line + raw bytes) is
preserved in `raw/`.

---

## 1. Versions observed (host state vs Root pins)

| Harness | Binary path | Host version | Root pin | Pin source | Match |
|---|---|---|---|---|---|
| codex | `~/.local/bin/codex` | `codex-cli 0.163.0` | `0.150.1` | `crates/root-agent-bundle/src/manifest.rs:19` | **MISMATCH** (expected — flagged as fallback risk in the milestone brief) |
| claude | `~/.local/bin/claude` | `2.1.260 (Claude Code)` | `2.1.260` | `crates/root-agent-bundle/src/manifest.rs:25` | **EXACT MATCH** |

Consequence: the **Claude surface can be probed AT PIN for free** — the help contract captured here is the
same binary version the manifest supports. For **codex**, the captured surface belongs to 0.163.0 and is
*yet another* drifted artifact: to probe or pin codex at 0.150.1 the exact-version binary would need to be
acquired, exactly like the OpenCode pin.

Raw evidence: `raw/versions.txt`.

## 2. Capture inventory

| File | Command | Why |
|---|---|---|
| `raw/versions.txt` | `which codex` / `codex --version` / `which claude` / `claude --version` | version state |
| `raw/codex-help.txt` | `codex --help` | top-level surface (138 lines) |
| `raw/codex-exec-help.txt` | `codex exec --help` | non-interactive surface (119 lines) |
| `raw/codex-resume-help.txt` | `codex resume --help` | resume subcommand *listed* by top-level help (116 lines) |
| `raw/codex-exec-resume-help.txt` | `codex exec resume --help` | non-interactive resume, listed by `exec --help` (87 lines) |
| `raw/claude-help.txt` | `claude --help` | full surface incl. Commands section (305 lines) |
| `raw/claude-print-help.txt` | `claude -p --help` | print-mode help (260 lines) |

Deliberately **not** run: `codex agents|apply|review|sandbox|mcp|… --help` and any claude subcommand help
(`mcp`, `plugin`, …) — outside the enumerated probe. **`claude config --help` was NOT run because `config`
is not a listed subcommand of claude 2.1.260** (command list in `claude-help.txt:262-305`; the probe
protocol said "if listed").

All commands returned immediately — no timeout kill was ever needed. (`gtimeout`/`timeout` are not
preinstalled on this host; the agent shell's own 30 s kill served as the safety net, and nothing hung.
`claude -p --help` did **not** start a session: help interception precedes execution — observed from the
instant identical output, a process observation, not an API claim.)

## 3. Codex 0.163.0 — surface extracted from help text

### 3.1 Non-interactive invocation

`exec` exists: `codex exec [OPTIONS] [PROMPT]`, alias `e`, described "Run Codex non-interactively"
(`codex-help.txt:11`, `codex-exec-help.txt:4`). Top-level usage also accepts a bare prompt (TUI, not
non-interactive). `exec` has its own subcommands: `resume`, `fork`, `review` (`codex-exec-help.txt:7-11`);
only `resume` was help-probed. PROMPT may be omitted or `-`, in which case it is read from **stdin**
(`codex-exec-help.txt:14-17`). `exec` is scoped smaller than the TUI: no `--search`, no `-a/--ask-for-approval`.

### 3.2 Flag checklist (exact spellings from `codex exec --help`)

| Probe need | Flag as found | Status |
|---|---|---|
| structured events | `--json` — "Print events to stdout as JSONL" (`codex-exec-help.txt:109-110`) | **FOUND** |
| `--experimental-json` | — | **ABSENT** in 0.163.0 (superseded by `--json`; 0.150.1 spelling unverified — NOT-VALIDATED) |
| structured final answer | `--output-schema <FILE>` — "JSON Schema file describing the model's final response shape" (`:100-101`) | **FOUND** |
| final message extraction | `-o, --output-last-message <FILE>` (`:112-113`) | **FOUND** |
| model identity (set) | `-m, --model <MODEL>` (`:41-42`); also `-c model="o3"` via `-c` | **FOUND** |
| model identity (report) | — | **ABSENT from help** — no flag documents reporting which model served (NOT-VALIDATED whether events carry it) |
| full-auto | `--full-auto` | **ABSENT** (not listed anywhere in captured surfaces) |
| sandbox | `-s, --sandbox <read-only\|workspace-write\|danger-full-access>` (`:54-57`) | **FOUND** |
| dangerously-* permission flags | `--dangerously-bypass-approvals-and-sandbox` (`:62-64`), `--dangerously-bypass-hook-trust` (`:66-68`) | **FOUND** (note: `--approve-for-me` `:59-60` is the milder auto-review path) |
| approval policy on exec | `-a, --ask-for-approval` | **ABSENT on `exec`** — present only on top-level help and `codex resume` (`codex-help.txt:114-120`, `codex-resume-help.txt:92-98`), with only two possible values: `on-request`, `never` |
| working dir | `-C, --cd <DIR>` — "Tell the agent to use the specified directory as its working root" (`:70-71`) | **FOUND** |
| additional writable dirs | `--add-dir <DIR>` (`:76-77`); repo-isolation `--worktree` (`:73-74`) | **FOUND** |
| git-repo guard bypass | `--skip-git-repo-check` (`:88-89`) | **FOUND** |
| timeout / budget / turn cap | — | **ABSENT from help** — no timeout, budget, turn, or step limit flag exists in any captured codex surface |
| config override channel | `-c, --config <key=value>` (TOML, dotted paths), `--enable/--disable <FEATURE>`, `-p, --profile`, `--strict-config`, `--ignore-user-config` (`:20-33`, `:94-95`) | **FOUND** — this is the *only* documented way to shape tool policy on `exec` |
| state hygiene | `--ephemeral` — "Run without persisting session files to disk" (`:91-92`) | **FOUND** |

### 3.3 Session id / continuity (extracted from help text)

- `codex exec resume [OPTIONS] [SESSION_ID] [PROMPT]` — non-interactive resume "by id or pick the most
  recent with --last"; SESSION_ID is a "Conversation/session id (UUID) or thread name" and PROMPT may be `-`
  for stdin (`codex-exec-resume-help.txt:4-12`). It carries the same event flags (`--json` `:80`,
  `--output-schema` `:77`, `--output-last-message` `:83`) and `--ephemeral`/`--skip-git-repo-check`.
- `codex resume [OPTIONS] [SESSION_ID] [PROMPT]` is the **interactive TUI** resume (adds `--last`, `--all`,
  `--include-non-interactive`, `-a`, `--search`, TUI flags) — a different tool than `exec resume`.
- **No flag to *set* a fresh session id exists** in any captured codex surface. The id must come from the
  event stream or resume-by-position. NOT-VALIDATED.
- Related state-handling subcommands (*listed*, not probed): `queue`, `archive`, `delete`, `unarchive`,
  `fork`, `migrate-rollouts` (`codex-help.txt:25-37`).

## 4. Claude 2.1.260 — surface extracted from help text

### 4.1 Print mode exists; the `-p --help` finding

`-p, --print` — "Print response and exit (useful for pipes)" plus a trust-dialog note
(`claude-help.txt:165-174`), and the banner "starts an interactive session by default, use -p/--print for
non-interactive output" (`:4-5`). **There is no separate print-mode help**: `claude -p --help` renders the
flag surface*identical* to `claude --help` (we diffed `raw/claude-help.txt` vs `raw/claude-print-help.txt`;
the only differences are the usage line `claude [options] [prompt]` (drop of `[command]`) and omission of
the Commands section). Print-mode scope is marked inline throughout: many flags literally say
"(only works with --print)" — `--fallback-model` `:88-94`, `--forward-subagent-text` `:102-105`,
`--include-hook-events` `:112-114`, `--include-partial-messages` `:115-117`, `--input-format` `:118-121`,
`--max-budget-usd` `:125-126`, `--no-session-persistence` `:138-140`, `--output-format` `:141-145`,
`--permission-prompts` `:150-157`, `--replay-user-messages` `:185-188`.

### 4.2 Flag checklist (exact spellings from `claude --help`)

| Probe need | Flag as found | Status |
|---|---|---|
| print mode | `-p, --print` (`:165`) | **FOUND** |
| output formats | `--output-format <format>` — choices `"text"` (default), `"json"` (single result), `"stream-json"` (realtime streaming) (`:141-145`) | **FOUND** |
| input format | `--input-format <format>` — `"text"` (default) or `"stream-json"` (realtime streaming input) (`:118-121`) | **FOUND** |
| partial events | `--include-partial-messages` — "(only works with --print and --output-format=stream-json)" (`:115-117`) | **FOUND** |
| hook lifecycle events | `--include-hook-events` (`:112-114`) | **FOUND** (bonus) |
| subagent event surfacing | `--forward-subagent-text` — adds `parent_tool_use_id` messages (`:102-105`) | **FOUND** (bonus) |
| input echo | `--replay-user-messages` (`:185-188`) | **FOUND** (bonus) |
| model (set) | `--model <model>` — alias (`fable`, `opus`, `sonnet`) or full name (`:129-133`); plus `--fallback-model` (`:88-94`), `--effort` (`:78-79`) | **FOUND** |
| model (report) | — | **ABSENT from help** — no documented field/flag showing which model served (NOT-VALIDATED) |
| turn cap | `--max-turns` | **ABSENT** — not present anywhere in the 2.1.260 help surface (see §7) |
| cost cap | `--max-budget-usd <amount>` — "Maximum dollar amount to spend on API calls (only works with --print)" (`:125-126`) | **FOUND** |
| time cap | — | **ABSENT from help** — no timeout flag in any captured surface |
| token reporting | — | **ABSENT from help** — nothing documents where usage shown (NOT-VALIDATED) |
| settings | `--settings <file-or-json>` (`:222-223`), `--setting-sources <user,project,local>` (`:220-221`) | **FOUND** |
| permission mode | `--permission-mode <mode>` — choices `"acceptEdits"`, `"auto"`, `"bypassPermissions"`, `"manual"`, `"dontAsk"`, `"plan"` (`:146-149`) | **FOUND** |
| permission prompting in print | `--permission-prompts <host\|none>` — `"none"`: "anything that would prompt is denied automatically" (`:150-157`) | **FOUND** (key for unattended runs) |
| tool allowlist | `--allowedTools, --allowed-tools <tools...>` — "Comma or space-separated…" (`:23-25`) | **FOUND** (both spellings) |
| tool denylist | `--disallowedTools, --disallowed-tools <tools...>` (`:75-77`) | **FOUND** (both spellings) |
| skip all permissions | `--dangerously-skip-permissions` (`:66-68`); opt-in gate `--allow-dangerously-skip-permissions` (`:19-22`) | **FOUND** |
| tool box control | `--tools <tools...>` — `""` disables all, `"default"` all tools, or named list (`:251-255`); `--restricted` strips code-running tools + WebFetch and settings writes (`:189-203`); `--mcp-config` + `--strict-mcp-config` (`:127-128`, `:224-225`) | **FOUND** |
| extra dirs | `--add-dir <directories...>` — "Additional directories to allow tool access to" (`:11-12`) | **FOUND** |
| session id (set) | `--session-id <uuid>` — "Use a specific session ID for the conversation (must be a valid UUID)" (`:218-219`) | **FOUND** |
| resume | `-r, --resume [value]` — by session ID or picker (`:204-206`); `-c, --continue` (`:64-65`); `--fork-session` — "create a new session ID instead of reusing the original" (`:99-101`) | **FOUND** |
| session persistence off | `--no-session-persistence` — "(only works with --print)" (`:138-140`) | **FOUND** |
| background id | `--bg, --background` — "Prints the id that `claude attach`, `logs`, `stop` and `rm` take" (`:33-40`) | **FOUND** |
| agents | `--agent <agent>` (`:13-14`), `--agents <json>` (inline JSON custom agents) (`:15-18`) | **FOUND** |
| skills | `--disable-slash-commands` — "Disable all skills" (`:74-74`); `--bare` notes "Skills still resolve via /skill-name" (`:41-55`); `--safe-mode` disables skills+plugins+hooks (`:207-217`); plugin loading `--plugin-dir`/`--plugin-url` (`:158-164`) | **FOUND** — note: no dedicated `--skill` flag; skills are addressed via slash-name |
| structured final answer | `--json-schema <schema>` — "JSON Schema for structured output validation" (`:122-124`) | **FOUND** (parallel to codex `--output-schema`) |
| system prompt | `--system-prompt`, `--append-system-prompt`, `--system-prompt-snapshot <on\|off>` (`:226-244`, `:26-27`) | **FOUND** |
| worktree | `-w, --worktree [name]` (`:259-260`), `--tmux` (`:247-250`) | **FOUND** |

**Not listed as subcommands:** there is **no `config` subcommand** in 2.1.260 (settings flow through
`--settings`/`--setting-sources` per help text); `mcp`, `plugin|plugins`, `auth`, `import`, `doctor`,
`agents`, `attach`, `logs`, `stop|kill`, `rm`, `respawn`, `setup-token`, `update|upgrade`, `gateway`,
`project`, `auto-mode`, `ultrareview` exist (`claude-help.txt:262-305`) — none probed beyond listing.

## 5. Pipeline-needs cross-check

Pipeline needs (from the skill-evaluation brief): non-interactive invocation, structured events, session id,
model identity, termination control, tool visibility, token/cost reporting.
Every cell is a help-text-only conclusion; **NOT-VALIDATED** marks anything behavioral.

| Need | Codex 0.163.0 (host, ≠ pin) | Claude 2.1.260 (host == pin) |
|---|---|---|
| Non-interactive invocation | `codex exec [PROMPT|-]` (+stdin) — FOUND | `-p/--print`, prompt arg; trust dialog auto-skipped per help — FOUND |
| Structured events | `--json` → "events … as JSONL" — FOUND; event vocabulary/shape **NOT-VALIDATED** | `--output-format json\|stream-json`, `--input-format stream-json`, `--include-partial-messages`, `--include-hook-events`, `--replay-user-messages`, `--forward-subagent-text` — FOUND; stream schema/shape **NOT-VALIDATED** |
| Structured final answer | `--output-schema <FILE>`; `-o/--output-last-message <FILE>` — FOUND | `--json-schema <schema>` — FOUND |
| Session id | issuance not documented; resume by `[SESSION_ID]`(UUID)/`--last` under `exec resume`; `--ephemeral` for stateless — SET **ABSENT**, capture **NOT-VALIDATED** | settable `--session-id <uuid>`; `-r/--resume`, `-c/--continue`, `--fork-session`, `--no-session-persistence`, `--bg` prints id — SET **FOUND**, capture **NOT-VALIDATED** |
| Model identity | set `-m/--model`; serving-model report **ABSENT from help** (NOT-VALIDATED) | set `--model` (alias or full name); serving-model report **ABSENT from help** (NOT-VALIDATED) |
| Termination | **ABSENT from help** — no timeout/budget/turn flag; process exit code semantics **NOT-VALIDATED** (help text documents no exit codes for either harness) | `--max-budget-usd` (cost cap) FOUND; no time/turn cap; `--permission-prompts none` bounds runtime by denying prompts; exit codes **NOT-VALIDATED** |
| Tool visibility | indirect only: sandbox modes, `--dangerously-*`, `-c` config/profile overrides, `--ignore-rules` — FOUND (mechanism), effect **NOT-VALIDATED** | direct: `--allowedTools/--disallowedTools` (both spellings), `--tools`, `--restricted`, `--mcp-config`+`--strict-mcp-config`, `--permission-mode`, `--permission-prompts` — FOUND |
| Token/cost reporting | **ABSENT from help** (NOT-VALIDATED whether it appears in JSONL events) | cap only (`--max-budget-usd`); per-run usage fields **ABSENT from help** (NOT-VALIDATED) |

### 5.1 Asymmetries worth freezing in the contract

- **codex `--json` is exec-scoped**: it appears in `codex exec --help` but not in top-level help — the TUI
  has no JSONL mode; `codex resume` (TUI) and `codex exec resume` (non-interactive) are different tools.
- **codex `exec` has no approval-policy flag** (`-a/--ask-for-approval`), while top-level and `codex resume`
  do; `exec` relies on `--sandbox`, `--approve-for-me`, or the bypass flags.
- **claude print help = full help**: `-p` hides only subcommands (usage line), not flags — print mode
  inherits the whole option surface including interactive-only-looking flags.
- **`--max-turns` is gone from claude 2.1.260 help**: the flag long used by agent-SDK-style callers to bound
  turns has no spelling in this surface. If Root's contract needs a turn cap, it is either renamed
  (`--effort`, `--autocompact <auto|tokens>` `:28-29` are adjacent), settings-only, or absent — **NOT-VALIDATED**.
- Both harnesses document **no exit-code table** in help; Root's adapter contract would have to define
  termination semantics itself.

## 6. NOT-VALIDATED register (behavioral claims deliberately not tested)

All of the following are plausible-seeming but out of scope for a zero-cost help probe. None is asserted:

1. That `codex exec --json` emits any particular event vocabulary, a thread/session-id field, or usage
   numbers (§5 rows "structured events", "session id", "token reporting").
2. That `--output-schema` is *enforced* by codex vs merely documented.
3. That `codex exec` honors the top-level `-a` flag for resume flows or reads `--ask-for-approval` via `-c`.
4. That `claude --output-format json` result objects contain `cost_usd`/`usage`, `session_id`, `model`
   fields (classic SDK shape — but 2.1.260's actual shape is undocumented in help).
5. That `--max-turns` truly no longer exists in 2.1.260 (it may be env/setting-only; this probe only
   establishes it is **not in the help surface**).
6. That any flag outlined above is compatible *across the two codex versions in play* (0.150.1 pin vs
   0.163.0 host) — flag drift 0.150.1→0.163.0 is unmeasured; only 0.163.0's surface is captured.
7. That every `-V/--version` on subcommands (`codex exec --version` exists per `:118-119`) outputs the same
   banner as top-level (not run — unnecessary).

## 7. Fallback readiness verdicts

### Claude — READY-AT-PIN (surface-level)

- **Version:** host == pin (`2.1.260`, `manifest.rs:25`). The entire comparison table is therefore valid
  *at pin* for free.
- **Cheap next probes (still zero-cost):** `claude mcp --help`, `claude plugin --help`,
  `claude agents --help`, and (with auth disabled or a fake key so no API call can succeed, which must be
  designed deliberately) disguised dry runs of `-p` with `--output-format stream-json` against a
  no-op prompt — but only after the pin's owner agrees, because any real run touches live auth.
- **Needs acquisition/decisions:** none for the surface. Gaps to resolve in the Root contract: turn cap
  mechanism (`--max-turns` absent from help), time cap (absent), exit-code semantics, serving-model and
  usage/cost reporting fields in stream-json (NOT-VALIDATED).
- **One-line verdict:** *Claude fallback is cheap to adopt at pin — its full contract is probeable for free;
  open questions are termination caps and event-schema details, not availability.*

### Codex — PIN-BLOCKED, SURFACE STRONG

- **Version:** host 0.163.0 ≠ pin 0.150.1. The captured 0.163.0 surface is **not** admissible evidence for
  the 0.150.1 pin: flags may exist, differ, or be renamed (this host's help already shows evolution patterns
  like the two-value `--ask-for-approval` and `--json` replacing `--experimental-json`).
- **Cheap next probes:** none useful until the exact binary arrives — probing 0.163.0 further (e.g.
  `codex exec fork --help`, `codex exec review --help`, `codex features --help` — all *listed*, not run)
  adds table rows for a version the pin does not support.
- **Needs acquisition:** a 0.150.1 binary (same acquisition problem as the OpenCode pin). After that, the
  identical read-only protocol (§2 commands against 0.150.1) re-validates the table in minutes.
- **Surface gaps even if pinned:** no session-id set/capture documented beyond `exec resume [SESSION_ID]`,
  no cost/timeout/turn caps in help, tool visibility only via config overrides, exit codes undocumented.
- **One-line verdict:** *Codex fallback cannot be frozen at pin from this host — 0.150.1 acquisition is the
  gate; its 0.163.0 surface is encouraging but contracts cannot be frozen on drifted versions.*

## 8. Probe protocol provenance

- Single source of truth: `raw/*.txt` files (each begins `$ <command>` then exact output).
- Versions cross-checked against `crates/root-agent-bundle/src/manifest.rs` (`:19` codex 0.150.1,
  `:22` opencode 1.18.27 — out of this probe's scope, `:25` claude 2.1.260).
- No prompt, task, install, update, or config change occurred. No git commits. Repository writes limited to
  this directory (`Docs/Eval/milestone-0/fallback/`).
