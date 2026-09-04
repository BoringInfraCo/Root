# Root v0.4.0

> A curated package manager for developer CLI tools, backed by Nix.

Root installs developer CLI tools through Nix, records what changed, and lets you
undo it — without needing to learn Nix.

[![CI](https://github.com/sgr0691/Root/actions/workflows/ci.yml/badge.svg)](https://github.com/sgr0691/Root/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## What v0.4.0 Changed

v0.4.0 is the **Portable Agent-Bundle** release.

- **`root agent-bundle`** — Explicit transfer of Codex or OpenCode working
  configuration between machines (`manifest.json` + content-addressed
  `blobs/`). This is **not** `root restore`, **not** Rootfile, and **not**
  `root.lock` integration. Lock schema is unchanged (package-only emit 2;
  max supported 3).
- **Codex S1** — exact version gate **0.150.1**. Never relaxed.
- **OpenCode S2** — exact version gate **1.18.27**. Never relaxed. JSONC
  comments and trailing commas are stripped before parse.
- **Lifecycle** — `inspect` (read-only), `export`, `plan` (plan hash, no
  writes), `apply` (`--apply` + `--plan-hash` + per-item `--approve`),
  `verify`, `enable-plan`, `enable`, `rollback --last` (byte-identical),
  `purge --yes`.
- **MCP is disabled until enable.** Export and apply write
  `enabled = false`. Enable needs namespaced provenance (`codex:id` /
  `opencode:id`), a current plan hash, per-item `--approve`, and env-var
  *presence*. Dummy tokens must never persist in config, journal, snapshots,
  or the bundle.
- **No credential or session transfer.** Known secret locations are
  excluded. Selected prompt/skill files are copied verbatim and may contain
  unrecognized secrets — review the bundle before transfer.
- **Hardening** — FIFO / symlink / non-regular blobs are rejected. Rollback
  restores the pre-mutation regular-file tree or refuses on drift.

## What v0.3.0 Changed

v0.3.0 is the **Pull-and-Verify** release. v0.2.6 was skipped.

- **`root plan models`** — Preview declared Ollama models. Plan never POSTs,
  never writes `root.lock`, and never creates `model-pull.json`.
- **`root models pull`** — Pull missing declared models by tag, compare the
  digest from `GET /api/tags`, and write a v3 verification record. JSON always
  reports `models_restored: false`. Weights are never deleted.
- **Lock schema v3** — Namespaced `models.<runtime>.<name>` object map.
  Package-only locks still emit schema 2. A non-empty models map emits 3.
  Schema 4+ is refused. The record's `addressability` is
  `verification_record_only`.
- **Status digest overlay** — `root status` compares the observed Ollama digest
  against the locked canonical `sha256:` digest. A mismatch is
  `model-digest-drift`. Root cannot pull by digest; a re-pull fetches the
  current tag, not the locked bits.
- **Restore and rollback honesty** — Package restore, dry-run, and rollback
  copy model lock entries when present. They do not pull or delete Ollama
  weights.
- **Rootfile is still tag + runtime only** — `[models."name"]` accepts
  `runtime = "ollama"` only. No digest field. No endpoint field.
- **Local Ollama only** — Root talks to `127.0.0.1:11434` with
  `GET /api/version`, `GET /api/tags`, and `POST /api/pull` (tag only). Other
  runtimes evaluate as unsupported.

The v3 models map is a verification record, not a bit-for-bit pin of weights.
`root restore` and `root rollback` remain package/Nix operations that may copy
that record. They do not pull models.

## What v0.2.5 Changed

v0.2.5 is the **Declared Environment Status** release:

- **Rootfile declarations** — Optional `[agents]` and `[models]` tables declare
  coding agents and Ollama-hosted models. v0.2.5 accepts agent `"*"` (presence
  only) and `runtime = "ollama"` for models.
- **Read-only inspection** — `root status` inspects declared agents (Codex,
  Claude Code, OpenCode, Pi) and Ollama models. It does not install agents,
  pull or delete models, or write them to `root.lock`.
- **Honest uncertainty** — Observations are `present`, `absent`, or `unknown`.
  Evaluations are `satisfied`, `missing`, `drifted`, `unknown`, or
  `unsupported`. A failed probe is never reported as missing.
- **Additive JSON** — Existing status fields keep their names and meanings.
  New results live under `inventory.agents` and `inventory.models`.
- **Lock safety** — Emitted locks remain schema v2. Locks with `version > 2`
  are refused before mutation; `root status` reports them without rewriting.
- **Compatibility** — Rootfiles that use `[agents]` or `[models]` require
  Root v0.2.5+ for mutating commands. v0.2.4 will drop those tables on rewrite.

`root sync`, `root restore`, and `root rollback` remain package/Nix operations.
A `present` agent or model is not a restoration claim.

## What v0.2.4 Changed

v0.2.4 is the **Restore Reliability** release:

- **Restore audit** — Full restore subsystem audit at `Docs/Restore/V0_2_4_RESTORE_AUDIT.md`.
- **Dry-run support** — `root restore root.lock --dry-run` shows the restore plan
  (install, remove, keep, update) without changing Rootfile, `root.lock`, or the
  Nix profile. It records a `RestorePlanned` event.
- **Pre-restore validation** — Lockfile schema, store paths, platform compatibility,
  Nix availability, and experimental features are all checked before any mutation.
  A missing Root profile is created during restore.
- **Partial failure recovery** — If restore fails mid-operation, Root automatically
  restores Rootfile, `root.lock`, and the Nix profile from the pre-restore snapshot.
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
  lock state, and the Root-managed profile, and inspects declared agents and
  models without mutating them.
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
| `root plan models [name]` | Preview declared Ollama model actions (no mutation) |
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
| `root models pull [name]` | Pull missing declared models by tag and write a v3 verification record |
| `root agent-bundle inspect --agent <codex or opencode>` | Read-only inspect of local Codex or OpenCode working config |
| `root agent-bundle export --agent <id> --out <dir>` | Export a versioned bundle (`manifest.json` + `blobs/`) |
| `root agent-bundle plan --bundle <dir>` | Preview apply (plan hash; no writes) |
| `root agent-bundle apply --bundle <dir> --apply --plan-hash <h> --approve <sha>` | Apply a reviewed bundle (MCP imported disabled) |
| `root agent-bundle verify --agent <id>` | Post-apply verification (read-only, secret-safe) |
| `root agent-bundle enable-plan --agent <id> --server <id>` | Preview enabling an imported MCP server |
| `root agent-bundle enable --agent <id> --server <id> --plan-hash <h> --approve <sha>` | Enable an imported MCP server (env must be present) |
| `root agent-bundle rollback --last` | Byte-identical rollback of the last agent-bundle snapshot |
| `root agent-bundle purge --yes` | Delete agent-bundle snapshots (requires `--yes`) |
| `root status` | Show machine identity, package drift, declared agent/model inspection, and locked digest overlay |
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
and tasks Root manages, plus optional `[agents]` (presence-only) and `[models]`
(tag + `runtime = "ollama"`) tables. `root models pull` can lock a declared
model as a v3 verification record. It is created automatically when you install
your first package.

```toml
[packages]
ripgrep = "latest"
ffmpeg  = "latest"
fd      = "latest"

[tasks]
build   = "cargo build --release"
test    = "cargo test --all"
lint    = "cargo clippy -- -D warnings"

[agents]
codex = "*"
claude = "*"
opencode = "*"
pi = "*"

[models."qwen3:8b"]
runtime = "ollama"

[settings]
snapshots     = true
verify_installs = true
```

`[agents]` are inspected by `root status` only. Root does not install those
agents. Agent values other than `"*"` are rejected. `root agent-bundle` is a
separate explicit transfer command (v0.4.0); it does not read Rootfile
`[agents]` and does not write `root.lock`. `[models]` accept
`runtime = "ollama"` only — no digest and no endpoint. `root status` inspects
them; `root plan models` previews; `root models pull` pulls the tag and writes
a v3 verification record in `root.lock`. A Rootfile that uses these tables must
be rewritten only by Root v0.2.5+. Model lock records require Root v0.3.0+.

### Sections

| Section | Required | Description |
|---------|----------|-------------|
| `[packages]` | No | Package name → version mappings (e.g., `ripgrep = "latest"`) |
| `[tasks]` | No | Task name → shell command mappings (e.g., `build = "cargo build"`) |
| `[agents]` | No | Agent name → `"*"` (presence-only). Inspected by `root status`; not installed |
| `[models]` | No | Model name → `{ runtime = "ollama" }` only. No digest or endpoint. Inspected by `root status`; pulled by tag with `root models pull` |
| `[settings]` | No | Global settings (`snapshots`, `verify_installs` — both default to `true`) |

Use `root list` to show installed packages, `root remove <package>` to uninstall,
and `root run <task-name>` to execute a task in the Root-managed environment.

## Key Concepts

| Concept | Description |
|---------|-------------|
| **Rootfile** | TOML file at `~/.root/Rootfile` — your intent (packages, tasks, and optional agent/model declarations) |
| **root.lock** | JSON file at `~/.root/root.lock` — pinned Nix package metadata (schema v2) plus optional v3 model verification records |
| **Snapshot** | JSON file at `~/.root/snapshots/` — a pre-mutation copy of the lock state for rollback |
| **Event ledger** | JSONL file at `~/.root/events.jsonl` — an append-only audit trail of every operation |
| **Mutation lock** | File at `~/.root/root.lockfile` — a process-level mutex preventing concurrent mutations |
| **Profile** | Nix profile at `~/.root/profiles/default` — an isolated Nix profile for Root-managed binaries |
| **agent-bundle** | Explicit Codex/OpenCode working-config transfer (`root agent-bundle`). Not `root restore`, not Rootfile, not `root.lock`. |

### How It Works

Root manages an isolated Nix profile at `~/.root/profiles/default` — it never
touches your default Nix or Homebrew profiles.

Every `root install` and `root rollback` creates a snapshot first. Snapshots
contain the full deterministic lock state. The event ledger at
`~/.root/events.jsonl` records every operation. Verification checks binaries
from the Root-managed profile, not from PATH.

## Limitations (v0.4.0)

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
- **Agents are inspected, not installed.** `root status` can report presence
  for declared Codex, Claude Code, OpenCode, and Pi installs. Root does not
  install agents or migrate credentials.
- **`root agent-bundle` is explicit transfer only.** It is not `root restore`
  and does not read Rootfile `[agents]` or write `root.lock`. Same-agent only
  (Codex → Codex, OpenCode → OpenCode); no cross-agent translation.
- **MCP stays disabled until enable.** Apply imports MCP declarations with
  `enabled = false`. Enable requires namespaced provenance (`codex:id` /
  `opencode:id`), a current plan hash, per-item `--approve`, and env-var
  presence. Dummy tokens must never persist in config, journal, snapshots, or
  the bundle.
- **Exact agent version gates.** Codex **0.150.1** and OpenCode **1.18.27**
  only. Other versions are refused. The gates are never relaxed in v0.4.0.
- **No credential or session transfer.** Bundles exclude known secret
  locations (`auth.json`, sessions, sqlite, `mcp-auth.json`). Selected
  prompt/skill files are copied verbatim and may contain unrecognized secrets.
- **Models are pull-and-verify, not a bit-for-bit pin.** `root models pull`
  pulls a declared tag and writes a v3 verification record. The lock does not
  make weights digest-addressable. `root restore` / `root rollback` / `root sync`
  copy model lock entries when present; they do not pull or delete Ollama
  weights. There is no `--relock`.
- **Root cannot pull by digest.** A `model-digest-drift` finding means the
  current tag no longer matches the locked digest. Re-pull fetches the current
  tag, not the locked bits. Live Ollama `@sha256` pull is not a Root product
  path.
- **Mixed-version Rootfile edits.** A Rootfile that contains `[agents]` or
  `[models]` must be rewritten only by Root v0.2.5+. v0.2.4 parses those
  tables and then drops them on any mutating rewrite. v3 model lock records
  require Root v0.3.0+.
- **Ollama is local and protocol-specific.** Root queries
  `http://127.0.0.1:11434/api/version` and `/api/tags`, and pulls with
  `POST /api/pull` by tag. Rootfile has no endpoint field. Other runtimes
  evaluate as `unsupported`. An endpoint that answers but lacks that contract
  evaluates as `unsupported` with `protocol_unsupported`.
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
  to resolve Nix flakes. Model pull requires a reachable local Ollama daemon.
- **No concurrent operations.** Root uses a file-based mutation lock that
  prevents multiple simultaneous operations. Model pull also takes an exclusive
  `model-pull.json` marker.
- **macOS is the primary platform.** macOS (Apple Silicon and Intel) is fully
  tested. Linux (aarch64 and x86_64) is supported by the codebase but not
  officially tested. The Ollama pull-and-verify backend was not smoke-tested
  on Linux in v0.3.0. Codex and OpenCode `root agent-bundle` Linux transfer
  was recorded for v0.4.0 (exact gates 0.150.1 and 1.18.27); it is not
  `root restore`. Windows is not available.

## Experimental Commands

The CLI includes additional commands that are **not part of the v0.4.0 public
surface**. They may change, break, or be removed without notice:

| Command | Status |
|---------|--------|
| `root import brew` | Experimental — imports Homebrew packages into a Rootfile |

These exist for development and early testing. Do not rely on them for
production use.

## Roadmap

- **v0.4.x** — Harden explicit `root agent-bundle` transfer (Codex / OpenCode)
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
