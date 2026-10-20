# 02 - global fallback to ~/.claude/CLAUDE.md

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - if the XDG-config `AGENTS.md` is
absent, `~/.claude/CLAUDE.md` is used, and that fallback is suppressed by
`OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`. First match wins (break).

Scenario `default` attaches the fallback. Scenario `disable-claude-prompt`
sets the gate and resolves an empty instruction set.

**Changed 2026-10-19 (live conformance).** The same gate also drops the
project-level `CLAUDE.md` candidate (upstream's project priority list is
`["AGENTS.md", ...(!disableClaudeCodePrompt ? ["CLAUDE.md"] : []),
"CONTEXT.md"]`), which the M1 reading left out; this case only exercises the
global half, the project half is covered by case
`19-flag-project-config-instructions`.
