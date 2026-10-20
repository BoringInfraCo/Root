# 16 - `OPENCODE_DISABLE_PROJECT_CONFIG` skips project `.opencode`

Contract: `UPSTREAM-CONTRACT.md` Sec.5.2 - the flag "skips all project
`opencode.json`/`.opencode` discovery"; Sec.4.1 lists `~/.opencode` as a config
directory independent of the project.

Scenario `default` discovers the project and the `~/.opencode` skill.
Scenario `disable-project-config` skips the walk-up `.opencode` entirely and
keeps only `~/.opencode`'s skill.

**Changed 2026-10-19 (live conformance).** The flag does more than the
contract text said: upstream skips the **project instruction walk-up** as well
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.2). This case has no
project instruction file, so its expectations are unchanged; the instruction
half is covered by cases `19-flag-project-config-instructions` and
`20-project-instruction-multilevel`.
