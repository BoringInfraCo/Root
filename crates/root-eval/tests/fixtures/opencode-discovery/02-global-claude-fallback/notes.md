# 02 - global fallback to ~/.claude/CLAUDE.md

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - if the XDG-config `AGENTS.md` is
absent, `~/.claude/CLAUDE.md` is used, and that fallback is suppressed by
`OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`. First match wins (break).

Scenario `default` attaches the fallback. Scenario `disable-claude-prompt`
sets the gate and resolves an empty instruction set.
