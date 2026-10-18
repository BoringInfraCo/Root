# 08 - plural `.opencode/skills/` config glob

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 2 - the config-dir glob is
`{skill,skills}/**/SKILL.md`, so both the singular and plural directory names
are scanned.

This case isolates the plural form; the skill is discovered under
`project/.opencode/skills/plural-skill/SKILL.md`.
