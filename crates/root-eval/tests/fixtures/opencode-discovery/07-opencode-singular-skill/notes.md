# 07 - singular `.opencode/skill/` config glob

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 item 2 - every `.opencode` directory
from the instance directory up to the worktree is a config directory, scanned
with the glob `{skill,skills}/**/SKILL.md`.

The instance directory is the project root, so `project/.opencode` is the
config directory and `skill/root-singular/SKILL.md` is discovered.
