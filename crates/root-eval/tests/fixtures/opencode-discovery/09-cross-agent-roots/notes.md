# 09 - cross-agent roots (global and walk-up)

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 1 - global cross-agent dirs
`~/.claude/skills/**/SKILL.md` and `~/.agents/skills/**/SKILL.md`, plus "the
same relative dirs found walking up from the instance directory, stopping at
the worktree root"; all of these are skipped entirely under
`OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`.

`claude-nested` proves the `**` recursion. Scenario `default` discovers all
five skills. Scenario `disable-claude-skills` sets the gate and discovers none
(the resolver applies the literal "skipped entirely" reading to both
`.claude` and `.agents` roots; recorded as an interpretation).
