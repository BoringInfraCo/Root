# Root v0.3.0 Pull-and-Verify Smoke Test

Manual release validation for declared Ollama models: plan (no mutation),
tag pull then digest verify, v3 lock write, status digest overlay, and
package-operation honesty.

This is **pull-and-verify**. It is not a model restore path. The v3 models map
is a verification record (`addressability: verification_record_only`). Root
talks to a local Ollama daemon at `127.0.0.1:11434`; it does not claim a
generic runtime adapter.

Run the full automated CI sequence first, then execute these checks on a
disposable Root directory with a reachable local Ollama daemon.

---

## Automated Gates

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build
target/debug/root --version
```

**Expected:** every command succeeds and the binary reports `root 0.3.0`.

---

## Prerequisites

- macOS (Apple Silicon or Intel). Linux Ollama backend was **not** smoke-tested
  in v0.3.0 — see [Appendix B](#appendix-b--linux).
- Local Ollama daemon at `127.0.0.1:11434` (`GET /api/version` succeeds).
- Internet access (Ollama tag pull).
- No existing `~/.root` directory (or back it up before these tests).
- `root` binary built from the v0.3.0 tree.

Rootfile `[models]` accept `runtime = "ollama"` only. Do not add `digest` or
`endpoint`. There is no `--relock` flag.

---

## 1. Rootfile Declaration

```bash
rm -rf ~/.root
mkdir -p ~/.root
cat > ~/.root/Rootfile <<'EOF'
[models."qwen3:8b"]
runtime = "ollama"
EOF
```

**Expected:**
- Rootfile contains only `runtime = "ollama"` for the model.
- No `digest` field.
- No `endpoint` field.

Reject extra Rootfile fields:

```bash
cat > /tmp/rootfile-bad-digest.toml <<'EOF'
[models."qwen3:8b"]
runtime = "ollama"
digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
EOF
# Copy over the live Rootfile only for this check, then restore.
cp ~/.root/Rootfile /tmp/rootfile-good.toml
cp /tmp/rootfile-bad-digest.toml ~/.root/Rootfile
root plan models; echo exit:$?
cp /tmp/rootfile-good.toml ~/.root/Rootfile
```

**Expected:**
- Command fails (unknown field `digest`).
- No `root.lock` written.
- No `model-pull.json` created.

```bash
root models pull --relock; echo exit:$?
root models pull --help
```

**Expected:**
- `--relock` is not a flag (`error: unexpected argument '--relock'` or clap
  unknown-argument).
- Help lists `root models pull [NAME]` only.

---

## 2. Plan (No Mutation)

```bash
root plan models
```

**Expected:**
- Output starts with `Unsupported operations:` including:
  - `digest_addressable_restore (ollama_api_pull_is_tag_only)`
  - `pull_by_digest (ollama_api_pull_is_tag_only)`
  - `delete_weights (not_in_v0.3_surface)`
  - `deterministic_restore (lock_is_verification_record_only)`
- Plan for `qwen3:8b` with `planned action: pull_tag_then_verify` (if the
  tag is not already present) or `already_verified` / `verify_only` if it is.
- Footer: `This is a preview. No changes have been made.`
- `~/.root/root.lock` does not exist (or is unchanged if it already did).
- `~/.root/model-pull.json` does not exist.
- Exit code 0.

```bash
root plan models --json
```

**Expected:**
- `"command": "plan models"`
- `"would_mutate": false`
- `"would_write_lock": false` on each model row
- `"addressability": "verification_record_only"`
- `unsupported_operations` includes `digest_addressable_restore` and
  `pull_by_digest`
- Exit code 0.

```bash
root plan models nope; echo exit:$?
```

**Expected:**
- Exit code 2.
- No lock write. No marker file.

---

## 3. Pull-and-Verify

```bash
root models pull --json
```

**Expected (tag missing locally):**
- Progress on stderr from `POST /api/pull` (tag `qwen3:8b` or resolved
  `qwen3:8b:latest` equivalent — Root pulls the declared tag).
- JSON `"command": "models pull"`.
- JSON `"models_restored": false`.
- JSON `"model_weights_deleted": false`.
- A result verb of `pulled_and_verified` (or `verified_and_locked` if the
  tag was already present and only the lock was missing).
- `"lock_written": true` for that row.
- `~/.root/root.lock` `version` is `3`.
- Lock entry under `models.ollama["qwen3:8b"]` has:
  - `"runtime": "ollama"`
  - `"observed_digest"` canonical `sha256:` + 64 lowercase hex
  - `"addressability": "verification_record_only"`
  - `"verification_method": "pull_tag_then_compare_tags_digest"` or
    `"inspect_tags_digest"`
  - optional loopback `"endpoint": "http://127.0.0.1:11434"` written by Root,
    not from Rootfile
- `~/.root/model-pull.json` is absent after the command returns.
- Exit code 0.

Re-run:

```bash
root models pull --json
```

**Expected:**
- Verb `already_verified`.
- `"lock_written": false`.
- `"models_restored": false`.
- Exit code 0.

Empty Rootfile:

```bash
# In a throwaway ROOT_DIR so this does not wipe the live test state:
ROOT_DIR=$(mktemp -d)
root models pull --json
echo exit:$?
rm -rf "$ROOT_DIR"
unset ROOT_DIR
```

**Expected:**
- Human: `No declared Ollama models.` (without `--json`) or JSON
  `"results": []` with `"models_restored": false`.
- Exit code 0.
- No lock. No marker.

---

## 4. Status Digest Overlay

```bash
root status
root status --json
```

**Expected:**
- Models section lists `qwen3:8b`, `ollama`, `present`, `satisfied`.
- Observed digest from `/api/tags` is shown raw.
- Locked digest is shown as `locked sha256:<64 hex>`.
- `digest match` (not `digest mismatch`).
- JSON `inventory.models[0]` includes `locked_digest` and
  `"digest_match": true`.
- No `model-digest-drift` issue.
- Exit code 0.

---

## 5. Restore / Rollback Honesty

These commands copy model lock entries. They do not pull or delete Ollama
weights.

```bash
root restore --dry-run --json
```

**Expected:**
- `"models_restored": false`
- `"model_weights_deleted": false`
- `"model_weights_retained": true`
- `model_note` states that Ollama weights were not pulled or deleted.
- Human text says models will not be pulled or deleted.
- Exit code 0.

```bash
cp ~/.root/root.lock /tmp/root-v3.lock
root restore --lock /tmp/root-v3.lock --json
```

**Expected:**
- `"models_restored": false`
- `model_lock_entries_written` is the inner-key copy count (1 for `qwen3:8b`).
- Ollama weights on disk / `GET /api/tags` are unchanged by restore.
- Exit code 0.

```bash
root rollback --last --json
```

**Expected (when a snapshot exists):**
- `"models_restored": false`
- Weights retained.
- Exit code 0.

---

## 6. Policy Deny

```bash
cat > ~/.root/policy.toml <<'EOF'
version = 1
[models]
pull = "deny"
EOF
root policy apply ~/.root/policy.toml
root models pull; echo exit:$?
```

**Expected:**
- Exit code 9.
- Message contains `Policy denied`.
- No new `model-pull.json`.
- Existing v3 lock left in place.

Remove the deny policy before further pulls.

---

## Appendix A — Live `@sha256` Probe (Not a Product Path)

**Required fact:** a live Ollama daemon may accept a pull name of the form
`tag@sha256:<hex>`. That is a backend probe result. It is **not** a Root
product path.

v0.3.0 product path:

1. Declare a tag in Rootfile with `runtime = "ollama"`.
2. `POST /api/pull` with that tag.
3. Compare the digest from `GET /api/tags`.
4. Write a v3 verification record.

v0.3.0 does **not**:

- accept `digest` or `endpoint` in Rootfile
- expose `--relock`
- pull by digest
- treat the lock as digest-addressable bits
- document `@sha256` as a user workflow

`root plan models` lists `digest_addressable_restore` and `pull_by_digest` as
unsupported (`ollama_api_pull_is_tag_only`).

Optional operator probe (do **not** add this to Rootfile or CI). If Ollama is
reachable:

```bash
# Appendix-only. Not a Root command.
curl -sS http://127.0.0.1:11434/api/version
# If you already have a tags digest from /api/tags, you may POST /api/pull
# with name "<tag>@sha256:<hex>" against the daemon. Whatever the daemon
# returns, Root still pulls by tag only.
```

This v0.3.0 docs pass did not re-run that live probe: no Ollama daemon was
listening on `127.0.0.1:11434`. The product rule does not depend on re-running
it. Even if the daemon accepts `@sha256` names, Root does not ship that path.

---

## Appendix B — Linux

Linux smoke for the Ollama backend was **not** run for v0.3.0.

This docs pass was on macOS. Docker was installed but the daemon was
unavailable, so no Linux container probe was executed.

Keep the existing limitation: macOS is the primary platform. Linux (aarch64
and x86_64) is supported by the codebase but not officially tested. **Do not
claim a tested Ollama backend on Linux in v0.3.0.**
