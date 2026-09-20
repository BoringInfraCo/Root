# Root Safe Install Protocol — Agent Guide

This project uses **Root** for safe, deterministic, and verifiable system-tool installation.

When you (the agent) need a system tool — a CLI, runtime, language server, database, PDF utility, media tool, or any native dependency — **use Root first**. Do not fall back to unsafe global installers unless Root is unavailable and the user explicitly approves an alternative.

---

## Core Principles

1. **Never use `brew install`** — Homebrew lacks deterministic lockfiles and rollbacks.
2. **Never use `curl ... | sh`** — Piped remote scripts are unverifiable and unrepeatable.
3. **Never use `npm install -g` or `pip install --user`** — Global language-level installs fragment the environment.
4. **Never edit `PATH` manually** — Root handles profile activation through Nix.
5. **Always prefer `--json` output** — All Root commands emit structured JSON for reliable agent consumption.

---

## Safe Install Protocol

The protocol is a 6-step cycle. Every step uses `--json` so the agent can parse results programmatically.

### 1. Doctor — inspect environment health

```bash
root doctor --json
```

Check `healthy` and `issues` in the response. If `nix_installed` is `false` or `root_initialized` is `false`, warn the user before proceeding.

### 2. Plan — check availability before touching the system

```bash
root plan install <package> --json
```

The `plan` command queries nixpkgs for the requested package. It returns search results as a JSON-wrapped string. If the package is not found (error exit), do not proceed with install.

### 3. Install — perform the install with automatic snapshot

```bash
root install <package> --json
```

Root creates a pre-install snapshot automatically. The JSON response confirms success or failure.

### 4. Verify — confirm the binary is executable

```bash
root verify <package> --json
```

Root runs each binary the package exposes (via `--help`, `-h`, `--version`) and reports success/failure per binary. If any binary fails, proceed to rollback.

### 5. Rollback — revert to the known-good state on failure

```bash
root rollback --last --json
```

Restores the exact package set from the most recent snapshot. Always run this if verification fails.

### 6. Summarize — tell the user what changed

After install or rollback, briefly explain:
- What was installed or removed
- Whether verification passed or failed
- Whether rollback was triggered
- What unchanged tools remain

---

## JSON Output Contract

Every Root command that supports `--json` follows this contract:

- **Success**: Returns a JSON object with command-specific fields. Always includes `success: true` (or command-appropriate success indicators).
- **Error**: Returns `{ "success": false, "message": "..." }` and exits with code 1.
- **Doctor**: Returns `DoctorReport` with `healthy`, `nix_installed`, `root_initialized`, and `issues` array.
- **Verify**: Returns `VerificationReport` with `package`, `success`, and `binaries` array.
- **List**: Returns `ListOutput` with `packages` array and `nix_profile` string.
- **History**: Returns `HistoryOutput` with `snapshots` array.

---

## Required Commands

| Command | Purpose |
|---|---|
| `root doctor --json` | Environment health check |
| `root plan install <pkg> --json` | Search nixpkgs for a package |
| `root install <pkg> --json` | Install with snapshot |
| `root verify <pkg> --json` | Verify installed binaries |
| `root rollback --last --json` | Revert to previous snapshot |
| `root list --json` | List managed packages |
| `root history --json` | Show snapshot history |

---

## Example Agent Behavior

**User**: "Help me extract text from PDFs in this repo."

**Agent**:
```bash
root doctor --json
# {"healthy": true, "nix_installed": true, "root_initialized": true, "issues": []}

command -v pdftotext || true
# (not found)

root plan install poppler --json
# {"success": true, "message": "* nixpkgs#poppler (24.08.0)\n  poppler PDF rendering library"}

root install poppler --json
# {"success": true, "message": "Successfully installed poppler"}

root verify poppler --json
# {"package": "poppler", "success": true, "binaries": [{"binary": "pdftotext", "success": true, ...}]}
```

**Agent summary**:
```
I installed poppler through Root instead of Brew. Root created a snapshot before
the install, verified pdftotext is working, and confirmed no other tools were
affected. Rollback is available if needed.
```

---

## Warning / Refusal Cases

The agent **must warn the user and ask for confirmation** before proceeding in these situations:

| Condition | Action |
|---|---|
| Root is not installed | Warn, suggest installing Nix + Root |
| Nix is missing and Root cannot init | Warn, do not use alternative installers |
| `root doctor` reports errors | Show issues, ask if user wants to proceed |
| Package resolution is ambiguous | Show search results, ask user to specify |
| Verification fails after install | Run rollback automatically, report the failure |
| Rollback is unavailable | Warn that state cannot be reverted |
| User explicitly requests Brew/curl/npm-g | Respect the request but warn about determinism loss |

---

## Fail-Closed Behavior

If Root is unavailable (not installed, Nix missing, or initialization failed), **do not** silently fall back to Brew or other installers. Instead:

1. Inform the user that Root is unavailable.
2. Offer to install Root (requires Nix: `curl -L https://nixos.org/nix/install | sh`).
3. Only proceed with an alternative if the user explicitly authorizes it.

---

## Engineering Continuity (v0.5)

Root also persists engineering work independently of this agent session. When a Root workspace exists for the repository, record durable state instead of relying on chat history:

- Inspect — `root workspace status --json`, `root goal show --json`
- Record — `root decision add "<decision>" --json`, `root finding add "<finding>" --json`
- Checkpoint — `root checkpoint create --message "<summary>" --json`
- Continue elsewhere — `root resume --json`, `root handoff --to claude --json`
- After interruption — `root recover --json`

Over MCP the same operations are tools: `workspace.status`, `work.record_decision`, `work.record_finding`, `continuity.checkpoint`, `continuity.resume`, `continuity.handoff`. Start it with `root mcp serve`; inspect it with `root mcp status --json`.

Record decisions and findings only when they are durable and evidence-backed — not chain-of-thought. Never persist credentials; Root refuses obvious secrets.
