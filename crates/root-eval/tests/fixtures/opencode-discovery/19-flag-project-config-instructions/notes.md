# 19 - the project-config gate kills the project walk-up; the prompt gate drops the project `CLAUDE.md`

Upstream `1.18.27` (`packages/opencode/src/session/instruction.ts`):

```
global  = [ <config>/AGENTS.md, …(!disableClaudeCodePrompt ? [<home>/.claude/CLAUDE.md] : []) ]
project = [ "AGENTS.md", …(!disableClaudeCodePrompt ? ["CLAUDE.md"] : []), "CONTEXT.md" ]

if (!OPENCODE_DISABLE_PROJECT_CONFIG)
  for (const name of project) {
    const found = await findUp(name, directory, worktree)   // every level, not just the nearest
    if (found.length > 0) { attach all of them; break }
  }
```

Measured live 2026-10-19 (`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md`
§3.2): with `OPENCODE_DISABLE_PROJECT_CONFIG=1` no project instruction file is
attached at all (the global one still is), and with
`OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1` a project-level `CLAUDE.md` is not a
candidate.

Scenarios:

- `agents-only` - the global XDG `AGENTS.md` and the project `AGENTS.md`
  attach (the project `CLAUDE.md` is outranked by `AGENTS.md`).
- `agents-only-disable-project-config` - only the global file attaches.
- `claude-only` - the global file and the project `CLAUDE.md` attach.
- `claude-only-disable-claude-prompt` - only the global file attaches.
