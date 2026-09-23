# Root v0.6.0

**Root is a persistent engineering environment for AI agents.**

Start in Codex. Continue in Claude. Pick it up tomorrow. The work stays where you left it.

Root began as a deterministic package manager for developer CLI tools, backed by Nix. That foundation is unchanged: declare intent in a `Rootfile`, Root pins exact store paths in `root.lock`, snapshots before every mutation, and installs to an isolated profile at `~/.root/profiles/default`. Every install is verified and undoable.

v0.5 makes the **work** durable on top of that environment. Workspaces, goals, decisions, findings, artifacts, provenance, and checkpoints persist independently of any agent, so another agent can resume without copying a transcript.

*Built for developers, coding agents, and reproducible dev machines.*

[![CI](https://github.com/BoringInfraCo/Root/actions/workflows/ci.yml/badge.svg)](https://github.com/BoringInfraCo/Root/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

[Docs](Docs/) · [Changelog](CHANGELOG.md) · [Smoke tests](Docs/Release/)

## What v0.6 Changed

v0.6 makes the workspace portable: durable work state moves through a verified workspace document, agent intent moves with the repository, and the deterministic environment is reconstructed from the carried `Rootfile` and `root.lock`. Another machine or supported harness can then resume without a transcript. Existing v0.4 environment management and v0.5 continuity commands are unchanged; the lock schema is still package-only emit 2 / max supported 3. Full history in `CHANGELOG.md`.

- **Canonical agent environment** — `root agent inspect|plan|diff` are read-only; `root agent apply|verify|capture|rollback|purge` are plan-first, hash-bound (`--plan-hash` + per-item `--approve`), and reuse the existing lock/journal/snapshot/rollback engine.
- **Project-scoped intent** — check in `<repo>/.root/agent.toml` and a `Rootfile` `[agents]` stanza (`env`, `default_target`); `root agent capture --from <agent> --apply --out .root/agent.toml` writes it with names and hashes only.
- **Unified checkpoint** — `root checkpoint create` now stores an immutable, names-only agent-environment reference alongside work, Git, and environment digests.
- **Environment-first restore** — `root restore [--dry-run] [--rebind]` reconciles the deterministic environment, then binds durable work state read-only and screens drift.
- **Harness-aware resume** — `root resume [--checkpoint <id>] --with <agent>` assembles a seven-step continuation package for `codex`, `opencode`, or `claude`; MCP `continuity.resume` accepts `with`.
- **Workspace transfer** — `root workspace export --out <file>` / `root workspace import <file> [--project <dir>]` move recorded work state as a versioned, hash-checked document. Git carries the repo and `.root/agent.toml`; the operator separately carries the Root environment files in v0.6.
- **Secret hygiene** — credential names only, everywhere; values are never read, logged, stored, or transferred. Claude MCP stays held.

## What v0.5.0 Changed

v0.5.0 adds engineering continuity on top of the deterministic environment. Existing v0.4 command behavior is unchanged; the lock schema is still package-only emit 2 / max supported 3. Full history in `CHANGELOG.md`.

- **Work state** — `root workspace`, `goal`, `decision`, `finding`, `artifact` persist goal/decisions/findings/artifact references with provenance and an append-only event ledger (SQLite schema v2 under `~/.root/work/`).
- **Checkpoints** — `root checkpoint create|list|show` capture immutable work + Git + environment references with a deterministic continuation summary.
- **Continuity** — `root resume` and `root handoff --to <agent>` project a small, newest-first continuation package (decisions ≤ 10, findings ≤ 10, artifacts ≤ 20) with drift detection.
- **Recovery** — `root recover` reports what durable state exists after an interruption and what Root can and cannot continue from.
- **MCP** — `root mcp serve` is a stdio shim to the local `rootd` Unix socket. `root connector` installs local packages. `root event` records inbound mail without starting an agent. `root checkpoint-sync` encrypts checkpoint references between paired installs. `root sync` still reconciles the Nix profile. `root computer grant` scopes the browser fixture to one visible target until it expires; the fixture does not attach to a desktop.
- **Adapters** — `root adapters list|inspect --agent codex|claude` for harness setup.
- **Secret protection** — work-state mutations refuse obvious credentials; this is a guard rail, not a complete scanner.

## What is Root?

One `Rootfile` per machine, undo anything.

```bash
root catalog              # browse 42 curated tools
root plan install ripgrep # preview, no changes
root install ripgrep      # install via Nix + lock + snapshot
root verify ripgrep       # check ~/.root/profiles/default/bin
root rollback --last      # undo it
```

Rootfile is intent, `root.lock` is truth (schema v2 packages, v3 models), snapshots are undo, the Nix profile is isolation. `root status`, `root history`, and `root doctor` tell you what drifted, what happened, and what's broken.

## Why Root?

- **Undo anything.** Every mutation snapshots first; `rollback --last` restores locked state.
- **Verified installs.** Binaries are checked in the Root profile, never global PATH.
- **Deterministic by default.** Curated Nix attributes + pinned store paths in `root.lock`.
- **No Nix to learn.** Plan / install / verify / rollback — Root speaks Nix for you.
- **Auditable.** Append-only event ledger + `--json` on every command.

## How It Works

1. **Declare** — packages in `~/.root/Rootfile` (`ripgrep = "latest"`).
2. **Pin** — Root resolves Nix attributes to store paths in `~/.root/root.lock`.
3. **Apply** — install into isolated `~/.root/profiles/default`, snapshot first, verify after.
4. **Undo** — `root rollback --last` restores last locked state; `root status` shows drift.

## Install

Root requires Nix (installer offers Determinate Nix if missing):

```bash
curl -fsSL https://boringinfra.company/root/install.sh | sh
root doctor
```

## Commands

Cheat-sheet — every command supports `--json`:

```bash
root catalog / search rg / plan install <pkg>  # discover + preview
root install <pkg> / remove <pkg> / update [pkg] # mutate (snapshot first)
root list / status / history / verify <pkg>     # inspect
root sync / restore --lock ./root.lock / rollback --last # reconcile + undo
root run <task> / sandbox create|run|list|destroy # execute + isolate
root models pull / plan models                   # Ollama pull-and-verify (v3 record)
root agent-bundle inspect|export|plan|apply|verify|rollback # explicit config transfer
root workspace init|status / goal set|show       # durable work state
root decision add|list|show / finding add|list|show # record + inspect work
root artifact add|list                           # reference existing files
root checkpoint create|list|show                 # immutable continuation points
root resume / handoff --to <agent>               # continuation + cross-agent handoff
root recover                                     # what survived an interruption
root mcp serve|daemon|status / capability list|inspect / adapters list|inspect
root agent inspect|plan|diff                     # canonical env + cross-harness (read-only)
root agent apply|verify|capture|rollback|purge   # hash-bound, plan-first translation
root workspace export|import                     # portable workspace transfer document
root restore [--dry-run] [--rebind] / resume --with <agent> # env-first restore + harness-aware resume
```

> `root import brew` is experimental and not part of the v0.6.0 public surface — may change or break without notice.

<details>
<summary>Exit codes & verify details</summary>

0 success, 1 failure, 2 bad args, 3 not found, 4 verify failed, 5 drift, 6 rollback failed, 7 Nix missing, 8 platform missing. `verify` checks `~/.root/profiles/default/bin`, never PATH.

</details>

## Agent Bundles

`root agent-bundle` explicitly transfers Codex / OpenCode / Claude working config between machines (`manifest.json` + `blobs/`). Same-agent only, no credentials, MCP imported disabled (Codex/OpenCode enable separately; Claude MCP is held in v0.4.1). See `Docs/Release/V0_4_AGENT_BUNDLE_SMOKE_TEST.md` and `Docs/Release/V0_4_1_CLAUDE_SMOKE_TEST.md`.

## Models

Declared Ollama models (`[models."qwen3:8b"] runtime = "ollama"`) are pull-and-verify, not a bit-pin: `plan models` previews, `models pull` fetches by tag and writes a v3 verification record. Restore/rollback copy the record; they never pull or delete weights.

## How Root Compares

|  | Root | brew | curl \| sh | raw Nix |
|---|---|---|---|---|
| Deterministic lock (`root.lock`) | yes | no | no | manual |
| Undo (`rollback --last`) | yes | no | no | manual |
| Post-install verify | yes | no | no | no |
| No Nix to learn | yes | yes | yes | no |

## What v0.4.1 Changed

Patch on the Portable Agent-Bundle release. Full history in `CHANGELOG.md`.

- **Claude adapter** — `--agent claude` on inspect/export/plan/apply/verify/rollback/purge, gated to **2.1.260** exactly.
- **Held-subset transfer** — allowlist `CLAUDE.md` + `settings.json` `model` only; native `~/.claude/skills` then shared skills; executables need `--include-executable` + `--approve`.
- **Claude MCP is held** — no disable-until-enable; `--include-mcp` / enable return `unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held.`
- **Never touches `.claude.json`** — apply/rollback snapshot `settings.json` only; stop Claude first.
- **Codex 0.150.1 / OpenCode 1.18.27 unchanged**, including MCP disable-until-enable.

## Limitations (v0.6.0)

- **Portable workspace, not sync** — the workspace transfer document (`root workspace export|import`) and Git are the only carriers across machines. No cloud sync, no team collaboration, no multi-writer conflict resolution; the registry is single-machine.
- **Environment transfer is reconstructed, not copied** — `root restore` rebuilds the deterministic Nix profile from `root.lock`; digests and references are recorded, never bit-for-bit machine images (`observed` is the honesty ceiling). Agent-environment apply is plan-first and re-reads live source content on the machine where it runs.
- **Curated catalog only** — 42 tools across 11 categories; arbitrary installs rejected. Run `root catalog`. `docker-client` is CLI only.
- **Undo covers Root only** — rollback restores Root lock/profile state, not Homebrew/manual changes; restore recovery is best-effort.
- **Agents + models are honest, not magic** — status inspects (never installs agents / pulls models); bundles are same-agent, credential-free; models are tag-pull verification records, digest drift needs re-pull.
- **Strict gates** — Codex 0.150.1, OpenCode 1.18.27, Claude 2.1.260 exactly; local Ollama `127.0.0.1:11434` only; no digest pull, no endpoint field.
- **Continuity is captured, not omniscient** — Root records goals/decisions/findings/artifact references and checkpoints. It cannot recover unrecorded conversations, unsaved editor state, or commands it did not observe. Secret detection is a conservative guard rail, not a complete scanner. Resume is capped (decisions ≤ 10, findings ≤ 10, artifacts ≤ 20).
- **MCP is local and unauthenticated** — stdio only, one workspace per process, server-side capability policy; see [Docs/MCP/SECURITY.md](Docs/MCP/SECURITY.md).
- **Online, serial, macOS-first** — network required, one mutation at a time (`root.lockfile` + `model-pull.json`), macOS tested / Linux best-effort / no Windows; Docker daemon needed for sandbox.
- **Nix required** — Root manages its own profile but doesn't bundle Nix; if a crash leaves `~/.root/root.lockfile`, run `root doctor` then remove it.

## Docs

- [Docs/](Docs/) — restore, sandbox, Nix audits, platform notes
- [Docs/Release/](Docs/Release/) — per-release smoke tests
- [CHANGELOG.md](CHANGELOG.md) — full version history (replaces old "What vX.Y.Z Changed" sections)
- [skills/](skills/) — agent packs (Codex / Claude / Cursor / generic)

## Development

```bash
cargo build
cargo test --all
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

## License

Apache 2.0 — see [LICENSE](LICENSE).
