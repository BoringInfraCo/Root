# Root v0.4.0 Agent-Bundle Smoke Test

> **v0.4.1 note:** After the v0.4.1 release, `root --version` reports
> `root 0.4.1`. The Codex and OpenCode smoke path in this document is
> unchanged. Claude is a separate v0.4.1 smoke:
> [`V0_4_1_CLAUDE_SMOKE_TEST.md`](V0_4_1_CLAUDE_SMOKE_TEST.md).

Manual release validation for explicit portable agent-bundle transfer:
inspect, export, plan, apply (MCP disabled), verify, enable-plan, enable
(per-item `--approve` + env presence), verify enabled, byte-identical
rollback, and purge.

This is **explicit transfer only**. It is not `root restore`. It does not
read Rootfile `[agents]` and does not write `root.lock`. Lock schema stays
package-only emit 2 / max supported 3.

Adapters:

- `--agent codex` — exact version gate **0.150.1** (never relaxed)
- `--agent opencode` — exact version gate **1.18.27** (never relaxed)

Run the full automated CI sequence first, then execute these checks in an
isolated directory. Never point `HOME` / `CODEX_HOME` / `XDG_CONFIG_HOME` /
`ROOT_DIR` at a real user home.

A runnable isolated script that follows this path is
[`v0_4_agent_bundle_smoke.sh`](v0_4_agent_bundle_smoke.sh):

```bash
cargo build
ROOT_BIN=target/debug/root Docs/Release/v0_4_agent_bundle_smoke.sh
```

---

## Automated Gates

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build
target/debug/root --version
```

**Expected:** every command succeeds and the binary reports `root 0.4.0`.

Nix is **not** required for this smoke path.

---

## Isolation

Every command in this document must run under isolated env. Do not skip this.

### Shared (both adapters)

```bash
ISO=$(mktemp -d /tmp/root-v04-XXXX)
export HOME="$ISO/home"
export CODEX_HOME="$HOME/.codex"
export ROOT_DIR="$ISO/root"
export TMPDIR="$ISO/tmp"
mkdir -p "$HOME/.codex" "$HOME/.agents/skills" "$ROOT_DIR" "$TMPDIR"
# Keep the real Codex / OpenCode binaries findable:
export PATH="/path/to/codex-or-opencode-bin:$PATH"
ROOT="$PWD/target/debug/root"
```

### OpenCode extra isolation

```bash
export XDG_CONFIG_HOME="$ISO/xdg-config"
unset OPENCODE_CONFIG_DIR
mkdir -p "$XDG_CONFIG_HOME/opencode"
```

OpenCode smoke **must** unset `OPENCODE_CONFIG_DIR`. Config is
`$XDG_CONFIG_HOME/opencode` (never macOS `~/Library/Application Support`).

| Variable | Codex | OpenCode |
|----------|-------|----------|
| `HOME` | required | required |
| `CODEX_HOME` | required | set (harmless; unused by OpenCode adapter) |
| `ROOT_DIR` | required | required |
| `TMPDIR` | required | required |
| `XDG_CONFIG_HOME` | optional | required |
| `OPENCODE_CONFIG_DIR` | n/a | **unset** |

Dummy tokens used below (`S1_DUMMY_TOKEN_not-a-real-secret`,
`S2_DUMMY_TOKEN_not-a-real-secret`) exist only to satisfy env-*presence*
checks. They must never appear in bundle files, live config, journal
(`$ROOT_DIR/agent-apply.json`), snapshots (`$ROOT_DIR/agent-snapshots/`),
or command stdout/JSON.

After each enable, scan:

```bash
# Must print nothing.
grep -R "DUMMY_TOKEN_not-a-real-secret" "$ISO" "$bundle" 2>/dev/null || true
```

---

## Prerequisites

- macOS (Apple Silicon or Intel) is the primary smoke host. Linux transfer
  was validated separately for S1/S2; this document is the operator path.
- `codex --version` → `codex-cli 0.150.1` on PATH for Codex sections.
- `opencode --version` → `1.18.27` on PATH for OpenCode sections.
- `root` binary built from the v0.4.0 tree (`target/debug/root`).
- Isolated env as above. Do not use a real `~/.codex` or
  `~/.config/opencode`.
- No Nix required. Do **not** run `root restore` as part of this test.

---

## Exit Codes (this command)

| Code | When |
|------|------|
| 0 | Success |
| 1 | Generic failure (missing `--approve`, missing provenance, missing env, unsupported source version, FIFO/non-regular blob) |
| 2 | Invalid arguments (unsupported `--agent`, apply without `--apply`, rollback without `--last`) |
| 4 | `verify` checks failed |
| 5 | Drift: plan hash stale vs current target |

`root agent-bundle` does not use Nix exit 7/8 and does not write `root.lock`.

---

## 1. Codex (0.150.1)

Use the shared isolation block. Confirm the gate:

```bash
codex --version
# Expected: codex-cli 0.150.1
```

Seed a source working tree (unknown fields must be held, not exported):

```bash
cat > "$CODEX_HOME/AGENTS.md" <<'EOF'
# Source AGENTS.md
Use Root for installs.
EOF
cat > "$CODEX_HOME/config.toml" <<'EOF'
model = "gpt-5"
model_reasoning_effort = "high"
service_tier = "fast"
experimental_widget = "hold-this"

[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_vars = ["S1_DUMMY_TOKEN"]
enabled = true
transport = "stdio"
EOF
mkdir -p "$HOME/.agents/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
printf '%s\n' '# docs-writer' 'Write concise docs.' > "$HOME/.agents/skills/docs-writer/SKILL.md"
printf '%s\n' '#!/bin/sh' 'echo repo-helper' > "$HOME/.agents/skills/repo-helper/run.sh"
chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
```

### 1.1 Inspect

```bash
"$ROOT" agent-bundle inspect --agent codex
"$ROOT" --json agent-bundle inspect --agent codex
echo exit:$?
```

**Expected:**
- Exit 0.
- `present: true`, `version: 0.150.1`, `version_supported: true`.
- `codex_home` is the isolated `$CODEX_HOME`, not the real user home.
- No writes under `$ROOT_DIR`.

Unsupported adapter:

```bash
"$ROOT" agent-bundle inspect --agent claude; echo exit:$?
```

**Expected:** exit 2 (`unsupported bundle adapter`).

### 1.2 Export

```bash
bundle="$ISO/codex-bundle"
"$ROOT" --json agent-bundle export --agent codex \
  --out "$bundle" --no-timestamp \
  --skill docs-writer --skill repo-helper \
  --include-executable repo-helper \
  --include-mcp github
echo exit:$?
```

**Expected:**
- Exit 0. Bundle dir did not exist before export.
- `manifest.json` + `blobs/<sha256>`.
- `adapter: "codex"`, `source_agent_version: "0.150.1"`.
- MCP `github.enabled` is **false** in the manifest (source was true).
- `needs_env` lists `S1_DUMMY_TOKEN` (name only).
- `experimental_widget` is held, not in `settings`.
- Dummy token value is **absent** (the source config never stored a value).
- Live `$CODEX_HOME/config.toml` is unchanged by export.

FIFO / non-regular blob rejection (optional operator check):

```bash
bad="$ISO/fifo-bundle"
cp -R "$bundle" "$bad"
rm -f "$bad/blobs/"*
mkfifo "$bad/blobs/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
"$ROOT" agent-bundle plan --bundle "$bad"; echo exit:$?
```

**Expected:** non-zero (exit 1). Message mentions regular / non-regular file.
Do not hang on the FIFO.

### 1.3 Plan (no writes)

Reset the target to a *different* pre-apply tree (second-machine stand-in):

```bash
printf '%s\n' '# Target AGENTS.md' 'old prompt' > "$CODEX_HOME/AGENTS.md"
cat > "$CODEX_HOME/config.toml" <<'EOF'
model = "gpt-4.1"
model_reasoning_effort = "medium"
service_tier = "flex"
target_only_experimental = "preserve-this-value"
EOF
rm -rf "$HOME/.agents/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
```

Snapshot the pre-apply tree:

```bash
python3 - <<'PY'
import hashlib, os
root = os.environ["HOME"]
h = hashlib.sha256()
for base in [os.environ["CODEX_HOME"], os.path.join(root, ".agents")]:
    for dirpath, _, files in os.walk(base):
        for name in sorted(files):
            path = os.path.join(dirpath, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            rel = os.path.relpath(path, root)
            h.update(rel.encode()); h.update(open(path, "rb").read())
print(h.hexdigest())
PY
# record as T0
```

```bash
"$ROOT" --json agent-bundle plan --bundle "$bundle"
echo exit:$?
```

**Expected:**
- Exit 0. Footer / JSON includes `plan_hash`.
- `needs_approval` lists per-item sha256 values (executable skill + MCP
  command hash). No global approval flag exists.
- MCP preview says declarations will be added **disabled**.
- Target tree hash still equals T0 (plan is read-only).
- No `$ROOT_DIR/agent-apply.json` and no `agent-snapshots/`.

### 1.4 Apply (MCP disabled)

Without `--apply`:

```bash
"$ROOT" agent-bundle apply --bundle "$bundle" --plan-hash "$PLAN_HASH"
echo exit:$?
```

**Expected:** exit 2. Message: plan only, no writes. T0 unchanged.

Without `--approve`:

```bash
"$ROOT" agent-bundle apply --bundle "$bundle" --apply --plan-hash "$PLAN_HASH"
echo exit:$?
```

**Expected:** exit 1. Message contains `hash-bound approval`. T0 unchanged.

With per-item hashes from plan JSON (`needs_approval[].sha256`):

```bash
"$ROOT" --json agent-bundle apply --bundle "$bundle" \
  --apply --plan-hash "$PLAN_HASH" \
  --approve "$SKILL_SHA" --approve "$MCP_SHA"
echo exit:$?
```

**Expected:**
- Exit 0.
- AGENTS.md and skills match exported blobs. `run.sh` mode **0755**.
- `config.toml` allowlisted settings updated; `target_only_experimental`
  preserved.
- `[mcp_servers.github] enabled = false`.
- `env_vars` lists the name `S1_DUMMY_TOKEN` only. Dummy value absent.
- Journal `$ROOT_DIR/agent-apply.json` phase `done`. Provenance key is
  `codex:github` (`journal::mcp_provenance_key`).
- Snapshot under `$ROOT_DIR/agent-snapshots/`.

Stale plan hash after apply:

```bash
"$ROOT" agent-bundle apply --bundle "$bundle" \
  --apply --plan-hash "$PLAN_HASH" \
  --approve "$SKILL_SHA" --approve "$MCP_SHA"
echo exit:$?
```

**Expected:** exit 5 (`Drift detected`).

### 1.5 Verify (post-apply, MCP still disabled)

```bash
"$ROOT" --json agent-bundle verify --agent codex
echo exit:$?
```

**Expected:**
- Exit 0. `success: true`. `version` is `0.150.1`.
- Checks include `binary_present`, `version_supported`, `config_parses`.
- MCP remains disabled. No secret values in JSON.

### 1.6 Enable-plan / enable

```bash
unset S1_DUMMY_TOKEN
"$ROOT" --json agent-bundle enable-plan --agent codex --server github
echo exit:$?
```

**Expected:** exit 0. JSON has `plan_hash`, `descriptor_hash`,
`needs_env: ["S1_DUMMY_TOKEN"]`. No mutation.

Missing approve:

```bash
"$ROOT" agent-bundle enable --agent codex --server github \
  --plan-hash "$ENABLE_PLAN_HASH"
echo exit:$?
```

**Expected:** exit 1 (`approval`).

Missing env (approve present):

```bash
"$ROOT" agent-bundle enable --agent codex --server github \
  --plan-hash "$ENABLE_PLAN_HASH" --approve "$DESCRIPTOR_SHA"
echo exit:$?
```

**Expected:** exit 1 (`secret references missing`). Config still disabled.

Enable with dummy token (presence only):

```bash
export S1_DUMMY_TOKEN=S1_DUMMY_TOKEN_not-a-real-secret
"$ROOT" --json agent-bundle enable --agent codex --server github \
  --plan-hash "$ENABLE_PLAN_HASH" --approve "$DESCRIPTOR_SHA"
echo exit:$?
unset S1_DUMMY_TOKEN
grep -R "S1_DUMMY_TOKEN_not-a-real-secret" "$ISO" "$bundle" && echo LEAK
```

**Expected:**
- Exit 0. `mcp_servers.github.enabled = true`.
- Dummy token **does not persist** anywhere under `$ISO` or the bundle.
- Config still has `env_vars = ["S1_DUMMY_TOKEN"]` (name only).

### 1.7 Verify enabled

```bash
"$ROOT" --json agent-bundle verify --agent codex
echo exit:$?
```

**Expected:** exit 0. Version still 0.150.1. Config still parses. Verify is
secret-safe (no token values).

### 1.8 Rollback (byte-identical)

Enable created its own snapshot. Roll that back first (MCP returns to
disabled, post-apply tree):

```bash
"$ROOT" --json agent-bundle rollback --last
echo exit:$?
```

**Expected:** exit 0. `enabled = false` again. AGENTS.md / skills still the
applied blobs.

Then roll back the apply snapshot:

```bash
"$ROOT" --json agent-bundle rollback --last
echo exit:$?
```

**Expected:**
- Exit 0.
- Target regular-file tree is **byte-identical** to T0 (pre-apply
  `AGENTS.md` and `config.toml`).
- Created skill files **and** their created parent dirs are gone.
- Rollback without `--last` exits 2.

### 1.9 Purge

```bash
"$ROOT" agent-bundle purge; echo exit:$?
"$ROOT" --json agent-bundle purge --yes
echo exit:$?
test ! -d "$ROOT_DIR/agent-snapshots" -o -z "$(ls -A "$ROOT_DIR/agent-snapshots" 2>/dev/null)"
```

**Expected:**
- Without `--yes`: non-zero, no snapshots deleted.
- With `--yes`: exit 0. Agent snapshots removed.
- `$ROOT_DIR/root.lock` was never created by this command.

---

## 2. OpenCode (1.18.27)

Start from a **fresh** isolated tree (do not reuse the Codex `$ISO`). Apply
the shared isolation **and** the OpenCode extra isolation (`XDG_CONFIG_HOME`,
`OPENCODE_CONFIG_DIR` unset).

```bash
opencode --version
# Expected: 1.18.27
OC="$XDG_CONFIG_HOME/opencode"
```

Seed source config as **JSONC with a trailing comma** (strip must accept it):

```bash
cat > "$OC/AGENTS.md" <<'EOF'
# Source AGENTS.md
Use Root for installs.
EOF
cat > "$OC/opencode.jsonc" <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-sonnet-4-5",
  "unknown_source_experimental": "hold-this",
  "mcp": {
    "github": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
      "enabled": true,
      "environment": {
        "S2_DUMMY_TOKEN": "{env:S2_DUMMY_TOKEN}"
      },
    }
  },
}
EOF
mkdir -p "$OC/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
printf '%s\n' '# docs-writer' 'Write concise docs.' > "$OC/skills/docs-writer/SKILL.md"
printf '%s\n' '#!/bin/sh' 'echo repo-helper' > "$HOME/.agents/skills/repo-helper/run.sh"
chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
```

### 2.1 Inspect

```bash
"$ROOT" --json agent-bundle inspect --agent opencode
echo exit:$?
```

**Expected:**
- Exit 0.
- `present: true`, `version: 1.18.27`, `version_supported: true`.
- `config_dir` is `$XDG_CONFIG_HOME/opencode`, not the real
  `~/.config/opencode` and not `OPENCODE_CONFIG_DIR`.

### 2.2 Export

```bash
bundle="$ISO/opencode-bundle"
"$ROOT" --json agent-bundle export --agent opencode \
  --out "$bundle" --no-timestamp \
  --skill docs-writer --skill repo-helper \
  --include-executable repo-helper \
  --include-mcp github
echo exit:$?
```

**Expected:**
- Exit 0.
- `adapter: "opencode"`, `source_agent_version: "1.18.27"`.
- JSONC trailing comma did not fail export.
- `settings` contains allowlisted `model` only. `$schema` and
  `unknown_source_experimental` are held.
- MCP `github.enabled` is **false**. `needs_env` lists `S2_DUMMY_TOKEN`.
- Dummy token value absent from the bundle.

### 2.3 Plan (no writes)

Replace the target with a different `opencode.json` (unknown keys must be
preserved on apply):

```bash
rm -f "$OC/opencode.jsonc"
cat > "$OC/AGENTS.md" <<'EOF'
# Target AGENTS.md
old prompt
EOF
cat > "$OC/opencode.json" <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "model": "openai/gpt-4.1",
  "target_only_experimental": "preserve-this-value"
}
EOF
rm -rf "$OC/skills" "$HOME/.agents/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
```

Record T0 over `$XDG_CONFIG_HOME/opencode` + `$HOME/.agents` regular files,
then:

```bash
"$ROOT" --json agent-bundle plan --bundle "$bundle"
echo exit:$?
```

**Expected:** exit 0, `plan_hash` present, tree still T0.

### 2.4 Apply (MCP disabled)

Same negative gates as Codex: no `--apply` → exit 2; no `--approve` → exit 1.

```bash
"$ROOT" --json agent-bundle apply --bundle "$bundle" \
  --apply --plan-hash "$PLAN_HASH" \
  --approve "$SKILL_SHA" --approve "$MCP_SHA"
echo exit:$?
```

**Expected:**
- Exit 0.
- AGENTS.md / skills match blobs. `run.sh` mode **0755**.
- `mcp.github.enabled` is **false**.
- Env is `{env:S2_DUMMY_TOKEN}` — dummy value absent.
- Target unknowns (`$schema`, `target_only_experimental`) preserved.
- Provenance key is `opencode:github` (not `codex:github`).
- Stale re-apply of the old plan hash → exit 5.

### 2.5 Verify (post-apply)

```bash
"$ROOT" --json agent-bundle verify --agent opencode
echo exit:$?
```

**Expected:** exit 0, version `1.18.27`, MCP still disabled.

### 2.6 Enable-plan / enable

```bash
unset S2_DUMMY_TOKEN
"$ROOT" --json agent-bundle enable-plan --agent opencode --server github
```

**Expected:** exit 0. `needs_env` includes `S2_DUMMY_TOKEN`.

Missing provenance / approve / env each fail with exit 1. Codex provenance
must not authorize this enable (namespaced keys).

```bash
export S2_DUMMY_TOKEN=S2_DUMMY_TOKEN_not-a-real-secret
"$ROOT" --json agent-bundle enable --agent opencode --server github \
  --plan-hash "$ENABLE_PLAN_HASH" --approve "$DESCRIPTOR_SHA"
echo exit:$?
unset S2_DUMMY_TOKEN
grep -R "S2_DUMMY_TOKEN_not-a-real-secret" "$ISO" "$bundle" && echo LEAK
```

**Expected:**
- Exit 0. `mcp.github.enabled: true`.
- Dummy token **does not persist**. Config still uses `{env:S2_DUMMY_TOKEN}`.

### 2.7 Verify enabled

```bash
"$ROOT" --json agent-bundle verify --agent opencode
echo exit:$?
```

**Expected:** exit 0. Version still 1.18.27. Secret-safe JSON.

### 2.8 Rollback (byte-identical)

```bash
"$ROOT" agent-bundle rollback --last   # enable → disabled post-apply
"$ROOT" agent-bundle rollback --last   # apply → T0
```

**Expected:** second rollback restores a **byte-identical** T0 regular-file
tree. Created skill files and created parent dirs are gone.

### 2.9 Purge

Same as Codex: `purge` without `--yes` refuses; `purge --yes` deletes
`$ROOT_DIR/agent-snapshots`. No `root.lock` written.

---

## 3. Honesty Checks (both adapters)

These must remain true after the sections above:

1. **Not restore.** `root restore` was not used and is not a substitute.
2. **Not Rootfile.** Isolated `$ROOT_DIR` has no Rootfile requirement.
   `[agents]` in a Rootfile is still presence-only for `root status`.
3. **Not lock integration.** `$ROOT_DIR/root.lock` is absent (or unchanged
   if you pre-created one). Schema constants stay emit 2 / max 3.
4. **Exact version gates.** A binary that is not Codex `0.150.1` or
   OpenCode `1.18.27` is refused on export/apply/verify. Do not relax.
5. **No credential/session transfer.** `auth.json`, sessions, sqlite, and
   `mcp-auth.json` are not copied.
6. **Dummy tokens never persist.** Grep of bundle + isolated tree is empty.
7. **FIFO / non-regular blobs rejected.**
8. **Rollback is byte-identical** for the regular-file tree it snapshotted.

Cleanup:

```bash
rm -rf "$ISO"
```

---

## Appendix A — What this smoke path is not

v0.4.0 product path:

1. Inspect a live Codex 0.150.1 or OpenCode 1.18.27 tree in isolation.
2. Export a reviewed bundle (MCP forced disabled).
3. Plan on the target (hash-bound approvals).
4. Apply with `--apply --plan-hash --approve`.
5. Enable later with provenance + plan hash + `--approve` + env presence.
6. Roll back to byte-identical pre-mutation bytes.

v0.4.0 does **not**:

- restore agents via `root restore` / Rootfile / `root.lock`
- transfer credentials, sessions, or dummy token values
- enable MCP on apply
- accept any Codex version other than 0.150.1
- accept any OpenCode version other than 1.18.27
- translate Codex bundles into OpenCode (or the reverse)
