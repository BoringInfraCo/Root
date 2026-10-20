# 11 - duplicate skill names: upstream is a race, the manifest records the candidates

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 - "Duplicate names: last-scanned wins on
disk with a warning log."

Documented scan order (module docs of the resolver in
`crates/root-eval/src/context.rs`): global `~/.claude/skills` first, then the
walk-up roots, then the config directories in the contract's listed order
(`${XDG_CONFIG_HOME}/opencode`, walk-up `.opencode` dirs, `~/.opencode`,
`OPENCODE_CONFIG_DIR`). Therefore the three candidates are:

1. `home:.claude/skills/dup/SKILL.md`
2. `xdg:opencode/skills/dup/SKILL.md`
3. `project:.opencode/skill/dup/SKILL.md`

**Changed 2026-10-19 (live conformance).** The case previously encoded
"last-scanned wins" with two pairwise override events (claude-global → xdg,
xdg → project) and an unqualified winner. Live measurement **REFUTES** the
determinism: upstream registers discovered skills with unbounded concurrency,
so the winner is decided by async completion order and flips between runs
(measured: 5-10 winner flips per 30 runs across four layouts;
`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §2.3). The resolver now
records **one** `duplicate-skill-name` warning carrying every candidate
(path, token, digest, in scan order) and names no winner, the effective skill
entry is flagged `ambiguous`, and the manifest reports `reproducible: false`.
The manifest hash pins the candidate set, not a winner.
