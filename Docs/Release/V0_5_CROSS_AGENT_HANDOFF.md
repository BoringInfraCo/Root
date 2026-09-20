# Root v0.5 — Cross-Agent Handoff Smoke Test

**Sprint:** 009 — Cross-Agent Continuity
**Harnesses:** Codex CLI → Root → Claude Code

This documents the repeatable fixture that proves engineering work begun in
Codex can be continued in Claude without copying a transcript. The automated
test lives at `crates/root-cli/tests/cross_agent.rs` and never invokes the real
`codex` or `claude` binaries: it uses synthetic harness identities over the same
`root mcp serve` stdio surface a real harness uses.

## Fixture

- A real Git repository (`campfire`) with one commit, initializing a Root
  workspace with `ROOT_DIR` pointed at an isolated state directory.
- Active goal: `Implement workspace invitations`.
- One `root mcp serve` child is started per harness identity.

## Exact MCP sequence

Session 1 — client `clientInfo.name = "codex"`:

```text
initialize            protocolVersion 2024-11-05, clientInfo.name = codex
notifications/initialized
tools/call work.record_decision  { statement: "Invitations expire after 24h",
                                   rationale: "security" }
tools/call work.record_finding   { statement: "Invite consumption fails inside
                                     the membership transaction",
                                   evidence_ref: "integration test output" }
tools/call continuity.checkpoint { message: "Endpoint implemented;
                                     transaction test failing" }
```

The Codex child is terminated (kill + wait on drop).

Session 2 — client `clientInfo.name = "claude"`:

```text
initialize            clientInfo.name = claude
notifications/initialized
tools/call continuity.resume  {}
tools/call continuity.handoff { to: "claude" }
```

## Expected outcomes

`continuity.resume` returns the latest checkpoint and the active work recorded
by Codex:

- goal statement `Implement workspace invitations`;
- the checkpoint id created by Codex;
- one active decision and one active finding (superseded entries excluded);
- repository, environment, and drift state.

`continuity.handoff` returns a projection of the same canonical state:

- `from == "codex"` (derived from the checkpoint's provenance);
- `to == "claude"` (normalized target);
- `goal`, `checkpoint`, `state`, `decisions`, `findings`, `artifacts`,
  `environment`, `drift`, and labeled `suggested_continuation`;
- `instructions` for the target harness;
- a rendered human-inspectable `Handoff` document.

Unknown targets (for example `to: "gemini"`) return an MCP tool error that lists
the supported adapters.

## Manual reproduction

```bash
# Inside an isolated Root state directory
export ROOT_DIR=/tmp/root-v05-demo
root workspace init
root goal set "Implement workspace invitations"
root mcp serve   # connect Codex, record work, then: root checkpoint create
root resume
root handoff --to claude
root adapters inspect --agent claude
```

## Acceptance checklist (SPRINT_009 §10)

- [x] Codex connects to Root.
- [x] Claude connects to Root.
- [x] Codex-created state remains harness-independent.
- [x] Codex can create or contribute to a checkpoint.
- [x] Claude can resume from that checkpoint.
- [x] Claude receives decisions and findings.
- [x] Claude receives environment state.
- [x] Claude receives drift warnings.
- [x] No transcript copy/paste is required.
- [x] Root state remains inspectable by a human.
- [x] Repeated fixture test succeeds.
- [x] Release smoke test is documented (this file).

Automated coverage: `cargo test -p root-cli --test cross_agent`.
