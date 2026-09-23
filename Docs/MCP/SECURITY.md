# Root MCP Security Model

This document describes what the local Root MCP server does and does not
protect. It is deliberately conservative. Root v0.5 is a single-user, local
tool; it is not a multi-tenant service.

## Workspace isolation

- A server process binds to exactly one workspace, discovered from the current
  directory and `ROOT_DIR`.
- All reads and mutations are scoped to that workspace's ID. Object IDs are
  additionally checked against the bound workspace on every lookup.
- One process cannot address another workspace through a tool call. There is no
  cross-workspace query, export, or mutation surface.

## Authorization capabilities

Capabilities are enforced server-side before dispatch:

| Capability | Governs |
|------------|---------|
| `read` | read-only tools |
| `record` | `work.record_decision`, `work.record_finding` |
| `checkpoint` | `continuity.checkpoint` |
| `environment_verify` | `environment.verify` |

An absent `~/.root/mcp.toml` allows every capability. An explicit `deny` removes
one. Enforcement is server-side; a client cannot grant itself a capability. A
denied call returns a tool error and performs no mutation.

## Mutation surface

The mutation surface is intentionally small:

- record a decision or finding;
- create an immutable checkpoint;
- (indirectly) start an agent-attributed session on `initialize`.

Root does **not** expose package installation, environment mutation, MCP server
configuration changes, Git mutation, shell execution, or filesystem writes
through MCP.

## Input validation

- `tools/call` `params` must be an object; `name` must be a string; `arguments`
  must be an object when present.
- Unknown tools and unknown methods are rejected (invalid params / method not
  found).
- Statements and optional strings are rejected when empty or longer than 10,000
  characters.
- Text that looks like an obvious credential is refused before persistence
  (PEM private keys, AWS/GitHub/Slack/OpenAI-style keys, bearer tokens,
  password/secret/token assignments). This is a guard rail, not a complete
  scanner.
- Malformed JSON yields a JSON-RPC parse error.

## Path handling

MCP exposes no arbitrary shell or filesystem mutation.

- `continuity.checkpoint` and `continuity.resume` derive artifact paths from
  records already stored in the workspace. MCP cannot submit a new artifact
  path, upload a file, or read an arbitrary path.
- `environment.status` / `environment.verify` read only `Rootfile`, `root.lock`,
  and the `profiles/default` reference under `ROOT_DIR`.

## Session attribution

- `initialize` creates one session attributed to the connecting client name.
  Re-initializing on the same connection keeps the original session.
- Recorded decisions, findings, and checkpoints carry that session's provenance
  (`source_type = agent`).
- Attribution is self-asserted by the client (`clientInfo.name`). Root does not
  authenticate the agent; it faithfully records what connected. Treat provenance
  as a claim, not cryptographic proof.

## Remaining limitations

- No authentication or encryption: any local process that can run `root mcp
  serve`, or connect to the socket named in `$ROOT_DIR/rootd.path`, can use
  the workspace. The socket is mode `0600` and is not under a world-readable
  name that includes workspace contents. There is no bearer token yet.
- No OS sandbox around the MCP process; isolation is by convention and by the
  narrow tool surface, not by privilege separation.
- Secret detection is heuristic and incomplete. Do not rely on it as a secrets
  manager.
- Provenance and session identity are not cryptographically verified.
- Denial is per capability, not per tool or per workspace; there is no RBAC.
- The daemon is a local Unix socket. There is no TCP listener.
