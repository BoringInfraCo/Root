# Milestone 0 — Upstream Contract Freeze: OpenCode `1.18.27`

Research date: 2026-10-18. Method: network reads only (npm registry JSON + GitHub API + raw source reads at tag `v1.18.27`). No local installs, no live runs.

Evidence labels used throughout:

- **CONFIRMED** — read directly in repo source at tag `v1.18.27` (per-file paths cited) or in registry/release metadata for exactly that version.
- **DOCUMENTED** — read on the docs site (docs reflect the *current* build, not `1.18.27`; use only as corroboration).
- **INFERRED** — deduced; flagged explicitly, needs live verification in Month‑Sentence 2 (S2) before any adapter ships.

## 1. Pin Existence

**Verdict: `opencode-ai@1.18.27` EXISTS. It is published to npm and tagged on GitHub. Root's pin at `crates/root-agent-bundle/src/manifest.rs:22` (`SUPPORTED_OPENCODE_VERSIONS = ["1.18.27"]`) is resolvable.**

### 1.1 npm package `opencode-ai` (CONFIRMED)

Source: `https://registry.npmjs.org/opencode-ai/1.18.27` (version document for exactly this release).

| Field | Value |
| --- | --- |
| name | `opencode-ai` |
| version | `1.18.27` |
| tarball | `https://registry.npmjs.org/opencode-ai/-/opencode-ai-1.18.27.tgz` |
| shasum (sha1) | `4c3c54227e0d3496b0914d19c98e70fd9c088108` |
| integrity (sha512) | `sha512-5xrG2gQEwV2sLus30SZX9GyLbPX3z57BCxddedDM0wx1bgnwlHVLOS/FD2uve7fEZlmkr7KYFbvs65ySz1rwzA==` |
| files | 4 files, 7,865 bytes unpacked (launcher stub; real binaries come from platform packages) |
| bin | `opencode` → `bin/opencode.exe` (resolved per-platform by `postinstall.mjs`) |
| publish mechanism | npm **trusted publishing** via GitHub Actions OIDC (`trustedPublisher: {id: "github"}`), `_npmUser: "GitHub Actions"` — not a human token, strong supply-chain signal |
| build provenance (`_resolved`) | `/home/runner/_work/opencode/opencode/packages/opencode/dist/opencode/opencode-ai-1.18.27.tgz` — built from the `opencode` monorepo, subpackage `packages/opencode` |
| maintainer | `thdxr <d@ironbay.co>` (OpenCode/SST core maintainer) |
| optionalDependencies | 12 platform packages, **all pinned at exactly `1.18.27`**: `opencode-{darwin,linux,windows}-{x64,arm64}`, `opencode-{linux-x64,linux-arm64,windows...}-musl`, and `opencode-{linux-x64,darwin-x64}-baseline` variants |

- The parent package is a tiny launcher; platform-specific binaries ship as the 12 companion packages above. The individual platform packages were **not fetched one-by-one** — their existence at 1.18.27 is INFERRED from the exact-pinned optionalDependencies list (low risk; npm resolves them from the same publish batch).
- Parents: `os: [darwin, linux, win32]`, `cpu: [arm64, x64]` — covers Root's full aarch64/x86_64 Apple + Linux matrix (musl vs gnu both present).

### 1.2 Publish date (CONFIRMED, derived)

- npm publish-processed timestamp from `_npmOperationalInternal.tmp` (`opencode-ai_1.18.27_1788385186029_...`): epoch ms `1788385186029` → **2026-09-02T21:39:46Z**.
- The packument's canonical `versions["1.18.27"].time` field could **not** be read directly: the full packument for `opencode-ai` (12,253 versions) is > 5 MB and truncated out on fetch. The timestamp above is npm's server-side processing marker from the version doc, which is the same publish batch. Marginal exact-to-the-second uncertainty; exact date stands.
- The sibling GitHub release confirms the same day (below).

### 1.3 GitHub release `v1.18.27` (CONFIRMED)

Source: `https://api.github.com/repos/anomalyco/opencode/releases/tags/v1.18.27` (release id 381579653).

- `html_url`: https://github.com/anomalyco/opencode/releases/tag/v1.18.27
- `tag_name`: `v1.18.27`, `draft: false`, `prerelease: false`, `immutable: true`, 40 assets.
- `published_at`: **2026-09-02T21:41:01Z**, `created_at`: 2026-09-02T21:40:42Z.
- `target_commitish`: `b04697366f05419e9bd7a92f841813dd976161c9`.
- Released by `opencode-agent[bot]` (same bot publishes every release in this repo).
- Repo org: **`anomalyco/opencode`** (the project formerly under `sst`; docs site footer is "© Anomaly", and npm's build path is `runner/_work/opencode/opencode`). Old GitHub URLs under `sst/opencode` now redirect — cite `anomalyco` everywhere.

### 1.4 Position in the release line (CONFIRMED from npm dist-tags + releases page)

Source: `https://registry.npmjs.org/-/package/opencode-ai/dist-tags`; GitHub releases list page.

- `dist-tags.latest = 1.19.3` (GitHub's web "Latest" badge shows `v1.19.0`, 11 Oct 2026).
- `1.18.27` is therefore **not latest**: the line runs `…1.18.26 → **1.18.27** → 1.18.28 → … → 1.18.35 → 1.19.0 → … → 1.19.3`. It is 9 npm releases behind `latest` (1.18.28–1.18.35, 1.19.x).
- Other dist-tags are pre-release noise: `beta` = `0.0.0-beta-…`, `dev` = `0.0.0-dev-202610161232`, `next`, plus ~30 experiment snapshot tags (`0.0.0-snapshot-*`) and legacy `latest-0` (1.0.142) / `latest-1` (1.1.4). No `1.18.x-latest` channel exists; pinning exact versions is the only safe strategy — which is what Root already does.
- Release notes nearby (from release pages/changelog): 1.18.29 fixed Codex OAuth model filtering; 1.18.31/32 fixed ACP chunk boundaries and Bedrock/Together streaming usage reporting — i.e., the neighboring releases tweak provider plumbing without renaming the core surface audited below. (DOCUMENTED/INFERRED — notes only skimmed, not diff-audited.)

## 2. Command Surface

All flags below are **CONFIRMED** by reading source at tag `v1.18.27`.

### 2.1 Global flags — `packages/opencode/src/index.ts`

Root-level yargs CLI (script `opencode`, `.strict()`, `--` values populated). Available to every subcommand, including `run`:

| Flag | Effect in source |
| --- | --- |
| `--print-logs` | sets `OPENCODE_PRINT_LOGS=1` (logs to stderr) |
| `--log-level` | `DEBUG \| INFO \| WARN \| ERROR` → `OPENCODE_LOG_LEVEL` |
| `--pure` | "run without external plugins" → sets `OPENCODE_PURE=1` |
| `--version` / `-v` | prints `InstallationVersion` |
| `--help` / `-h` | help output |

Subcommands: `acp, mcp, tui, attach, run, generate, debug, account, providers, agent, upgrade, uninstall, serve, web, models, stats, export, import, github, pr, session, plugin, db`.

- **Audit questions answered:** `--format json` → yes (per-command, below). `--model` → yes (per-command, below). `--dir` → yes (per-command, below). `--pure` → **yes, but as a global flag**, not a run-specific option. `--print-logs` → **yes, but as a global flag**, not a run-specific option.
- The docs site's "Global Flags" table (`https://opencode.ai/docs/cli/`) lists exactly these five — matches the source (DOCUMENTED corroboration, version-unverified).

### 2.2 `opencode run [message..]` — `packages/opencode/src/cli/cmd/run.ts`

Options exactly as declared in `builder()`:

| Flag | Details |
| --- | --- |
| positional `message..` | prompt words; joined; stdin pipe content is appended to a TTY-less invocation |
| `--command` | run a slash-command instead of a prompt |
| `--continue` / `-c`, `--session` / `-s` | resume sessions; `--fork` forks first (requires one of the two) |
| `--share` | share session |
| `--model` / `-m` | `provider/model` string, split on first `/` |
| `--agent` | agent name (falls back to default agent with a warning if missing/subagent) |
| `--format` | choices **`default` \| `json`**, default `default`. **`--format json` exists.** |
| `--file` / `-f` (array) | attach files/dirs |
| `--title` | session title (first 50 prompt chars fallback) |
| `--attach <url>` | attach to running server instead of in-process one |
| `--password` / `-p`, `--username` / `-u` | basic auth for attach |
| `--dir` | **`--dir` exists** — `process.chdir()` to the resolved path for local runs; remote directory path when `--attach` |
| `--port` | port for the local server (default: random; run normally uses an in-process server over injected `fetch`, `baseUrl http://opencode.internal`) |
| `--variant` | model variant / reasoning effort (`high`, `max`, `minimal`, …) |
| `--thinking` | show thinking blocks — **default `false` in non-interactive mode** (reasoning events are suppressed unless set) |
| `--interactive` / `-i` | interactive split-footer mode |
| `--auto` | auto-approve asks (reply `once`); aliases `--yolo`, `--dangerously-skip-permissions` (hidden) |
| hidden: `--mini`, `--replay` (default true), `--replay-limit`, `--demo` | interactive-only plumbing |

Non-interactive execution path (from `handler`): creates or resolves a session → optionally seeds extra permission rules → starts a concurrent event loop over `client.event.subscribe()` → calls `client.session.prompt({sessionID, agent, model, variant, parts})` (or `session.command` for `--command`) → when the server reports `session.status` with `type: "idle"`, the loop breaks and the process exits. Failed prompt/command responses produce `emit("error", …)` and `process.exitCode = 1`; session errors collected by the loop also set exit code 1. No other exit-code differentiation exists at run level.

## 3. Event Schema

### 3.1 What `run --format json` writes (CONFIRMED, `packages/opencode/src/cli/cmd/run.ts` `emit()`)

NDJSON, one line per event, `process.stdout` + `EOL`. Line envelope:

```json
{ "type": "<event>", "timestamp": <epoch ms int>, "sessionID": "ses_…", "…payload": … }
```

Emitted types and payload keys (all keyed off filtered events in the consume loop):

| JSON `type` | Source condition | Payload | Notes |
| --- | --- | --- | --- |
| `step_start` | `message.part.updated` where `part.type === "step-start"` | `{ part: StepStartPart }` | every LLM step start |
| `step_finish` | `part.type === "step-finish"` | `{ part: StepFinishPart }` | carries **tokens + cost** (see 3.2) |
| `text` | `part.type === "text"` and `part.time.end` set | `{ part: TextPart }` | **final text only** — no deltas streamed |
| `reasoning` | `part.type === "reasoning"` and `part.time.end` set | `{ part: ReasoningPart }` | only when `--thinking` is passed (off by default) |
| `tool_use` | `part.type === "tool"` and `part.state.status === "completed" \| "error"` | `{ part: ToolPart }` | one event per terminal tool state; includes deny-rejections |
| `error` | `session.error` event, or the prompt/command call itself fails | `{ error: AssistantError }` | error may carry `{name, data}` union member (3.2) |

**Not present in the JSON stream** (all CONFIRMED by reading the loop): no `session.created`/`session.updated`, no `message.updated`, no `message.part.delta`, no `session.status` (the idle event the loop waits on is *not* mirrored), no `permission.asked` mirror, and **no final/done/flush event**. Termination is detectable only as **stream EOF** (+ process exit code).

### 3.2 Canonical types backing the payloads (CONFIRMED)

Source: `packages/schema/src/v1/session.ts` (re-exported via `packages/schema/src/session-v1.ts` and alias `packages/core/src/v1/session.ts`), `packages/schema/src/v1/permission.ts`, `packages/schema/src/session-status-event.ts`, `packages/schema/src/session-id.ts`.

- ID prefixes: `SessionID` = `ses_…` (descending sort) — `packages/schema/src/session-id.ts`; `MessageID` = `msg_…`; `PartID` = `prt_…`.
- All IDs on parts: `partBase = { id: PartID, sessionID: SessionID, messageID: MessageID }`.
- `ToolPart`: `{ …partBase, type: "tool", tool: string, callID: string, state: ToolState, metadata?: Record }`. `ToolState` union by `status`:
  - `pending`: `{ input, raw }`
  - `running`: `{ input, title?, metadata?, time: { start } }`
  - `completed`: `{ input, output: string, title, metadata, time: { start, end, compacted? }, attachments?: FilePart[] }`
  - `error`: `{ input, error: string, metadata?: { interrupted?… }, time: { start, end } }`
  - → **tool name ✓, arguments ✓ (`state.input`), result ✓ (`state.output` string)**.
- `StepFinishPart`: `{ …partBase, type: "step-finish", reason: string, snapshot?, cost: number, tokens: { total?, input, output, reasoning, cache: { read, write } } }` → **tokens ✓, cost ✓ per step**.
- `TextPart`: `{ …partBase, type: "text", text, synthetic?, ignored?, time?: { start, end? }, metadata? }`.
- `ReasoningPart`: `{ …partBase, type: "reasoning", text, metadata?, time: { start, end? } }`.
- Part type union has 12 members (text, subtask, reasoning, file, tool, step-start, step-finish, snapshot, patch, agent, retry, compaction) — discriminated by `type`.
- Assistant message info (server-side types; not emitted to JSON by `run`): `{ …messageBase, role: "assistant", time: { created, completed? }, error?, parentID, modelID, providerID, mode, agent, path: { cwd, root }, summary?, cost, tokens: {…}, structured?, variant?, finish? }` → **observed model id and cumulative cost/tokens live only in message info, not in the JSON stream** (see Conformance).
- `User` message info: `{ role: "user", agent: string, model: { providerID, modelID, variant? }, time.created, … }`.
- `SessionInfo` (server-side): `{ id: "ses_…", slug, projectID, workspaceID?, directory, parentID?, summary?, cost?, tokens?, share?, title, agent?, model?: { id, providerID, variant? }, version, metadata?, time: { created, updated, compacting?, archived? }, permission?: Ruleset, revert? }`.
- Error union for `error` events (discriminator `name`, payload under `data`): `ProviderAuthError { providerID, message }`, `UnknownError { message, ref? }`, `MessageOutputLengthError`, `MessageAbortedError { message }`, `StructuredOutputError { message, retries }`, `ContextOverflowError { message, responseBody? }`, `ContentFilterError { message }`, `APIError { message, statusCode?, isRetryable, responseHeaders?, responseBody?, metadata? }`. Defined in `packages/schema/src/v1/session.ts` (same shapes via `NamedError` factories in `packages/core/src/v1/session.ts`).
- Server event inventory (what `event.subscribe()` actually streams; used by the run loop): `session.created|updated|deleted` `{sessionID, info}`, `message.updated` `{sessionID, info}`, `message.removed`, `message.part.updated` `{sessionID, part, time}`, `message.part.removed`, `message.part.delta` `{sessionID, messageID, partID, field, delta}`, `session.diff`, `session.error {sessionID?, error}`, `session.status {sessionID, status: idle | busy | retry {attempt, message, action?, next}}`, deprecated `session.idle`, `permission.asked {id, sessionID, permission, patterns, metadata, always, tool?}`, `permission.replied {sessionID, requestID, reply}`.

**How the model with which a message was produced can be identified:** in `--format json` it *cannot* be, from the stream alone. Every emitted payload is a part; parts carry `messageID` but nothing about provider/model. (CONFIRMED absence; the only identity-bearing artifacts are server-side `message.updated`/`SessionInfo`, which run does not mirror.) The **requested** model is the `--model provider/model` string supplied by the harness; mismatch resolution requires a `--attach` query or reading the SQLite DB (INFERRED workaround; untested).

## 4. Skills & Rules

### 4.1 Skill discovery (CONFIRMED — `packages/opencode/src/skill/index.ts`, `packages/opencode/src/skill/discovery.ts`)

Scanned sources in one discovery pass:

1. Global cross-agent dirs (skipped entirely under `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`/external-skills disable flags — see `packages/core/src/flag/flag.ts` + docs):
   - `~/.claude/skills/**/SKILL.md`
   - `~/.agents/skills/**/SKILL.md`
   - the same relative dirs found walking **up** from the instance directory, stopping at the worktree root
2. Each config directory (`ConfigPaths.directories()`: `${XDG_CONFIG_HOME:-~/.config}/opencode`, every `.opencode` dir from the instance directory up to the worktree, `~/.opencode`, `OPENCODE_CONFIG_DIR`) with glob `{skill,skills}/**/SKILL.md`.
3. Explicit `skills.paths` entries from `opencode.json` — `~/` expanded against home, relative resolved against the instance directory.
4. Remote `skills.urls` — pulled from `<url>/index.json`, downloaded into `${XDG_CACHE_HOME:-~/.cache}/opencode/skills/<skillname>` with `.opencode-version` swap logic (CONFIRMED in `discovery.ts`).
5. A built-in skill **`customize-opencode`** (location `<built-in>`, body from `SkillPlugin.CustomizeOpencodeContent`) registered before disk scan so a disk skill of the same name overrides it.

SKILL.md handling:

- Frontmatter must contain string `name`; `description` optional string. Invalid frontmatter → the load emits a `session.error` event and the skill is skipped (each bad file is an error line, run does not die).
- Duplicate names: last-scanned wins on disk with a warning log; built-ins are preferred to be overridden by disk.
- Same skill body is exposed as a **tool result** via the native tool (below), and availability is advertised to the model through `fmt(list)` — `## Available Skills` bullet list (verbose mode uses `<available_skills><skill>…</skill></available_skills>` XML) — CONFIRMED formatter; the exact prompt-injection site is in prompt building (INFERRED, prompt file not read).

### 4.2 Native `skill` tool (CONFIRMED — `packages/opencode/src/tool/skill.ts`)

- Tool id `skill`, parameters `{ name: string }` ("The name of the skill from available_skills").
- Execution: `ctx.ask({ permission: "skill", patterns: [name], always: [name] })` — i.e. a skill load passes through the permission engine (name-keyed allow/deny possible); with default rules it resolves to allow (the default ruleset is `"*": "allow"`, Section 5), then returns:
  ```text
  <skill_content name="…">
  # Skill: …
  <body of SKILL.md>
  Base directory for this skill: <dirname of SKILL.md>
  …
  <skill_files><file>…</file>…</skill_files>   (ripgrep sample, limit 10, excludes SKILL.md)
  </skill_content>
  ```
  with tool `title` = `Loaded skill: name` and tool `metadata: { name, dir }`.
- **Observable in `--format json`: yes** — a `tool_use` event whose `part.tool === "skill"`, terminal state carrying the full content in `state.output`. A denied skill load surfaces as `tool_use` with `state.status === "error"`. No other skill-specific event exists (INFERRED from the absence of any other emit path in the run loop).

### 4.3 Rules loading (CONFIRMED — `packages/opencode/src/session/instruction.ts`)

- Global level: `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` first; if absent, `~/.claude/CLAUDE.md` (suppressed by `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`). **First match wins (break).**
- Project level: walking up from the instance directory to the worktree root, first name in priority order `AGENTS.md` → `CLAUDE.md` → `CONTEXT.md` (deprecated) that matches anywhere in that span; **one file per message**.
- `instructions` config array: `http(s)://` URLs are fetched (5s timeout) as additional instruction blocks; bare paths are globbed (absolute / `~/`-expanded / relative to directory-worktree span, or to the global config dir when `OPENCODE_DISABLE_PROJECT_CONFIG`).
- Content is injected into the system prompt as `Instructions from: <path>` blocks. Companion `AGENTS.md`/`CLAUDE.md`/`CONTEXT.md` files in directories above a read file are also attached lazily per message.
- Precedence summary: global AGENTS.md over global CLAUDE.md, but project-level files are found independently of the global pick (both can attach). `OPENCODE_DISABLE_PROJECT_CONFIG` switches the relative-instruction glob base to the global config dir.

*Opportunity for S2:* the `AGENTS.md` preference chain means Root-authored global rules land at the path `~/.config/opencode/AGENTS.md` without touching `.claude` (bundle allowlist in `manifest.rs` already permits exactly `OpenCodeHome/AGENTS.md` and `OpenCodeHome/skills/*/…`).

## 5. Permissions & State

### 5.1 Non-interactive permission behavior (CONFIRMED throughout)

- **`opencode run` never blocks on a permission prompt.** On `permission.asked` the run loop immediately replies: `reject` by default (with a stderr line `permission requested: … ; auto-rejecting`), or `once` when `--auto`/`--yolo`/`--dangerously-skip-permissions` is given.
- The session is additionally seeded with deny rules for interactive-only prompts: `question`, `plan_enter`, `plan_exit` (all `deny "*"`).
- `Permission.Reply` = `once | always | reject`; requests carry `patterns[]` and optional `tool { messageID, callID }`.
- Default ruleset for the primary **build** agent (`packages/opencode/src/agent/agent.ts`):
  ```json
  { "doom_loop": "ask",
    "external_directory": { "*": "ask", "<whitelisted skill/tmp/glob dirs>": "allow" },
    "read": { "*": "allow", "*.env": "ask", "*.env.*": "ask", "*.env.example": "allow" },
    "question": "deny", "plan_enter": "deny", "plan_exit": "deny", "*": "allow" }
  ```
  **The catch-all default is `allow`** — bash/edit/etc. run freely without prompts in a stock `opencode run`. Denials require explicit configuration: `permission` in `opencode.json` (project, `.opencode/opencode.json`, or global), agent frontmatter `permission`, or `OPENCODE_PERMISSION` env JSON (`JSON.parse` in `config.ts`).
- Repeated identical tool calls (3× same tool+input) trigger a `doom_loop` permission ask — in stock non-interactive runs that ask is auto-rejected and the run stops; `experimental.continue_loop_on_deny: true` flips stop-on-deny behavior. Rejection also surfaces as a tool error part.
- Denial/rejection surface: `tool` part flips to `state.status = "error"` with `state.error = errorMessage(err)` (string) — reason string for rule-denied calls is `Permission.DeniedError`'s message ("The user has specified a rule which prevents you from using this specific tool call…"); aborts mid-cleanup keep running tool parts and set `error: "Tool execution aborted"`, `metadata.interrupted: true` (CONFIRMED — `packages/opencode/src/session/processor.ts`).

### 5.2 State written outside cwd (CONFIRMED unless noted)

`packages/core/src/global.ts` (XDG-based): app name `opencode`, directories are **created at module load even for a one-shot `opencode run`**:

| Path | Purpose |
| --- | --- |
| `${XDG_DATA_HOME:-~/.local/share}/opencode/` | session DB + `log/` + `repos/` + (plan mode) `plans/` |
| `${XDG_CACHE_HOME:-~/.cache}/opencode/` | `bin/` (downloaded provider/LSP binaries), skill pull cache `skills/` |
| `${XDG_CONFIG_HOME:-~/.config}/opencode/` | global config (`config.json`, `opencode.json`, `opencode.jsonc`), `AGENTS.md`, `auth.json` (auth.json — DOCUMENTED via docs) |
| `${XDG_STATE_HOME:-~/.local/state}/opencode/` | locks (`Flock.setGlobal({ state })`) |
| `$TMPDIR/opencode/` | temp files |
| `~` = `OPENCODE_TEST_HOME ?? os.homedir()` | home override only in this path lookup |

- SQLite DB exactly (`packages/core/src/database/database.ts`): `${data}/opencode.db` when channel ∈ {latest, beta, prod} or `OPENCODE_DISABLE_CHANNEL_DB=1`; otherwise `opencode-<channel>.db`; `OPENCODE_DB` may force `:memory:` or an absolute path (relative names join under data dir). WAL mode; all `MessageTable/PartTable/SessionTable` live here. Strong lever for hermetic fixtures.
- Config side effects (CONFIRMED, `packages/opencode/src/config/config.ts`): seeds a global `config.json` with only `$schema` if missing; **rewrites an existing `opencode.json` to insert `$schema` if it lacks one** (best-effort, silent failure); ensures a `.gitignore` in each config dir; background `npm install @opencode-ai/plugin` for every config directory — with shared npm cache side effects (external plugins disabled under `--pure`/`OPENCODE_PURE`).
- `OPENCODE_DISABLE_PROJECT_CONFIG` (runtime flag, `packages/core/src/flag/flag.ts`): skips all project `opencode.json`/`.opencode` discovery.
- NOT FOUND anywhere at this tag: a `--pure` variant of per-command scope, a "telemetry-free" toggle (docs list envs; telemetry present via OTEL flags only).

## 6. Conformance Verdict

Scope: what `opencode run --format json` at `1.18.27` can feed Root's harness golden fixtures.

| Audit fixture | Obtainable at 1.18.27 | Mechanism / caveat |
| --- | --- | --- |
| no-tool success | **YES** | `step_start` + `text` + `step_finish` lines with `sessionID` on each; `sessionID`, timestamps, `reason` all present |
| one tool call | **YES** | `tool_use` line; `tool` name, `callID`, `state.input` (arguments), `state.output` (result), `state.time.start/end` |
| denied permission | **YES — config required first** | Seed permission config (project `opencode.json` or `OPENCODE_PERMISSION`); observe `tool_use` with `state.status="error"`. Stock no-config behavior is allow-all, so a denied fixture cannot be produced unconfigured. `question`/plan_* is hard-deny regardless |
| provider error | **YES** | `error` line with `error.name/data` union member (e.g. `APIError {message, statusCode, isRetryable, …}`, `ProviderAuthError`); retried attempts show as `session.status=retry` server-side but are **not mirrored** to JSON |
| timeout/cancel | **PARTIAL** | No run-level `--timeout`. Real cancel (SIGINT/abort) → `error` line `MessageAbortedError` + tool parts marked `Tool execution aborted`/`interrupted`; provider header timeouts become retryable `APIError` and eventually `APIError {timeoutMs}` in metadata. Needs a live repro in S2 |
| skill load | **YES** | `tool_use` with `part.tool === "skill"`, full `<skill_content>` in `state.output`, `metadata {name, dir}`; place a skill under `.opencode/skill/<name>/SKILL.md` (or global home) and prompt the model to invoke it |

Required-field table:

| Requirement | Present? | Where |
| --- | --- | --- |
| session id | ✓ | every JSON line (`sessionID`, `ses_…`) |
| requested model identity | ✓ (harness-side) | `--model provider/model`, `--variant` |
| observed model identity | ✗ **not in JSON stream** | model id only in non-emitted `message.updated`/`SessionInfo`; resolve via attach/DB or accept the gap |
| message parts | ✓ (final states) | `text`/`reasoning` finals; **no parts deltas** |
| tool name + args + result | ✓ | `tool_use.part.{tool,state.input,state.output,callID}` |
| errors | ✓ | `error` event (`{name, data}`) |
| finish signal | ✓ (path) | `step_finish.part.reason` + stream EOF + exit code; **no dedicated terminal event** |
| token usage | ✓ per step | `step_finish.part.tokens {input, output, reasoning, cache.read/write, total?}` |
| cost | ✓ per step | `step_finish.part.cost` (assistant cumulative cost visible only server-side) |

Top conformance risks for the adapter (priority order):

1. **Model-identity gap**: no emitted event names the provider/model actually used. Adapter must either (a) correlate `messageID` via a `--attach` server session (requires `opencode serve`), (b) read the SQLite DB after the run, or (c) treat requested-from-CLI as the model identity for fixtures. Decide before S2; likely (c) with a recorded assertion.
2. **No terminal event / silent exits**: termination = EOF + exit code 0/1 only. Adapter must handle timeouts of its own (kill + parse partial stream) and cannot rely on a "session finished" JSON line. Also: while the in-process server means no TCP listener by default (`--port` only if given), so follow-up API calls from the same harness process require `--attach` to a `serve` session.
3. **Deny-default inversion + environment exit effects**: default rules are allow-all (`"*": "allow"`) — deterministic fixtures must inject explicit permission config; runs auto-create `${XDG_*}/opencode` dirs, install plugins in background (npm), and auto-write `$schema` into a config file it touched. Harness must pin `HOME`/XDG paths to a scratch dir (or `OPENCODE_DB=:memory:` + `--pure`) and snapshot the tree before/after.

Also confirmed viable: provider identity observed nowhere else; skill names/descriptions injected via system prompt (`fmt`) — a load may be triggered only if the model chooses to call `skill`; fixture skill prompts should be explicit.

**Fallback picture (for completeness): the pin exists, so no downgrade is needed.** If Root ever needs a newer stable line (1.19.0/1.19.3), re-audit: minor versions changed provider streaming plumbing (18.29–18.32 release notes); the CLI/event surface audited here should be re-read at whatever tag is pinned. **No S2 changes to the manifest pin are proposed** — `SUPPORTED_OPENCODE_VERSIONS = ["1.18.27"]` already matches the published package.

## 7. Sources

1. npm version doc (1.18.27): https://registry.npmjs.org/opencode-ai/1.18.27
2. npm dist-tags: https://registry.npmjs.org/-/package/opencode-ai/dist-tags
3. GitHub release: https://api.github.com/repos/anomalyco/opencode/releases/tags/v1.18.27 (+ https://github.com/anomalyco/opencode/releases/tag/v1.18.27)
4. Repo tree listings (tag v1.18.27): https://api.github.com/repos/anomalyco/opencode/contents/packages/opencode/src and /contents/packages/core/src
5. `packages/opencode/src/index.ts` (global flags) — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/index.ts
6. `packages/opencode/src/cli/cmd/run.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/cli/cmd/run.ts
7. `packages/opencode/src/cli/cmd/run/index.ts` sibling dir listing — https://api.github.com/repos/anomalyco/opencode/tree/v1.18.27/packages/opencode/src/cli/cmd/run (only referenced, contents not read)
8. `packages/schema/src/v1/session.ts` + re-exports (`packages/schema/src/session-v1.ts`, `packages/core/src/v1/session.ts`) — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/schema/src/v1/session.ts
9. `packages/schema/src/v1/permission.ts` + `packages/core/src/v1/permission.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/schema/src/v1/permission.ts
10. `packages/schema/src/session-status-event.ts`, `packages/schema/src/session-id.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/schema/src/session-status-event.ts and …/session-id.ts
11. `packages/opencode/src/skill/index.ts` + `discovery.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/skill/index.ts and …/discovery.ts
12. `packages/opencode/src/tool/skill.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/tool/skill.ts
13. `packages/opencode/src/session/instruction.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/session/instruction.ts
14. `packages/opencode/src/config/config.ts` + `paths.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/config/config.ts and …/paths.ts
15. `packages/core/src/global.ts`, `packages/core/src/database/database.ts`, `packages/core/src/flag/flag.ts` — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/core/src/global.ts, …/database/database.ts, …/flag/flag.ts
16. `packages/opencode/src/session/processor.ts` (tool error / doom_loop / abort handling) — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/session/processor.ts
17. `packages/opencode/src/agent/agent.ts` (default ruleset) — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/agent/agent.ts
18. `packages/opencode/src/session/message-v2.ts` (salvage/replay semantics) — https://raw.githubusercontent.com/anomalyco/opencode/v1.18.27/packages/opencode/src/session/message-v2.ts
19. Docs site (DOCUMENTED, current build — version-unverified): https://opencode.ai/docs/cli/ (flags tables), https://opencode.ai/docs/ (install footprint), npm page listing (versions counters): https://www.npmjs.com/package/opencode-ai
20. Changelog snippets on releases/changelog pages: https://opencode.ai/zh/changelog and https://github.com/anomalyco/opencode/releases

Everything not marked DOCUMENTED or INFERRED was read in source at tag `v1.18.27`.
