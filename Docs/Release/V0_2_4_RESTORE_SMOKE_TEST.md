# Root v0.2.4 Restore Reliability Smoke Test

Manual release validation focused on restore reliability: clean restore, dry run,
invalid lockfile rejection, partial failure with automatic rollback, drift
detection, and pre-mutation validation hardening.

Run the full automated CI sequence first, then execute these checks on a
disposable Root directory with real Nix.

---

## Automated Gates

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build
target/debug/root --version
```

**Expected:** every command succeeds and the binary reports `root 0.2.4`.

---

## Prerequisites

- macOS (Apple Silicon or Intel) or Linux
- Nix installed with `nix-command` and `flakes` experimental features enabled
- Internet access (for Nix builds and binary cache)
- No existing `~/.root` directory (or back it up before these tests)
- `root` binary built from the v0.2.4 tag

---

## 1. Clean Restore

Basic happy path: fresh install, backup lockfile, wipe state, restore from the
backup, verify the system is clean.

```bash
rm -rf ~/.root
root install terraform
```

**Expected:**
- Install succeeds.
- `terraform version` reports a version.
- Snapshot saved.

```bash
cp ~/.root/root.lock /tmp/root.lock
rm -rf ~/.root
root restore /tmp/root.lock
```

**Expected:**
- Restore begins from the saved lockfile.
- Output shows:
  - `Restored Root profile from /tmp/root.lock`
  - `Installed: terraform.`
  - `Snapshot saved: <snapshot-id>`
- Nix profile is recreated with terraform.
- Exit code 0.

```bash
root status
```

**Expected:**
- State is `Healthy`.
- `rootfile_packages`, `lockfile_packages`, and `profile_packages` all match
  (each `1` for terraform).
- No drift issues listed.
- Exit code 0.

```bash
root verify terraform
```

**Expected:**
- Binary `terraform` found in `~/.root/profiles/default/bin/terraform`.
- `terraform version` executes successfully.
- Verification SUCCESS.
- Exit code 0.

```bash
root history --limit 5
```

**Expected:**
- Install event for terraform with status `Completed`.
- Restore event with type `Restore` and status `Completed`.
- Both events have timestamps and snapshot IDs.

---

## 2. Dry Run

Verify that `--dry-run` shows a clear plan without mutating anything.

**Setup:** Ensure you have a working Root state (from Test 1) and a lockfile
at `/tmp/root.lock` that differs from the current profile (e.g., contains
different packages).

```bash
# Add a second package to a copy of the lockfile
cp /tmp/root.lock /tmp/root-extended.lock
# Manually add ffmpeg to the lockfile packages list
python3 -c "
import json
with open('/tmp/root-extended.lock') as f:
    lock = json.load(f)
# Add ffmpeg (simulate a lockfile with more packages)
import copy
ffmpeg_pkg = copy.deepcopy(lock['packages'][0])
ffmpeg_pkg['name'] = 'ffmpeg'
ffmpeg_pkg['installable'] = lock['packages'][0]['installable'].replace('terraform', 'ffmpeg')
ffmpeg_pkg['store_path'] = lock['packages'][0]['store_path'].replace('terraform', 'ffmpeg')
ffmpeg_pkg['store_paths'] = {k: v.replace('terraform', 'ffmpeg') for k, v in ffmpeg_pkg['store_paths'].items()}
ffmpeg_pkg['outputs'] = {k: type(v)(v.replace('terraform', 'ffmpeg')) if isinstance(v, str) else v for k, v in ffmpeg_pkg.get('outputs', {}).items()}
lock['packages'].append(ffmpeg_pkg)
with open('/tmp/root-extended.lock', 'w') as f:
    json.dump(lock, f, indent=2)
print('Added ffmpeg to lockfile')
"
```

```bash
root restore /tmp/root-extended.lock --dry-run
```

**Expected:**
- Output shows `Restore plan` with clearly labelled sections:
  - `Will install:` (ffmpeg).
  - `Will keep:` (terraform — already installed).
  - `Will remove:` (empty or absent if nothing to remove).
  - `Will update:` (empty or absent).
- Total packages listed matches lockfile content (2).
- **No mutation occurred.**
- `~/.root/profiles/default/bin/` does NOT contain ffmpeg.
- Exit code 0.

**Dry run with no changes:**

```bash
root restore /tmp/root.lock --dry-run
```

**Expected:**
- Plan shows `No changes needed.`
- All packages listed as `Will keep:`.
- Exit code 0.

### JSON dry run

```bash
root restore /tmp/root-extended.lock --dry-run --json
```

**Expected:**
- Valid JSON with fields: `success`, `lock_path`, `will_install`, `will_remove`,
  `will_keep`, `will_update`, `total_packages`.
- `success` is `true`.
- `will_install` contains `["ffmpeg"]`.
- Exit code 0.

**Cleanup:**

```bash
rm -f /tmp/root-extended.lock
```

---

## 3. Invalid Lockfile (`.drv` Injection)

Inject a `.drv` path into the lockfile's output path and verify Root rejects the
restore **before** any Nix mutation or profile change.

**Setup:** Start from a clean, valid Root state (terraform installed from Test 1).

```bash
# Save a clean copy
cp /tmp/root.lock /tmp/root-clean.lock

# Inject .drv paths into the output fields
cp /tmp/root.lock /tmp/root-drv.lock
python3 -c "
import json
with open('/tmp/root-drv.lock') as f:
    lock = json.load(f)
if lock['packages']:
    lock['packages'][0]['store_path'] = '/nix/store/xxxxx-fake-0.0.0.drv'
    for output_name in lock['packages'][0].get('outputs', {}):
        lock['packages'][0]['outputs'][output_name] = '/nix/store/xxxxx-fake-0.0.0.drv'
    for key in lock['packages'][0].get('store_paths', {}):
        lock['packages'][0]['store_paths'][key] = '/nix/store/xxxxx-fake-0.0.0.drv'
with open('/tmp/root-drv.lock', 'w') as f:
    json.dump(lock, f, indent=2)
"
```

```bash
root restore /tmp/root-drv.lock
```

**Expected:**
- Root **refuses** the mutation with a clear error message.
- Error mentions `.drv` path or derivation path in store path / output.
- Error mentions "failed validation before restore".
- **No mutation occurred** — the Nix profile is unchanged.
- `~/.root/profiles/default/bin/terraform` still works.
- Exit code 4 (verification failure).

```bash
root verify terraform
```

**Expected:**
- terraform still functional — the restore was rejected before any changes.
- Verification SUCCESS.

```bash
root status
```

**Expected:**
- State is `Healthy` (no drift).
- Original terraform install intact.

**Cleanup:**

```bash
cp /tmp/root-clean.lock ~/.root/root.lock
rm -f /tmp/root-drv.lock /tmp/root-clean.lock
```

---

## 4. Partial Failure with Automatic Rollback

Simulate a scenario where a restoration begins but one or more packages fail to
install. Root must preserve the previous state, record a failure event, and
automatically roll back the Nix profile.

**Setup method A — mock an unavailable package via the lockfile:**

Use a lockfile that references a package known to be unavailable.

```bash
# Create a lockfile with a package that resolves to a known-bad Nix attribute
cp /tmp/root.lock /tmp/root-bad.lock
python3 -c "
import json
with open('/tmp/root-bad.lock') as f:
    lock = json.load(f)
# Reuse the first package's structure but point to 'missing_pkg'
# which the MockNixAdapter treats as NotFound (or use a real
# attribute that Nix will fail to resolve)
lock['packages'][0]['name'] = 'nonexistent-pkg'
lock['packages'][0]['installable'] = 'nixpkgs#nonexistent-pkg'
with open('/tmp/root-bad.lock', 'w') as f:
    json.dump(lock, f, indent=2)
"
```

If the mock adapter is not available in production, alternatively corrupt the
profile path so Nix fails mid-mutation:

```bash
# Alternative setup: install a package, save state, then restore
# a lockfile with a package whose Nix attribute will fail
root install ffmpeg
cp ~/.root/root.lock /tmp/root-ffmpeg.lock
```

Then attempt a restore from a lockfile containing a package that
cannot be resolved:

```bash
root restore /tmp/root-bad.lock
```

**Expected:**
- Restore fails with an error message.
- Error indicates the failure phase (e.g., "package installation" or
  "pre-restore validation").
- **Previous Rootfile and root.lock are preserved.**
- **Automatic rollback** message appears: "Root automatically rolled
  back your Nix profile to its pre-restore state."
- Profile is restored to the state before the failed restore.
- Exit code 1.

```bash
root status
```

**Expected:**
- State is `Healthy`.
- Previous packages still listed (terraform or ffmpeg from setup).

```bash
root verify terraform
```

**Expected:**
- terraform still functions.
- Verification SUCCESS.

```bash
root history --limit 5
```

**Expected:**
- A `Restore` event with status `Failed`.
- A `RestoreRecovered` event with status `Completed` (automatic rollback).
- Error details recorded in the failure event.

**Cleanup:**

```bash
rm -f /tmp/root-bad.lock /tmp/root-ffmpeg.lock
```

---

## 5. Drift Detection

Create intentional mismatches between Rootfile, lockfile, and Nix profile, then
verify `root status` explains each drift clearly.

### 5a. Rootfile-Lockfile Mismatch

Add a package to Rootfile without running `root lock`.

```bash
echo '[packages]
bat = "latest"' >> ~/.root/Rootfile
```

```bash
root status
```

**Expected:**
- State is `Drifted`.
- A drift issue with category `rootfile-lockfile-mismatch`:
  `"Package 'bat' is in Rootfile but not in root.lock"`.
- Suggestion: `"Run 'root lock' to regenerate root.lock from Rootfile intent"`.

Revert:

```bash
# Remove the bat entry from Rootfile
python3 -c "
import toml
with open('$HOME/.root/Rootfile') as f:
    rf = toml.load(f)
rf.get('packages', {}).pop('bat', None)
with open('$HOME/.root/Rootfile', 'w') as f:
    toml.dump(rf, f)
"
```

### 5b. Lockfile-Profile Mismatch (Missing from Profile)

Manually remove a package from the Nix profile without updating the lockfile.

```bash
# Using Nix directly to remove terraform from the profile
nix profile remove ~/.root/profiles/default terraform 2>/dev/null || true
```

```bash
root status
```

**Expected:**
- State is `NeedsAttention`.
- A drift issue with category `lockfile-profile-mismatch`:
  `"Package 'terraform' is in root.lock but not in Nix profile"`.
- Suggestion: `"Run 'root sync' to install the locked package"`.

```bash
# Reinstall via sync
root sync
root status
```

**Expected:**
- terraform reinstalled.
- State returns to `Healthy`.
- No drift issues.

### 5c. Profile-Lockfile Mismatch (Extra in Profile)

Install a package via Nix directly (outside Root), creating a package in the
profile that the lockfile does not know about.

```bash
nix profile install ~/.root/profiles/default nixpkgs#bat 2>/dev/null || true

# If the above doesn't work, simulate by adding a package directly
nix profile add --profile ~/.root/profiles/default nixpkgs#bat 2>/dev/null || true
```

```bash
root status
```

**Expected:**
- State is `Drifted`.
- A drift issue with category `profile-lockfile-mismatch`:
  `"Package 'bat' is in Nix profile but not in root.lock"`.
- Suggestion: `"Run 'root sync' to remove the extra package"`.

```bash
root sync
root status
```

**Expected:**
- Extra package removed from profile.
- State returns to `Healthy`.
- No drift issues.

### 5d. Platform Mismatch

Manually edit the lockfile to set a different platform.

```bash
python3 -c "
import json
with open('$HOME/.root/root.lock') as f:
    lock = json.load(f)
lock['platform'] = 'x86_64-linux'
with open('$HOME/.root/root.lock', 'w') as f:
    json.dump(lock, f, indent=2)
"
```

```bash
root status
```

**Expected:**
- A drift issue with category `platform-mismatch`:
  `"root.lock was created on platform 'x86_64-linux' but current platform is
   '<actual-platform>'"`.
- Suggestion: `"Regenerate root.lock on the current platform"`.

```bash
# Fix the platform
root lock
root status
```

**Expected:**
- Platform corrected.
- State is `Healthy`.

### JSON drift output

```bash
root status --json
```

**Expected:**
- Valid JSON with `healthy`, `state`, `drift_details` fields.
- When drift exists, `drift_details` is an array of objects each with
  `category`, `description`, and `suggestion`.
- When healthy, `drift_details` is an empty array.
- Exit code 0.

---

## 6. Pre-Mutation Validation Chain

Verify that restore refuses to proceed when any pre-condition fails, before any
mutation occurs.

### 6a. Missing Nix

```bash
PATH=/usr/bin:/bin:/usr/sbin:/sbin root restore /tmp/root.lock
```

**Expected:**
- Error: "Restore validation failed: Nix is not available."
- Suggestion to install Nix.
- Exit code 7 (Nix unavailable).

### 6b. No Root Profile

**Setup:** Temporarily remove the Nix profile symlink.

```bash
mv ~/.root/profiles/default /tmp/root-profile-backup
root restore /tmp/root.lock
```

**Expected:**
- Restore succeeds and recreates `~/.root/profiles/default` via `nix profile add`.
- Locked packages are installed.
- Exit code 0.

**Cleanup:**

```bash
mv /tmp/root-profile-backup ~/.root/profiles/default
```

### 6c. Malformed Lockfile (Invalid JSON)

```bash
echo "this is not json" > /tmp/root-bad-json.lock
root restore /tmp/root-bad-json.lock
```

**Expected:**
- Error about invalid JSON, parse failure, or corrupt lockfile.
- Exit code 1.

**Cleanup:**

```bash
rm -f /tmp/root-bad-json.lock
```

---

## 7. Restore from Default Lockfile (No `--lock`)

When no `--lock` path is given, restore should use the current `~/.root/root.lock`
and reconcile the profile against it (like a self-healing sync).

```bash
# Remove terraform directly from Nix profile
nix profile remove ~/.root/profiles/default terraform 2>/dev/null || true

# Restore from the default lockfile location
root restore
```

**Expected:**
- Output: `Restored Root profile from ~/.root/root.lock` (or the resolved path).
- terraform reinstalled.
- Snapshot saved.
- Exit code 0.

```bash
root verify terraform
```

**Expected:**
- terraform functional.
- Verification SUCCESS.

---

## 8. Restore with Policy Denial

Verify that policy enforcement blocks a restore before any mutation.

```bash
# Apply a policy that denies restore
cat > /tmp/root-deny-restore.toml << 'EOF'
version = 1

[packages]
restore = "deny"
EOF

root policy apply /tmp/root-deny-restore.toml
root restore /tmp/root.lock
```

**Expected:**
- Error: "Policy denied: restore is not permitted."
- Exit code 9 (policy denial).
- No mutation.

**Cleanup:**

```bash
root policy apply /dev/null 2>/dev/null || rm ~/.root/policy.toml 2>/dev/null || true
rm -f /tmp/root-deny-restore.toml
```

---

## Validation Checklist

| Test Path                                              | Status | Notes |
|--------------------------------------------------------|--------|-------|
| 1. Clean restore — install, backup, wipe, restore, verify | ☐    | |
| 1b. Post-restore `root status` shows Healthy             | ☐    | |
| 1c. Post-restore `root verify terraform` succeeds        | ☐    | |
| 2. Dry run — shows install/remove/keep/update plan       | ☐    | |
| 2b. Dry run with no changes shows "No changes needed"    | ☐    | |
| 2c. JSON dry run is valid structured output              | ☐    | |
| 3. Invalid lockfile — `.drv` rejection before mutation   | ☐    | |
| 3b. State preserved after `.drv` rejection               | ☐    | |
| 4. Partial failure — automatic rollback                  | ☐    | |
| 4b. Previous Rootfile/root.lock preserved after failure  | ☐    | |
| 4c. Failure and recovery events in history               | ☐    | |
| 5a. Rootfile-lockfile mismatch drift detection           | ☐    | |
| 5b. Lockfile-profile mismatch drift detection            | ☐    | |
| 5c. Profile-lockfile mismatch drift detection            | ☐    | |
| 5d. Platform mismatch drift detection                    | ☐    | |
| 5e. JSON status output includes drift_details array      | ☐    | |
| 6a. Pre-mutation validation: missing Nix                 | ☐    | |
| 6b. Pre-mutation validation: missing profile             | ☐    | |
| 6c. Pre-mutation validation: malformed lockfile          | ☐    | |
| 7. Restore from default lockfile (no `--lock`)           | ☐    | |
| 8. Policy denial before restore mutation                 | ☐    | |
| No panics or crashes on any error path                   | ☐    | |
| Automatic rollback restores profile to pre-restore state | ☐    | |
| History records restore, failure, and recovery events    | ☐    | |

---

## Cleanup

```bash
# Remove temporary files
rm -f /tmp/root.lock /tmp/root-extended.lock /tmp/root-clean.lock
rm -f /tmp/root-drv.lock /tmp/root-bad.lock /tmp/root-ffmpeg.lock
rm -f /tmp/root-bad-json.lock /tmp/root-deny-restore.toml
rm -rf /tmp/root-profile-backup 2>/dev/null || true

# Remove any lingering Nix profile extras (installed outside Root)
nix profile remove --profile ~/.root/profiles/default bat 2>/dev/null || true
```

If you used a disposable Root directory via `ROOT_DIR`:

```bash
rm -rf "$ROOT_DIR"
unset ROOT_DIR
```
