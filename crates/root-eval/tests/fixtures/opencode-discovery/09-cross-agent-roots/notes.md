# 09 - cross-agent roots (global and walk-up)

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 1 - global cross-agent dirs
`~/.claude/skills/**/SKILL.md` and `~/.agents/skills/**/SKILL.md`, plus "the
same relative dirs found walking up from the instance directory, stopping at
the worktree root".

`claude-nested` proves the `**` recursion. Scenario `default` discovers all
five skills.

**Changed 2026-10-19 (live conformance).** The case previously encoded the
reading that `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` skips *both* cross-agent
families ("skipped entirely"), and its `disable-claude-skills` scenario
expected zero skills. That reading is **REFUTED** by the live probe
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.1, upstream
`skill/discovery.ts`: `if (!disableExternalSkills) { if (!disableClaudeCodeSkills)
targets.push(".claude"); targets.push(".agents") … }`). The flag now removes
the `.claude` roots only, so the scenario expects the two `.agents` skills.
`OPENCODE_DISABLE_EXTERNAL_SKILLS` (the flag that does remove the `.agents`
roots) is covered by the new case `18-flag-gates-external-skills`.
