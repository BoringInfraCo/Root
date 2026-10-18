# 14 - explicit `skills.paths` entries

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 3 - explicit `skills.paths`
entries from `opencode.json`: `~/` is expanded against home, relative entries
resolve against the instance directory, and absolute entries are used as-is.

The project config `.opencode/opencode.json` declares three entry forms:
`~/home-skills` (home-relative), `rel-skills` (instance-relative), and an
absolute path that does not exist (accepted, discovers nothing).

`OPENCODE_DISABLE_PROJECT_CONFIG` skips the project `.opencode` directory
(contract Sec.5.2: "skips all project `opencode.json`/`.opencode` discovery"), so
scenario `disable-project-config` resolves no skills.

Scenario `config-dir-override` sets `OPENCODE_CONFIG_DIR=alt-config`; that
directory's `skill/` glob and its `opencode.json` (`rel-alt-paths`) are scanned
last. Its files use the `configdir:` token prefix because the directory is
outside the four declared roots.
