# Restore Notes — v0.2.4

Restore brings a Root-managed environment to the exact state described by a
lockfile. This document explains the restore lifecycle, validation, recovery,
and troubleshooting.

---

## Restore Lifecycle

```
CLI: root restore [root.lock] [--dry-run]
  │
  ├── restore_dry_run()            (--dry-run only)
  │     ├── Read + validate lockfile
  │     ├── Compare profile vs lock
  │     ├── Compute plan
  │     └── Record RestorePlanned event
  │
  └── restore()                    (actual restore)
        ├── Read + validate lockfile
        ├── Policy check
        ├── MutationGuard::acquire()
        ├── Capture pre-restore snapshot
        ├── reconcile_profile_to_lock()
        │     ├── Snapshot current state
        │     ├── Install missing packages
        │     ├── Verify each install
        │     ├── Validate mutation results
        │     ├── Remove extra packages
        │     └── Write Rootfile + root.lock
        └── On failure:
              ├── Record RestoreFailed event
              ├── Attempt rollback to snapshot
              ├── Record RestoreRecovered event
              └── Return clear error with next steps
```

## Dry-Run Behavior

`root restore root.lock --dry-run`:

- Reads and validates the target lockfile
- Compares current Root-managed profile against the target lock
- Reports:
  - **Will install** — packages in lock but not in profile
  - **Will remove** — packages in profile but not in lock
  - **Will keep** — packages matching in both (same name + store paths)
  - **Will update** — packages in profile by name but with different store paths
- Does **not** mutate: Rootfile, root.lock, Nix profile, event ledger
- Records a `RestorePlanned` event (non-mutating append to events.jsonl)

## Validation

### Pre-restore

| Check | Description | Failure message |
|-------|-------------|-----------------|
| Lockfile schema | Valid JSON, v2 or v1 format | "Lockfile at ... failed validation before restore" |
| Store paths | All paths start with `/nix/store/`, no `.drv` in output fields | "invalid store paths" |
| Platform | Lockfile platform matches current platform | "lockfile platform ... does not match current platform" |
| Nix availability | `nix` binary found in PATH | "Nix is not available" |
| Experimental features | `nix-command` and `flakes` enabled | "experimental feature ... is not enabled" |
| Mutation lock | No other Root mutation running | "Another Root mutation is in progress" |
| Policy | Restore action not denied | "Policy denied restore" |

### During restore

| Check | Description | Failure message |
|-------|-------------|-----------------|
| Generation check | Profile generation readable before install | "failed to check generation for ..." |
| Install result | Nix profile add succeeds | "failed to install ..." |
| Output paths | Locked store paths appear in `nix profile list --json` | "verification failed for ..." |
| Mutation validation | Generation changed, no `.drv` paths, binaries exist | "mutation validation failed for ..." |
| Remove result | Nix profile remove succeeds | "failed to remove ..." |

### Post-restore

| Check | Description | Failure action |
|-------|-------------|----------------|
| Lockfile write | Atomic write succeeds | Error propagated (previous lock preserved) |
| Rootfile write | Rootfile matches restored package set | Error propagated (previous Rootfile preserved) |

## Partial Failure Recovery

If restore fails at any point:

1. **Rootfile and root.lock are restored from the pre-restore snapshot** if
   automatic recovery runs. They are only overwritten with the target lock after
   Nix mutations succeed; if a later metadata write fails, recovery rewrites
   both files from the snapshot.
2. **Failure event is recorded** with failure phase (e.g., "package installation",
   "profile verification", "package removal").
3. **Automatic rollback is attempted** — Root recreates the pre-restore state
   by restoring the snapshot taken before mutations began.
4. **If rollback succeeds**: Clear message says state was recovered.
5. **If rollback fails**: Clear message explains what to do next.

### Recovery scenarios

| Failure point | Rootfile/lock preserved? | Rollback possible? |
|---------------|--------------------------|-------------------|
| Pre-restore validation | Yes (not touched) | N/A (no mutation yet) |
| Policy check | Yes (not touched) | N/A (no mutation yet) |
| Install package A | Yes (not written yet) | Yes (snapshot available) |
| Verify install A | Yes (not written yet) | Yes (snapshot available) |
| Remove package B | Yes (not written yet) | Partial (package A may be installed) |
| Post-restore validation | Partially (Nix profile done) | Yes (snapshot available) |

## Drift Detection

`root status` detects restore-related drift:

| Drift type | Detected when | Example |
|------------|--------------|---------|
| Rootfile-lockfile | Package in Rootfile missing from lock | Rootfile has `terraform` but lock does not |
| Lockfile-profile | Package in lock missing from profile | Lock has `kubectl` but profile does not |
| Profile-lockfile | Package in profile missing from lock | Profile has `jq` but lock does not |
| Missing output | Package installed but store path absent | `terraform` profile entry lacks expected `/nix/store/...` |
| `.drv` path | Lockfile references derivation as output | `store_path` ends in `.drv` |
| Platform mismatch | Lockfile platform differs from current | Lock has `x86_64-linux` on `aarch64-darwin` |

## Event Ledger

Restore records these events:

| Event type | Status | When |
|-----------|--------|------|
| `RestorePlanned` | `Planned` | During `--dry-run` |
| `Restore` | `Started` | Start of restore |
| `Restore` | `Completed` | Successful restore |
| `Restore` | `Failed` | Failed restore |
| `RestoreRecovered` | `Completed` | Automatic rollback succeeded |
| `RestoreRecovered` | `Failed` | Automatic rollback failed |

Restore events include:
- `failure_phase`: where the failure occurred
- `installed_count`, `removed_count`, `kept_count`: package counts

## Troubleshooting

### "Restore validation failed: lockfile at ... contains invalid store paths"

The lockfile has invalid store paths (e.g., paths outside `/nix/store/` or `.drv`
paths in output fields). Regenerate the lockfile:

```bash
root lock
```

### "Restore validation failed: lockfile platform ... does not match current platform"

The lockfile was created on a different platform. Use a lockfile from the
current platform or regenerate:

```bash
root lock
```

### "Restore validation failed: Nix is not available"

Nix is not installed or not in PATH. Install Nix:

```bash
sh <(curl -L https://nixos.org/nix/install)
```

### "Restore failed during ..."

Restore encountered an error during the specified phase. If automatic recovery
succeeded, your system is back to its pre-restore state. If not:

```bash
root status        # assess the state
root rollback --last  # attempt manual rollback
root doctor        # diagnose issues
```

### Stale mutation lock

If Root crashes during restore, the mutation lock may persist:

```bash
root doctor        # check if lock is stale
rm ~/.root/root.lockfile  # manual removal if stale
```

---

## Files

| Path | Purpose |
|------|---------|
| `~/.root/root.lock` | Active lockfile (overwritten on successful restore) |
| `~/.root/Rootfile` | Package version manifest (overwritten on successful restore) |
| `~/.root/root.lockfile` | Mutation guard lock (PID-based) |
| `~/.root/profiles/default` | Nix profile (mutated during restore) |
| `~/.root/snapshots/` | Pre-restore state snapshots |
| `~/.root/events.jsonl` | Event ledger |
