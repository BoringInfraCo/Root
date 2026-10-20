# 03 - project walk-up attaches every level of the winning name

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - walking up from the instance
directory to the worktree root, the first name in priority order that matches
anywhere in the span wins.

The instance directory is `worktree/applications/service`. Two `AGENTS.md`
files exist (`worktree/AGENTS.md` and `worktree/applications/AGENTS.md`); the
name `AGENTS.md` wins the priority list, so **both** files attach - innermost
first. `worktree/CLAUDE.md` is deliberately absent so the selection is
unambiguous.

**Changed 2026-10-19 (live conformance).** The case previously expected only
the nearest `AGENTS.md` ("one file per message"). Upstream's `findUp` returns
*every* match of the winning name and the system prompt carries one
`Instructions from: <path>` block per match, so the "one file per message"
reading of contract §4.3 is **REFUTED**
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.2, "Multi-level
attachment"). The case now expects two project instructions, and the dedicated
multi-level case is `20-project-instruction-multilevel`.
