# Root Continuity

Continuity binds durable work state to observable repository and environment
state through immutable checkpoints, then projects a small continuation package.

## Checkpoints

A checkpoint is an immutable record of work, Git, environment references, and a
deterministic continuation summary. It is never rewritten.

```bash
root checkpoint create --message "Endpoint implemented"
root checkpoint list
root checkpoint show --last
root checkpoint show <id>
```

## Drift

Drift compares recorded checkpoint facts to current observed state. It never
mutates Git or the working tree and never promotes a normal change to
`blocking`.

- `repository.head` / `repository.branch` — warning
- `repository.dirty` — informational
- `artifact.missing` / `artifact.changed` — warning
- `environment.rootfile` / `environment.lock` — warning
- `environment.profile` — informational

## Resume

`resume` selects the latest checkpoint (or `--checkpoint <id>`) and produces an
inspectable continuation package: goal, active decisions, active findings,
artifact references, repository/environment state, drift, and labeled
suggestions.

Resume is bounded deterministically (newest first):

| Item | Cap | Omitted field |
|------|-----|---------------|
| decisions | 10 | `decisions_omitted` |
| findings | 10 | `findings_omitted` |
| artifacts | 20 | `artifacts_omitted` |

`current_state` still reports full active counts; only the listed arrays are
capped.

```bash
root resume
root resume --checkpoint <id>
```

## Handoff

`handoff` reuses `resume`, then adds `from` (checkpoint provenance), a normalized
`to` target, and harness-specific instructions.

```bash
root handoff --to claude
```

Handoff is a projection, not a transcript conversion. It does not read, copy, or
interpret agent conversations.

## Recovery

See [../Recovery/README.md](../Recovery/README.md).

## Boundaries

Root captures what it observes. It does not capture agent conversations, editor
state, or commands it did not run. `resume` and `handoff` mark generated
suggestions as `Suggestion (not verified)`.
