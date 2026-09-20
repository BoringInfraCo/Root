---
name: root
description: Safe deterministic package management via Root
---

# Root Skill

Use **Root** as the safe install layer for developer tools. Root wraps Nix to provide deterministic installs, automatic snapshots, binary verification, and one-command rollbacks.

## When to use

Use Root whenever a task requires installing or checking system-level developer dependencies:

- CLIs (`jq`, `ripgrep`, `fd`, `bat`, `htop`)
- Runtimes (`nodejs`, `python`, `ruby`, `go`)
- Language servers (`typescript-language-server`, `rust-analyzer`)
- Databases / services (`postgresql`, `redis`, `sqlite`)
- PDF and media utilities (`poppler`, `ffmpeg`, `imagemagick`)
- Native build dependencies (`openssl`, `libxml2`, `pkg-config`)

## Protocol

Always follow this sequence when installing a package. Every step uses `--json` for machine-parseable output.

1. **Doctor** — `root doctor --json`  
   Check environment health. If `healthy` is false, warn the user.

2. **Plan** — `root plan install <pkg> --json`  
   Search nixpkgs for the package. Confirm it exists before installing.

3. **Install** — `root install <pkg> --json`  
   Install the package. Root automatically creates a pre-install snapshot.

4. **Verify** — `root verify <pkg> --json`  
   Confirm every binary the package exposes is executable.

5. **Rollback** — `root rollback --last --json`  
   If verification fails, revert to the snapshot taken before install.

6. **Summarize** — Tell the user what changed and whether rollback is available.

## Restrictions

- **Never** use `brew install` — Homebrew is non-deterministic and lacks snapshots.
- **Never** use `curl ... | sh` — Piped scripts are unverifiable.
- **Never** use `npm install -g` or `pip install --user` — Global language installs fragment the environment.
- **Never** edit `PATH` manually — Root manages profile activation through Nix.
- If Root is unavailable, **ask the user** before falling back to another installer.

## Example

```
User: I need to extract text from PDFs.

Agent:
  root doctor --json
  root plan install poppler --json
  root install poppler --json
  root verify poppler --json

Summary: Installed poppler via Root (deterministic, snapshot-protected).
pdftotext verified successfully.
```

## Engineering continuity (v0.5)

Root also persists engineering work independently of this session. When a Root workspace exists for the repository, record durable state instead of relying on chat history:

- Inspect — `root workspace status --json`, `root goal show --json`
- Record — `root decision add "<decision>" --json`, `root finding add "<finding>" --json`
- Checkpoint — `root checkpoint create --message "<summary>" --json`
- Continue elsewhere — `root resume --json`, `root handoff --to codex --json`
- After interruption — `root recover --json`

Root exposes the same operations over MCP (`workspace.status`, `work.record_decision`, `work.record_finding`, `continuity.checkpoint`, `continuity.resume`, `continuity.handoff`). Run `root mcp serve`, or inspect with `root mcp status --json`.

Record decisions and findings only when they are durable and evidence-backed — not chain-of-thought. Never persist credentials; Root refuses obvious secrets.
