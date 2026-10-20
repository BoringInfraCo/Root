# 18 - the external-skill gates are separate from the Claude-skills gate

Upstream `1.18.27` (`packages/core/src/flag/flag.ts`,
`packages/opencode/src/skill/discovery.ts`):

```
disableExternalSkills    = OPENCODE_DISABLE_EXTERNAL_SKILLS
disableClaudeCodeSkills  = OPENCODE_DISABLE_CLAUDE_CODE || OPENCODE_DISABLE_CLAUDE_CODE_SKILLS
disableClaudeCodePrompt  = OPENCODE_DISABLE_CLAUDE_CODE || OPENCODE_DISABLE_CLAUDE_CODE_PROMPT

if (!disableExternalSkills) {
  if (!disableClaudeCodeSkills) targets.push(".claude")
  targets.push(".agents")
  … global home roots, then the walk-up roots …
}
```

So `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` removes the **`.claude` roots only**;
the `.agents` roots belong to `OPENCODE_DISABLE_EXTERNAL_SKILLS`, and the broad
`OPENCODE_DISABLE_CLAUDE_CODE` removes the `.claude` roots only as well (it
does not disable external skills). Measured live 2026-10-19
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.1).

Scenarios:

- `default` - all five skills are discoverable.
- `disable-claude-skills` - only the two `.claude` skills are gone; the
  `.agents` skills stay visible.
- `disable-external-skills` - both `.claude` and `.agents` skills are gone.
- `disable-claude-code` (broad) - same skill set as `disable-claude-skills`
  (the broad gate implies prompt **and** skills, never external skills); the
  manifest hash still differs because the flag projection differs.
- `disable-claude-skills-and-external` - only the `.opencode` config-dir
  skill survives.
