# Root v0.2.4

> A curated package manager for developer CLI tools, backed by Nix.

Root installs developer CLI tools through Nix, records what changed, and lets you
undo it — without needing to learn Nix.

[![CI](https://github.com/sgr0691/Root/actions/workflows/ci.yml/badge.svg)](https://github.com/sgr0691/Root/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## What v0.2.4 Changed

v0.2.4 is the **Restore Reliability** release:

- **Restore audit** — Full restore subsystem audit at `Docs/Restore/V0_2_4_RESTORE_AUDIT.md`.
- **Dry-run support** — `root restore root.lock --dry-run` shows the restore plan
  (install, remove, keep, update) without changing Rootfile, `root.lock`, or the
  Nix profile. It records a `RestorePlanned` event.
- **Pre-restore validation** — Lockfile schema, store paths, platform compatibility,
  Nix availability, experimental features, and profile existence are all checked
  before any mutation.
- **Partial failure recovery** — If restore fails mid-operation, Root automatically
  rolls back the Nix profile to its pre-restore state and preserves the previous
  Rootfile and root.lock.
- **Drift detection in status** — `root status` now detects missing output paths,
  `.drv` paths in lockfiles, and platform mismatches in addition to existing
  name-based drift checks.
- **Restore event ledger** — Restore operations record `RestoreStarted`,
  `RestorePlanned`, `RestoreCompleted`, `RestoreFailed`, and `RestoreRecovered`
  events with package counts, failure phase, and duration.
- **Error normalization** — Restore failures produce clear, actionable messages
  with suggested next steps instead of raw Nix output.
- **New docs** — Restore audit, restore notes, and a dedicated smoke test document.

## What v0.2.3 Changed

v0.2.3 is the **Sandbox Hardening** release:

- **Lifecycle validation** — Sandboxes follow a strict state machine: Created →
  Running → Completed/Destroyed. Invalid state transitions are rejected.
- **Cleanup guarantees** — Destroy always attempts cleanup; failed or interrupted
  runs trigger cleanup; stale sandboxes are detectable.
- **Resource limits** — Docker containers are created with memory (2 GB default)
  and CPU (2 core) limits. Run `root sandbox create` with `--memory` and `--cpus`.
- **Timeout handling** — `root sandbox run` accepts `--timeout` (default 300s).
  Timed-out runs are terminated and recorded in the event ledger.
- **Post-create and post-destroy validation** — Container existence, reachability,
  and cleanup are verified after each operation.
- **Event ledger integration** — Every sandbox action (create, run, timeout,
  failure, destroy, cleanup) is recorded with timestamp, sandbox ID, and result.
- **Error normalization** — Sandbox failures produce clear messages for Docker
  unavailable, image pull failure, startup failure, timeout, resource limits,
  permission denied, and cleanup failure.
- **Sandbox audit** — Full subsystem audit at `Docs/Sandbox/V0_2_3_SANDBOX_AUDIT.md`.
- **New docs** — Sandbox notes and a dedicated smoke test document.

## What v0.2.2 Changed

v0.2.2 is the **Nix Reliability & Recovery** release:

- **Nix command audit** — Every Nix invocation catalogued with expected outputs,
  exit codes, failure modes, and error-handling gaps. See
  `Docs/Nix/V0_2_2_NIX_COMMAND_AUDIT.md`.
- **Experimental feature detection** — `root doctor` probes for `nix-command`
  and `flakes` support and explains how to enable them when missing.
- **Profile generation validation** — After every mutation (install, update,
  rollback, restore), Root validates that the Nix profile generation actually
  changed and expected output paths are present.
- **Store path hardening** — Derivation paths (`.drv`) are strictly separated
  from output paths at every layer. Lockfile validation rejects `.drv` paths
  in output fields before any mutation.
- **Error normalization** — All Nix failure modes produce clear, actionable
  messages without leaking raw Nix output. Covers missing Nix, disabled
  features, missing attributes, network failures, profile conflicts, and more.
- **Installer validation** — `root init --install-nix` now explains what will
  happen, requires explicit confirmation, detects platform, and runs a
  post-install probe.
- **New docs** — Nix reliability notes and a dedicated smoke test document.

## What v0.2.1 Changed

v0.2.1 is the **Performance & Reliability** release:

- **Faster search** — Query lowercased once instead of per-package (42×).
  `SearchMatch` and `CatalogEntry` use `&'static` lifetime strings, eliminating
  per-result heap allocations.
- **Content-aware file I/O** — Lockfile writes skip disk I/O when serialized
  output matches the existing file. `build_v2_lock` eliminates wasteful
  v2→v1→v2 conversions.
- **Bounded event history** — `root history --limit N` bounds in-memory event
  retention with a fixed-size rolling buffer (no O(N) memory blowup for large
  ledgers).
- **Smarter status for empty states** — Nix profile check is skipped when
  Rootfile and lockfile are both empty. Status stays entirely local.
- **Graceful error handling** — Malformed event lines in `events.jsonl` are
  skipped instead of failing. Status handles missing Rootfile, missing lockfile,
  unavailable Nix, and missing profile without panicking.
- **24 new tests** — Coverage for search, lockfile, history, status, plan, and
  catalog. Plus a Nix-avoidance test suite ensuring core commands don't shell
  out to Nix unnecessarily.

## What v0.2.0 Changed

v0.2.0 is the **Roadmap Phases 1–6** release:

- **Complete package workflow** — Search the curated catalog and update one or
  all managed packages with `root search` and `root update`.
- **Machine reproducibility** — Reconcile current v2 locks with `root sync` or
  rebuild a Root-managed profile from a shared lock with `root restore`.
- **Reproducible execution** — Define `[tasks]` in `Rootfile` and execute tasks,
  workflow files, or ad hoc commands through `root run`.
- **Permissions and policies** — Inspect active permissions with
  `root permissions` and activate TOML policies with `root policy apply`.
- **Docker-backed sandboxes** — Create, execute in, list, and destroy disposable
  Root-managed sandboxes through `root sandbox`.
- **Machine drift reporting** — `root status` compares Rootfile intent,
  lock state, and the Root-managed profile while maintaining machine identity.
- **Structured history** — Execution, policy, sandbox, restore, and update
  decisions are recorded in the event ledger and exposed through JSON output.

## Install

Root requires **Nix**. If Nix is not found, the installer offers to install it
for you using the [official Determinate Systems installer](https://install.determinate.systems/nix).

```bash
curl -fsSL https://raw.githubusercontent.com/sgr0691/Root/main/scripts/install.sh | sh
```

The installer will:

1. Check for Nix.
2. If Nix is missing, explain the dependency and ask for confirmation.
3. If confirmed, install Nix (this may modify your shell profile and create
   `/nix`).
4. Download and install the Root binary.
5. Run `root doctor` to verify everything is ready.

### Manual install (install Nix first)

If you prefer to install Nix yourself:

```bash
# Install Nix (one of these options):
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install

# Or use the official multi-user installer:
# sh <(curl -L https://nixos.org/nix/install)

# Then install Root:
curl -fsSL https://raw.githubusercontent.com/sgr0691/Root/main/scripts/install.sh | sh
```

The Root installer downloads the Root binary to a temporary directory, verifies
its SHA-256 checksum against the published checksum file, extracts it, and
installs it to `/usr/local/bin`. If any verification step fails, the installer
exits without installing. Use `--yes` to skip the Nix installation prompt
(e.g., for CI environments). Use `--dry-run` to preview what would be done.

## Quickstart

```bash
# 1. Browse the curated catalog
root catalog

# 2. Preview what install would do
root plan install ripgrep

# 3. Install
root install ripgrep

# 4. Verify the binary works
root verify ripgrep

# 5. See what happened
root history

# 6. Undo the install
root rollback --last
```

That's it. Every install is recorded, every binary is verified, and every change
can be undone — all without learning Nix.

Install times vary depending on network speed and Nix store state. First installs
may take several minutes while Nix resolves and downloads dependencies.

## Core Commands

All commands support `--json` for structured output (useful for scripting).

| Command | Description |
|---------|-------------|
| `root init [--install-nix]` | Initialize Root directory structure (auto-run on first mutation) |
| `root catalog` | Browse the curated package catalog |
| `root search rg` | Search package names, aliases, categories, and metadata |
| `root plan install ripgrep` | Preview what an install would do (no changes made) |
| `root install ripgrep` | Install a package via Nix with deterministic lock |
| `root list` | List installed packages |
| `root remove <package>` | Remove an installed package |
| `root update [package]` | Update one package or all Rootfile packages |
| `root lock` | Regenerate deterministic lockfile from current Rootfile |
| `root sync` | Reconcile the Root profile with `root.lock` |
| `root restore --lock ./root.lock` | Restore from a local or shared lockfile |
| `root restore --lock ./root.lock --dry-run` | Preview restore plan without mutating Rootfile, lock, or profile |
| `root run <task>` | Run a Rootfile task in the Root-managed environment |
| `root run <workflow-file>` | Run commands from a TOML workflow file |
| `root run -- <command...>` | Run an ad hoc command in the Root-managed environment |
| `root sandbox create [--name <name>] [--image <image>]` | Create a Docker-backed disposable sandbox |
| `root sandbox run <id> -- <command...>` | Execute a command in a running sandbox |
| `root sandbox list` | List all Root-managed sandboxes |
| `root sandbox destroy <id>` | Destroy a Root-managed sandbox |
| `root status` | Show machine identity and Root-managed drift |
| `root doctor` | Check that Root and Nix are ready |
| `root history` | Show snapshot summaries and event ledger |
| `root verify ripgrep` | Verify installed package binaries are functional |
| `root rollback --last` | Roll back to the last snapshot using locked state |
| `root permissions` | Show the active policy configuration |
| `root policy apply policy.toml` | Validate and activate a policy file |
| `root import brew` | (*Experimental*) Import Homebrew packages into a Rootfile |

## Supported Packages

Root curates a catalog of **42 developer CLI tools** across **eleven categories**:

| Category | Packages |
|----------|----------|
| media | ffmpeg, imagemagick, poppler |
| search | ripgrep, fd, fzf |
| dev | bat, bun, eza, gh, git-lfs, gnumake, httpie, jq, just, nodejs, openssl, pkg-config, python3, sqlite, tree, uv |
| net | curl, wget |
| language | go, rustup |
| database | postgresql, redis |
| infrastructure | terraform, kubectl, helm, k9s, docker-client |
| security | age, sops |
| editor | neovim |
| git | git-delta, lazygit |
| terminal | tmux, zoxide, direnv, starship |

Run `root catalog` to see the full list with Nix attributes and verification
commands at any time. Each package's metadata is defined in the `PackageSpec`
catalog inside `root-core`, making new packages easy to add.

## Why curated packages first?

Root uses a curated allowlist for safety:

1. **Predictable behavior.** Every supported package has well-known Nix
   attribute names, binary names, and verification commands. No surprises.
2. **Deterministic installs.** The package catalog provides the metadata
   needed for fully deterministic v2 lockfiles (correct binary names, proper
   store path tracking).
3. **Error prevention.** Unsupported packages are rejected before any Nix
   call — no waiting for a failed Nix build or wrong attribute name.
4. **Testable surface.** The curated set is easy to test end-to-end. Every
   package is validated for unique names, valid attributes, and at least one
   verification command.

Arbitrary `root install <anything>` support is planned for a future release.
Until then, unsupported packages get a clear error message with the full catalog.

## What v0.1.9 Changed

v0.1.9 is the **Stability & Hardening** release:

- **Verification no longer falls back to global PATH** — `root verify` requires
  binaries in `~/.root/profiles/default/bin`. If a binary is missing there,
  verification fails even if it exists elsewhere on PATH.
- **Non-standard tool verification fixed** — Correct arguments for `go version`,
  `terraform version`, `kubectl version --client`, `helm version --short`,
  `tmux -V`, and `direnv version`.
- **Nix error normalization improved** — Clear messages for missing experimental
  features and profile symlink conflicts.
- **Onboarding improved** — Doctor and init now explain why Root needs Nix
  and how to resolve common issues.
- **Release versioning hardened** — All version references now consistent.
- **Linux compatibility documented** — Investigation doc at `Docs/Platform/`.

### Example fixes

```bash
# Verification now correctly uses Root profile, not PATH
root verify go           # uses ~/.root/profiles/default/bin/go
root verify terraform    # uses `terraform version` not `--version`
```

## What v0.1.8 Changed

v0.1.8 is the **Developer Productivity Tools** release:

- **Expanded catalog** — From 37 to 42 curated packages. New category: `git`.
  New packages: git-delta, zoxide, direnv, starship, lazygit.
- **New aliases** — `delta` → git-delta, `z` → zoxide, `lg` → lazygit.
- **Developer productivity section** — These five tools are frequently
  recommended in terminal, Git, and productivity workflows.
- **Alias regression tests** — Plan and install tests for every new alias.

### Example usage

```bash
root plan install delta
root plan install z
root plan install lg

root install git-delta
root install zoxide
root install direnv
root install starship
root install lazygit
```

## What v0.1.7 Changed

v0.1.7 is the **Package Catalog Expansion** release:

- **Expanded catalog** — From 24 to 37 curated packages across ten categories.
  New categories: `language`, `database`, `infrastructure`, `security`, `editor`,
  `terminal`. New packages: go, rustup, postgresql, redis, terraform, kubectl,
  helm, k9s, docker-client, age, sops, neovim, tmux.
- **`docker-client`** — Installs the Docker CLI only, not Docker Desktop or a
  Docker daemon.
- **New aliases** — `golang` → go, `postgres` → postgresql, `tf` → terraform,
  `kube` → kubectl, `docker` → docker-client, `nvim` → neovim.
- **Verification improvements** — Package-specific verify commands added for
  go (`go version`), terraform (`terraform version`), kubectl
  (`kubectl version --client`), helm (`helm version --short`), and
  tmux (`tmux -V`).
- **Alias regression tests** — Every new alias has plan and install tests
  verifying canonical name storage in the lockfile.

## What v0.1.6 Changed

v0.1.6 is the **Drv Path Fix & Install UX** release:

- **Fixed `.drv` path leak in output verification** — `nix build --no-link --print-out-paths --json` returns both drv paths and output paths. The `.drv` path was being assigned as the `"out"` output, causing verification to fail with `Installed profile did not contain locked Nix store path ... .drv`. Now `.drv` paths are filtered out during extraction, and guards reject them at every layer.
- **Install script auto-elevation** — `curl ... | sh` now automatically uses `sudo` for the install step when needed. No more `sudo curl ... | sh` confusion or "No write permission" errors.
- **Early rejection of `.drv` output paths** — If a resolved package only has a `.drv` path, Root fails with a clear internal error instead of a misleading profile-verification failure.
- **Verification guard** — `verify_profile_contains_outputs` rejects `.drv` paths before checking the profile, with a clear error message.

## What v0.1.3 Changed

v0.1.3 is the **Curated Package Catalog** release:

- **Expanded catalog** — From 4 to 24 curated packages across `media`,
  `search`, `dev`, and `net` categories. Includes fd, bat, eza, fzf,
  git-lfs, gh, httpie, just, tree, sqlite, imagemagick, wget, curl,
  gnumake, pkg-config, openssl, python3, nodejs, bun, and uv.
- **`root catalog` command** — Lists all supported packages grouped by
  category.
- **Rich `PackageSpec` metadata** — Each package defines aliases, Nix
  attributes, expected binaries, per-binary verification commands,
  category, and description. The catalog is easy to extend.
- **Better unsupported-package errors** — Rejection messages now show
  categorized package lists so users can discover alternatives.
- **Full verification coverage** — Every supported package has at least
  one verification command. `root verify <pkg>` checks the Root-managed
  profile path, not the user's global PATH.

## Rootfile (`~/.root/Rootfile`)

The Rootfile is a TOML file at `~/.root/Rootfile` that declares which packages
and tasks Root manages. It is created automatically when you install your first
package.

```toml
[packages]
ripgrep = "latest"
ffmpeg  = "latest"
fd      = "latest"

[tasks]
build   = "cargo build --release"
test    = "cargo test --all"
lint    = "cargo clippy -- -D warnings"

[settings]
snapshots     = true
verify_installs = true
```

### Sections

| Section | Required | Description |
|---------|----------|-------------|
| `[packages]` | No | Package name → version mappings (e.g., `ripgrep = "latest"`) |
| `[tasks]` | No | Task name → shell command mappings (e.g., `build = "cargo build"`) |
| `[settings]` | No | Global settings (`snapshots`, `verify_installs` — both default to `true`) |

Use `root list` to show installed packages, `root remove <package>` to uninstall,
and `root run <task-name>` to execute a task in the Root-managed environment.

## Key Concepts

| Concept | Description |
|---------|-------------|
| **Rootfile** | TOML file at `~/.root/Rootfile` — your intent (packages and tasks you want) |
| **root.lock** | JSON file at `~/.root/root.lock` — the deterministic lock with pinned Nix metadata |
| **Snapshot** | JSON file at `~/.root/snapshots/` — a pre-mutation copy of the lock state for rollback |
| **Event ledger** | JSONL file at `~/.root/events.jsonl` — an append-only audit trail of every operation |
| **Mutation lock** | File at `~/.root/root.lockfile` — a process-level mutex preventing concurrent mutations |
| **Profile** | Nix profile at `~/.root/profiles/default` — an isolated Nix profile for Root-managed binaries |

### How It Works

Root manages an isolated Nix profile at `~/.root/profiles/default` — it never
touches your default Nix or Homebrew profiles.

Every `root install` and `root rollback` creates a snapshot first. Snapshots
contain the full deterministic lock state. The event ledger at
`~/.root/events.jsonl` records every operation. Verification checks binaries
from the Root-managed profile, not from PATH.

## Limitations (v0.2.4)

- **Curated catalog only.** Root supports a curated catalog only — 42 packages
  across eleven categories. Arbitrary `root install <anything>` is not yet
  supported. Unsupported packages are rejected with a clear categorized
  message.
- **`docker-client` installs the Docker CLI only**, not Docker Desktop or a
  Docker daemon. You need a separate Docker daemon to run containers.
- **Sandboxing requires an available Docker daemon.** Root fails with a
  capability error when Docker is unavailable and does not claim isolation.
- **Machine sharing is file-based.** Phase 6 supports local and Git-shared
  `Rootfile` and `root.lock` workflows; hosted multi-device sync is deferred.
- **`root run` is reproducible execution, not isolation.** Use `root sandbox run`
  when a disposable Docker container boundary is required.
- **Rollback applies only to Root-managed packages.** Root cannot undo
  changes made by Homebrew, manual installs, or other tools.
- **Nix must be installed.** Root manages a Nix profile but does not
  bundle Nix.
- **Mutation lock recovery.** If Root crashes during a mutation, the mutation
  lock (`~/.root/root.lockfile`) may need to be deleted manually to unblock
  future operations. Run `root doctor` first if you encounter lock errors.
- **Restore rollback is best-effort.** If a restore fails and recovery also
  fails, Root preserves the previous Rootfile and root.lock but the Nix profile
  may be in an inconsistent state. Run `root rollback --last` to attempt manual
  recovery.
- **Offline not supported.** Every install and update requires network access
  to resolve Nix flakes.
- **No concurrent operations.** Root uses a file-based mutation lock that
  prevents multiple simultaneous operations.
- **macOS is the primary platform.** macOS (Apple Silicon and Intel) is fully
  tested. Linux (aarch64 and x86_64) is supported by the codebase but not
  officially tested. Windows is not available.

## Experimental Commands

The CLI includes additional commands that are **not part of the v0.2.4 public
surface**. They may change, break, or be removed without notice:

| Command | Status |
|---------|--------|
| `root import brew` | Experimental — imports Homebrew packages into a Rootfile |

These exist for development and early testing. Do not rely on them for
production use.

## Roadmap

- **v0.2.x** — Harden the Phase 1–6 package, runtime, policy, sandbox, sync, and restore surface
- **v0.3** — Local model management with an Ollama-compatible adapter
- **Later** — AI-native manifests, residency policies, and explainable routing

See [Docs](Docs/) for the full plan.

## Safety

- Snapshots before every mutation
- Rollback by locked state — not by package name
- Nix profile isolation — no global PATH pollution
- Structured event ledger — every change is recorded
- Post-install and post-rollback profile verification
- Mutation lock prevents concurrent operations (with stale-PID recovery)
- Atomic writes prevent lockfile corruption on crash
- Snapshot content hashes are validated on read
- All Nix operations target `~/.root/profiles/default`, not the user profile

## Development

```bash
cargo build
cargo test --all
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

## License

Apache 2.0
