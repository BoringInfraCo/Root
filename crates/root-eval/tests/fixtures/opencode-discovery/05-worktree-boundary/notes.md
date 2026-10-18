# 05 - worktree-root boundary

Contract: `UPSTREAM-CONTRACT.md` Sec.4.3 - the project walk-up runs "from the
instance directory to the worktree root". Nothing above the worktree root is
scanned.

`outside/AGENTS.md` sits above both scenario worktrees. Scenario `inside`
resolves `worktree/AGENTS.md` (positive). Scenario `outside-not-scanned` has no
instruction inside its worktree and must resolve an empty instruction set even
though `outside/AGENTS.md` exists two levels above the worktree root
(negative).
