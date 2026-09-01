# Changelog

All notable changes to Root are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.5] - 2026-09-01

### Added

- **Declared environment status.** Optional Rootfile `[agents]` and `[models]` tables are inspected by `root status` as `present | absent | unknown` and evaluated as `satisfied | missing | drifted | unknown | unsupported`. Supported agents: Codex, Claude Code, OpenCode, and Pi. Supported model runtime: Ollama on `127.0.0.1:11434` via `GET /api/version` and `GET /api/tags`.
- **Typed inventory JSON.** `StatusReport` gains an additive `inventory` object with `agents` and `models` arrays. Namespaced drift categories include `agent-missing`, `agent-observation-unknown`, `agent-not-supported-by-this-release`, `model-missing`, `model-observation-unknown`, `model-runtime-not-supported-by-this-release`, and `model-runtime-protocol-unsupported`.
- **Future lock schema guard.** Locks with `version > 2` are refused before install, update, remove, lock, sync, restore, restore dry-run, and rollback. `root status` reports them as `NeedsAttention` without rewriting.

### Changed

- README updated for v0.2.5.
- `root status` human output adds `Agents` and `Models` sections when declarations are present. Existing package headings and JSON fields are unchanged.
- Empty `[agents]` / `[models]` tables are omitted when serializing Rootfile so package-only rewrites do not introduce new sections.

### Fixed

- Future `root.lock` schema versions can no longer be parsed and rewritten as v2 package locks by mutating commands.

### Tests Added

- Rootfile inventory round-trip, empty-section omission, constraint rejection, and control-character validation.
- Future lock version peek/guard tests in `root-lockfile`.
- Agent/model observation fixtures (present, absent, unknown, unsupported, protocol mismatch, sanitization, Codex/Claude/OpenCode/Pi flags, Ollama HTTP).
- Status JSON legacy-field contract, inventory aggregation, rewrite-path preservation, and mutation refusal for version-3 locks.

## [0.2.4] - 2026-06-24

### Added

- **Restore audit.** Full restore subsystem audit at Docs/Restore/V0_2_4_RESTORE_AUDIT.md covering entry points, validation flow, Nix operations, mutation flow, event recording, rollback/recovery, drift detection, 13 failure modes, and 10 gaps. (Phase 1)
- **Dry-run support.** `root restore root.lock --dry-run` reports the restore plan (will install, remove, keep, update) without mutating the Rootfile, root.lock, or Nix profile. It does append a `RestorePlanned` event to the event ledger. Supports human and JSON output. (Phase 2)
- **Pre-restore validation.** Lockfile schema, store paths, platform compatibility, Nix availability, and experimental features (nix-command, flakes) are validated before any mutation. `.drv` paths in outputs are rejected with clear errors. A missing Root profile is allowed; restore creates it. (Phase 3)
- **Partial failure recovery.** If restore fails mid-operation, Root captures a pre-restore snapshot and restores Rootfile, root.lock, and the Nix profile from that snapshot. If recovery fails, clear instructions are provided for manual rollback. (Phase 4)
- **Strengthened drift detection.** `root status` now detects missing output paths per package, `.drv` paths in lockfiles, and platform mismatches in addition to existing name-based drift checks. (Phase 5)
- **Restore event ledger.** New `RestorePlanned` and `RestoreRecovered` event types, new `Planned` event status, and event fields for `failure_phase`, `installed_count`, `removed_count`, `kept_count`. Restore operations record detailed events at every stage. (Phase 6)
- **Restore error normalization.** Clear, actionable error messages for all restore failure modes: invalid lockfile, incompatible platform, missing Nix, missing experimental features, `.drv` output paths, profile validation failure, partial restore failure, failed recovery, stale mutation lock, and permission denied. (Phase 7)
- **Restore smoke tests.** New smoke test document at Docs/Release/V0_2_4_RESTORE_SMOKE_TEST.md covering clean restore, dry-run, invalid lockfile, partial failure, and drift detection scenarios. (Phase 8)
- **Restore documentation.** New reference document at Docs/Restore/V0_2_4_RESTORE_NOTES.md. (Phase 9)

### Changed

- README updated for v0.2.4.
- `RootEventType` gains `RestorePlanned` and `RestoreRecovered` variants.
- `RootEventStatus` gains `Planned` variant.
- `RootEvent` gains `failure_phase`, `installed_count`, `removed_count`, `kept_count` fields.
- `RestoreReport` renamed conceptually — restore output now includes automatic rollback reporting on failure.
- Validation failures now record a `Restore` / `Failed` event before returning.

### Fixed

- Restore no longer requires an existing Nix profile; a missing `~/.root/profiles/default` is created by `nix profile add` during restore.
- Automatic restore recovery now rewrites `root.lock` and Rootfile from the pre-restore snapshot, not only the Nix profile.
- `root status` marks `platform-mismatch` as unhealthy (`NeedsAttention`).

### Tests Added

- `test_restore_partial_failure_rolls_back_profile` — mid-restore install failure automatically rolls back the Nix profile and preserves Rootfile/`root.lock`.
- `test_restore_with_no_existing_lockfile` — restore from a shared lock when no local `root.lock` exists.
- `test_restore_from_v1_lockfile` — v1 lock fallback via `to_v2()`.
- `test_restore_recovers_stale_mutation_lock` — dead-PID `root.lockfile` is recovered and restore proceeds.
- `test_restore_blocked_by_live_mutation_lock` — live mutation lock blocks restore.
- `test_restore_creates_missing_profile` — restore proceeds and installs when the Root profile does not exist yet.
- `test_restore_rollback_restores_lock_and_rootfile` — auto-rollback rewrites Rootfile and `root.lock` from the snapshot.

## [0.2.3] - 2026-06-24

### Added

- **Sandbox lifecycle validation.** Sandboxes follow a strict state machine (Created → Running → Completed/Failed → Destroyed). Invalid transitions are rejected with clear errors. (Phase 2)
- **Cleanup guarantees.** Destroy always attempts cleanup. Failed and timed-out runs trigger automatic cleanup. Stale sandboxes detectable via `root sandbox list`. (Phase 3)
- **Resource limits.** `root sandbox create` accepts `--memory` (default 2g) and `--cpus` (default 2.0). Docker containers are created with these limits. (Phase 4)
- **Timeout handling.** `root sandbox run` accepts `--timeout` (default 300s). Timed-out runs are killed, cleaned up, and recorded in the event ledger. (Phase 5)
- **Sandbox validation.** Post-create validation verifies container exists and is reachable. Post-destroy validation verifies container is removed. (Phase 6)
- **Event ledger integration.** Every sandbox action (create, run, timeout, failure, destroy, cleanup) is recorded with sandbox ID, timestamp, and result. (Phase 7)
- **Sandbox error normalization.** Clear, actionable messages for Docker unavailable, image pull failure, container startup failure, timeout, resource limit exceeded, permission denied, and cleanup failure. (Phase 8)
- **Sandbox audit.** Full subsystem audit at Docs/Sandbox/V0_2_3_SANDBOX_AUDIT.md. (Phase 1)
- **Sandbox smoke tests.** New smoke test document at Docs/Release/V0_2_3_SANDBOX_SMOKE_TEST.md. (Phase 9)
- **Sandbox documentation.** New reference document at Docs/Sandbox/V0_2_3_SANDBOX_NOTES.md. (Phase 10)
- **30 new tests** covering lifecycle validation, cleanup, resource limits, timeout, validation, event recording, and error normalization (38 total in root-sandbox).

### Changed

- README updated for v0.2.3.
- SandboxProvider trait updated with `create(memory, cpus)`, `run_command(timeout)`, `check_exists`, `check_reachable`.
- SandboxInstance uses typed `SandboxState` enum instead of string status.
- RootEvent gains `sandbox_id` field for sandbox operation tracking.

### Fixed

- Sandbox state transitions now validated — running a destroyed sandbox is rejected early.
- Docker errors normalized into user-friendly messages.
- Containers are validated after create and destroyed on validation failure.

## [0.2.2] - 2026-06-23

### Added

- **Nix command audit.** Comprehensive catalog of all 12 nix subcommands Root uses, their expected outputs, failure modes, and error-handling gaps. Docs/Nix/V0_2_2_NIX_COMMAND_AUDIT.md. (Phase 1)
- **Experimental feature probe.** `root doctor` now detects when `nix-command` or `flakes` are disabled and explains how to enable them. (Phase 2)
- **Profile generation validation.** After every mutation (install, update, rollback, restore), Root validates the Nix profile generation changed and expected output paths are present. (Phase 3)
- **Store path hardening.** Derivation paths (.drv) are strictly separated from output paths. Lockfile and snapshot validation rejects .drv paths in output fields before any mutation. (Phase 4)
- **Error normalization.** All Nix failure modes produce clear, actionable messages without leaking raw Nix output. Covers 12+ failure modes. (Phase 5)
- **Installer validation.** `root init --install-nix` now explains what will happen, requires explicit confirmation, detects platform, and runs post-install probe. (Phase 6)
- **Nix reliability smoke tests.** New smoke test document at Docs/Release/V0_2_2_NIX_RELIABILITY_SMOKE_TEST.md. (Phase 7)
- **Nix reliability notes.** New reference document at Docs/Nix/V0_2_2_NIX_RELIABILITY_NOTES.md. (Phase 8)
- **24+ new tests** covering experimental feature detection, profile validation, store path validation, error normalization, and installer validation.

### Changed

- README updated for v0.2.2.
- All Nix error handling produces normalized user-facing messages.

### Fixed

- `.drv` paths in lockfile output fields now rejected early with clear error.
- Missing experimental features produce clear diagnostic instead of confusing Nix errors.
- Installer explains actions before running and validates post-install state.

## [0.2.1] - 2026-06-22

### Performance

- **Search**: Query is lowercased once instead of per-package (42×). `SearchMatch` and `CatalogEntry` use `&'static [&'static str]` for aliases and binaries, eliminating per-result heap allocations. (Phase 2)
- **Lockfile**: Content-aware write — `save_lock_v2` and `save_lock` compare serialized output to existing file and skip the write if unchanged. Zero disk I/O when no changes occurred. (Phase 3)
- **build_v2_lock**: Refactored to accept `&RootLockV2` directly, eliminating wasteful v2→v1→v2 conversion cycle in `install`, `update`, and `lock`. (Phase 3)
- **Event ledger**: `root history --limit N` added. `read_events_with_limit(limit)` bounds in-memory event retention to N entries using a fixed-size rolling buffer, so large ledgers never consume more than O(N) memory. (Phase 4)
- **Status**: Nix profile check is skipped when Rootfile and lockfile both have zero packages. Status is entirely local-only for empty states. (Phase 5)

### Memory

- `SearchMatch` aliases and binaries fields changed from `Vec<String>` to `&'static [&'static str]` (zero allocation).
- `CatalogEntry` aliases and binaries fields changed from `Vec<String>` to `&'static [&'static str]`.
- `SearchMatch.matched_fields` changed from `Vec<String>` to `Vec<&'static str>`.
- Removed dead code: `legacy_lock_from_v2`, `legacy_package_from_v2`.

### Reliability

- Malformed event lines in `events.jsonl` are now gracefully skipped instead of potentially failing history.
- `RootLockV2` now derives `Default` for consistent construction patterns.
- Status command handles missing Rootfile, missing lockfile, unavailable Nix, and missing profile without panicking.
- `RootLock::write_to_file` and `RootLockV2::write_to_file` handle existing files gracefully.

### Tests Added

24 new tests covering:

| Test | Phase |
|------|-------|
| `test_search_output_format_preserved` | 2 |
| `test_search_aliases_resolve_correctly` | 2 |
| `test_search_category_works` | 2 |
| `test_search_description_works` | 2 |
| `test_lockfile_unchanged_does_not_rewrite` | 3 |
| `test_lockfile_parse_v2_compatibility` | 3 |
| `test_history_with_limit_returns_bounded_events` | 4 |
| `test_history_handles_malformed_events_gracefully` | 4 |
| `test_history_events_ordered_recent_first` | 4 |
| `test_status_with_missing_rootfile_and_lock` | 5 |
| `test_status_missing_profile_no_panic` | 5 |
| `test_search_does_not_call_nix` | 6 |
| `test_catalog_does_not_call_nix` | 6 |
| `test_history_does_not_call_nix` | 6 |
| `test_status_does_not_call_nix_for_empty_state` | 6 |
| `test_plan_rejects_unsupported_before_nix` | 6 |

## [0.2.0] - 2026-06-10

### Added

- `root search <query>` across curated package names, aliases, categories,
  descriptions, binaries, and Nix attributes.
- `root update [package]` with deterministic re-resolution, pre-mutation
  snapshots, profile verification, lock updates, and history records.
- v2-compatible `root sync` and `root restore [--lock <path>]` for local and
  Git-shared machine restoration workflows.
- `root run <task|workflow-file|-- command...>` and `[tasks]` Rootfile support,
  with Root-profile-first PATH handling and structured execution history.
- `root permissions` and `root policy apply <file>` with package, command,
  sandbox, resource, and agent-approval rules.
- Docker-backed `root sandbox create`, `run`, `list`, and `destroy` commands
  behind a `SandboxProvider` abstraction and mock provider tests.
- `root status` machine identity and drift reporting across Rootfile, lockfile,
  and Root-managed profile state.
- Policy, execution, sandbox, restore, and update event metadata in history.

### Changed

- Workspace version advanced to 0.2.0 for the first release of Roadmap Phases
  1–6.
- Current v0.1 commands remain backward compatible while the supported public
  CLI surface expands.
- Policy denials occur before snapshots or Root-managed machine mutations.

### Fixed

- `root sync` now handles current v2 lockfiles instead of rejecting them.
- Sandbox policy actions use explicit create, run, and destroy permissions
  rather than package-sync policy settings.
- `root status` no longer reports a healthy machine when the Root-managed Nix
  profile cannot be inspected; it reports `NeedsAttention` with a doctor hint.

## [0.1.9] - 2026-06-08

### Added

- **Live install validation matrix.** `Docs/Release/V0_1_9_INSTALL_VALIDATION.md` with
  12 representative packages and cross-cutting checks.
- **New smoke test document.** `Docs/Release/V0_1_9_SMOKE_TEST.md` covering 12 test
  paths (fresh machine, missing Nix, catalog, install, verify, history, rollback,
  alias, lockfile, legacy detection, and Linux compatibility).
- **Linux compatibility investigation.** `Docs/Platform/Linux_Compatibility.md`
  documenting what works today, what is macOS-specific, and what would need to change.
- **Verification overrides for non-standard tools.** Added correct arguments for
  `go version`, `terraform version`, `kubectl version --client`, `helm version --short`,
  `tmux -V`, and `direnv version`.
- **Default binary metadata for all 42 packages.** The `package_default_binaries` table
  now covers every supported package, ensuring verification works even without explicit
  lockfile binary metadata.
- **Rollback event tests.** `test_rollback_event_recorded_on_success` and
  `test_rollback_failure_preserves_lockfile_and_rootfile` verify rollback correctness.
- **Verification tests.** `test_verify_missing_profile_binary_fails_even_if_global_exists`,
  `test_verify_multi_binary_package_reports_each_binary`, and
  `test_verify_non_standard_args_are_correct` harden verification coverage.
- **Nix error normalization for flakes/profile issues.** Better error messages when
  experimental features are missing or profile symlinks conflict.

### Changed

- **Verification no longer falls back to global PATH.** `resolve_binary_path` now
  checks only the Root-managed profile paths (`~/.root/profiles/default/bin`). If a
  binary is missing from the profile, verification fails — even if the binary exists
  elsewhere on PATH.
- **Doctor onboarding messages improved.** Missing-Nix description now explains why
  Root uses Nix. Experimental-features detection suggests how to enable them.
- **Init output improved.** Clearer next-step instructions explaining Nix's role.
- **Version bumped to 0.1.9** across workspace.

### Fixed

- Verification could silently pass against a global PATH binary while the Root profile
  binary was missing. Now fails with a clear "not found in Root profile" error.
- Missing `go`, `terraform`, `kubectl`, `helm`, `tmux`, `direnv` verification overrides
  caused generic strategy fallback (e.g., `--version` instead of `version`).
- Nix error normalization did not handle "experimental feature not enabled" or profile
  symlink conflict errors.
- Doctor Nix availability error branch did not distinguish experimental-features errors
  from other failures.

## [0.1.8] - 2026-06-06

### Added

- **Developer productivity tools.** From 37 to 42 curated packages. New
  category: `git`. New packages: git-delta, zoxide, direnv, starship, lazygit.
- **New aliases.** `delta` → git-delta, `z` → zoxide, `lg` → lazygit.
- **Alias regression tests.** Plan and install tests for delta, z, and lg
  aliases, verifying canonical name storage in the lockfile.
- **Category expansion tests.** Error message category listing test now
  covers all eleven categories.

### Changed

- **README updated.** Expanded package table with new `git` category, new
  terminal packages (zoxide, direnv, starship), v0.1.8 changelog section,
  updated limitations.
- **CHANGELOG.md** — this entry.
- **Smoke test docs updated.** Added manual tests for new packages and
  aliases.

## [0.1.7] - 2026-06-06

### Added

- **Package catalog expansion.** From 24 to 37 curated packages across ten
  categories. New categories: `language`, `database`, `infrastructure`,
  `security`, `editor`, `terminal`. New packages: go, rustup, postgresql,
  redis, terraform, kubectl, helm, k9s, docker-client, age, sops, neovim, tmux.
- **New aliases.** `golang` → go, `postgres` → postgresql, `tf` → terraform,
  `kube` → kubectl, `docker` → docker-client, `nvim` → neovim.
- **Verification coverage improvements.** Package-specific verify commands
  for go (`go version`), terraform (`terraform version`), kubectl
  (`kubectl version --client`), helm (`helm version --short`), and
  tmux (`tmux -V`).
- **Alias regression tests.** Plan and install tests for every new alias,
  verifying canonical name storage in the lockfile.
- **Category expansion tests.** Error message category listing test now
  covers all ten categories.

### Changed

- **README updated.** Expanded package table with six new categories,
  v0.1.7 changelog section, updated limitations.
- **CHANGELOG.md** — this entry.
- **Smoke test docs updated.** Added manual tests for new packages and
  aliases.

## [0.1.6] - 2026-06-06

### Fixed

- **`.drv` path leak in output verification.** `nix build --no-link --print-out-paths --json`
  returns both drv paths and output paths. The `.drv` path was being assigned as the `"out"`
  output, causing verification to fail with `Installed profile did not contain locked Nix store
  path ... .drv`. Now `.drv` paths are filtered out during extraction, and guards reject them at
  every layer.
- **Install script auto-elevation.** `curl ... | sh` now automatically uses `sudo` for the
  install step when needed.
- **Early rejection of `.drv` output paths.** If a resolved package only has a `.drv` path,
  Root fails with a clear internal error instead of a misleading profile-verification failure.

### Added

- **Verification guard.** `verify_profile_contains_outputs` rejects `.drv` paths before
  checking the profile, with a clear error message.

## [0.1.5] - 2026-06-05

### Fixed

- **`nix profile install` deprecated in newer Nix.** Migrated to
  `nix profile add` in both `install()` and `install_installable()`.
  Nix 2.24+ emits a deprecation warning for `install` and some versions
  reject it outright.
- **Profile path conflict with Nix symlink management.** `init_root_dir()`
  previously created `~/.root/profiles/default` as a plain directory, but
  Nix's `--profile` flag manages that path as a symlink. This caused
  `error: reading symbolic link ".../default": Invalid argument`. The
  directory is no longer pre-created; broken symlinks and empty
  directories at that path are cleaned up so Nix can manage it.
- **Doctor false negatives on profile path.** The doctor's `exists()` /
  `is_dir()` checks did not account for the profile path being a valid
  symlink (the normal Nix-managed state). Now uses `symlink_metadata()`
  to accept either a symlink or a directory.

## [0.1.4] - 2026-06-05

### Fixed

- **Install script SHA256 verification.** The computed hash included a
  trailing `-` (stdin indicator) because `sha256sum` output was not piped
  through `awk`. Both `sha256sum` and `shasum` are now grouped with parens
  so the pipe always applies.
- **`nix-command` and `flakes` experimental features.** All `nix` CLI
  invocations now automatically pass
  `--extra-experimental-features nix-command flakes`, so Root works on
  fresh Nix installations without manual `nix.conf` configuration.

## [0.1.3] - 2026-06-05

### Added

- **Expanded curated package catalog.** From 4 to 24 packages across four
  categories (`media`, `search`, `dev`, `net`). New packages: fd, bat, eza,
  fzf, git-lfs, gh, httpie, just, tree, sqlite, imagemagick, wget, curl,
  gnumake, pkg-config, openssl, python3, nodejs, bun, uv.
- **Rich `PackageSpec` metadata structure.** Each package now defines
  aliases, Nix attribute, expected binaries, per-binary verification
  commands, category, and description. The catalog lives in a single
  `SUPPORTED_PACKAGES` const slice that is easy to extend.
- **`root catalog` command.** Lists all supported packages grouped by
  category. Supports `--json` for structured output.
- **Better `root plan install`.** The plan command now shows a complete
  step-by-step preview including supported package check, Nix metadata
  resolution, snapshot creation, lockfile update, history event recording,
  and rollback availability. Unsupported packages are rejected before any
  Nix calls.
- **Categorized unsupported-package errors.** The error message for
  unsupported packages now groups packages by category, helping users
  discover alternatives.
- **User-friendly error messages.** Custom error formatter wraps common
  failure modes (Nix missing, package not found, platform missing, stale
  lockfile, rollback unavailable) with clear next-step instructions. Raw
  Nix output is not dumped unless `--json` is used.
- **Verification coverage for all packages.** Every supported package has
  at least one verification command. `root verify <pkg>` checks binaries
  from the Root-managed profile binary path, not the user's global PATH.
  Added `openssl` verification override (uses `version` instead of
  `--version`).
- **Package catalog tests.** Validates: unique names, non-empty Nix
  attributes, at least one binary per package, at least one verify command
  per package, verify binary matches expected binaries, aliases don't
  collide with package names, unsupported packages rejected before Nix
  calls, catalog output includes all packages, resolve-by-alias works.
- **Alias resolution.** `resolve_package` now matches both canonical names
  and aliases (e.g., `rg` resolves to `ripgrep`, `node` to `nodejs`).
- **CHANGELOG.md** — this file.
- **Release smoke test docs.** See `Docs/Release/V0_1_3_SMOKE_TEST.md`.

### Changed

- **README updated.** Full supported package table, "Why curated packages
  first?" explanation, "Try Root in 60 seconds" section, and example flow
  with `root catalog`.
- **`root plan install` output.** Changed `verify_args` to `verify_commands`
  showing per-binary verification (e.g., `ffmpeg -version`).
- **Init/doctor/history output.** Updated to reference `root catalog`
  instead of listing packages inline.
- **Doctor suggestions improved.** Error and warning messages now point to
  concrete first commands (`root init --install-nix`, `root install ffmpeg`).
- **Alias canonicalization.** `root install rg` now correctly installs
  `ripgrep`, `root install node` installs `nodejs`, etc. The lockfile
  stores the canonical name and preserves the original input as
  `requested`.
- **Poppler verification.** Changed from `-h` to `-v` to match actual binary
  behavior.

## [0.1.2] - 2026-06-01

### Added

- **Deterministic Nix metadata resolution.** `nix build --print-out-paths`,
  `nix eval`, and `nix flake metadata --json` capture real Nix store paths,
  package versions, and pinned nixpkgs revision. "latest" is never written
  to root.lock.
- **RootLockV2 schema.** New JSON format with `installable`, `drv_path`,
  `store_paths`, `outputs`, `meta`, and `content_hash` fields.
- **Snapshot v2.** Snapshots store the full RootLockV2 state, enabling
  rollback by locked installable rather than by package name.
- **Rollback by locked state.** `root rollback --last` uses saved
  installables (e.g., `github:NixOS/nixpkgs/<rev>#<attr>`) instead of
  resolving `nixpkgs#<pkg>`.
- **Legacy detection.** `root doctor` detects v1 locks, "latest" versions,
  placeholder store paths, and unknown nixpkgs revisions.
- **Nix metadata verification.** Post-install and post-rollback checks
  that profile store paths match locked store paths.
- **Event recording.** Operations are recorded to `~/.root/events.jsonl`
  with type, status, package, and snapshot IDs.

### Changed

- **Lockfile format.** v1 locks are migrated to v2 on write. The `lock`
  subcommand regenerates deterministic metadata for all Rootfile entries.
- **`sync` refuses v2 lockfiles.** v0.1.2 manages profile state
  automatically during install and rollback; `sync` is deprecated.
- **Test infrastructure.** Deterministic mock Nix adapter produces stable
  store paths, nar hashes, and package versions.

### Fixed

- Placeholder store paths are never written to root.lock.
- Rollback verifies profile store paths match locked paths.
- Nix error normalization catches "attribute missing from derivation" and
  "no outputs found" cases.

## Known Limitations

- Curated catalog only (42 packages). Arbitrary `root install <anything>`
  is not yet supported. Unsupported packages are rejected with a clear
  categorized message.
- `docker-client` installs the Docker CLI only, not Docker Desktop or a
  Docker daemon. A separate daemon is needed to run containers.
- Rollback applies only to Root-managed packages. Cannot undo Homebrew or
  manual changes.
- Nix must be pre-installed or installed via `root init --install-nix`.
- Stale lockfile (`~/.root/root.lockfile`) must be deleted manually if Root
  crashes during a mutation.
- macOS only (Apple Silicon and Intel). Linux is detected but not officially
  supported.

## Upgrade Notes

### Upgrading from 0.1.1 to 0.1.2

- Existing v1 lockfiles are automatically migrated to v2 on the next
  `root install` or `root lock` command.
- After upgrading, run `root lock` to regenerate deterministic metadata
  for all packages in Rootfile.
- `root sync` no longer works with v2 lockfiles. Use `root install` and
  `root rollback` instead.

### Upgrading from 0.1.8 to 0.1.9

- No breaking changes. Existing v2 lockfiles, snapshots, and events are
  fully compatible.
- Verification now requires binaries in `~/.root/profiles/default/bin`. If
  you previously relied on global PATH fallback, ensure your Root profile
  path is in `$PATH` before your system paths.
- Non-standard tool verification commands are now correct: `go version`,
  `terraform version`, `kubectl version --client`, `helm version --short`,
  `tmux -V`, `direnv version`.

### Upgrading from 0.1.7 to 0.1.8

- No breaking changes. Existing v2 lockfiles, snapshots, and events are
  fully compatible.
- The curated catalog expanded from 37 to 42 packages. New category: `git`.
  New aliases: `delta`, `z`, `lg`.

### Upgrading from 0.1.6 to 0.1.7

- No breaking changes. Existing v2 lockfiles, snapshots, and events are
  fully compatible.
- The curated catalog expanded from 24 to 37 packages with six new
  categories. Run `root catalog` to browse the full list.
- New aliases: `golang`, `postgres`, `tf`, `kube`, `docker`, `nvim`.

### Upgrading from 0.1.2 to 0.1.3

- No breaking changes. Existing v2 lockfiles, snapshots, and events are
  fully compatible.
- The curated catalog expanded from 4 to 24 packages. Run `root catalog`
  to browse the full list.
- Error messages are now user-friendly. Use `--json` to see raw error
  details if needed.
