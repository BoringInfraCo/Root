# Root Recovery

`root recover` answers: *what durable state exists after an interrupted
session, and what can Root safely continue from?*

```bash
root recover
root recover --json
```

## What it reports

- **workspace** — id and name.
- **last_checkpoint** — id, message, created at, human age, and work revision
  (`null` when no checkpoint exists; this is not an error).
- **repository** — branch, HEAD, dirty state.
- **environment** — observed Rootfile / root.lock / profile references.
- **drift** — deterministic comparison against the latest checkpoint
  (`none` when there is no checkpoint).
- **work_state** — availability, active decision/finding counts, and artifact
  count.
- **recoverable** — the specific persisted items Root can continue from.
- **not_recoverable** — a fixed, honest list.
- **recommended_action** — deterministic next step.

## Recoverable (when present)

- `goal`
- `decisions`
- `findings`
- `artifact references`
- `checkpoint reference`
- `environment reference`

Only items Root actually persisted are listed.

## Not recoverable (always)

- `unrecorded agent conversation`
- `unsaved editor state`
- `commands not observed by Root`

## Recommended action

- drift is `warning` or `blocking` → `Inspect drift before resuming.`
- otherwise, a checkpoint exists → `Resume from <checkpoint id>.`
- otherwise → `Create a checkpoint to establish a durable continuation point.`

## What Root does not claim

Recovery is not autonomous restoration. Root reconstructs the durable record
and points at the evidence; it does not recreate a conversation, recover
unsaved buffers, replay unobserved commands, or guarantee environment
reproduction beyond the references it captured.
