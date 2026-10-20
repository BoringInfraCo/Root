# 20 - every level of the winning project instruction name attaches

Upstream `1.18.27` (`packages/opencode/src/session/instruction.ts`) walks the
project span with `findUp(name, directory, worktree)`, which returns **every**
match of that name between the instance directory and the worktree root — not
just the nearest one. The system prompt then carries one
`Instructions from: <path>` block per match, innermost first. Measured live
2026-10-19: with `AGENTS.md` at `worktree/app/` and at `worktree/`, upstream
attaches both blocks
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.2, "Multi-level
attachment").

Scenarios:

- `default` - two project instructions attach (`worktree/app/AGENTS.md` first,
  then `worktree/AGENTS.md`).
- `disable-project-config` - the whole project walk-up is skipped.
