# Root — Agent Guide

Root is a deterministic package manager (built on Nix, no Nix knowledge required).  
Binary: `crates/root-cli/src/main.rs` → produces `root` CLI.

## Commands

```bash
cargo build
cargo test --all                          # all workspace tests
cargo test -p root-core                   # single crate
cargo fmt --all -- --check                # formatting check (no config, uses defaults)
cargo clippy --all-targets --all-features -- -D warnings
```

CI order: `fmt` → `clippy` → `test` (`.github/workflows/ci.yml`).

Release: tag `v*` triggers cross-compile for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.

## Release Process

### Prerequisites (before tagging)

1. Bump `version` in root `Cargo.toml` (workspace-level).
2. Build and verify the binary:
   ```bash
   cargo build && target/debug/root --version
   ```
3. Run full CI locally:
   ```bash
   cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
   ```
4. Update CHANGELOG.md with changes since last release (under `## [X.Y.Z] - YYYY-MM-DD`).
5. Update README.md if new features or notable fixes exist:
   - Add a `## What vX.Y.Z Changed` section after the current top section
   - Update the title `# Root vX.Y.Z` (if minor or major release)
   - Update `## Limitations (vX.Y.Z)` section header
   - Update experimental-commands prefix to match new version
6. Verify that `Docs/Release/` smoke test docs for the *previous* version are still accurate or have been reviewed for the new version.
7. Commit the version bump.
8. Tag and push:
   ```bash
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

### Version Consistency Checklist

Before tagging, verify all of these match the **new** version (X.Y.Z):

| Source | Check | Expected |
|--------|-------|----------|
| `Cargo.toml` | `workspace.package.version` | `X.Y.Z` |
| `Cargo.lock` | `root-cli`, `root-core`, `root-doctor`, `root-lockfile`, `root-nix`, `root-snapshot`, `root-verify`, `root-agent` | `X.Y.Z` (run `cargo metadata --no-deps --format-version 1`) |
| `cargo build && root --version` | Binary banner | contains `X.Y.Z` |
| `CHANGELOG.md` | Has `## [X.Y.Z]` entry | present |
| `README.md` | Title `# Root vX.Y.Z` | correct |
| `README.md` | `## Limitations (vX.Y.Z)` | correct |
| `README.md` | `not part of the vX.Y.Z public surface` | correct |
| `crates/root-lockfile` | `ROOT_LOCK_SCHEMA_VERSION` (package-only emit) | `2` |
| `crates/root-lockfile` | `ROOT_LOCK_MAX_SUPPORTED_VERSION` (v3 models map) | `3` |
| `Docs/Release/` smoke test docs | Title version references | up to date (check previous version smoke tests) |
| Git tag | `git tag` output | `vX.Y.Z` exists after push |

> **Policy note:** Old release notes in README (e.g., "What v0.1.8 Changed") are historical documentation and must not be rewritten. Git tags must never be deleted or recreated. If a tag was missed, accept the gap — do not retroactively create it.

## Testing quirks

- `MockNixAdapter` (in `root-nix`) lets unit tests run without real Nix.
- Tests that set `ROOT_DIR` env var **must** be serialized with `TEST_MUTEX: Mutex<()>` (see `root-core/src/lib.rs`). Cross-crate test pollution risk.
- Mock recognizes special packages: `"missing_pkg"` → `NotFound`, `"bad_platform_pkg"` → `PlatformMissing`.

## Architecture

8 crates. No async (all `std::process::Command`).

```
root-cli (bin) → root-core → root-lockfile, root-nix, root-snapshot, root-doctor, root-verify
root-agent                   # defined but NOT wired into CLI yet
```

`NixAdapter` trait in `root-nix` has two impls: `RealNixAdapter` (shells out to `nix` CLI) and `MockNixAdapter` (in-memory).

## CLI

Every command supports `--json` flag for structured output. Exit codes:

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic failure |
| 2 | Invalid arguments / unsupported import |
| 3 | Package not found |
| 4 | Verification failed |
| 5 | Drift detected |
| 6 | Rollback failed |
| 7 | Nix unavailable |
| 8 | Platform missing |

## Config

- `~/.root/Rootfile` — user TOML (package → version mappings; optional `[agents]` / `[models]` with `runtime = "ollama"` only)
- `~/.root/root.lock` — auto-generated JSON lockfile (schema v2 package-only; v3 when `models` is present; max supported 3)
- `~/.root/snapshots/` — JSON snapshot files
- `~/.root/model-pull.json` — exclusive in-progress model-pull marker (v0.3+)

## Design docs

`Docs/Core/` has the full spec chain (PRD → TECH_SPEC → ARCHITECTURE → UX_FLOWS → IMPLEMENTATION_PLAN → AGENT_SKILL_SPEC).

## Skill packs

`skills/` contains agent onboarding files for Codex (`skills/codex/AGENTS.md`), Claude (`skills/claude/SKILL.md`), Cursor (`skills/cursor/root.mdc`), and generic agents (`skills/generic/ROOT_AGENT_PROTOCOL.md`). These teach agents to use Root instead of Brew/curl for system tool installs.
