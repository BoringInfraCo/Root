# 04 - same/adjacent-level priority and cross-level name priority

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - priority order
`AGENTS.md` -> `CLAUDE.md` -> `CONTEXT.md` (deprecated). The wording is
"first name in priority order ... that matches anywhere in that span", so the
name is chosen first, then the nearest file with that name.

Scenarios:
- `all` - all three names in one directory: `AGENTS.md` wins.
- `claude-context` - `CLAUDE.md` wins over `CONTEXT.md`.
- `context-only` - the deprecated `CONTEXT.md` is the last resort.
- `cross-level` - `worktree/AGENTS.md` beats the nearer
  `worktree/app/CLAUDE.md`; this is the literal contract reading and is
  recorded as an interpretation in the phase report.
