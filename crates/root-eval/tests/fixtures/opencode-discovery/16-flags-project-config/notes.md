# 16 - `OPENCODE_DISABLE_PROJECT_CONFIG` skips project `.opencode`

Contract: `UPSTREAM-CONTRACT.md` Sec.5.2 - the flag "skips all project
`opencode.json`/`.opencode` discovery"; Sec.4.1 lists `~/.opencode` as a config
directory independent of the project.

Scenario `default` discovers the project and the `~/.opencode` skill.
Scenario `disable-project-config` skips the walk-up `.opencode` entirely and
keeps only `~/.opencode`'s skill.
