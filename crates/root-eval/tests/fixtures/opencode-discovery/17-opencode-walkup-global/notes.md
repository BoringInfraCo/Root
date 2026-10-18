# 17 - walk-up `.opencode` config dirs plus `~/.opencode`

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 2 - config directories are
`${XDG_CONFIG_HOME}/opencode`, every `.opencode` dir from the instance
directory up to the worktree, and `~/.opencode`.

The instance directory (`worktree/app`) has no `.opencode`, the worktree root
does (`walkup-skill`), and the home-level `~/.opencode/skills` contributes
`global-plural`. Scenario `disable-project-config` drops the walk-up dir and
keeps the home-level one.
