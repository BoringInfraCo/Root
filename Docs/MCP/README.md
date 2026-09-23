# Root MCP Interface

Root exposes a local MCP interface so coding agents can read and record
engineering continuity state. `rootd` listens on a Unix socket. The path is
short (`/tmp/rootd-<hash>.sock`) because macOS limits socket paths to 104
bytes; `$ROOT_DIR/rootd.path` records it. `root mcp serve` is the
stdio shim agents already launch: it connects to that socket, starting `rootd`
if it is not already running. Non-stdio connections present the bearer token
in `$ROOT_DIR/rootd.token`. MCP is an interface, not the product.

## Commands

```bash
root mcp serve              # stdio shim to rootd (newline-delimited JSON-RPC 2.0)
root mcp daemon [--http 127.0.0.1:PORT]   # foreground rootd; HTTP is opt-in loopback
root mcp status             # workspace, capabilities, policy source, and exposed tools
root capability list        # registered capabilities and their namespaces
root capability inspect NAME
root connector install|list|inspect|enable|disable|remove
root connector auth plan|bind|revoke
root approval list|approve|deny
root event list|watch|ack|ingest|deliver
root event route add|list
```

## Agent setup

Agents start the server with `root mcp serve` from inside the target repository
and `ROOT_DIR` pointing at the Root state directory. Codex and Claude
instructions live in `root adapters inspect --agent codex|claude`. See
[`Docs/Release/V0_5_CROSS_AGENT_HANDOFF.md`](../Release/V0_5_CROSS_AGENT_HANDOFF.md).

## Protocol

- Revision: `2024-11-05` (unknown client revisions fall back to this).
- HTTP, when enabled, is Streamable HTTP `POST /mcp` on `127.0.0.1` only.
  `Authorization: Bearer` is required. `initialize` returns `Mcp-Session-Id`;
  later calls send it back. `GET` is refused. `DELETE` ends the session.
  Protocol `2026-07-28` is rejected. The token is never accepted in the URL.
- Methods: `initialize`, `notifications/initialized`, `ping`, `tools/list`,
  `tools/call`.

## Tools

| Tool | Capability | Purpose |
|------|-----------|---------|
| `workspace.get` | read | Workspace identity and repository |
| `workspace.status` | read | Identity, active goal, work counts |
| `work.get_goal` | read | Active goal |
| `work.list_decisions` | read | Recorded decisions |
| `work.record_decision` | record | Record a decision (agent-attributed) |
| `work.list_findings` | read | Recorded findings |
| `work.record_finding` | record | Record an evidence-backed finding |
| `work.list_artifacts` | read | Recorded artifact references |
| `continuity.checkpoint` | checkpoint | Create an immutable checkpoint |
| `continuity.resume` | read | Produce a continuation package |
| `continuity.handoff` | read | Produce a portable handoff package |
| `environment.status` | read | Observed Root environment state |
| `environment.verify` | environment_verify | Honest observed status (never claims verification) |

## Authorization

Capabilities are enforced server-side. An absent `~/.root/mcp.toml` grants all
capabilities; an explicit `deny` removes one:

```toml
[capabilities]
read = "allow"
record = "deny"
checkpoint = "allow"
environment_verify = "allow"
```

See [SECURITY.md](SECURITY.md) for the security model and limitations.
