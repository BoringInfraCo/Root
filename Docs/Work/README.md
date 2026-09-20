# Root Work State

Root's Work State Engine gives engineering work a durable, harness-independent
home under `~/.root/work/<workspace-id>/state.db` (SQLite, schema v2).

## Workspace

A workspace is bound to one Git repository (path + remote identity). It is
created explicitly:

```bash
root workspace init
root workspace status
```

Root records work under its own directory. It does not write mutable work state
into the repository working tree.

## Goals

One active goal per workspace. Setting a new goal supersedes the previous one
(the old goal is kept, never deleted).

```bash
root goal set "Implement workspace invitations"
root goal show
```

## Decisions

Durable choices with optional rationale and provenance.

```bash
root decision add "Invitations expire after 24h" --rationale "security"
root decision list
root decision show <id>
```

## Findings

Evidence-backed observations. `--evidence` may point at a path, command, or
note.

```bash
root finding add "Consumption fails inside the transaction" --evidence "test output"
root finding list
root finding show <id>
```

## Artifacts

References to existing files (path + content fingerprint). Root records
references; it never creates or copies files.

```bash
root artifact add src/create.ts
root artifact list
```

## Provenance

Every durable record is attributed through a `provenance` row: `source_type`
(`human`, `agent`, `root`, `import`), optional `agent`, `harness`, `session_id`,
and `evidence_ref`. MCP-created records are attributed to the connecting agent
session; CLI-created records default to `human`.

## Events

Every accepted mutation also appends to the append-only `work_events` ledger
(sequence, event type, entity, payload, timestamp). The work revision is the
event count at the time it is read.

## Secrets

Root refuses to persist text that looks like an obvious credential (private
keys, AWS/GitHub/Slack/OpenAI-style keys, bearer tokens, password/secret/token
assignments). This is a conservative guard rail, not a complete scanner, and it
never rejects artifact file paths.
