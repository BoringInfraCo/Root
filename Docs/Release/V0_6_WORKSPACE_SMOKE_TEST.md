# Root v0.6 — Workspace Smoke Test

**Sprint:** 013 + 014 — Unified Checkpoint / Restore / Resume Lifecycle + Acceptance
**Version:** v0.6.0
**Harnesses:** Claude Code → Root → Codex CLI (real binaries on real machines; synthetic identities in CI)
**Theme:** *Root lets you pick up your work somewhere else.*

This is the manual, end-to-end smoke test for the v0.6 portable workspace. It
complements the automated coverage:

- `cargo test -p root-cli --test cross_agent`
- `cargo test -p root-cli --test resume`
- `cargo test -p root-cli --test recover`
- `cargo test -p root-cli --test resume_with`
- `cargo test -p root-cli --test checkpoint_agent`
- `cargo test -p root-cli --test restore_bind`
- `cargo test -p root-cli --test workspace_transfer`
- `cargo test -p root-cli --test workspace_pointer`
- `cargo test -p root-cli --test agent`

> **Hard constraint (carried from v0.5):** Root does not preserve conversations.
> No transcript is copied between machines or harnesses. Only recorded engineering
> state (goal, decisions, findings, artifact references, checkpoints, provenance,
> environment references) travels. Secrets never travel: credential *names* only.

## Setup

Two isolated Root directories model "Mac A" and "Mac B"; a single Git fixture
repo is cloned for the second machine, and a workspace transfer document is the
only work state that crosses.

```bash
# Mac A — isolated Root directory, home, and fixture repository
export ROOT_DIR_A=/tmp/root-v06-smoke-a
export ROOT_DIR_B=/tmp/root-v06-smoke-b
export HOME_A=/tmp/root-v06-home-a
export HOME_B=/tmp/root-v06-home-b
rm -rf "$ROOT_DIR_A" "$ROOT_DIR_B" "$HOME_A" "$HOME_B"
mkdir -p "$ROOT_DIR_A" "$ROOT_DIR_B" "$HOME_A/.claude" "$HOME_B/.codex"

export FIXTURE=/tmp/root-v06-campfire
rm -rf "$FIXTURE"
mkdir -p "$FIXTURE" && cd "$FIXTURE"
git init -q
printf '# campfire\n' > README.md
printf '.root/workspace.json\n' > .gitignore
git add -A && git -c user.email=root@example.com -c user.name=Root commit -qm initial

export ROOT=/path/to/root          # binary built from the release tag

# Binary version check (AGENTS.md release process)
"$ROOT" --version                  # must contain 0.6.0

# Fake harness shims: `--version` only, deterministic output (mirrors the test shims)
export FAKEBIN=/tmp/root-v06-fakebin
rm -rf "$FAKEBIN" && mkdir -p "$FAKEBIN"
printf '#!/bin/sh\nprintf "codex-cli 0.150.1\\n"\n' > "$FAKEBIN/codex"
printf '#!/bin/sh\nprintf "1.18.27\\n"\n'         > "$FAKEBIN/opencode"
printf '#!/bin/sh\nprintf "2.1.260\\n"\n'         > "$FAKEBIN/claude"
printf '#!/bin/sh\nexit 0\n'                         > "$FAKEBIN/nix"
chmod +x "$FAKEBIN"/codex "$FAKEBIN"/opencode "$FAKEBIN"/claude "$FAKEBIN"/nix
export PATH="$FAKEBIN:$PATH"

# Empty, valid deterministic environment for the smoke fixture.
printf '[packages]\n' > "$ROOT_DIR_A/Rootfile"
printf '{"version":2,"platform":"","packages":[]}\n' > "$ROOT_DIR_A/root.lock"
```

The shims exercise the exact version-gate paths deterministically. For the full
cross-harness acceptance use real Claude Code 2.1.260 and Codex CLI 0.150.1, or
synthetic identities driving `root mcp serve` over stdio (the
`V0_5_CROSS_AGENT_HANDOFF.md` pattern); never invoke real agent binaries in CI.

Step 1–5 run as **Mac A** (`ROOT_DIR=$ROOT_DIR_A`). Steps 6–14 run as **Mac B**
(`ROOT_DIR=$ROOT_DIR_B`) with the second clone. Set `ROOT_DIR` explicitly for
each step:

```bash
export ROOT_DIR="$ROOT_DIR_A"      # Mac A
export HOME="$HOME_A"
export CLAUDE_CONFIG_DIR="$HOME_A/.claude"
# later:
export ROOT_DIR="$ROOT_DIR_B"      # Mac B
export HOME="$HOME_B"
export CODEX_HOME="$HOME_B/.codex"
```

Home isolation is mandatory: capture intentionally inspects native and shared
agent configuration. Without isolated homes this smoke test would read the
operator's real `~/.claude`, `~/.codex`, or `~/.agents/skills` trees.

## 1. Workspace init + goal

```bash
export ROOT_DIR="$ROOT_DIR_A"
cd "$FIXTURE"
$ROOT workspace init
$ROOT goal set "Implement workspace invitations with expiring, hashed invite tokens"
$ROOT workspace status
```

Expected: workspace name `campfire`, a `root_ws_...` id, and a `state.db` path
under `$ROOT_DIR_A/work/<id>/`; active goal statement; `Decisions 0`,
`Findings 0`, `Artifacts 0`; a work revision baseline. When `init` is run with
`--write-pointer`, `<repo>/.root/workspace.json` is written (workspace id + Root
directory hint only).
Actual: **PASS (2026-09-22, hermetic release fixture).** Workspace initialized,
goal persisted, counts were 0/0/0, and the work revision baseline was present.

## 2. Deterministic environment restore (dry-run, then restore)

```bash
$ROOT restore --dry-run
$ROOT restore
```

Expected dry-run: a `Restore plan` listing will-install / will-keep / will-update
(per `root.lock`), honesty lines for models (`models were left unchanged`), and a
`Work state` block naming the bound workspace; **nothing is mutated** — the work
database bytes and mtime are unchanged and no `state.db-wal` / `state.db-shm`
sidecars appear.

Expected restore: `Restored Root profile from ...`; environment restored from the
lockfile; the `Work state` block re-binds the workspace by repo identity and
reports `latest checkpoint (none)` on a fresh workspace (never fabricated).
`--rebind` repairs a malformed project pointer from the Root index, or fails with
`run` hint if no workspace is known. With `--dry-run --rebind`, the proposed
rebind is reported (current state + target) and the pointer file is **not**
written; a real `--rebind` writes the pointer only after environment
restoration succeeds.
Actual: **PASS (2026-09-22).** Dry-run reported no package changes and no
checkpoint; restore completed and bound the same workspace without fabricating
a checkpoint.

## 3. Claude records decisions / findings / artifacts

Start `$ROOT mcp serve` and connect a Claude client (or drive stdio directly),
`clientInfo.name = claude`:

```text
initialize            clientInfo.name = claude, protocolVersion 2024-11-05
notifications/initialized
tools/call work.record_decision { statement: "Use Authorization-Code + PKCE (S256); no implicit flow",
                                  rationale: "public client; code_challenge S256, state+nonce verified" }
tools/call work.record_decision { statement: "Tokens live in macOS keychain (service root-oauth); never in repo, env files, or Root state" }
tools/call work.record_finding  { statement: "Token refresh round-trips against local IdP fixture",
                                  evidence_ref: "cargo test oauth_pkce_refresh — 6 passed" }
```

Then record file artifacts from the CLI:

```bash
printf 'export const pkce = 1\n' > src_auth_pkce.ts
printf 'export const routes = 1\n' > src_auth_routes.ts
$ROOT artifact add src_auth_pkce.ts
$ROOT artifact add src_auth_routes.ts
```

Expected: each MCP mutation returns `isError: false` with a non-null
`provenance_id`; provenance `source_type = agent`, `agent = claude`. Artifacts
record a path and a content fingerprint.
Actual: **PASS (2026-09-22, stdio MCP).** Two decisions and one finding were
recorded with non-null Claude agent provenance; both artifacts carried SHA-256
fingerprints.

## 4. Unified checkpoint (agent environment captured)

First capture the canonical agent environment into the project, then checkpoint:

```bash
mkdir -p .root
$ROOT agent capture --from claude --apply --out .root/agent.toml
git add .root/agent.toml src_auth_pkce.ts src_auth_routes.ts
git -c user.email=root@example.com -c user.name=Root commit -qm 'PKCE slice'
$ROOT checkpoint create --message "PKCE login working against local IdP; refresh test green"
$ROOT checkpoint list
```

Expected: a `root_cp_...` id; `environment: observed` when `$ROOT_DIR_A/Rootfile`
and `root.lock` exist; the checkpoint output includes an `Agent environment`
section (adapter `claude`, source version, skills, MCP server ids, and
`credential refs ... (names only)`); `Repository` shows branch, short HEAD, and
`clean`; the list row carries the checkpoint author as `from=codex` and the
captured harness as `env=claude`. The stored `agent_env_ref` is a
names-only summary — no secret values — and a secret-shaped summary is refused
atomically (no checkpoint row, no event).
Actual: **PASS (2026-09-22).** Clean checkpoint `root_cp_...` captured the
Claude 2.1.260 canonical environment, two decisions, one finding, two artifacts,
and observed Rootfile/root.lock digests.

## 5. Export workspace

```bash
$ROOT workspace export --out /tmp/campfire.rootws.json
cp /tmp/campfire.rootws.json "$ROOT_DIR_B" 2>/dev/null || true   # operator carries the document
```

Expected: `Workspace exported.` with the document path, byte count, and
`payload sha256`; counts for goals / decisions / findings / artifacts /
checkpoints / events. The source database is opened read-only: export never
migrates and never writes the source. Re-running without `--force` is refused if
the output exists; a tampered document fails the integrity check on import.
Actual: **PASS (2026-09-22).** Export produced one goal, two decisions, one
finding, two artifacts, one checkpoint, nine events, and a 64-character payload
digest. Tamper/read-only behavior also passed the automated transfer suite.

## 6. Import into a fresh ROOT_DIR + different project path

```bash
export ROOT_DIR="$ROOT_DIR_B"
export HOME="$HOME_B"
export CODEX_HOME="$HOME_B/.codex"
export FIXTURE_B=/tmp/root-v06-campfire-b
rm -rf "$FIXTURE_B"
git clone -q "$FIXTURE" "$FIXTURE_B"
cd "$FIXTURE_B"
$ROOT workspace import /tmp/campfire.rootws.json --project "$PWD" --write-pointer
cp "$ROOT_DIR_A/Rootfile" "$ROOT_DIR_B/Rootfile"
cp "$ROOT_DIR_A/root.lock" "$ROOT_DIR_B/root.lock"
$ROOT restore
```

Expected: `Workspace imported.` preserving the same `root_ws_...` id and name, the
imported counts, and the latest checkpoint id; a fresh `state.db` is created under
`$ROOT_DIR_B/work/<id>/` and the index is updated for this repository. Import
refuses an existing workspace identity, a non-filesystem-safe id, an already
present database, and a payload-digest mismatch — always into a fresh workspace.

Then `root restore` reconstructs the deterministic environment first and binds the
imported work state; digests either match the checkpoint or drift is itemized
(`environment.rootfile` / `environment.lock`), never silently promoted to
`verified`.

The explicit `Rootfile` / `root.lock` copy is intentional in v0.6. The workspace
document carries durable work state, not machine environment files. Automatic,
encrypted cross-machine synchronization is a post-v0.6 capability.
Actual: **PASS (2026-09-22).** Fresh import preserved the workspace and latest
checkpoint ids. After carrying Rootfile/root.lock, restore bound the imported
workspace and matched the checkpoint environment.

## 7. `resume --with codex` (7 steps)

```bash
$ROOT resume --with codex
$ROOT resume --with codex --json > /tmp/campfire.resume.json
```

Expected: `Root Resume --with Codex`, workspace + checkpoint header, then exactly
seven observable steps, each `ok`/`failed` with a detail:

| # | Step | Expected |
|---|------|----------|
| 1 | `prepare config` | target presence + version; MCP snippet ready |
| 2 | `map skills/instructions` | mapped portable / requires-review / unsupported counts (no apply) |
| 3 | `credential refs` | names only; `secret scan: N refused` |
| 4 | `restore workspace` | verify-only; environment matches checkpoint (else "run `root restore`") |
| 5 | `repo drift check` | `none` on a clean tree, else itemized |
| 6 | `assemble work state` | decisions/findings/artifacts + omitted count (caps 10/10/20) |
| 7 | `launch continuation` | ready; `resume never applies configuration` |

Human output additionally renders `Goal` / `Checkpoint` / `Decisions` /
`Findings` / `Relevant artifacts` / `Environment` / `Drift` /
`Suggested continuation` / `Instructions (Codex)` / `Next`, with suggestions
labeled `Suggestion (not verified):`. `--json` carries the same structs plus
`steps[]`. An unknown `--with gemini` fails closed listing supported targets
(`codex`, `opencode`, `claude`). `--with` with no value resolves the repo
Rootfile `[agents].default_target`.
Actual: **PASS (2026-09-22).** All seven steps were present, ordered, and `ok`;
mapping was available from Claude to Codex, the environment matched, repo drift
was none, and omitted count was zero.

## 8. Codex 4-question check

Start a new `$ROOT mcp serve`, connect a Codex client, and answer from the
continuation package only (no transcript, no side channel):

```text
tools/call continuity.resume { with: "codex" }
tools/call continuity.resume {}          # legacy v0.5 shape still works
```

| Question | Expected source | Actual |
|----------|-----------------|--------|
| What is the goal? | `goal.statement` verbatim | |
| What was done? | decisions + checkpoint message | |
| What constraints hold? | key decision (keychain-only tokens) | |
| What is next? | `suggested_continuation` / unresolved work, labeled not-verified | |

Expected: `continuity.resume { with }` returns the harness-aware
`ResumeWithReport`; without `with` it returns the legacy `{ resume, rendered }`
shape. Provenance is self-asserted (MCP is local/unauthenticated); no tool call
executes a shell.
Actual: **PASS (2026-09-22, stdio MCP).** Targeted resume returned the
harness-aware report; legacy resume retained its v0.5 shape. Goal, completed
work, keychain-only constraint, and labeled next suggestion were all answerable
from the continuation package.

## 9. Codex continues + second checkpoint

Record a Codex decision/finding and create a second checkpoint:

```text
tools/call work.record_decision { statement: "Validate invite inside the membership transaction" }
tools/call continuity.checkpoint { message: "Invite validation hardened; tests green" }
```

```bash
$ROOT checkpoint list
$ROOT resume
```

Expected: a second `root_cp_...` with `from`/provenance `agent = codex`; the list
shows both checkpoints and distinguishes `from=codex` from the captured
`env=claude`; `root resume` (no `--with`) stays byte-compatible with the
v0.5 struct + render and lists both checkpoints. The first checkpoint is immutable
after the new work: `root checkpoint show <first-id>` is byte-identical to capture
(revision and continuation summary unchanged).
Actual: **PASS (2026-09-22).** The second checkpoint preserved the first,
recorded Codex provenance, and human output now distinguishes `from=codex` from
the captured `env=claude`.

## 10. Drift test

```bash
printf 'export {}\n' > work.ts
git add -A && git -c user.email=root@example.com -c user.name=Root commit -qm more
$ROOT resume --json
$ROOT resume --with codex --json
```

Expected: `drift.level` = `warning` with a `repository.head` item; the `--with`
`repo drift check` step surfaces the same item. Drift never mutates Git or the
working tree, and normal change is never promoted to `blocking`.
Actual: **PASS (2026-09-22).** Both resume paths reported warning-level
`repository.head` drift and did not mutate the repository.

## 11. Recover test

```bash
$ROOT recover
```

Expected sections: `Root Recovery`, `Last durable checkpoint`, `Repository`,
`Environment`, `Work state`, `Recoverable`, `Not recoverable`,
`Recommended action`.

| Field | Expected | Actual |
|-------|----------|--------|
| `last_checkpoint.id` | latest `root_cp_...` | |
| `recoverable` | goal, decisions, findings, artifact references, checkpoint reference | |
| `not_recoverable` | the three fixed items | |
| `recommended_action` | `Inspect drift before resuming.` (drift present) | |

Actual: **PASS (2026-09-22).** Recovery named the latest checkpoint, returned
the fixed recoverable/non-recoverable sets, and recommended inspecting drift.

## 12. Secret probe

```bash
$ROOT decision add "client secret is sk-live-abc123, use it"
# Expected: refused: value looks like a credential (secret guard rail).
# No row written, no event appended (store atomicity, v0.5 pattern).

$ROOT decision list                 # unchanged; no secret row
$ROOT resume --with codex           # step 3: "secret scan: N refused"
```

Expected: the store-level mutation is refused with no row and no event; the MCP
`work.record_*` path returns `isError: true` for the same input; artifact paths
are never refused. `resume --with` step 3 reports the canonical-environment scan
count — `0 refused` on a clean workspace; if a secret-shaped value is present in
the canonical environment it is counted (e.g. `1 refused`) and a warning is
emitted, but the value is never read, printed, or copied.
Actual: **PASS after release fix (2026-09-22).** The exact `sk-live-abc123`
probe is refused by the shared store guard; the MCP path uses the same guard,
and focused store/MCP regression tests are green.

## 13. Resume-quality eval spot-check

The Sprint 014 resume-quality eval loop (metrics M1–M6: goal understood,
no repeated work, no violated decisions, artifacts found, next known, no
hallucinated state) is a deterministic eval harness, not a product feature. It
is automated in `crates/root-cli/tests/resume_quality.rs` (`resume_quality_matrix_meets_the_gate`):

```bash
cargo test -p root-cli --test resume_quality -- --nocapture
```

The harness runs 3 task briefs (PKCE login slice, invitation expiry change,
drifted workspace) × 2 harness pairs (codex→claude, claude→codex) × 4 drift
variants (clean, head-changed, artifact-missing, env-changed), emitting a
`report.json` with per-run `{task, pair, variant, M1..M6}`. Agent B receives ONLY
`root resume --with <B> --json` plus the rendered text and the repo at the
recorded HEAD.

Expected: M1/M3/M6 honesty-critical metrics at 100%; M2/M4/M5 at or above the
agreed threshold (>= 80%) across the matrix; drift level/kind correct in every
variant. No hosted runner, no model-graded score, no transcript persistence.
Record the observed pass-rates here.
Actual: **PASS (2026-09-22).** M1=1.000 M2=1.000 M3=1.000 M4=1.000
M5=1.000 M6=1.000 (24/24 runs).

## 14. Backward compatibility

Verify the v0.4 environment surface and the v0.5 continuity surface are unchanged:

```bash
# v0.4 environment
$ROOT catalog
$ROOT plan install ripgrep
$ROOT status
$ROOT history
$ROOT adapters list
$ROOT agent-bundle inspect --agent codex

# v0.5 continuity
$ROOT workspace status
$ROOT goal show
$ROOT decision list
$ROOT finding list
$ROOT artifact list
$ROOT checkpoint list
$ROOT resume
$ROOT handoff --to claude
$ROOT recover
$ROOT mcp status
```

Expected: catalog/plan/status/history behave as before; no v0.4 or v0.5 command
was removed or renamed; `resume` without `--with` is byte-compatible (struct +
render); caps 10/10/20, omitted counts, drift kinds/levels, the fixed
`NOT_RECOVERABLE` triple, and secret refusal are unchanged. A v0.5 `state.db`
(schema v2) opens under v0.6 and migrates additively to schema v3 with a
pre-migration backup; read-only inspection never migrates.
Actual: **PASS (2026-09-22).** The complete serialized workspace suite and
backward-compatibility matrix passed, including v2→v3 migration/backup, legacy
resume shape, caps, drift levels, fixed recovery triple, and secret refusal.

## Command reference (v0.6 surface)

Every command supports `--json`.

```text
root workspace init [--write-pointer] | status
root workspace export [--checkpoint <id>] --out <path> [-o] [--force]
root workspace import <file> [--project <path>] [--write-pointer]
root goal set <goal> | show
root decision add <statement> [--rationale <text>] | list | show <id>
root finding add <statement> [--evidence <ref>] | list | show <id>
root artifact add <path> | list
root checkpoint create [--message <msg>] | list | show [<id>] [--last]
root restore [--lock <path>] [--dry-run] [--rebind]
root resume [--checkpoint <id>] [--with [<agent>]]
root handoff [--to [<agent>]]
root recover
root mcp serve | status
root adapters list | inspect --agent <codex|claude>
root agent inspect <agent>
root agent plan [--from <agent>] [--to <agent>] [--env <file>]
root agent diff <agent_a> <agent_b>
root agent apply [--to <agent>] [--plan-hash <hash>] [--approve <sha256>...] [--apply] [--env <file>]
root agent verify --agent <agent>
root agent capture --from <agent> [--out <file>] [--apply] [--force] [--allow-outside-repo]
root agent rollback --last
root agent purge [--id <id> | --all] [--yes]
root agent-bundle inspect|export|plan|apply|verify|rollback --last|enable-plan|enable|purge --yes
```

## Acceptance

Sprint 014 §6 release checkboxes, restated for this smoke run:

- [x] 12-step cross-harness test passes with hermetic Claude/Codex identities,
      no manual config/toolchain/briefing copies, and no secret copies.
- [x] 4-question check (goal / history / constraints / next) answered from the
      package only.
- [x] Resume-quality eval at gate (M1/M3/M6 100%, M2/M4/M5 ≥ agreed threshold);
      report archived.
- [x] Checkpoints immutable; resume bounded (10/10/20 + omitted); drift branches
      honest; `recover` honest.
- [x] Secrets refused at store + MCP + `--with` step 3; artifact paths unaffected.
- [x] Compat matrix green; migration tests green; this smoke doc filled with
      actuals.
- [x] MCP still local/unauthenticated with documented limits
      (`Docs/MCP/SECURITY.md` reviewed).
- [x] README (`What v0.6 Changed`, title, Limitations header) + CHANGELOG
      (`[0.6.0]`) updated; `Docs/{Work,Continuity,Recovery,MCP,Release}` reviewed.
- [x] `cargo test --all`, `cargo fmt --all -- --check`,
      `cargo clippy --all-targets --all-features -- -D warnings` pass.

Release-hardware note: an earlier pass on this machine used deterministic
shims because the installed harnesses were Codex 0.149.1, Claude 2.1.204, and
OpenCode 0.4.26. Release builds for `aarch64-apple-darwin` and
`x86_64-apple-darwin` had already passed locally. The local macOS runner still
cannot link the Linux targets. The GitHub-hosted release cross-builds for
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` are the Linux gate;
they passed on `main` (CI run 35801581784) together with formatting, strict
Clippy, and `cargo test --all`.

Real-binary sign-off: **PASS (2026-09-22).** The same 14 steps were rerun with
`target/release/root` (`root 0.6.0`) and the exact supported binaries on
`PATH`, in isolated homes (no operator Claude/Codex/OpenCode config, no agent
session started):

| Binary | `root agent inspect` | Source |
| --- | --- | --- |
| Codex | `0.150.1`, `version_supported: true` | `~/.local/bin/codex` (`codex-cli 0.150.1`) |
| Claude Code | `2.1.260`, `version_supported: true` | `~/.local/bin/claude` (`2.1.260 (Claude Code)`) |
| OpenCode | `1.18.27`, `version_supported: true` | GitHub `anomalyco/opencode` `v1.18.27` `opencode-darwin-arm64` (the copy already on `PATH` was 1.18.32, so the pinned 1.18.27 binary was placed first) |

`root adapters list` reported Codex `codex-cli 0.150.1` and Claude Code
`2.1.260 (Claude Code)`, both supported. Claude MCP recorded decisions and the
finding as `source_type=agent`, `agent=claude`. `agent capture --from claude`
stored `source_agent_version` `2.1.260`. After import, `resume --with codex`
ran all seven steps `ok` against Codex 0.150.1. The second checkpoint listed
`from=codex` and `env=claude`. The `sk-live-abc123` decision was refused on
the CLI and on MCP, and no row was written. Drift, recover, catalog, plan,
history, handoff, and `mcp status` matched the expectations above.
