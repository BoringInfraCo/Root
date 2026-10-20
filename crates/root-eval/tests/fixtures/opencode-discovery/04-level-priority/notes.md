# 04 - same/adjacent-level priority and cross-level name priority

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - priority order
`AGENTS.md` -> `CLAUDE.md` -> `CONTEXT.md` (deprecated). The wording is
"first name in priority order ... that matches anywhere in that span", so the
name is chosen first, then every level that holds that name attaches
(innermost first).

Scenarios:
- `all` - all three names in one directory: `AGENTS.md` wins.
- `claude-context` - `CLAUDE.md` wins over `CONTEXT.md`.
- `context-only` - the deprecated `CONTEXT.md` is the last resort.
- `cross-level` - `worktree/AGENTS.md` beats the nearer
  `worktree/app/CLAUDE.md`; this is the literal contract reading and is
  recorded as an interpretation in the phase report.

**Changed 2026-10-19 (live conformance).** The name-priority reading above is
**CONFIRMED** live (a farther `AGENTS.md` still outranks a nearer
`CLAUDE.md`), but the "nearest file with that name" half of it is **REFUTED**:
upstream attaches *every* level of the winning name
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.2). The scenarios here
have a single level per name, so their expectations are unchanged; the
multi-level shape is covered by cases `03` and `20`.
