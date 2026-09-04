#!/usr/bin/env bash
# Root v0.4.0 agent-bundle isolated smoke.
#
# Exercises inspect → export → plan → apply (MCP stays disabled) → verify →
# enable-plan → enable (--approve + env) → verify → rollback (byte-identical
# to post-apply) → purge.
#
# Isolation: throwaway TMPDIR/HOME/ROOT_DIR/CODEX_HOME (and XDG_CONFIG_HOME
# for OpenCode). OPENCODE_CONFIG_DIR is unset so resolution is
# $XDG_CONFIG_HOME/opencode, never the operator's ~/.codex, ~/.config/opencode,
# or credentials. Dummy MCP env tokens are passed only to `enable` and must
# never appear in real homes.
#
# Usage (from a built tree):
#   ROOT_BIN=target/debug/root Docs/Release/v0_4_agent_bundle_smoke.sh
#
# Do not source this file.

set -euo pipefail

if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  echo "FAIL: do not source this script; execute it" >&2
  return 1 2>/dev/null || exit 1
fi

SUPPORTED_CODEX="0.150.1"
SUPPORTED_OPENCODE="1.18.27"
MCP_ID="github"
CODEX_TOKEN_NAME="S1_DUMMY_TOKEN"
CODEX_TOKEN_VALUE="S1_DUMMY_TOKEN_not-a-real-secret"
OPENCODE_TOKEN_NAME="S2_DUMMY_TOKEN"
OPENCODE_TOKEN_VALUE="S2_DUMMY_TOKEN_not-a-real-secret"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
FAILED=0
WORK=""
REAL_HOME="${HOME:-}"
REAL_CODEX_HOME="${CODEX_HOME:-}"
REAL_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-}"
REAL_OPENCODE_CONFIG_DIR="${OPENCODE_CONFIG_DIR:-}"
REAL_ROOT_DIR="${ROOT_DIR:-}"
FP_BEFORE=""
CLEANED=0

usage_banner() {
  cat <<EOF
Root v0.4.0 agent-bundle isolated smoke
  WORK=$WORK
  REAL_HOME=$REAL_HOME
  Isolated HOME/ROOT_DIR/CODEX_HOME/XDG_CONFIG_HOME under WORK
  OPENCODE_CONFIG_DIR unset (OpenCode uses \$XDG_CONFIG_HOME/opencode)
  Dummy MCP tokens never exported into real homes
EOF
}

pass() {
  echo "PASS: $1"
  PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
  echo "FAIL: $1"
  FAIL_COUNT=$((FAIL_COUNT + 1))
  FAILED=1
}

skip() {
  echo "SKIP: $1"
  SKIP_COUNT=$((SKIP_COUNT + 1))
}

json_get() {
  python3 -c 'import json,sys
path=sys.argv[2].split(".")
with open(sys.argv[1]) as f:
    d=json.load(f)
for p in path:
    if isinstance(d, list) and p.isdigit():
        d=d[int(p)]
    else:
        d=d[p]
if isinstance(d, bool):
    sys.stdout.write("true" if d else "false")
elif d is None:
    pass
elif isinstance(d, (dict, list)):
    json.dump(d, sys.stdout)
else:
    sys.stdout.write(str(d))
print()
' "$1" "$2"
}

json_approvals() {
  python3 -c 'import json,sys
with open(sys.argv[1]) as f:
    d=json.load(f)
for item in d.get("needs_approval") or []:
    sha=item.get("sha256") or ""
    if sha:
        print(sha)
' "$1"
}

fingerprint_homes() {
  python3 -c 'import hashlib, json, os, sys
home=os.environ["REAL_HOME"]
paths=[]
for rel in (
    ".codex/config.toml",
    ".codex/auth.json",
    ".codex/AGENTS.md",
    ".config/opencode/opencode.json",
    ".config/opencode/opencode.jsonc",
    ".config/opencode/AGENTS.md",
    ".root/root.lock",
    ".root/Rootfile",
    ".root/agent-apply.json",
    ".local/share/opencode/mcp-auth.json",
):
    paths.append(os.path.join(home, rel))
for rel in (".config/opencode", ".root/agent-snapshots"):
    root=os.path.join(home, rel)
    if os.path.isdir(root):
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames.sort(); filenames.sort()
            for name in filenames:
                paths.append(os.path.join(dirpath, name))
seen=set(); entries=[]
for path in paths:
    if path in seen:
        continue
    seen.add(path)
    if not os.path.lexists(path):
        entries.append({"path": path, "present": False})
        continue
    st=os.lstat(path)
    item={"path": path, "present": True, "mode": st.st_mode, "size": st.st_size, "mtime_ns": st.st_mtime_ns}
    if os.path.isfile(path) and not os.path.islink(path) and st.st_size <= 8*1024*1024:
        h=hashlib.sha256()
        with open(path, "rb") as f:
            h.update(f.read())
        item["sha256"]=h.hexdigest()
    entries.append(item)
entries.sort(key=lambda e: e["path"])
json.dump(entries, sys.stdout)
'
}

assert_real_homes_unchanged() {
  local after
  after="$(fingerprint_homes)"
  python3 -c 'import json,sys
before=json.loads(open(sys.argv[1]).read())
after=json.loads(sys.argv[2])
if before != after:
    b={e["path"]: e for e in before}
    a={e["path"]: e for e in after}
    added=sorted(set(a)-set(b))
    removed=sorted(set(b)-set(a))
    changed=sorted(p for p in set(a)&set(b) if a[p]!=b[p])
    print("real home fingerprint drift")
    if added:
        print("  added: " + ", ".join(added[:20]))
    if removed:
        print("  removed: " + ", ".join(removed[:20]))
    if changed:
        print("  changed: " + ", ".join(changed[:20]))
    sys.exit(1)
' "$FP_BEFORE" "$after"
}

assert_dummy_tokens_absent_from_real_homes() {
  python3 -c 'import os,sys
needles=[
    os.environ["CODEX_TOKEN_NAME"],
    os.environ["CODEX_TOKEN_VALUE"],
    os.environ["OPENCODE_TOKEN_NAME"],
    os.environ["OPENCODE_TOKEN_VALUE"],
]
home=os.environ["REAL_HOME"]
roots=[]
for rel in (
    ".codex",
    ".config/opencode",
    ".root",
    ".agents",
    ".local/share/opencode",
    "Library/Application Support/opencode",
):
    path=os.path.join(home, rel)
    if os.path.isdir(path):
        roots.append(path)
hits=[]
for root in roots:
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        for name in filenames:
            path=os.path.join(dirpath, name)
            try:
                with open(path, "rb") as f:
                    data=f.read(8*1024*1024)
            except OSError:
                continue
            try:
                text=data.decode("utf-8")
            except UnicodeDecodeError:
                continue
            for needle in needles:
                if needle in text:
                    hits.append(path + ": " + needle)
if hits:
    print("dummy MCP token leaked into real home:")
    print("\n".join(hits[:20]))
    sys.exit(1)
'
}

assert_token_value_absent() {
  local path="$1"
  local value="$2"
  if grep -F -q -- "$value" "$path" 2>/dev/null; then
    echo "secret value leaked into $path"
    return 1
  fi
  return 0
}

cleanup() {
  local status=$?
  unset "$CODEX_TOKEN_NAME" "$OPENCODE_TOKEN_NAME" || true
  if [ -n "${REAL_HOME:-}" ]; then
    export HOME="$REAL_HOME"
  fi
  if [ -n "${REAL_CODEX_HOME:-}" ]; then
    export CODEX_HOME="$REAL_CODEX_HOME"
  else
    unset CODEX_HOME || true
  fi
  if [ -n "${REAL_XDG_CONFIG_HOME:-}" ]; then
    export XDG_CONFIG_HOME="$REAL_XDG_CONFIG_HOME"
  else
    unset XDG_CONFIG_HOME || true
  fi
  if [ -n "${REAL_OPENCODE_CONFIG_DIR:-}" ]; then
    export OPENCODE_CONFIG_DIR="$REAL_OPENCODE_CONFIG_DIR"
  else
    unset OPENCODE_CONFIG_DIR || true
  fi
  if [ -n "${REAL_ROOT_DIR:-}" ]; then
    export ROOT_DIR="$REAL_ROOT_DIR"
  else
    unset ROOT_DIR || true
  fi
  if [ "$CLEANED" -eq 0 ] && [ -n "$FP_BEFORE" ] && [ -f "$FP_BEFORE" ]; then
    if ! assert_real_homes_unchanged; then
      echo "FAIL: isolation: real homes mutated" >&2
      FAILED=1
      FAIL_COUNT=$((FAIL_COUNT + 1))
    elif ! assert_dummy_tokens_absent_from_real_homes; then
      echo "FAIL: isolation: dummy MCP token leaked into real home" >&2
      FAILED=1
      FAIL_COUNT=$((FAIL_COUNT + 1))
    else
      echo "PASS: isolation: real homes unchanged, dummy tokens absent"
      PASS_COUNT=$((PASS_COUNT + 1))
    fi
  fi
  unset CODEX_TOKEN_NAME CODEX_TOKEN_VALUE OPENCODE_TOKEN_NAME OPENCODE_TOKEN_VALUE || true
  if [ -n "$WORK" ] && [ -d "$WORK" ]; then
    rm -rf "$WORK"
  fi
  CLEANED=1
  echo
  echo "Summary: PASS=$PASS_COUNT FAIL=$FAIL_COUNT SKIP=$SKIP_COUNT"
  if [ "$FAILED" -ne 0 ]; then
    exit 1
  fi
  exit "$status"
}

trap cleanup EXIT INT HUP TERM

if [ -z "$REAL_HOME" ] || [ ! -d "$REAL_HOME" ]; then
  echo "FAIL: HOME is unset or not a directory; refusing to run" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
if [ -n "${ROOT_BIN:-}" ]; then
  :
elif [ -x "$REPO_ROOT/target/debug/root" ]; then
  ROOT_BIN="$REPO_ROOT/target/debug/root"
elif [ -x "$REPO_ROOT/target/release/root" ]; then
  ROOT_BIN="$REPO_ROOT/target/release/root"
else
  echo "FAIL: root binary not found. Build with cargo build or set ROOT_BIN." >&2
  exit 1
fi
if [ ! -x "$ROOT_BIN" ]; then
  echo "FAIL: ROOT_BIN is not executable: $ROOT_BIN" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "FAIL: python3 is required to parse --json output" >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/root-v04-agent-bundle-smoke.XXXXXX")"
mkdir -p "$WORK/tmp" "$WORK/logs" "$WORK/fingerprints" "$WORK/bundles"
export TMPDIR="$WORK/tmp"
FP_BEFORE="$WORK/fingerprints/before.json"
export REAL_HOME
export CODEX_TOKEN_NAME CODEX_TOKEN_VALUE OPENCODE_TOKEN_NAME OPENCODE_TOKEN_VALUE
fingerprint_homes >"$FP_BEFORE"

usage_banner
echo "  ROOT_BIN=$ROOT_BIN"

ROOT_VER="$("$ROOT_BIN" --version 2>/dev/null || true)"
echo "  $ROOT_VER"
case "$ROOT_VER" in
  *0.4.0*) pass "root --version reports 0.4.0" ;;
  *) fail "root --version is not 0.4.0 (got: $ROOT_VER)" ;;
esac

isolate_run() {
  local run="$1"
  mkdir -p \
    "$run/home/.codex" \
    "$run/home/.agents/skills" \
    "$run/home/.config" \
    "$run/xdg-config/opencode" \
    "$run/xdg-data" \
    "$run/xdg-state" \
    "$run/xdg-cache" \
    "$run/root" \
    "$run/tmp" \
    "$run/logs"
  export HOME="$run/home"
  export ROOT_DIR="$run/root"
  export CODEX_HOME="$run/home/.codex"
  export XDG_CONFIG_HOME="$run/xdg-config"
  export XDG_DATA_HOME="$run/xdg-data"
  export XDG_STATE_HOME="$run/xdg-state"
  export XDG_CACHE_HOME="$run/xdg-cache"
  export TMPDIR="$run/tmp"
  unset OPENCODE_CONFIG_DIR || true
  unset OPENCODE_CONFIG || true
  if [ "$HOME" = "$REAL_HOME" ]; then
    echo "FAIL: isolation collapsed onto real HOME" >&2
    return 1
  fi
  if [ "$CODEX_HOME" = "$REAL_HOME/.codex" ] || [ "$CODEX_HOME" = "${REAL_CODEX_HOME:-}" ]; then
    echo "FAIL: isolation collapsed onto real CODEX_HOME" >&2
    return 1
  fi
}

run_root() {
  env -u OPENCODE_CONFIG_DIR -u OPENCODE_CONFIG \
    HOME="$HOME" \
    ROOT_DIR="$ROOT_DIR" \
    CODEX_HOME="$CODEX_HOME" \
    XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_STATE_HOME="$XDG_STATE_HOME" \
    XDG_CACHE_HOME="$XDG_CACHE_HOME" \
    TMPDIR="$TMPDIR" \
    PATH="$PATH" \
    "$ROOT_BIN" "$@"
}

write_codex_source() {
  mkdir -p \
    "$CODEX_HOME" \
    "$HOME/.agents/skills/docs-writer" \
    "$HOME/.agents/skills/repo-helper"
  cat >"$CODEX_HOME/AGENTS.md" <<'EOF'
# Source AGENTS.md
Use Root for installs.
EOF
  cat >"$CODEX_HOME/config.toml" <<EOF
model = "gpt-5"
model_reasoning_effort = "high"
service_tier = "fast"
experimental_widget = "hold-this"

[mcp_servers.${MCP_ID}]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_vars = ["${CODEX_TOKEN_NAME}"]
enabled = true
transport = "stdio"
EOF
  cat >"$HOME/.agents/skills/docs-writer/SKILL.md" <<'EOF'
# docs-writer
Write concise docs.
EOF
  cat >"$HOME/.agents/skills/repo-helper/run.sh" <<'EOF'
#!/bin/sh
echo repo-helper
EOF
  chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
}

write_codex_target() {
  mkdir -p "$CODEX_HOME"
  cat >"$CODEX_HOME/AGENTS.md" <<'EOF'
# Target AGENTS.md
old prompt
EOF
  cat >"$CODEX_HOME/config.toml" <<'EOF'
model = "gpt-4.1"
model_reasoning_effort = "medium"
service_tier = "flex"
target_only_experimental = "preserve-this-value"
EOF
  rm -rf "$HOME/.agents/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
  mkdir -p "$HOME/.agents/skills"
}

write_opencode_source() {
  local oc="$XDG_CONFIG_HOME/opencode"
  mkdir -p \
    "$oc/skills/docs-writer" \
    "$HOME/.agents/skills/repo-helper"
  cat >"$oc/AGENTS.md" <<'EOF'
# Source AGENTS.md
Use Root for installs.
EOF
  cat >"$oc/opencode.jsonc" <<EOF
{
  "\$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-sonnet-4-5",
  "unknown_source_experimental": "hold-this",
  "mcp": {
    "${MCP_ID}": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
      "enabled": true,
      "environment": {
        "${OPENCODE_TOKEN_NAME}": "{env:${OPENCODE_TOKEN_NAME}}"
      },
    }
  },
}
EOF
  cat >"$oc/skills/docs-writer/SKILL.md" <<'EOF'
# docs-writer
Write concise docs.
EOF
  cat >"$HOME/.agents/skills/repo-helper/run.sh" <<'EOF'
#!/bin/sh
echo repo-helper
EOF
  chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
}

write_opencode_target() {
  local oc="$XDG_CONFIG_HOME/opencode"
  mkdir -p "$oc"
  rm -f "$oc/opencode.jsonc"
  cat >"$oc/AGENTS.md" <<'EOF'
# Target AGENTS.md
old prompt
EOF
  cat >"$oc/opencode.json" <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "model": "openai/gpt-4.1",
  "target_only_experimental": "preserve-this-value"
}
EOF
  rm -rf "$oc/skills" "$HOME/.agents/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
}

codex_mcp_enabled() {
  python3 -c 'import sys, tomllib
path, server = sys.argv[1], sys.argv[2]
with open(path, "rb") as f:
    data = tomllib.load(f)
print("true" if data.get("mcp_servers", {}).get(server, {}).get("enabled") else "false")
' "$1" "$2"
}

opencode_mcp_enabled() {
  python3 -c 'import json,sys
path, server = sys.argv[1], sys.argv[2]
data=json.load(open(path))
print("true" if data.get("mcp", {}).get(server, {}).get("enabled") else "false")
' "$1" "$2"
}

journal_has_namespaced_key() {
  python3 -c 'import json,sys
path, key = sys.argv[1], sys.argv[2]
data=json.load(open(path))
prov=data.get("mcp_provenance") or {}
if key not in prov:
    print("missing namespaced provenance key " + key + "; have " + ",".join(sorted(prov)))
    sys.exit(1)
bare=key.split(":", 1)[-1]
if bare in prov:
    print("un-namespaced provenance key leaked: " + bare)
    sys.exit(1)
' "$1" "$2"
}

smoke_adapter() {
  local agent="$1"
  local required_version="$2"
  local token_name="$3"
  local token_value="$4"
  local run="$WORK/runs/$agent"
  local logs="$run/logs"
  local bundle="$WORK/bundles/$agent"
  local config_path agents_path inspect_home_field
  local plan_json apply_json enable_plan_json
  local plan_hash enable_hash descriptor_hash
  local after_apply_config after_apply_agents t0_config t0_agents
  local inspect_home live_home
  local approve_args hash
  local rc

  isolate_run "$run"
  mkdir -p "$logs"

  echo
  echo "======== adapter: $agent ========"

  if [ "$agent" = "codex" ]; then
    write_codex_source
    config_path="$CODEX_HOME/config.toml"
    agents_path="$CODEX_HOME/AGENTS.md"
    inspect_home_field="codex_home"
    live_home="$CODEX_HOME"
  else
    write_opencode_source
    config_path="$XDG_CONFIG_HOME/opencode/opencode.json"
    agents_path="$XDG_CONFIG_HOME/opencode/AGENTS.md"
    inspect_home_field="config_dir"
    live_home="$XDG_CONFIG_HOME/opencode"
  fi

  if ! run_root --json agent-bundle inspect --agent "$agent" >"$logs/inspect.json" 2>"$logs/inspect.err"; then
    fail "$agent inspect"
    cat "$logs/inspect.err" >&2 || true
    return 1
  fi
  inspect_home="$(json_get "$logs/inspect.json" "$inspect_home_field")"
  if [ "$(json_get "$logs/inspect.json" "present")" != "true" ]; then
    fail "$agent inspect: CLI not present"
    return 1
  fi
  case "$inspect_home" in
    "$live_home") ;;
    *)
      fail "$agent inspect: home is $inspect_home, expected isolated $live_home"
      return 1
      ;;
  esac
  case "$inspect_home" in
    "$REAL_HOME"/*)
      fail "$agent inspect: leaked real HOME path $inspect_home"
      return 1
      ;;
  esac
  if [ "$(json_get "$logs/inspect.json" "version")" != "$required_version" ]; then
    fail "$agent inspect: version $(json_get "$logs/inspect.json" version) is not $required_version"
    return 1
  fi
  if [ "$(json_get "$logs/inspect.json" "version_supported")" != "true" ]; then
    fail "$agent inspect: version_supported is not true (gate is $required_version)"
    return 1
  fi
  pass "$agent inspect (isolated home, version $required_version)"

  rm -rf "$bundle"
  if ! run_root --json agent-bundle export \
    --agent "$agent" \
    --out "$bundle" \
    --skill docs-writer \
    --skill repo-helper \
    --include-mcp "$MCP_ID" \
    --include-executable repo-helper \
    --no-timestamp \
    >"$logs/export.json" 2>"$logs/export.err"; then
    fail "$agent export"
    cat "$logs/export.err" >&2 || true
    return 1
  fi
  if [ ! -f "$bundle/manifest.json" ]; then
    fail "$agent export: manifest.json missing"
    return 1
  fi
  if [ "$(json_get "$logs/export.json" "adapter")" != "$agent" ]; then
    fail "$agent export: adapter mismatch"
    return 1
  fi
  if [ "$(json_get "$logs/export.json" "source_agent_version")" != "$required_version" ]; then
    fail "$agent export: source_agent_version is not $required_version"
    return 1
  fi
  python3 -c 'import json,sys
m=json.load(open(sys.argv[1]))
mcp_id=sys.argv[2]
entry=(m.get("mcp") or {}).get(mcp_id)
if entry is None:
    raise SystemExit("exported MCP missing")
if entry.get("enabled") is not False:
    raise SystemExit("exported MCP must be enabled=false")
' "$logs/export.json" "$MCP_ID"
  pass "$agent export (MCP disabled in bundle, version-gated $required_version)"

  if [ "$agent" = "codex" ]; then
    write_codex_target
    config_path="$CODEX_HOME/config.toml"
    agents_path="$CODEX_HOME/AGENTS.md"
  else
    write_opencode_target
    config_path="$XDG_CONFIG_HOME/opencode/opencode.json"
    agents_path="$XDG_CONFIG_HOME/opencode/AGENTS.md"
  fi
  t0_config="$logs/t0.config"
  t0_agents="$logs/t0.agents"
  cp "$config_path" "$t0_config"
  cp "$agents_path" "$t0_agents"

  plan_json="$logs/plan.json"
  if ! run_root --json agent-bundle plan --bundle "$bundle" >"$plan_json" 2>"$logs/plan.err"; then
    fail "$agent plan"
    cat "$logs/plan.err" >&2 || true
    return 1
  fi
  plan_hash="$(json_get "$plan_json" "plan_hash")"
  if [ -z "$plan_hash" ]; then
    fail "$agent plan: empty plan_hash"
    return 1
  fi
  pass "$agent plan (hash $plan_hash)"

  approve_args=()
  while IFS= read -r hash; do
    [ -n "$hash" ] || continue
    approve_args+=(--approve "$hash")
  done <<EOF
$(json_approvals "$plan_json")
EOF
  if [ "${#approve_args[@]}" -eq 0 ]; then
    fail "$agent apply: plan listed no --approve hashes (MCP/executable require hash-bound approval)"
    return 1
  fi

  apply_json="$logs/apply.json"
  if ! run_root --json agent-bundle apply \
    --bundle "$bundle" \
    --apply \
    --plan-hash "$plan_hash" \
    "${approve_args[@]}" \
    >"$apply_json" 2>"$logs/apply.err"; then
    fail "$agent apply"
    cat "$logs/apply.err" >&2 || true
    return 1
  fi
  if [ "$agent" = "codex" ]; then
    if [ "$(codex_mcp_enabled "$config_path" "$MCP_ID")" != "false" ]; then
      fail "$agent apply: MCP $MCP_ID is not disabled"
      return 1
    fi
  else
    if [ "$(opencode_mcp_enabled "$config_path" "$MCP_ID")" != "false" ]; then
      fail "$agent apply: MCP $MCP_ID is not disabled"
      return 1
    fi
  fi
  if ! assert_token_value_absent "$config_path" "$token_value"; then
    fail "$agent apply: dummy token value written into config"
    return 1
  fi
  if ! journal_has_namespaced_key "$ROOT_DIR/agent-apply.json" "$agent:$MCP_ID"; then
    fail "$agent apply: namespaced provenance $agent:$MCP_ID missing"
    return 1
  fi
  pass "$agent apply (MCP stays disabled, provenance $agent:$MCP_ID)"

  if ! run_root --json agent-bundle verify --agent "$agent" >"$logs/verify-after-apply.json" 2>"$logs/verify-after-apply.err"; then
    fail "$agent verify after apply"
    cat "$logs/verify-after-apply.err" >&2 || true
    return 1
  fi
  if [ "$(json_get "$logs/verify-after-apply.json" "success")" != "true" ]; then
    fail "$agent verify after apply: success=false"
    return 1
  fi
  pass "$agent verify after apply"

  after_apply_config="$logs/after-apply.config"
  after_apply_agents="$logs/after-apply.agents"
  cp "$config_path" "$after_apply_config"
  cp "$agents_path" "$after_apply_agents"

  enable_plan_json="$logs/enable-plan.json"
  if ! run_root --json agent-bundle enable-plan --agent "$agent" --server "$MCP_ID" \
    >"$enable_plan_json" 2>"$logs/enable-plan.err"; then
    fail "$agent enable-plan"
    cat "$logs/enable-plan.err" >&2 || true
    return 1
  fi
  enable_hash="$(json_get "$enable_plan_json" "plan_hash")"
  descriptor_hash="$(json_get "$enable_plan_json" "descriptor_hash")"
  if [ -z "$enable_hash" ] || [ -z "$descriptor_hash" ]; then
    fail "$agent enable-plan: missing plan_hash or descriptor_hash"
    return 1
  fi
  pass "$agent enable-plan"

  rc=0
  run_root --json agent-bundle enable \
    --agent "$agent" \
    --server "$MCP_ID" \
    --plan-hash "$enable_hash" \
    --approve "$descriptor_hash" \
    >"$logs/enable-missing-env.json" 2>"$logs/enable-missing-env.err" || rc=$?
  if [ "$rc" -eq 0 ]; then
    fail "$agent enable: succeeded without required env $token_name"
    return 1
  fi
  pass "$agent enable refused without env $token_name (exit $rc)"

  if ! env -u OPENCODE_CONFIG_DIR -u OPENCODE_CONFIG \
    HOME="$HOME" \
    ROOT_DIR="$ROOT_DIR" \
    CODEX_HOME="$CODEX_HOME" \
    XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_STATE_HOME="$XDG_STATE_HOME" \
    XDG_CACHE_HOME="$XDG_CACHE_HOME" \
    TMPDIR="$TMPDIR" \
    PATH="$PATH" \
    "$token_name=$token_value" \
    "$ROOT_BIN" --json agent-bundle enable \
      --agent "$agent" \
      --server "$MCP_ID" \
      --plan-hash "$enable_hash" \
      --approve "$descriptor_hash" \
      >"$logs/enable.json" 2>"$logs/enable.err"; then
    fail "$agent enable with --approve + env"
    cat "$logs/enable.err" >&2 || true
    return 1
  fi
  if [ "$agent" = "codex" ]; then
    if [ "$(codex_mcp_enabled "$config_path" "$MCP_ID")" != "true" ]; then
      fail "$agent enable: MCP $MCP_ID is not enabled"
      return 1
    fi
  else
    if [ "$(opencode_mcp_enabled "$config_path" "$MCP_ID")" != "true" ]; then
      fail "$agent enable: MCP $MCP_ID is not enabled"
      return 1
    fi
  fi
  if ! assert_token_value_absent "$config_path" "$token_value"; then
    fail "$agent enable: dummy token value written into config"
    return 1
  fi
  pass "$agent enable (--approve $descriptor_hash + env $token_name)"

  if ! run_root --json agent-bundle verify --agent "$agent" >"$logs/verify-after-enable.json" 2>"$logs/verify-after-enable.err"; then
    fail "$agent verify after enable"
    cat "$logs/verify-after-enable.err" >&2 || true
    return 1
  fi
  if [ "$(json_get "$logs/verify-after-enable.json" "success")" != "true" ]; then
    fail "$agent verify after enable: success=false"
    return 1
  fi
  pass "$agent verify after enable"

  if ! run_root --json agent-bundle rollback --last >"$logs/rollback.json" 2>"$logs/rollback.err"; then
    fail "$agent rollback --last"
    cat "$logs/rollback.err" >&2 || true
    return 1
  fi
  if ! cmp -s "$config_path" "$after_apply_config"; then
    fail "$agent rollback: config is not byte-identical to post-apply"
    return 1
  fi
  if ! cmp -s "$agents_path" "$after_apply_agents"; then
    fail "$agent rollback: AGENTS.md is not byte-identical to post-apply"
    return 1
  fi
  if [ "$agent" = "codex" ]; then
    if [ "$(codex_mcp_enabled "$config_path" "$MCP_ID")" != "false" ]; then
      fail "$agent rollback: MCP $MCP_ID is not disabled after rollback"
      return 1
    fi
  else
    if [ "$(opencode_mcp_enabled "$config_path" "$MCP_ID")" != "false" ]; then
      fail "$agent rollback: MCP $MCP_ID is not disabled after rollback"
      return 1
    fi
  fi
  pass "$agent rollback --last (byte-identical to post-apply, MCP disabled)"

  if ! run_root --json agent-bundle rollback --last >"$logs/rollback-apply.json" 2>"$logs/rollback-apply.err"; then
    fail "$agent rollback --last (apply snapshot)"
    cat "$logs/rollback-apply.err" >&2 || true
    return 1
  fi
  if ! cmp -s "$config_path" "$t0_config"; then
    fail "$agent rollback: config is not byte-identical to pre-apply T0"
    return 1
  fi
  if ! cmp -s "$agents_path" "$t0_agents"; then
    fail "$agent rollback: AGENTS.md is not byte-identical to pre-apply T0"
    return 1
  fi
  if [ -e "$HOME/.agents/skills/repo-helper/run.sh" ] || [ -e "$HOME/.agents/skills/docs-writer/SKILL.md" ]; then
    fail "$agent rollback: created skills were not tombstoned"
    return 1
  fi
  pass "$agent rollback --last (byte-identical to pre-apply T0, skills tombstoned)"

  if ! run_root --json agent-bundle purge --yes >"$logs/purge.json" 2>"$logs/purge.err"; then
    fail "$agent purge --yes"
    cat "$logs/purge.err" >&2 || true
    return 1
  fi
  if [ -d "$ROOT_DIR/agent-snapshots" ] && [ -n "$(ls -A "$ROOT_DIR/agent-snapshots" 2>/dev/null || true)" ]; then
    fail "$agent purge: agent-snapshots still populated"
    return 1
  fi
  pass "$agent purge --yes"
}

CODEX_BIN="$(command -v codex || true)"
OPENCODE_BIN="$(command -v opencode || true)"
CODEX_VER=""
OPENCODE_VER=""

echo
echo "======== host CLIs ========"
if [ -z "$CODEX_BIN" ]; then
  fail "codex CLI is not on PATH (required; exact version $SUPPORTED_CODEX)"
else
  CODEX_VER="$(codex --version 2>/dev/null | tr -d '\r' || true)"
  echo "codex: $CODEX_BIN ($CODEX_VER)"
  case "$CODEX_VER" in
    *" $SUPPORTED_CODEX"|*"$SUPPORTED_CODEX")
      pass "codex --version is $SUPPORTED_CODEX"
      ;;
    *)
      fail "codex --version is not $SUPPORTED_CODEX (got: $CODEX_VER)"
      ;;
  esac
fi

if [ -z "$OPENCODE_BIN" ]; then
  skip "opencode CLI is not on PATH (S2 adapter skipped; Codex-only)"
else
  OPENCODE_VER="$(opencode --version 2>/dev/null | tr -d '\r' || true)"
  echo "opencode: $OPENCODE_BIN ($OPENCODE_VER)"
  case "$OPENCODE_VER" in
    *"$SUPPORTED_OPENCODE"*)
      pass "opencode --version is $SUPPORTED_OPENCODE"
      ;;
    *)
      fail "opencode --version is not $SUPPORTED_OPENCODE (got: $OPENCODE_VER)"
      ;;
  esac
fi

run_adapter() {
  local before="$FAILED"
  if ! smoke_adapter "$@"; then
    if [ "$FAILED" -eq "$before" ]; then
      fail "$1 adapter aborted unexpectedly"
    fi
    return 1
  fi
  return 0
}

if [ -n "$CODEX_BIN" ]; then
  case "$CODEX_VER" in
    *" $SUPPORTED_CODEX"|*"$SUPPORTED_CODEX")
      run_adapter "codex" "$SUPPORTED_CODEX" "$CODEX_TOKEN_NAME" "$CODEX_TOKEN_VALUE" || true
      ;;
  esac
fi

if [ -n "$OPENCODE_BIN" ]; then
  case "$OPENCODE_VER" in
    *"$SUPPORTED_OPENCODE"*)
      run_adapter "opencode" "$SUPPORTED_OPENCODE" "$OPENCODE_TOKEN_NAME" "$OPENCODE_TOKEN_VALUE" || true
      ;;
  esac
fi

if [ "$FAILED" -ne 0 ]; then
  exit 1
fi
exit 0
