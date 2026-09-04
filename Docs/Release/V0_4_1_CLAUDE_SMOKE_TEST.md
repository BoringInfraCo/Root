# Root v0.4.1 Claude Agent-Bundle Smoke Test

Manual release validation for explicit Claude S3 portable agent-bundle
transfer: inspect, export (no MCP), plan, apply (`settings.json` `model`
+ skills; hash-bound `--approve`), verify, byte-identical rollback, and
purge. MCP is **held**. Apply never reads, writes, or snapshots
`.claude.json`.

This is **explicit transfer only**. It is not `root restore`. It does not
read Rootfile `[agents]` and does not write `root.lock`. Lock schema stays
package-only emit 2 / max supported 3.

Adapter:

- `--agent claude` — exact version gate **2.1.260** (never relaxed)

Codex 0.150.1 and OpenCode 1.18.27 remain on the v0.4.0 smoke path
([`V0_4_AGENT_BUNDLE_SMOKE_TEST.md`](V0_4_AGENT_BUNDLE_SMOKE_TEST.md)).
Do not mix those adapters into this document.

Run the full automated CI sequence first, then execute these checks in an
isolated directory. Never point `HOME` / `CLAUDE_CONFIG_DIR` / `ROOT_DIR`
at a real user home. Never use a real `~/.claude`.

A runnable isolated script that follows this path is
[`v0_4_1_claude_smoke.sh`](v0_4_1_claude_smoke.sh) (landed by the sibling
smoke-script task if present):

```bash
cargo build
ROOT_BIN=target/debug/root Docs/Release/v0_4_1_claude_smoke.sh
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

**Expected:** every command succeeds and the binary reports `root 0.4.1`.

Nix is **not** required for this smoke path.

---

## Isolation

Every command in this document must run under isolated env. Do not skip this.
Never use a real `~/.claude` or a real `$HOME/.claude.json`.

```bash
ISO=$(mktemp -d /tmp/root-v041-XXXX)
export HOME="$ISO/home"
export CLAUDE_CONFIG_DIR="$HOME/.claude"
export ROOT_DIR="$ISO/root"
export TMPDIR="$ISO/tmp"
mkdir -p "$HOME/.claude" "$HOME/.agents/skills" "$ROOT_DIR" "$TMPDIR"
# Keep the real Claude binary findable:
export PATH="/path/to/claude-bin:$PATH"
ROOT="$PWD/target/debug/root"
```

When `CLAUDE_CONFIG_DIR` is set, `.claude.json` lives **inside** that
dir (`$CLAUDE_CONFIG_DIR/.claude.json`). When it is unset, the file is
the sibling `$HOME/.claude.json` (not `$HOME/.claude/.claude.json`).
This smoke path **sets** `CLAUDE_CONFIG_DIR` so both Claude home and
global state resolve to the isolated dir.

| Variable | Claude |
|----------|--------|
| `HOME` | required |
| `CLAUDE_CONFIG_DIR` | required (set; non-empty) |
| `ROOT_DIR` | required |
| `TMPDIR` | required |

Do **not** export `CODEX_HOME` / `XDG_CONFIG_HOME` / `OPENCODE_CONFIG_DIR`
as part of this path.

---

## Prerequisites

- macOS (Apple Silicon or Intel) is the primary smoke host.
- Frozen `claude --version` → `2.1.260 (Claude Code)` on PATH.
- `root` binary built from the v0.4.1 tree (`target/debug/root`).
- Isolated env as above. Do not use a real `~/.claude`.
- No Nix required. Do **not** run `root restore` as part of this test.
- Stop any running Claude process before apply/rollback of
  `settings.json`.

---

## Exit Codes (this command)

| Code | When |
|------|------|
| 0 | Success |
| 1 | Generic failure (held MCP, missing `--approve`, unsupported source version, FIFO/non-regular blob) |
| 2 | Invalid arguments (unsupported `--agent`, apply without `--apply`, rollback without `--last`) |
| 4 | `verify` checks failed |
| 5 | Drift: plan hash stale vs current target |

`root agent-bundle` does not use Nix exit 7/8 and does not write `root.lock`.

Stable Claude MCP held error (exact string):

```
unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held.
```

JSON `message` equals that string. Human output prefixes `Error: `.

---

## 1. Claude (2.1.260)

Use the isolation block. Confirm the gate:

```bash
claude --version
# Expected: 2.1.260 (Claude Code)
```

Stop Claude if it is running (required before apply/rollback of
`settings.json`):

```bash
if pgrep -x claude >/dev/null 2>&1; then
  echo "STOP: a claude process is running. Stop it before apply/rollback."
  exit 1
fi
```

Seed a source working tree. Unknown settings keys must be held, not
exported. `.claude.json` is a canary: inspect may list user-scope MCP
*names*, but apply must never mutate this file.

```bash
CFG="$CLAUDE_CONFIG_DIR"
cat > "$CFG/CLAUDE.md" <<'EOF'
# Source CLAUDE.md
Use Root for installs.
EOF
cat > "$CFG/settings.json" <<'EOF'
{
  "model": "claude-sonnet-4-6",
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {"Stop": []}
}
EOF
cat > "$CFG/.claude.json" <<'EOF'
{"oauth":"do-not-copy","mcpServers":{"github":{"command":"npx"}}}
EOF
mkdir -p "$CFG/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
printf '%s\n' '# docs-writer' 'Write concise docs.' > "$CFG/skills/docs-writer/SKILL.md"
printf '%s\n' '#!/bin/sh' 'echo repo-helper' > "$HOME/.agents/skills/repo-helper/run.sh"
chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
```

Record the `.claude.json` canary hash (must be unchanged after every
mutation in this document):

```bash
python3 - <<'PY'
import hashlib, os
p = os.path.join(os.environ["CLAUDE_CONFIG_DIR"], ".claude.json")
print(hashlib.sha256(open(p, "rb").read()).hexdigest())
PY
# record as CLAUDE_JSON_T0
```

### 1.1 Inspect (isolated)

```bash
"$ROOT" agent-bundle inspect --agent claude
"$ROOT" --json agent-bundle inspect --agent claude
echo exit:$?
```

**Expected:**
- Exit 0.
- `present: true`, `version: 2.1.260`, `version_supported: true`.
- `config_dir` and `global_state_dir` are the isolated
  `$CLAUDE_CONFIG_DIR`, not the real user home.
- `claude_md_present: true`, `settings_present: true`.
- Skills include `docs-writer` and `repo-helper`.
- `mcp_servers` may list `github` (user-scope names only). Held reasons
  include the stable MCP held error. Inspect does not write
  `.claude.json`.
- No writes under `$ROOT_DIR`.

### 1.2 Export (no `--include-mcp`)

```bash
bundle="$ISO/claude-bundle"
"$ROOT" --json agent-bundle export --agent claude \
  --out "$bundle" --no-timestamp \
  --skill docs-writer --skill repo-helper \
  --include-executable repo-helper
echo exit:$?
```

**Expected:**
- Exit 0. Bundle dir did not exist before export.
- `manifest.json` + `blobs/<sha256>`.
- `adapter: "claude"`, `source_agent_version: "2.1.260"`.
- `settings` contains allowlisted `model` only. `permissions` / `hooks`
  are held, not in `settings`.
- `mcp` is **empty**. Files include `CLAUDE.md` (scope `claude_home`)
  and native/shared skills. Executable `run.sh` is present because of
  `--include-executable`.
- `.claude.json` is **not** a bundle file. Canary hash still equals
  `CLAUDE_JSON_T0`. Live `$CFG` is unchanged by export.

`--include-mcp` must fail with the stable held error (no bundle written):

```bash
"$ROOT" --json agent-bundle export --agent claude \
  --out "$ISO/claude-bundle-mcp" --no-timestamp \
  --include-mcp github
echo exit:$?
```

**Expected:** non-zero (exit 1). JSON `message` is exactly
`unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held.`
`$ISO/claude-bundle-mcp` does not exist.

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

Reset the target to a *different* pre-apply tree (second-machine stand-in).
Keep the `.claude.json` canary bytes:

```bash
printf '%s\n' '# Target CLAUDE.md' 'old prompt' > "$CFG/CLAUDE.md"
cat > "$CFG/settings.json" <<'EOF'
{
  "model": "old-model",
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {"Stop": []},
  "target_only_experimental": "preserve-this-value"
}
EOF
rm -rf "$CFG/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
```

Snapshot the pre-apply tree (regular files under `$CLAUDE_CONFIG_DIR`
and `$HOME/.agents`):

```bash
python3 - <<'PY'
import hashlib, os
root = os.environ["HOME"]
h = hashlib.sha256()
for base in [os.environ["CLAUDE_CONFIG_DIR"], os.path.join(root, ".agents")]:
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

Confirm `.claude.json` is still `CLAUDE_JSON_T0`.

```bash
"$ROOT" --json agent-bundle plan --bundle "$bundle"
echo exit:$?
```

**Expected:**
- Exit 0. Footer / JSON includes `plan_hash`.
- `needs_approval` lists per-item sha256 values for the executable
  skill. No global approval flag exists. No MCP approvals.
- Plan preconditions / will-create / will-update do **not** mention
  `.claude.json`.
- Target tree hash still equals T0 (plan is read-only).
- No `$ROOT_DIR/agent-apply.json` and no `agent-snapshots/`.

### 1.4 Apply (settings + skills; MCP held)

Confirm Claude is still not running, then:

Without `--apply`:

```bash
"$ROOT" agent-bundle apply --bundle "$bundle" --plan-hash "$PLAN_HASH"
echo exit:$?
```

**Expected:** exit 2. Message: plan only, no writes. T0 unchanged.
`.claude.json` still `CLAUDE_JSON_T0`.

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
  --approve "$SKILL_SHA"
echo exit:$?
```

**Expected:**
- Exit 0.
- `CLAUDE.md` and skills match exported blobs. `run.sh` mode **0755**.
  Native skill is under `$CFG/skills/docs-writer`. Shared skill is under
  `$HOME/.agents/skills/repo-helper`.
- `settings.json` `model` updated to `claude-sonnet-4-6`. Unknown target
  keys (`permissions`, `hooks`, `target_only_experimental`) preserved.
- `.claude.json` bytes **unchanged** (`CLAUDE_JSON_T0`).
- Snapshots under `$ROOT_DIR/agent-snapshots` do **not** include
  `.claude.json` as a file or as a `rel`.
- Journal `$ROOT_DIR/agent-apply.json` phase `done`.

Prove the snapshot omit:

```bash
python3 - <<'PY'
import os, json, pathlib
root = pathlib.Path(os.environ["ROOT_DIR"]) / "agent-snapshots"
hits = []
for p in root.rglob("*"):
    if not p.is_file():
        continue
    if p.name == ".claude.json" or "claude.json" in p.name:
        hits.append(str(p))
        continue
    if p.suffix == ".json":
        try:
            text = p.read_text()
        except OSError:
            continue
        if ".claude.json" in text:
            hits.append(str(p))
if hits:
    raise SystemExit("FAIL: snapshot mentions .claude.json: " + ", ".join(hits))
print("ok: no .claude.json in snapshots")
PY
```

Stale plan hash after apply:

```bash
"$ROOT" agent-bundle apply --bundle "$bundle" \
  --apply --plan-hash "$PLAN_HASH" \
  --approve "$SKILL_SHA"
echo exit:$?
```

**Expected:** exit 5 (`Drift detected`). `.claude.json` still
`CLAUDE_JSON_T0`.

### 1.5 Verify (post-apply)

```bash
"$ROOT" --json agent-bundle verify --agent claude
echo exit:$?
```

**Expected:**
- Exit 0. `success: true`. `version` is `2.1.260`.
- Checks include `binary_present`, `version_supported`, `config_parses`.
- No secret values in JSON. `.claude.json` still `CLAUDE_JSON_T0`.

### 1.6 Held MCP: enable-plan / enable

```bash
"$ROOT" --json agent-bundle enable-plan --agent claude --server github
echo exit:$?
"$ROOT" --json agent-bundle enable --agent claude --server github \
  --plan-hash deadbeef --approve deadbeef
echo exit:$?
```

**Expected:** both non-zero (exit 1). JSON `message` is exactly
`unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held.`
No mutation. `.claude.json` still `CLAUDE_JSON_T0`. No additional
snapshot.

### 1.7 Rollback (byte-identical)

Confirm Claude is still not running.

```bash
"$ROOT" --json agent-bundle rollback --last
echo exit:$?
```

**Expected:**
- Exit 0.
- Target regular-file tree is **byte-identical** to T0 (pre-apply
  `CLAUDE.md` and `settings.json`, including unknown keys).
- Created skill files **and** their created parent dirs are gone.
- `.claude.json` still `CLAUDE_JSON_T0` (rollback did not restore or
  rewrite it; it was never snapshotted).
- Rollback without `--last` exits 2.

### 1.8 Unsupported version refused

Do this in a subshell so the real `claude` stays on PATH afterwards:

```bash
(
  stub="$ISO/bad-claude-bin"
  mkdir -p "$stub"
  printf '%s\n' '#!/bin/sh' 'echo "2.1.259 (Claude Code)"' > "$stub/claude"
  chmod 0755 "$stub/claude"
  export PATH="$stub:$PATH"
  "$ROOT" --json agent-bundle inspect --agent claude
  "$ROOT" agent-bundle export --agent claude \
    --out "$ISO/bad-ver-bundle" --no-timestamp
  echo export_exit:$?
)
```

**Expected:**
- Inspect: `version: 2.1.259`, `version_supported: false`.
- Export: non-zero (exit 1). Message mentions `2.1.259` and `2.1.260`.
- `$ISO/bad-ver-bundle` does not exist.

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
- `.claude.json` still `CLAUDE_JSON_T0`.

---

## 2. Honesty Checks

These must remain true after the sections above:

1. **Not restore.** `root restore` was not used and is not a substitute.
2. **Not Rootfile.** Isolated `$ROOT_DIR` has no Rootfile requirement.
   `[agents]` in a Rootfile is still presence-only for `root status`.
3. **Not lock integration.** `$ROOT_DIR/root.lock` is absent (or unchanged
   if you pre-created one). Schema constants stay emit 2 / max 3.
4. **Exact version gate.** A binary that is not Claude Code `2.1.260` is
   refused on export/apply/verify. Do not relax. Codex `0.150.1` and
   OpenCode `1.18.27` are unchanged (separate smoke).
5. **Claude MCP is held.** Do not claim disable-until-enable for Claude.
   `--include-mcp`, `enable-plan`, and `enable` return the stable held
   error. Nonempty `mcp` bundles are invalid.
6. **No `.claude.json` mutation.** Apply, rollback, and snapshots never
   read/write/include `.claude.json`. Canary hash stays `CLAUDE_JSON_T0`.
7. **No credential/session transfer.** `.credentials.json`, Keychain,
   sessions, transcripts, and OAuth fields are not copied.
8. **FIFO / non-regular blobs rejected.**
9. **Rollback is byte-identical** for the regular-file tree it snapshotted.
10. **Stop Claude** during apply/rollback of `settings.json`.

Cleanup:

```bash
rm -rf "$ISO"
```

---

## Appendix A — What this smoke path is not

v0.4.1 Claude product path:

1. Inspect a live Claude Code 2.1.260 tree in isolation
   (`HOME` / `CLAUDE_CONFIG_DIR` / `ROOT_DIR` / `TMPDIR`).
2. Export a reviewed held-subset bundle (no `--include-mcp`).
3. Plan on the target (hash-bound approvals for executables).
4. Apply with `--apply --plan-hash --approve` (`settings.json` `model`
   + `CLAUDE.md` + skills). Stop Claude first.
5. Verify. Roll back to byte-identical pre-mutation bytes.
6. Prove `.claude.json` was never mutated or snapshotted.

v0.4.1 does **not**:

- restore agents via `root restore` / Rootfile / `root.lock`
- transfer credentials, sessions, OAuth, or `.claude.json`
- enable or disable Claude MCP (MCP is held)
- accept any Claude Code version other than 2.1.260
- relax Codex 0.150.1 or OpenCode 1.18.27
- translate Claude bundles into Codex/OpenCode (or the reverse)
- claim disable-until-enable for Claude
