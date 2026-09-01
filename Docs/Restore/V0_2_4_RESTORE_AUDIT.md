# Restore Audit — Root v0.2.4

## 0.2.4 closeout

v0.2.4 shipped restore reliability around the existing `reconcile_profile_to_lock()` engine. This audit describes that **current** implementation, not the pre-0.2.4 snapshot.

**Shipped in 0.2.4**

| Item | Where |
|------|--------|
| `--dry-run` (plan only; no Rootfile / `root.lock` / Nix profile mutation) | CLI `Commands::Restore { dry_run }`; `restore_dry_run()` |
| Pre-mutation `restore_validate()` (store paths, Nix, experimental features, profile, platform, `.drv` outputs) | `restore()` and `restore_dry_run()` |
| Best-effort automatic rollback of the Nix profile after mid-restore failure | `attempt_rollback_to_snapshot()` from `restore()` error path |
| `RestorePlanned` / `RestoreRecovered` event types, `Planned` status | `crates/root-core/src/events.rs` |
| `failure_phase` inferred into Failed-event **messages** and user-facing errors | `infer_restore_failure_phase()` |
| `failure_phase`, `installed_count`, `removed_count`, `kept_count` fields on `RootEvent` | `events.rs` (struct fields exist; `record_event` currently leaves them `None`) |
| Status drift: missing outputs, `.drv` in lock, platform mismatch | `status()` |

**Still deferred / inherent** (not claimed as 0.2.4 fixes)

- Non-atomic Nix mutations vs lockfile/Rootfile writes
- No atomicity across multiple `nix profile` invocations
- v1 lock fallback can produce unpinned installables
- No timeout for Nix during restore
- No restore directly from a snapshot file (lock path only)
- Snapshot dedup/retention
- Policy still at entry points only (`reconcile_profile_to_lock` is shared with `sync`)
- Mutation lock still needs manual delete after unreadable lock / some crash cases
- Event recording still uses `let _ =` on most restore paths (swallows write failures)
- Dry-run records `RestorePlanned` (append-only `events.jsonl`) even though it does not mutate Rootfile, `root.lock`, or the Nix profile
- Crash between Nix ops and file writes is not auto-recovered (snapshot + rollback is the recovery)

If automatic rollback itself fails, the user runs `root rollback --last`.

---

## 1. Restore Entry Points

### CLI: `Commands::Restore`

**File:** `crates/root-cli/src/main.rs:95-102`

```rust
Restore {
    #[arg(long, value_name = "PATH")]
    lock: Option<std::path::PathBuf>,
    #[arg(long)]
    dry_run: bool,
}
```

Accepts optional `--lock <PATH>` (defaults to `~/.root/root.lock`) and `--dry-run`.

Dispatched at `main.rs:901-946`:

- `--dry-run` → `root_core::restore_dry_run(&adapter, lock.as_deref())`
- otherwise → `root_core::restore(&adapter, lock.as_deref())`

Output is formatted via `handle_structured` (JSON or human-readable).

- **Restore:** lists `Installed`, `Removed`, `Unchanged`, and `Snapshot saved`.
- **Dry-run:** lists `Will install`, `Will remove`, `Will keep`, `Will update`, or `No changes needed`.

CLI parse coverage: `parses_phase_one_commands` (`main.rs:1164`) verifies `--lock ./root.lock`.

### Core entry point: `pub fn restore()`

**File:** `crates/root-core/src/lib.rs:3001-3113`

```
fn restore(adapter: &impl NixAdapter, lock_path: Option<&Path>) -> Result<RestoreReport>
```

Steps in order:

1. `root_lockfile::init_root_dir()` — ensures `~/.root` tree exists
2. Resolve lock path (`selected_lock_path`): provided path or default `~/.root/root.lock`
3. Read lockfile: `RootLockV2::read_from_file()` with `.or_else(|_| RootLock::read_from_file(...).map(|lock| lock.to_v2()))`
4. `restore_validate(adapter, &target_lock, &selected_lock_path)` — reject invalid lockfiles, missing Nix, missing experimental features, missing profile, platform mismatch, and `.drv` outputs **before** any mutation
5. `enforce_policy(PolicyAction::Restore, None)` — check policy allows restore at all
6. Per-package policy check: `enforce_policy(PolicyAction::Restore, Some(&package.name))`
7. `MutationGuard::acquire()` — acquire the mutation lockfile (PID-based)
8. Capture a **pre-restore snapshot** of the current lock (`get_or_create_lock_v2` + `Snapshot::create_from_v2`). Creation is best-effort (`.ok()`); if it fails, later auto-rollback is skipped
9. `reconcile_profile_to_lock(adapter, &target_lock, ...)` — the core mutation logic (takes its own snapshot as well)
10. On success: return `RestoreReport { success, lock_path, installed, removed, unchanged, snapshot_id }`
11. On error:
    - `infer_restore_failure_phase(&e)`
    - record `Restore` / `Failed` (phase + error in the message)
    - `attempt_rollback_to_snapshot(adapter, snapshot)` when a pre-restore snapshot exists
    - record `RestoreRecovered` / `Completed` or `Failed`
    - return a user-facing error that says Rootfile/`root.lock` were preserved and either that the profile was rolled back, or to run `root rollback --last`

Validation / policy failures in steps 4–6 return via `?` **before** the reconcile `match`, so they do not take the auto-rollback path (nothing has been mutated yet).

### Dry-run: `pub fn restore_dry_run()`

**File:** `crates/root-core/src/lib.rs:3115-3184`

```
fn restore_dry_run(adapter: &impl NixAdapter, lock_path: Option<&Path>) -> Result<RestorePlanReport>
```

Steps:

1. `init_root_dir()`
2. Resolve and read the target lock (v2, then v1→v2 fallback) — same as `restore()`
3. `restore_validate(...)` — same pre-mutation checks; invalid locks are rejected
4. Inspect current profile via `profile_packages(adapter)`
5. Classify each target package:
   - **keep** — `locked_package_installed` (name + store paths match)
   - **update** — same name in profile, different store paths
   - **install** — name not in profile
6. **remove** — profile entries whose names are not in the target lock
7. Record `RestorePlanned` / `Planned` on `events.jsonl`
8. Return `RestorePlanReport`

Does **not**: acquire `MutationGuard`, enforce policy, snapshot, call Nix install/remove, or write Rootfile / `root.lock`.

Does append one ledger event (`RestorePlanned`).

### `RestoreReport` / `RestorePlanReport`

**File:** `crates/root-core/src/lib.rs:2298-2335`

```rust
pub struct RestorePlanReport {
    pub lock_path: String,
    pub will_install: Vec<String>,
    pub will_remove: Vec<String>,
    pub will_keep: Vec<String>,
    pub will_update: Vec<String>,
    pub total_packages: usize,
}

pub struct RestoreReport {
    pub success: bool,
    pub lock_path: String,
    pub installed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub snapshot_id: String,
}
```

### Test coverage

See §9 Current Gaps for the full matrix. Named tests in `lib.rs`:

| Test | Line (approx.) |
|------|----------------|
| `test_restore_from_shared_v2_lock` | 3965 |
| `test_restore_rejects_invalid_lockfile_before_mutation` | 5721 |
| `test_restore_dry_run_does_not_mutate` | 6108 |
| `test_restore_dry_run_reports_sets_correctly` | 6164 |
| `test_restore_dry_run_rejects_invalid_lockfile` | 6217 |
| `test_restore_validation_rejects_drv_output_path` | 6240 |
| `test_restore_status_detects_platform_mismatch` | 6288 |
| `test_restore_records_event_on_success` | 6327 |
| `test_restore_records_event_on_validation_failure` | 6369 |
| `test_restore_partial_failure_rolls_back_profile` | expected (sibling coverage) |
| `test_mutation_guard_acquires_and_releases` | 5260 |
| `test_mutation_guard_stale_lock_recovery` | 5277 |

---

## 2. Lockfile Validation Flow

### Reading: v2 with v1 fallback

**Target lock (restore / dry-run):** `lib.rs:3007-3008` and `3124-3125` — read the selected path, v2 then v1 `to_v2()`.

**Current lock (snapshot / reconcile):** `get_or_create_lock_v2()` at `lib.rs:1098-1112`

```rust
fn get_or_create_lock_v2() -> Result<RootLockV2> {
    let lock = RootLockV2::read_from_file(&path)
        .or_else(|_| RootLock::read_from_file(&path).map(|lock| lock.to_v2()))?;
    root_lockfile::validate_store_paths(&lock)?;
    Ok(lock)
}
```

- First tries `RootLockV2::read_from_file` (JSON deserialization)
- On failure, falls back to v1 (`RootLock::read_from_file`) and converts via `to_v2()`
- `get_or_create_lock_v2()` also calls `validate_store_paths` after a successful read of the **active** `~/.root/root.lock`
- Restore's **target** lock is validated by `restore_validate()`, not by `get_or_create_lock_v2()`

Used by: `restore()` (pre-restore snapshot), `restore_dry_run()` (profile compare only uses `profile_packages`), `sync()`, `reconcile_profile_to_lock()`, `status()`, `rollback_last()`.

### `restore_validate()`

**File:** `crates/root-core/src/lib.rs:2897-2999`

Called by both `restore()` and `restore_dry_run()` before mutation (and before dry-run planning). Checks, in order:

| Check | Failure message prefix |
|-------|------------------------|
| `validate_store_paths` | `Restore validation failed: lockfile at {} contains invalid store paths` |
| `adapter.check_availability()` | `Restore validation failed: Nix is not available` |
| `adapter.probe_experimental_features()` | missing `nix-command` / `flakes` / both / nixpkgs resolution |
| `adapter.profile_exists()` | `Restore validation failed: Root profile does not exist` |
| lock `platform` vs `detect_platform()` | `lockfile platform '{}' does not match current platform '{}'` |
| `.drv` suffix on `store_path`, `store_paths`, `outputs.*.store_path` | `package '{}' has a .drv path ...` |

### `RootLockV2::read_from_file`

**File:** `crates/root-lockfile/src/lib.rs:334-337`

```rust
pub fn read_from_file(path: &Path) -> Result<Self> {
    let content = fs::read_to_string(path)?;
    Self::read_from_str(&content)
}
```

- Reads file to string, deserializes via `serde_json`
- Deserialization failures return generic `context("Failed to parse root.lock v2 JSON")` error (`read_from_str` at 328-331)

### `RootLock::read_from_file` (v1 fallback)

**File:** `crates/root-lockfile/src/lib.rs:238-241`

Same pattern but for v1 schema. `to_v2()` (`lib.rs:307-325`) maps each `LockedPackage` via `LockedPackageV2::from` (`lib.rs:403-436`): synthetic `outputs` / `store_paths` from the single `store_path`, and `installable: Some(package.attribute)`.

### `validate_store_paths`

**File:** `crates/root-lockfile/src/lib.rs:629-703`

Validates every package in the lock:

1. `drv_path` (if present/non-empty) → must end in `.drv`
2. `outputs.*.store_path` → must NOT end in `.drv`, must start with `/nix/store/`
3. `store_paths.*` → same as outputs
4. `store_path` (primary) → same as outputs

### Error types

**File:** `crates/root-lockfile/src/lib.rs:598-612`

| Variant | Meaning |
|---------|---------|
| `DrvInOutputField { package, package_short, found }` | A `.drv` path where a realized output was expected |
| `OutputNotInStore { package, found }` | Path doesn't start with `/nix/store/` |

### Where validation gates mutation

| Caller | Location | Timing |
|--------|----------|--------|
| `restore()` | `lib.rs:3009` via `restore_validate` | Before policy, `MutationGuard`, and `reconcile_profile_to_lock` |
| `restore_dry_run()` | `lib.rs:3127` via `restore_validate` | Before plan computation; no mutation follows |
| `attempt_rollback_to_snapshot()` | `lib.rs:2862-2863` | Before Nix remove/install during auto-rollback |
| `sync()` | `lib.rs:2745` | Before `reconcile_profile_to_lock` |
| `install()` | `lib.rs:1601-1603` | After Nix install, before saving lockfile |
| `rollback_last()` | `lib.rs:1915-1918` | Before any Nix mutation |
| `get_or_create_lock_v2()` | `lib.rs:1104` | On every active-lockfile read |

---

## 3. Nix Operations Used

### RealNixAdapter implementations

**File:** `crates/root-nix/src/lib.rs`

| Operation | Method | Shell command | Line |
|-----------|--------|---------------|------|
| Profile generation | `profile_generation()` | Reads profile symlink target (parses `-NNN-link` suffix) | 500-518 |
| Profile exists | `profile_exists()` | `profile_path.exists()` | 520-522 |
| Check availability | `check_availability()` | `nix --version` | 528-534 |
| Experimental features | `probe_experimental_features()` | `nix eval nixpkgs#hello` | 536 |
| Search | `search()` | `nix search nixpkgs <pkg>` | 567-569 |
| Install (by attribute) | `install()` | `nix profile add nixpkgs#<pkg> --profile <path>` | 571-580 |
| Install (by installable) | `install_installable()` | `nix profile add <installable> --profile <path>` | 582-590 |
| List | `list()` | `nix profile list --profile <path>` | 592-595 |
| Remove | `remove()` | `nix profile remove <pkg> --profile <path>` | 597-605 |
| Profile JSON list | `profile_list_json()` | `nix profile list --json --profile <path>` | 607-614 |
| Flake metadata | `flake_metadata()` | `nix flake metadata --json <flake>` | 616-628 |
| Eval metadata | `eval_package_metadata()` | `nix eval --json <pkg>.meta` | 630-649 |
| Build outputs | `build_output_paths()` | `nix build --no-link --print-out-paths --json <installable>` | 651 |
| Derivation path | `derivation_path()` | `nix eval --raw <pkg>.drvPath` | 699-717 |
| Path info | `path_info()` | `nix path-info --json --closure-size <path>` | 719 |

**Profile directory:** `~/.root/profiles/default`  
**All commands pass:** `--profile <profile_path>` for profile operations.

Restore uses: availability + experimental-feature probe + `profile_exists` (validation); `profile_list_json` / `list` (inspect); `profile_generation`, `install_installable` / `install`, `remove` (reconcile and auto-rollback).

### MockNixAdapter (testing)

**File:** `crates/root-nix/src/lib.rs:742` (struct), `789` (`impl NixAdapter`)

In-memory adapter with a `Vec<String>` of installed packages and atomic generation counter. Special packages (`ensure_available_package` at 775-786):

- `"missing_pkg"` → `NixError::NotFound`
- `"bad_platform_pkg"` → `NixError::PlatformMissing`

`profile_list_json` generates synthetic JSON from internal state; `profile_list_json_override` allows injecting arbitrary JSON for edge case testing.

---

## 4. Profile Mutation Flow — `reconcile_profile_to_lock()`

**File:** `crates/root-core/src/lib.rs:2554-2723`

This is the core reconciliation engine used by `restore()` and `sync()`.

### Parameters

- `adapter: &impl NixAdapter`
- `target_lock: &RootLockV2` — the desired state
- `snapshot_reason: &str` — label for the pre-mutation snapshot
- `command: &str` — command string for event recording (e.g., `"root restore"`)
- `event_type: events::RootEventType` — event type discrimination (`Restore`, `Update`, etc.)

### Step-by-step flow

**Step 1 — Snapshot current state (`lib.rs:2561-2563`):**

```rust
let current_lock = get_or_create_lock_v2()?;
let snapshot = Snapshot::create_from_v2(snapshot_reason, &current_lock)?;
```

Captures the current lockfile state as a snapshot file in `~/.root/snapshots/`.

`restore()` also snapshots **before** this call (`lib.rs:3017-3024`). Auto-rollback uses that outer snapshot. Reconcile's snapshot is the one attached to Completed/per-package Failed events from this function.

**Step 2 — Profile inspection (`lib.rs:2565`):**

```rust
let profile_entries = profile_packages(adapter)?;
```

- First tries `adapter.profile_list_json()` to get structured JSON with store paths
- Falls back to `adapter.list()` (legacy text format) with empty store paths (`lib.rs:2506-2523`)

**Step 3 — Build set of locked package names (`lib.rs:2566-2570`):**

```rust
let locked_names: BTreeSet<&str> = target_lock.packages.iter().map(...).collect();
```

**Step 4 — Install missing packages (`lib.rs:2574-2675`):**

For each target package not already installed (`locked_package_installed` checks name + store_paths):

1. **Get before-generation** via `adapter.profile_generation()`
2. **Install** using adapter — prefers `install_installable` if package has an `installable` field, else uses `install` (by name)
3. **Verify profile contains outputs** — `verify_profile_contains_outputs()` (`lib.rs:1286-1311`) checks each store path is present in `nix profile list --json`
4. **Validate mutation result** — `validate_mutation_result()` (`lib.rs:1313`) checks generation changed, expected store paths, no `.drv` in outputs
5. On failure at any sub-step: records a `RootEventStatus::Failed` event with the error (`let _ = record_event(...)`), then propagates the error up

**Step 5 — Remove extra packages (`lib.rs:2677-2697`):**

For each profile entry not in the target lock:

- Calls `adapter.remove(&entry.package)`
- On failure: records a failed event, propagates error

**Step 6 — Save state (`lib.rs:2699-2700`):**

```rust
save_lock_v2(target_lock)?;
write_rootfile_from_v2_lock(target_lock)?;
```

- Writes the target lockfile to disk
- Rebuilds `Rootfile` from scratch: clears existing Rootfile, reinserts all packages from the target lock

**Step 7 — Record completion event (`lib.rs:2702-2715`):**

```rust
let _ = events::record_event(event_type, RootEventStatus::Completed, command, ...)?;
```

The `?` here **does** propagate a ledger write failure after Nix + file writes have already succeeded.

### Return type: `ProfileReconcileReport`

**File:** `lib.rs:2344-2349`

```rust
struct ProfileReconcileReport {
    installed: Vec<String>,
    removed: Vec<String>,
    unchanged: Vec<String>,
    snapshot_id: String,
}
```

### Key observation: Non-atomic between Nix mutations and file writes

If the process crashes between a successful Nix install/remove and `save_lock_v2()` / `write_rootfile_from_v2_lock()`, the Nix profile and lockfile/Rootfile will be inconsistent.

On a **caught** `reconcile_profile_to_lock` error, `restore()` now attempts `attempt_rollback_to_snapshot()` (best-effort profile rollback). A hard crash never reaches that path; recovery is the pre-restore snapshot plus `root rollback --last` / `root sync`.

---

## 5. Event Recording Flow

**File:** `crates/root-core/src/events.rs`

### Storage format

- Append-only JSONL file at `~/.root/events.jsonl`
- Each event is a single JSON object on one line

### Event types relevant to restore

```rust
pub enum RootEventType {
    Restore,            // events.rs:17
    RestorePlanned,     // events.rs:18
    RestoreRecovered,   // events.rs:19
    Install,
    Update,             // also used by sync
    Remove,
    Rollback,
    Verification,
    VerificationFailed,
    Doctor,
    Execution,
    Policy,
    Sandbox,
}
```

There is no separate `RestoreStarted` / `RestoreCompleted` / `RestoreFailed` **type**. Success and failure are `Restore` plus `RootEventStatus`. `restore()` does not record a `Started` event.

### Event statuses

```rust
pub enum RootEventStatus {
    Started,
    Planned,      // events.rs:28 — used by dry-run
    Completed,
    Failed,
    Verified,
    Timeout,
}
```

### Extra fields on `RootEvent` (`events.rs:61-67`)

```rust
pub failure_phase: Option<String>,
pub installed_count: Option<usize>,
pub removed_count: Option<usize>,
pub kept_count: Option<usize>,
```

`create_event` (`events.rs:147-178`) always initializes these to `None`. `record_event` (`events.rs:232-251`) does not populate them. Restore puts the inferred phase in the **message** string via `infer_restore_failure_phase()`.

### Recording during restore

**`restore_dry_run()` (`lib.rs:3160-3174`):**

| Point | Type / Status | Notes |
|-------|---------------|--------|
| After computing the plan | `RestorePlanned` / `Planned` | `let _ = record_event(...)` — swallows write errors |

**`reconcile_profile_to_lock()`:**

| Point | Status | Notes |
|-------|--------|-------|
| Per-package generation check failure | `Failed` | Before any install attempt |
| Per-package install failure | `Failed` | Nix install error |
| Per-package verification failure | `Failed` | `verify_profile_contains_outputs` |
| Per-package mutation validation failure | `Failed` | `validate_mutation_result` |
| Per-package remove failure | `Failed` | `adapter.remove` |
| Final completion | `Completed` | After all Nix ops + lockfile/Rootfile save; `?` on write |

**`restore()` error path (`lib.rs:3046-3110`):**

| Point | Type / Status | Notes |
|-------|---------------|--------|
| After reconcile `Err` | `Restore` / `Failed` | Message includes inferred `failure_phase` |
| Auto-rollback `Ok` | `RestoreRecovered` / `Completed` | |
| Auto-rollback `Err` | `RestoreRecovered` / `Failed` | |

All three of those wrapper writes use `let _ = record_event(...)` (no `?`).

The `record_event` function (`events.rs:232-251`) creates a `RootEvent` and appends it:

```rust
pub fn record_event(
    event_type, status, command,
    package, snapshot_id, restored_snapshot_id, message
) -> Result<RootEvent>
```

---

## 6. Rollback/Recovery Behavior

### Automatic rollback on restore failure

**`infer_restore_failure_phase()`** — `lib.rs:2829-2855`

Maps the error string to a phase: `pre-install check`, `package installation`, `profile verification`, `mutation validation`, `package removal`, `pre-restore validation`, `policy check`, `lock acquisition`, or `unknown phase`.

**`attempt_rollback_to_snapshot()`** — `lib.rs:2857-2895`

Best-effort Nix-profile rollback to a snapshot lock:

1. `snapshot.restored_lock()` then `validate_store_paths`
2. Remove profile entries not in the snapshot lock
3. Install snapshot packages not present (`install_installable` if set, else `install`)
4. Does **not** rewrite Rootfile / `root.lock` (those are only written after Nix ops succeed in reconcile)

If this also fails, `restore()` tells the user to run `root rollback --last`.

If no pre-restore snapshot was captured (`get_or_create_lock_v2` / `Snapshot::create_from_v2` failed), auto-rollback is skipped.

### `rollback_last()` (manual)

**File:** `crates/root-core/src/lib.rs:1904-2085`

User-facing recovery when auto-rollback fails, or after a crash that never reached the error path.

Rollback steps:

1. **Acquire mutation lock** — ensures exclusive access
2. **List snapshots** — reads `~/.root/snapshots/*.json`, takes most recent (`snaps[0]`)
3. **Get target lock** from snapshot via `last_snap.restored_lock()` (`root-snapshot/src/lib.rs:126-141`):
   - If `lock.version == 0` and `packages` is non-empty: reconstruct a v2 lock from the legacy `packages` vec
   - Otherwise: use stored `lock: RootLockV2` directly
4. **Validate** snapshot lock store paths
5. **Compute diff** between current and target lock
6. **Create pre-rollback snapshot** — ensures rollback can be rolled-forward
7. **Execute Nix changes first** — remove, then install (generation check + `verify_profile_contains_outputs` + `validate_mutation_result`)
8. **Update lockfile and Rootfile** — only after Nix operations succeed
9. **Record rollback event**

**Critical design choice:** Step 8 happens **after** all Nix mutations succeed, meaning a crash between steps 7 and 8 leaves Nix profile in the rolled-back state while lockfile/Rootfile still reflect the pre-rollback state. Running `root sync` would fix inconsistency.

### Snapshot integrity

**File:** `crates/root-snapshot/src/lib.rs:102-124`

When reading a snapshot, `Snapshot::read()` verifies the `lock_content_hash`:

```rust
let lock_content = serde_json::to_vec(&snapshot.lock)?;
let computed = compute_sha256(&lock_content);
if computed != snapshot.lock_content_hash {
    bail!("Snapshot ... lock content hash mismatch ... corrupted or tampered");
}
```

### Snapshot content

A snapshot stores a full serialized `RootLockV2` plus metadata:

- `id` (e.g., `snap_20250101_120000_123456`)
- `created_at` (UTC timestamp)
- `reason` (e.g., `"before restore from /path/to/lock"`)
- `package_count`
- `lock_content_hash` (SHA-256 of the JSON lock)
- `lock: RootLockV2` (full lock state)
- `packages: Vec<LockedPackage>` (legacy v1 field for backward compat)

---

## 7. Drift Detection Behavior

### `status()` function

**File:** `crates/root-core/src/lib.rs:3419-3582`

Compares three sources of truth:

1. **Rootfile contents** (user intent) — via `get_or_create_rootfile()`
2. **root.lock** (deterministic lock) — via `get_or_create_lock_v2()`
3. **Nix profile** (actual installed state) — via `profile_packages(adapter)`

Drift categories detected:

| Category | When | Severity | Suggestion |
|----------|------|----------|------------|
| `rootfile-lockfile-mismatch` | Package in Rootfile not in lockfile | Unhealthy | `root lock` |
| `profile-unavailable` | Nix profile cannot be inspected | Unhealthy | `root doctor` |
| `lockfile-profile-mismatch` | Package in lockfile not in Nix profile | Unhealthy | `root sync` |
| `lockfile-output-missing` | Package in profile by name but expected store paths absent | Unhealthy | `root sync` |
| `lockfile-has-drv-path` | Locked `store_path` ends in `.drv` | Unhealthy | `root lock` |
| `profile-lockfile-mismatch` | Package in Nix profile not in lockfile | Unhealthy | `root sync` |
| `platform-mismatch` | Lock `platform` ≠ current platform | Recorded (does not by itself set `healthy = false`) | regenerate lock |

State classification (`lib.rs:3560-3568`):

- `"Healthy"` — no issues (`healthy` true)
- `"NeedsAttention"` — `lockfile-profile-mismatch`, `profile-unavailable`, or `lockfile-output-missing`
- `"Drifted"` — other mismatches

`platform-mismatch` is recorded on `drift_details` but does not flip `healthy` to false, so a platform-only mismatch can still report `"Healthy"`.

### `doctor()` diagnostics

**File:** `crates/root-doctor/src/lib.rs:32-543`

Called via `root doctor` → `root_core::doctor(&adapter)` (`lib.rs:2087`) → `root_doctor::run_diagnostics(&adapter)`

Checks in order:

1. **Nix availability** — `nix --version` probe
2. **Experimental features** — `nix eval nixpkgs#hello`, parses stderr for feature-missing messages
3. **Root directory structure** — subdirectories `snapshots`, `profiles`, `logs`, `cache`
4. **Config files** — reads Rootfile and root.lock, checks for:
   - Corrupted/unparseable files
   - Legacy schema version
   - Floating `"latest"` versions
   - Placeholder store paths
   - Unknown nixpkgs revision
5. **Drift detection** — compares Rootfile ↔ lockfile ↔ profile using `profile_list_json()`
6. **PATH & shadow detection** — checks `~/.root/profiles/default/bin` is in PATH and no other binary shadows Root-managed ones

---

## 8. Known Failure Modes

### 8.1 Invalid lockfile

**What happens:** `restore_validate()` → `validate_store_paths()` before any mutation.  
**Code path:** `restore()` → `lib.rs:3009` → `restore_validate` `lib.rs:2902-2905` → `root-lockfile/src/lib.rs:629-703`  
**Error messages:**

- `"Restore validation failed: lockfile at {} contains invalid store paths"`
- underlying `"Invalid Root lockfile: package X.Y has a derivation path where an output path was expected"`

**Test:** `test_restore_rejects_invalid_lockfile_before_mutation` (`lib.rs:5721`)  
**Exit code:** 1 (generic failure)  
**Recovery:** N/A — Rootfile / lock / profile not touched. No auto-rollback.

### 8.2 `.drv` path in output path

**What happens:** Caught at several layers:

1. At resolution time: `deterministic_package_from_resolution()` (`lib.rs:1165-1172`) rejects `.drv` outputs
2. At lock validation: `validate_store_paths()` (`root-lockfile/src/lib.rs:667-669`) rejects `.drv` in `store_paths`
3. At restore entry: `restore_validate()` (`lib.rs:2969-2996`) rejects `.drv` on `store_path`, `store_paths`, and `outputs`
4. At verify time: `verify_profile_contains_outputs()` (`lib.rs:1291-1296`) refuses to treat `.drv` as an installed output

**Error examples:**

- `"Restore validation failed: package '{}' has a .drv path as its store path."`
- `"Root resolved a derivation path but no realized output path for {}. Expected an output store path, got: {}"`
- `"Invalid Root lockfile: package {}.out has a derivation path where an output path was expected"`

**Test:** `test_restore_validation_rejects_drv_output_path` (`lib.rs:6240`)  
**Recovery:** User must fix the lockfile or provide a valid one. Restore is gated.

### 8.3 Missing package metadata (unsupported package)

**What happens:** Restore from an externally-created lockfile can contain packages Root doesn't know about via `resolve_package()`.  
**Code path:** `reconcile_profile_to_lock()` does NOT call `resolve_package()` to gate installation — it installs whatever packages are in the lockfile. `resolve_package()` is only used for `binaries` in `validate_mutation_result()` (`lib.rs:2644-2645`), returning `&[]` for unknown packages.  
**Impact:** Binary validation is skipped for unknown packages. Install and verify still run. Minimal risk.

### 8.4 Missing Nix

**What happens:** `restore_validate()` fails before mutation if `check_availability` is false. If Nix disappears mid-reconcile, adapter methods return `NixError::NotInstalled` (exit code 7).  
**Code path (gate):** `restore_validate` `lib.rs:2907-2915`  
**Code path (mid-flight):** any Nix call from `reconcile_profile_to_lock()` → adapter method → `run_command()` → `Command::new("nix")` fails.  
**Error:** `"Restore validation failed: Nix is not available"` or `"Failed to install '{}': Nix is not installed or not available on PATH."`  
**Exit code:** 7 (mid-flight) / 1 (validation gate)  
**Test:** `test_diagnostics_no_nix` (`root-doctor/src/lib.rs:736`)

### 8.5 Missing experimental features

**What happens:** `restore_validate()` probes via `adapter.probe_experimental_features()` and fails closed before mutation.  
**Code path:** `restore_validate` `lib.rs:2917-2950`  
**Error:** `"Restore validation failed: Nix experimental feature 'nix-command' is not enabled."` (or `flakes`, or both)  
**Recovery:** Follow doctor suggestion to add `experimental-features = nix-command flakes` to `nix.conf`.

### 8.6 Package resolution failure

**What happens:** Not applicable to restore — restore does NOT call `resolve_locked_package()`. It uses the installable string already stored in the lockfile (`package.installable`).  
**Potential failure:** If the installable references a nixpkgs revision that no longer exists (e.g., a Git SHA that was garbage-collected), `adapter.install_installable()` will fail.  
**Error:** From Nix — store path not found, or build failure. Phase: `package installation`. Auto-rollback attempted.

### 8.7 Install failure mid-restore

**What happens:** `reconcile_profile_to_lock()` loops over target packages. If the third of five packages fails to install:

1. A `Failed` event is recorded for that package
2. The **entire function** returns an error
3. `restore()` infers phase `package installation`, records `Restore` / `Failed`
4. **Automatic rollback** via `attempt_rollback_to_snapshot()` using the pre-restore snapshot
5. `RestoreRecovered` is recorded (`Completed` or `Failed`)
6. The mutation lock is released (via `Drop`)

**Impact:** Partial Nix installs are rolled back when auto-rollback succeeds. Rootfile and `root.lock` were not written yet.  
**Code path:** install error `lib.rs:2606-2620`; recovery `lib.rs:3046-3100`  
**Error:** `"root restore failed to install '{}': {}"` wrapped as `"Restore failed during package installation."`  
**If rollback fails:** user is told to run `root rollback --last`.  
**Test:** `test_restore_partial_failure_rolls_back_profile`

### 8.8 Profile mutation failure

**What happens:** After a successful `adapter.install_installable()`, `validate_mutation_result()` checks generation changed and expected paths exist.  
**Code path:** `lib.rs:2646-2672` then `restore()` error path  
**Error:** `"root restore mutation validation failed for '{}': Profile mutation validation failed: ..."` — phase `mutation validation`  
**Cause (non-exhaustive):** Generation didn't change (Nix profile wasn't actually updated). Missing output paths. `.drv` paths in outputs.  
**Recovery:** Auto-rollback, then `root rollback --last` if that fails.

### 8.9 Verification failure after restore

**What happens:** `verify_profile_contains_outputs()` (`lib.rs:1286-1311`) checks every store path in the package's `store_paths` map appears in `nix profile list --json` output.  
**Code path:** `lib.rs:2622-2641` then `restore()` error path  
**Error:** `"Installed profile did not contain locked Nix store path {}"` or `"Refusing to verify .drv path as an installed output"` — phase `profile verification`  
**Recovery:** Auto-rollback, then `root rollback --last` if that fails.

### 8.10 Interrupted restore (crash between Nix mutations and lockfile write)

**What happens:** If the process is killed (no `Err` path, so no auto-rollback):

- **Case A (installed some packages, crash before `save_lock_v2`):** Nix profile has packages that lockfile doesn't know about. `root sync` will detect drift (`profile-lockfile-mismatch`) and attempt to remove extra packages. `root rollback --last` can restore the pre-restore snapshot.
- **Case B (installed some packages, crash after `save_lock_v2`):** Lockfile is written but Rootfile may be in the wrong state (file ops are atomic via `atomic_write` but separated). Running `root sync` will detect rootfile-lockfile drift.
- **Case C (crash during remove loop):** Some packages removed from profile but still in lockfile. `root sync` will re-install them.

**Safeguards:**

- Pre-restore snapshot (outer) plus reconcile snapshot preserve prior lock state
- Caught errors trigger `attempt_rollback_to_snapshot`
- `root rollback --last` can restore the snapshot
- `root sync` reconciles profile with lockfile

**Current limitation:** A hard crash is not automatically recovered. Snapshot + auto-rollback (on caught errors) is the recovery design; crash recovery remains manual.

### 8.11 Stale mutation lock

**What happens:** `MutationGuard::acquire()` (`lib.rs:599-637`) at restore start (not taken by dry-run).

- If lock file exists: checks if the PID listed in the lockfile is alive via `kill -0 <PID>`
- If PID is alive: error — `"Another Root mutation is in progress (PID {}). If this is unexpected, delete ~/.root/root.lockfile and try again."`
- If PID is dead: removes stale lock, retries
- If lock file is unreadable / malformed: error — `"Lock file ~/.root/root.lockfile exists and could not be read. Delete it manually and try again."`

**Drop:** Lock file is removed on `MutationGuard` drop (`lib.rs:674-678`).

**Tests:** `test_mutation_guard_stale_lock_recovery` (`lib.rs:5277`), `test_mutation_guard_acquires_and_releases` (`lib.rs:5260`) — same-process lock; not a true two-process restore.

Unreadable lock still requires **manual delete**. A crash that leaves a live-looking PID (or a lock the process cannot unlink) also needs manual delete.

### 8.12 Profile drift before restore

**What happens:** `reconcile_profile_to_lock()` reads current profile via `profile_packages(adapter)` (`lib.rs:2565`). If the profile was manually modified (e.g., `nix profile remove` outside Root), restore will correctly handle it — extra packages are removed, missing packages installed.  
**If profile is completely broken** (e.g., symlink corrupted): `profile_packages()` returns error → `reconcile_profile_to_lock()` fails; auto-rollback is attempted if a snapshot exists. `restore_validate` already requires `profile_exists()`.  
**Recovery:** `root doctor` to diagnose profile issues.

### 8.13 Profile drift after restore

**What happens:** The restore is a point-in-time reconciliation. After restore succeeds, any subsequent manual Nix operations will cause drift.  
**Detection:** `root status` or `root doctor --check` will detect `lockfile-profile-mismatch`, `profile-lockfile-mismatch`, or `lockfile-output-missing`.  
**Recovery:** `root sync` re-reconciles.

### 8.14 Dry-run of an invalid lock

**What happens:** `restore_dry_run()` calls `restore_validate()` and returns without a plan.  
**Test:** `test_restore_dry_run_rejects_invalid_lockfile` (`lib.rs:6217`)  
**Side effect:** no `RestorePlanned` event (recording happens after validation).

---

## 9. Current Gaps and Limitations

### Closed by 0.2.4

#### 9.1 Automatic rollback on failure — CLOSED (best-effort)

If `reconcile_profile_to_lock()` fails mid-way, `restore()` infers a failure phase, records `Restore` / `Failed`, and calls `attempt_rollback_to_snapshot()` against the pre-restore snapshot.

- Success: `RestoreRecovered` / `Completed`; user is told the Nix profile was rolled back and Rootfile/`root.lock` were preserved.
- Failure: `RestoreRecovered` / `Failed`; user is told to run `root rollback --last`.
- No snapshot: rollback skipped; user is pointed at `root status` / `root doctor`.

This is **best-effort**, not a transaction. It does not cover hard crashes (see remaining 9.2).

#### 9.7 `--dry-run` — CLOSED

`root restore --dry-run` is implemented (`restore_dry_run()`, CLI flag). It reports will-install / will-remove / will-keep / will-update, runs `restore_validate()`, and does not mutate Rootfile, `root.lock`, or the Nix profile.

It **does** append a `RestorePlanned` event to `events.jsonl` (append-only ledger). That ledger write is an accepted remaining caveat, not a claim that dry-run is side-effect-free on disk.

### Remaining / deferred

#### 9.2 Non-atomic Nix + file writes (inherent)

The sequence in `reconcile_profile_to_lock()` is still:

1. Nix installs (multiple, sequential)
2. Nix removes (multiple, sequential)
3. Write lockfile
4. Write Rootfile

A crash between any of these steps leaves inconsistent state. Caught errors now auto-rollback the **profile**; a crash never enters that path. The pre-restore snapshot is the recovery mechanism (`root rollback --last`).

#### 9.3 No atomicity between multiple Nix operations

Each `adapter.install_installable()` and `adapter.remove()` is a separate `nix profile` invocation. If the user kills the process during the loop, some packages may be installed but not others. Nix profiles do not support transactional batch operations. Auto-rollback only runs if the process lives to handle `Err`.

#### 9.4 Legacy v1 lock fallback can produce unpinned installables

When restoring from a v1 lockfile, `LockedPackageV2::from` sets `installable: Some(package.attribute)` where `attribute` is just the package name (e.g., `"ffmpeg"`). The resulting lock lacks a pinned nixpkgs revision in the installable (no `github:NixOS/nixpkgs/<rev>#ffmpeg`), meaning the resolved package depends on what `nixpkgs` currently points to.

#### 9.5 No timeout for Nix operations during restore

Long-running Nix builds during restore have no configurable timeout. A stuck build blocks the mutation lock indefinitely.

#### 9.6 No restore from snapshot file

Restore only accepts a lockfile path. Restoring from a snapshot (which also contains a full lock) requires manually extracting the lock from the snapshot JSON file.

#### 9.8 Test coverage

| Scenario | Tested? |
|----------|---------|
| Restore from shared v2 lock | **Yes** — `test_restore_from_shared_v2_lock` |
| Restore rejects invalid lockfile pre-mutation | **Yes** — `test_restore_rejects_invalid_lockfile_before_mutation` |
| Dry-run does not mutate / reports sets / rejects invalid | **Yes** — `test_restore_dry_run_does_not_mutate`, `test_restore_dry_run_reports_sets_correctly`, `test_restore_dry_run_rejects_invalid_lockfile` |
| `.drv` output path rejected | **Yes** — `test_restore_validation_rejects_drv_output_path` |
| Platform mismatch in status | **Yes** — `test_restore_status_detects_platform_mismatch` |
| Restore events on success and validation failure | **Yes** — `test_restore_records_event_on_success`, `test_restore_records_event_on_validation_failure` |
| Mid-restore failure auto-rollback | **Yes** — `test_restore_partial_failure_rolls_back_profile` |
| Restore with no existing lockfile | **Yes** |
| Restore from v1 lockfile | **Yes** |
| Stale mutation lock recovery | **Yes** — `test_restore_recovers_stale_mutation_lock` |
| Live mutation lock blocks restore | **Yes** — `test_restore_blocked_by_live_mutation_lock` (same-process lock; not a true two-process test) |
| Crash between Nix ops and file writes | **No** (inherent; snapshot + auto-rollback is the recovery) |
| Concurrent two-process restore | Covered by the live mutation lock test (same-process lock), not a true two-process test |

Related (not restore-specific): `test_rollback_v2_verifies_store_paths`, `test_sync_rejects_invalid_lockfile`.

#### 9.9 Policy enforcement only at entry points

`enforce_policy(PolicyAction::Restore, ...)` is called at the `restore()` top level (`lib.rs:3030-3033`), not within `reconcile_profile_to_lock()`. Dry-run does not enforce policy. `reconcile_profile_to_lock()` is shared with `sync()`, which does its own policy check at its entry point (`lib.rs:2727-2731`).

#### 9.10 Snapshot deduplication / retention

Every call to `reconcile_profile_to_lock()` creates a snapshot before any change, even if the current lock hasn't changed since the last snapshot. `restore()` may create an additional outer snapshot for auto-rollback. There is no periodic cleanup or retention policy for snapshots.

#### 9.11 Mutation lock still needs manual delete in some cases

Stale (dead-PID) locks are recovered automatically. Unreadable / malformed locks (`test_mutation_guard_malformed_lock_fails_safely`) and some crash leftovers still require `rm ~/.root/root.lockfile`.

#### 9.12 Event recording swallows most write failures

Restore failure, recovery, and dry-run paths use `let _ = events::record_event(...)` and ignore ledger I/O errors. Reconcile's success path uses `let _ = record_event(...)?`, so a ledger write failure can still fail restore **after** Nix + file writes.

`failure_phase` / count fields on `RootEvent` are unused by `record_event`; phase is only in the message.

#### 9.13 Dry-run still appends to the event ledger

`--dry-run` does not mutate Rootfile, `root.lock`, or the Nix profile. It does append `RestorePlanned` to `events.jsonl`.
