# Root

**A persistent engineering environment for AI agents.**

Start in Codex. Continue in Claude. Pick it up tomorrow. The work stays where you left it.

Root installs developer tools from a `Rootfile`, pins them in `root.lock`, and keeps the work so another agent can resume. v0.7 adds one local MCP interface for connectors, events, and fixtures.

[![CI](https://github.com/BoringInfraCo/Root/actions/workflows/ci.yml/badge.svg)](https://github.com/BoringInfraCo/Root/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

[Docs](Docs/) · [Changelog](CHANGELOG.md) · [Smoke tests](Docs/Release/) · [MCP security](Docs/MCP/SECURITY.md)

## Why Root

Each harness wants its own setup, and a new chat does not remember the last one. Root keeps the machine and the work in one place.

- **Undo.** Every install snapshots first. `root rollback --last` restores the lock.
- **Verified.** Binaries are checked in `~/.root/profiles/default`, not on the global PATH.
- **Resumable.** `root checkpoint create` and `root resume --with` hand the next agent a continuation, not a transcript.
- **One local interface.** `rootd` serves MCP. Connectors, approvals, and events are shared by Codex, Claude, and OpenCode.

## How it works

1. **Declare** packages in `~/.root/Rootfile`.
2. **Pin** them to Nix store paths in `root.lock`.
3. **Apply** into an isolated profile. Snapshot first, verify after.
4. **Resume** from a checkpoint, or pull a handed route with `root event pull`.

## Install

Nix is required. The installer offers Determinate Nix if it is missing.

```bash
curl -fsSL https://boringinfra.company/root/install.sh | sh
root doctor
```

## Quick start

```bash
root plan install ripgrep
root install ripgrep
root verify ripgrep
root rollback --last
```

```bash
root workspace init
root checkpoint create
root resume --with codex
root mcp serve
```

Every command accepts `--json`. `root --help` lists the rest. v0.4 through v0.7 are in [CHANGELOG.md](CHANGELOG.md).

## v0.7

- **MCP.** `root mcp serve` talks to `rootd` on a mode `0600` Unix socket. Loopback HTTP is opt-in and needs the bearer token.
- **Connectors and events.** Packages install by digest. Writes wait on `root approval`. `root event pull` returns a handed route and does not start an agent.
- **Checkpoint references.** `root checkpoint-sync` moves ciphertext over a folder you choose. `root sync` still reconciles the Nix profile. Git carries source.
- **Fixtures.** Browser grants, `messages.local`, and `finance.local` stay on this machine. A payment intent does not move money.

`root import brew` is experimental and outside the v0.7.0 public surface.

## Compare

|  | Root | brew | curl \| sh | raw Nix |
|---|---|---|---|---|
| Deterministic lock (`root.lock`) | yes | no | no | manual |
| Undo (`rollback --last`) | yes | no | no | manual |
| Post-install verify | yes | no | no | no |
| No Nix to learn | yes | yes | yes | no |

## Limits

- Curated catalog, 42 tools. Arbitrary packages are rejected.
- Codex 0.150.1, OpenCode 1.18.27, and Claude Code 2.1.260 exactly.
- No cloud account. Checkpoint sync is a folder of ciphertext.
- Connector network and filesystem grants stay `none`.

## Develop

```bash
cargo test --all
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

## License

Apache 2.0 — see [LICENSE](LICENSE).
