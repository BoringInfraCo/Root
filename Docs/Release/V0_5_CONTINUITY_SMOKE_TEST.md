# Root v0.5 — Continuity Smoke Test

**Sprint:** 010 — Recovery and Release Hardening
**Version:** v0.5.0
**Harnesses:** Codex CLI → Root → Claude Code (synthetic identities in CI)

This is a manual smoke test for the full v0.5 continuity loop. It complements
the automated coverage:

- `cargo test -p root-cli --test cross_agent`
- `cargo test -p root-cli --test recover`
- `cargo test -p root-cli --test resume`
- `cargo test -p root-continuity --test long_running`

## Setup

```bash
export ROOT_DIR=/tmp/root-v05-smoke
rm -rf "$ROOT_DIR"
mkdir -p /tmp/campfire && cd /tmp/campfire
git init -q
printf '# campfire\n' > README.md
git add -A && git -c user.email=root@example.com -c user.name=Root commit -qm initial
export ROOT=/path/to/root          # built binary
```

## 1. Workspace init

```bash
$ROOT workspace init
```

Expected: workspace name `campfire`, a `root_ws_...` id, and a `state.db` path.
Actual: ______________________________________________

## 2. Goal set

```bash
$ROOT goal set "Implement workspace invitations"
$ROOT workspace status
```

Expected: active goal statement; `Decisions 0`, `Findings 0`, `Artifacts 0`.
Actual: ______________________________________________

## 3. Codex connects and records work

Start `$ROOT mcp serve` as the server, connect a Codex client (or drive stdio
directly), and send:

```text
initialize            clientInfo.name = codex, protocolVersion 2024-11-05
notifications/initialized
tools/call work.record_decision { statement: "Invitations expire after 24h",
                                  rationale: "security" }
tools/call work.record_finding  { statement: "Invite consumption fails inside
                                   the membership transaction",
                                   evidence_ref: "integration test output" }
```

Expected: each call returns `isError: false` with a non-null `provenance_id`.
Actual: ______________________________________________

## 4. Checkpoint

```text
tools/call continuity.checkpoint { message: "Endpoint implemented; test failing" }
```

Or from the CLI:

```bash
$ROOT checkpoint create --message "Endpoint implemented; test failing"
```

Expected: a `root_cp_...` id; `environment_status` = `observed` when
`$ROOT_DIR/Rootfile` and `root.lock` exist, otherwise `missing`.
Actual: ______________________________________________

## 5. Codex exits

Terminate the Codex MCP process. The workspace and checkpoint must survive.

```bash
$ROOT checkpoint list
```

Expected: the checkpoint is listed.
Actual: ______________________________________________

## 6. Claude connects and resumes

Start a new `$ROOT mcp serve` and connect a Claude client
(`clientInfo.name = claude`):

```text
initialize
notifications/initialized
tools/call continuity.resume {}
tools/call continuity.handoff { to: "claude" }
```

Expected `resume` fields:

| Field | Expected | Actual |
|-------|----------|--------|
| `goal.statement` | `Implement workspace invitations` | |
| `checkpoint.id` | the Codex checkpoint id | |
| `decisions` | 1 active decision | |
| `findings` | 1 active finding | |
| `environment_state.status` | observed/missing (honest) | |
| `drift.level` | `none` on a clean tree | |

Expected `handoff` fields:

| Field | Expected | Actual |
|-------|----------|--------|
| `from` | `codex` | |
| `to` | `claude` | |
| `instructions` | present for the target | |
| `suggested_continuation` | labeled `Suggestion (not verified):` | |

## 7. Claude continuation

Record a follow-up decision as Claude and confirm it is attributed to Claude:

```text
tools/call work.record_decision { statement: "Validate invite inside the transaction" }
```

Expected: new decision `provenance_id` → `source_type = agent`, `agent = claude`.
Actual: ______________________________________________

## 8. Drift test

```bash
printf 'export {}\n' > work.ts
git add -A && git -c user.email=root@example.com -c user.name=Root commit -qm more
$ROOT resume --json
```

Expected: `drift.level` = `warning` with a `repository.head` item.
Actual: ______________________________________________

## 9. Recover test

Interrupt the session and run:

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

## 10. Backward compatibility

Verify the deterministic environment surface still works for v0.4 users:

```bash
$ROOT catalog
$ROOT plan install ripgrep
$ROOT status
$ROOT history
$ROOT adapters list
$ROOT mcp status
```

Expected: catalog/plan/status/history behave as before; no v0.4 command was
removed or renamed.
Actual: ______________________________________________

## Acceptance

- [ ] workspace init and goal set succeed.
- [ ] Codex records decisions/findings through MCP.
- [ ] checkpoint captures work, Git, and environment references.
- [ ] Claude resumes the Codex-created checkpoint without a transcript.
- [ ] handoff derives `from = codex`, `to = claude`, and instructions.
- [ ] drift is detected after HEAD change.
- [ ] recover reports durable state and the fixed not-recoverable list.
- [ ] resume output stays concise for large workspaces.
- [ ] v0.4 environment commands remain functional.
