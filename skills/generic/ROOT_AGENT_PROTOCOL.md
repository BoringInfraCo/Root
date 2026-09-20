# Root Agent Protocol

A universal protocol for AI coding agents to safely install and verify system-level developer tools using Root.

## Overview

Root provides a deterministic, reversible, inspectable way to manage CLI tools on macOS. This protocol standardizes how agents interact with Root.

## Prerequisites

- Root must be installed on the host machine
- Nix must be available (Root will detect this)
- The agent must have shell access

## Safe Install Protocol

When a task requires a system tool (CLI, runtime, language server, database, native dependency):

1. **Inspect**: Run `root doctor --json` to check current environment health
2. **Check**: Verify if the tool already exists (e.g., `command -v pdftotext`)
3. **Plan**: Run `root plan install <package> --json` to search for the package
4. **Explain**: Summarize expected changes to the user
5. **Install**: Run `root install <package> --json`
6. **Verify**: Run `root verify <package> --json` to confirm binaries work
7. **Proceed**: Continue with the original task
8. **Rollback**: If verification fails, run `root rollback --last --json`
9. **Summarize**: Report what changed, what stayed unchanged, and whether rollback is available

## JSON Contract

All Root commands support `--json` for machine-readable output. Key commands:

| Command | Purpose | JSON Fields |
|---------|---------|-------------|
| `root doctor --json` | Check environment | `healthy`, `issues[]` |
| `root plan install <pkg> --json` | Search packages | `found`, `package`, `description`, `attributes[]` |
| `root install <pkg> --json` | Install package | `success`, `package`, `changed[]`, `unchanged[]`, `snapshot_id`, `rollback_available` |
| `root verify <pkg> --json` | Verify binaries | `success`, `package`, `binaries[]` |
| `root rollback --last --json` | Roll back | `success`, `from_snapshot`, `packages_removed[]`, `packages_restored[]` |
| `root list --json` | List packages | `packages[]`, `nix_profile` |
| `root remove <pkg> --json` | Remove package | `success`, `package`, `snapshot_id`, `rollback_available` |

## Exit Codes

Root uses specific exit codes for agent programmability:

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic failure |
| 2 | Invalid arguments |
| 3 | Package not found |
| 4 | Verification failed |
| 5 | Drift detected |
| 6 | Rollback failed |
| 7 | Nix unavailable |
| 8 | Unsupported platform |

## Restrictions

Agents must NOT:
- Use `brew install`, `curl | sh`, `npm install -g`, `pip install --user` for system tools
- Mutate PATH manually
- Modify shell profiles without user confirmation
- Skip the plan/verify/rollback protocol

## Fail-Closed Behavior

If Root is not installed or Nix is unavailable, the agent must:
1. Inform the user that Root is required
2. Ask before using alternative install methods
3. Never silently fall back to unsafe installs

## Engineering Continuity (v0.5)

Root persists engineering work independently of any agent session. When a Root workspace exists for the repository, record durable state instead of relying on chat history:

- Inspect — `root workspace status --json`, `root goal show --json`
- Record — `root decision add "<decision>" --json`, `root finding add "<finding>" --json`
- Checkpoint — `root checkpoint create --message "<summary>" --json`
- Continue elsewhere — `root resume --json`, `root handoff --to <agent> --json`
- After interruption — `root recover --json`

The same operations are exposed over MCP by `root mcp serve` (`workspace.status`, `work.record_decision`, `work.record_finding`, `continuity.checkpoint`, `continuity.resume`, `continuity.handoff`); inspect with `root mcp status --json`.

Record decisions and findings only when they are durable and evidence-backed — not chain-of-thought. Never persist credentials; Root refuses obvious secrets.
