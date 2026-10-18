# 11 - duplicate skill names: last-scanned wins

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 - "Duplicate names: last-scanned wins on
disk with a warning log."

Documented scan order (module docs of the resolver in
`crates/root-eval/src/context.rs`): global `~/.claude/skills` first, then config
directories in the contract's listed order (`${XDG_CONFIG_HOME}/opencode`,
walk-up `.opencode` dirs, `~/.opencode`, `OPENCODE_CONFIG_DIR`). Therefore:

1. `home:.claude/skills/dup/SKILL.md` is overridden by
2. `xdg:opencode/skills/dup/SKILL.md`, which is overridden by
3. `project:.opencode/skill/dup/SKILL.md` (winner).

Both override events are recorded in `discovery_warnings`.
