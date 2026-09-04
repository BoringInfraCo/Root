#!/usr/bin/env bash
# Root v0.4.1 Claude held-subset isolated smoke.
#
# Exercises inspect → export (no --include-mcp) → held MCP refusals
# (--include-mcp, enable-plan, enable) → plan → apply (--apply --plan-hash
# + per-item --approve for the executable skill) → verify → settings/canary/
# snapshot proofs → rollback --last (byte-identical CLAUDE.md/settings.json,
# created skills tombstoned).
#
# Isolation: throwaway TMPDIR/HOME/ROOT_DIR/CLAUDE_CONFIG_DIR. Never uses the
# operator's real ~/.claude or ~/.claude.json. Auth tokens
# ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, and CLAUDE_CODE_OAUTH_TOKEN are
# unset for every probe and Root invocation.
#
# Usage (from a built tree):
#   ROOT_BIN=target/debug/root Docs/Release/v0_4_1_claude_smoke.sh
#
# Do not source this file. Do not run this against a real home.

set -euo pipefail

if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  echo "FAIL: do not source this script; execute it" >&2
  return 1 2>/dev/null || exit 1
fi

SUPPORTED_CLAUDE="2.1.260"
SUPPORTED_ROOT="0.4.1"
HELD_ERROR="unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held."
MCP_ID="github"
SOURCE_MODEL="claude-sonnet-4-6"
TARGET_MODEL="old-model"

PASS_COUNT=0
FAIL_COUNT=0
FAILED=0
WORK=""
REAL_HOME="${HOME:-}"
REAL_ROOT_DIR="${ROOT_DIR:-}"
REAL_CLAUDE_CONFIG_DIR="${CLAUDE_CONFIG_DIR:-}"
REAL_TMPDIR="${TMPDIR:-}"
REAL_CODEX_HOME="${CODEX_HOME:-}"
REAL_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-}"
REAL_OPENCODE_CONFIG_DIR="${OPENCODE_CONFIG_DIR:-}"
REAL_OPENCODE_CONFIG="${OPENCODE_CONFIG:-}"
SAVED_ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY-}"
SAVED_ANTHROPIC_AUTH_TOKEN="${ANTHROPIC_AUTH_TOKEN-}"
SAVED_CLAUDE_CODE_OAUTH_TOKEN="${CLAUDE_CODE_OAUTH_TOKEN-}"
HAD_ANTHROPIC_API_KEY=0
HAD_ANTHROPIC_AUTH_TOKEN=0
HAD_CLAUDE_CODE_OAUTH_TOKEN=0
if [ -n "${ANTHROPIC_API_KEY+x}" ]; then HAD_ANTHROPIC_API_KEY=1; fi
if [ -n "${ANTHROPIC_AUTH_TOKEN+x}" ]; then HAD_ANTHROPIC_AUTH_TOKEN=1; fi
if [ -n "${CLAUDE_CODE_OAUTH_TOKEN+x}" ]; then HAD_CLAUDE_CODE_OAUTH_TOKEN=1; fi
FP_BEFORE=""
CLEANED=0

# Never carry operator auth into this process.
unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN CLAUDE_CODE_OAUTH_TOKEN || true

usage_banner() {
  cat <<EOF
Root v0.4.1 Claude held-subset isolated smoke
  WORK=$WORK
  REAL_HOME=$REAL_HOME
  Isolated HOME/ROOT_DIR/CLAUDE_CONFIG_DIR/TMPDIR under WORK
  Auth tokens unset (ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, CLAUDE_CODE_OAUTH_TOKEN)
  Never uses real ~/.claude or ~/.claude.json
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

parse_claude_xyz() {
  python3 -c 'import sys
text=sys.stdin.read().replace("\r","").strip()
tokens=text.split()
version=None
if len(tokens)==1:
    version=tokens[0]
elif len(tokens)==3 and tokens[1]=="(Claude" and tokens[2]=="Code)":
    version=tokens[0]
elif len(tokens)==2 and tokens[0]=="claude":
    version=tokens[1]
if version is None:
    sys.exit(0)
parts=version.split(".")
if len(parts)==3 and all(p.isdigit() and p for p in parts):
    sys.stdout.write(version)
'
}

fingerprint_real_claude() {
  python3 -c 'import hashlib, json, os, sys
home=os.environ["REAL_HOME"]
paths=[
    os.path.join(home, ".claude.json"),
    os.path.join(home, ".claude", "settings.json"),
]
entries=[]
for path in paths:
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
json.dump(entries, sys.stdout)
'
}

assert_real_claude_unchanged() {
  local after
  after="$(fingerprint_real_claude)"
  python3 -c 'import json,sys
before=json.loads(open(sys.argv[1]).read())
after=json.loads(sys.argv[2])
if before != after:
    b={e["path"]: e for e in before}
    a={e["path"]: e for e in after}
    added=sorted(set(a)-set(b))
    removed=sorted(set(b)-set(a))
    changed=sorted(p for p in set(a)&set(b) if a[p]!=b[p])
    print("real ~/.claude.json / ~/.claude/settings.json fingerprint drift")
    if added:
        print("  added: " + ", ".join(added[:20]))
    if removed:
        print("  removed: " + ", ".join(removed[:20]))
    if changed:
        print("  changed: " + ", ".join(changed[:20]))
    sys.exit(1)
' "$FP_BEFORE" "$after"
}

restore_operator_env() {
  if [ -n "${REAL_HOME:-}" ]; then
    export HOME="$REAL_HOME"
  fi
  if [ -n "${REAL_ROOT_DIR:-}" ]; then
    export ROOT_DIR="$REAL_ROOT_DIR"
  else
    unset ROOT_DIR || true
  fi
  if [ -n "${REAL_CLAUDE_CONFIG_DIR:-}" ]; then
    export CLAUDE_CONFIG_DIR="$REAL_CLAUDE_CONFIG_DIR"
  else
    unset CLAUDE_CONFIG_DIR || true
  fi
  if [ -n "${REAL_TMPDIR:-}" ]; then
    export TMPDIR="$REAL_TMPDIR"
  else
    unset TMPDIR || true
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
  if [ -n "${REAL_OPENCODE_CONFIG:-}" ]; then
    export OPENCODE_CONFIG="$REAL_OPENCODE_CONFIG"
  else
    unset OPENCODE_CONFIG || true
  fi
  if [ "$HAD_ANTHROPIC_API_KEY" -eq 1 ]; then
    export ANTHROPIC_API_KEY="$SAVED_ANTHROPIC_API_KEY"
  else
    unset ANTHROPIC_API_KEY || true
  fi
  if [ "$HAD_ANTHROPIC_AUTH_TOKEN" -eq 1 ]; then
    export ANTHROPIC_AUTH_TOKEN="$SAVED_ANTHROPIC_AUTH_TOKEN"
  else
    unset ANTHROPIC_AUTH_TOKEN || true
  fi
  if [ "$HAD_CLAUDE_CODE_OAUTH_TOKEN" -eq 1 ]; then
    export CLAUDE_CODE_OAUTH_TOKEN="$SAVED_CLAUDE_CODE_OAUTH_TOKEN"
  else
    unset CLAUDE_CODE_OAUTH_TOKEN || true
  fi
}

cleanup() {
  local status=$?
  restore_operator_env
  if [ "$CLEANED" -eq 0 ] && [ -n "$FP_BEFORE" ] && [ -f "$FP_BEFORE" ]; then
    if ! assert_real_claude_unchanged; then
      echo "FAIL: isolation: real ~/.claude.json or ~/.claude/settings.json mutated" >&2
      FAILED=1
      FAIL_COUNT=$((FAIL_COUNT + 1))
    else
      echo "PASS: isolation: real ~/.claude.json and ~/.claude/settings.json unchanged"
      PASS_COUNT=$((PASS_COUNT + 1))
    fi
  fi
  if [ -n "$WORK" ] && [ -d "$WORK" ]; then
    rm -rf "$WORK"
  fi
  CLEANED=1
  echo
  echo "Summary: PASS=$PASS_COUNT FAIL=$FAIL_COUNT"
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

WORK="$(mktemp -d "${TMPDIR:-/tmp}/root-v041-claude-smoke.XXXXXX")"
mkdir -p "$WORK/tmp" "$WORK/logs" "$WORK/fingerprints" "$WORK/bundles"
export TMPDIR="$WORK/tmp"
FP_BEFORE="$WORK/fingerprints/before.json"
export REAL_HOME
fingerprint_real_claude >"$FP_BEFORE"

isolate_run() {
  local run="$1"
  mkdir -p \
    "$run/home/.agents/skills" \
    "$run/home/.claude" \
    "$run/claude-config" \
    "$run/root" \
    "$run/tmp" \
    "$run/logs"
  export HOME="$run/home"
  export ROOT_DIR="$run/root"
  export CLAUDE_CONFIG_DIR="$run/claude-config"
  export TMPDIR="$run/tmp"
  unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN CLAUDE_CODE_OAUTH_TOKEN || true
  unset CODEX_HOME OPENCODE_CONFIG_DIR OPENCODE_CONFIG || true
  if [ "$HOME" = "$REAL_HOME" ]; then
    echo "FAIL: isolation collapsed onto real HOME" >&2
    return 1
  fi
  if [ "$CLAUDE_CONFIG_DIR" = "$REAL_HOME/.claude" ] || [ "$CLAUDE_CONFIG_DIR" = "${REAL_CLAUDE_CONFIG_DIR:-}" ]; then
    echo "FAIL: isolation collapsed onto real Claude config dir" >&2
    return 1
  fi
  case "$HOME" in
    "$WORK"/*) ;;
    *)
      echo "FAIL: isolated HOME is not under WORK" >&2
      return 1
      ;;
  esac
  case "$CLAUDE_CONFIG_DIR" in
    "$WORK"/*) ;;
    *)
      echo "FAIL: isolated CLAUDE_CONFIG_DIR is not under WORK" >&2
      return 1
      ;;
  esac
  case "$ROOT_DIR" in
    "$WORK"/*) ;;
    *)
      echo "FAIL: isolated ROOT_DIR is not under WORK" >&2
      return 1
      ;;
  esac
}

run_root() {
  env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN \
    -u CODEX_HOME -u OPENCODE_CONFIG_DIR -u OPENCODE_CONFIG \
    HOME="$HOME" \
    ROOT_DIR="$ROOT_DIR" \
    CLAUDE_CONFIG_DIR="$CLAUDE_CONFIG_DIR" \
    TMPDIR="$TMPDIR" \
    PATH="$PATH" \
    "$ROOT_BIN" "$@"
}

run_claude() {
  env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN \
    HOME="$HOME" \
    CLAUDE_CONFIG_DIR="$CLAUDE_CONFIG_DIR" \
    TMPDIR="$TMPDIR" \
    PATH="$PATH" \
    claude "$@"
}

write_canary() {
  printf '%s' '{"oauth":"do-not-copy","mcpServers":{"github":{"command":"npx"}}}' >"$1"
}

write_source() {
  mkdir -p \
    "$CLAUDE_CONFIG_DIR/skills/docs-writer" \
    "$HOME/.agents/skills/repo-helper"
  cat >"$CLAUDE_CONFIG_DIR/CLAUDE.md" <<'EOF'
# Source CLAUDE.md
Use Root for installs.
EOF
  cat >"$CLAUDE_CONFIG_DIR/settings.json" <<EOF
{
  "model": "${SOURCE_MODEL}",
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {"Stop": []}
}
EOF
  write_canary "$CLAUDE_CONFIG_DIR/.claude.json"
  cat >"$CLAUDE_CONFIG_DIR/skills/docs-writer/SKILL.md" <<'EOF'
# docs-writer
Write concise docs.
EOF
  cat >"$HOME/.agents/skills/repo-helper/run.sh" <<'EOF'
#!/bin/sh
echo repo-helper
EOF
  chmod 0755 "$HOME/.agents/skills/repo-helper/run.sh"
}

write_target() {
  mkdir -p "$CLAUDE_CONFIG_DIR"
  cat >"$CLAUDE_CONFIG_DIR/CLAUDE.md" <<'EOF'
# Target CLAUDE.md
old prompt
EOF
  cat >"$CLAUDE_CONFIG_DIR/settings.json" <<EOF
{
  "model": "${TARGET_MODEL}",
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {"Stop": []},
  "target_only_experimental": "preserve-this-value"
}
EOF
  write_canary "$CLAUDE_CONFIG_DIR/.claude.json"
  rm -rf "$CLAUDE_CONFIG_DIR/skills/docs-writer" "$HOME/.agents/skills/repo-helper"
  mkdir -p "$CLAUDE_CONFIG_DIR/skills" "$HOME/.agents/skills"
}

assert_held_error() {
  local label="$1"
  shift
  local out="$logs/${label}.json"
  local err="$logs/${label}.err"
  local rc=0
  run_root --json "$@" >"$out" 2>"$err" || rc=$?
  if [ "$rc" -eq 0 ]; then
    fail "$label: succeeded but MCP is held"
    return 1
  fi
  local msg=""
  if [ -s "$out" ]; then
    msg="$(json_get "$out" "message" 2>/dev/null || true)"
  fi
  if [ "$msg" != "$HELD_ERROR" ]; then
    fail "$label: expected exact '$HELD_ERROR' (got: ${msg:-$(tr '\n' ' ' <"$err")})"
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    return 1
  fi
  pass "$label (exact held error)"
}

assert_no_claude_json_in_snapshots() {
  python3 -c 'import json,os,sys
root=os.path.join(os.environ["ROOT_DIR"], "agent-snapshots")
if not os.path.isdir(root):
    print("agent-snapshots directory missing after apply")
    sys.exit(1)
hits=[]
for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
    dirnames.sort(); filenames.sort()
    for name in filenames:
        path=os.path.join(dirpath, name)
        if name == ".claude.json" or "claude.json" in name:
            hits.append("file:"+path)
            continue
        try:
            text=open(path, "r", encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        if ".claude.json" not in text:
            continue
        rels=[]
        if name.endswith(".json"):
            try:
                data=json.loads(text)
            except Exception:
                data=None
            if isinstance(data, dict):
                for entry in data.get("entries") or []:
                    rel=str(entry.get("rel") or "")
                    scope=str(entry.get("scope") or "")
                    if rel == ".claude.json" or "claude.json" in rel:
                        rels.append("%s:%s" % (scope, rel))
        if rels or ".claude.json" in text:
            hits.append(path + ((" " + ",".join(rels)) if rels else " (text mentions .claude.json)"))
if hits:
    print("agent-snapshots contain .claude.json:")
    print("\n".join(hits[:20]))
    sys.exit(1)
'
}

assert_settings_patched() {
  python3 -c 'import json,sys
path, model = sys.argv[1], sys.argv[2]
data=json.load(open(path))
got=data.get("model")
if got != model:
    print("settings.json model is %r, expected %r" % (got, model))
    sys.exit(1)
if data.get("permissions") != {"allow": ["Bash(ls)"]}:
    print("settings.json permissions not preserved: %r" % (data.get("permissions"),))
    sys.exit(1)
if data.get("hooks") != {"Stop": []}:
    print("settings.json hooks not preserved: %r" % (data.get("hooks"),))
    sys.exit(1)
if data.get("target_only_experimental") != "preserve-this-value":
    print("settings.json target_only_experimental not preserved: %r" % (data.get("target_only_experimental"),))
    sys.exit(1)
' "$1" "$2"
}

assert_mode_executable() {
  python3 -c 'import os,stat,sys
path=sys.argv[1]
mode=os.stat(path).st_mode
if mode & stat.S_IXUSR == 0:
    print("%s is not owner-executable (mode %o)" % (path, mode))
    sys.exit(1)
' "$1"
}

usage_banner
echo "  ROOT_BIN=$ROOT_BIN"

ROOT_VER="$("$ROOT_BIN" --version 2>/dev/null || true)"
echo "  $ROOT_VER"
if python3 -c 'import re,sys; sys.exit(0 if re.search(r"\b0\.4\.1\b", sys.argv[1]) else 1)' "$ROOT_VER"; then
  pass "root --version reports 0.4.1"
else
  fail "root --version is not 0.4.1 (got: $ROOT_VER)"
fi

RUN="$WORK/runs/claude"
logs="$RUN/logs"
bundle="$WORK/bundles/claude"
isolate_run "$RUN"
mkdir -p "$logs"

echo
echo "======== host CLI: claude ========"
CLAUDE_BIN="$(command -v claude || true)"
CLAUDE_VER_RAW=""
CLAUDE_VER=""
if [ -z "$CLAUDE_BIN" ]; then
  fail "claude CLI is not on PATH (required; exact version $SUPPORTED_CLAUDE)"
else
  CLAUDE_VER_RAW="$(run_claude --version 2>/dev/null | tr -d '\r' || true)"
  CLAUDE_VER="$(printf '%s\n' "$CLAUDE_VER_RAW" | parse_claude_xyz)"
  echo "claude: $CLAUDE_BIN ($CLAUDE_VER_RAW)"
  if [ "$CLAUDE_VER" = "$SUPPORTED_CLAUDE" ]; then
    pass "claude --version is $SUPPORTED_CLAUDE"
  else
    fail "claude --version is not $SUPPORTED_CLAUDE (parsed: '${CLAUDE_VER:-empty}' from: $CLAUDE_VER_RAW)"
  fi
fi

if [ "$FAILED" -ne 0 ]; then
  echo "Skipping Claude held-subset procedure (version/host requirements not met)"
  exit 1
fi

echo
echo "======== adapter: claude (held MCP) ========"

write_source
cp "$CLAUDE_CONFIG_DIR/.claude.json" "$logs/canary.claude.json"

if ! run_root --json agent-bundle inspect --agent claude >"$logs/inspect.json" 2>"$logs/inspect.err"; then
  fail "claude inspect"
  cat "$logs/inspect.err" >&2 || true
  exit 1
fi
inspect_dir="$(json_get "$logs/inspect.json" "config_dir")"
inspect_state="$(json_get "$logs/inspect.json" "global_state_dir")"
if [ "$(json_get "$logs/inspect.json" "present")" != "true" ]; then
  fail "claude inspect: CLI not present"
  exit 1
fi
if [ "$inspect_dir" != "$CLAUDE_CONFIG_DIR" ]; then
  fail "claude inspect: config_dir is $inspect_dir, expected isolated $CLAUDE_CONFIG_DIR"
  exit 1
fi
if [ "$inspect_state" != "$CLAUDE_CONFIG_DIR" ]; then
  fail "claude inspect: global_state_dir is $inspect_state, expected isolated $CLAUDE_CONFIG_DIR"
  exit 1
fi
case "$inspect_dir" in
  "$REAL_HOME"/*)
    fail "claude inspect: leaked real HOME path $inspect_dir"
    exit 1
    ;;
esac
if [ "$(json_get "$logs/inspect.json" "version")" != "$SUPPORTED_CLAUDE" ]; then
  fail "claude inspect: version $(json_get "$logs/inspect.json" version) is not $SUPPORTED_CLAUDE"
  exit 1
fi
if [ "$(json_get "$logs/inspect.json" "version_supported")" != "true" ]; then
  fail "claude inspect: version_supported is not true (gate is $SUPPORTED_CLAUDE)"
  exit 1
fi
python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
skills=d.get("skills") or []
for need in ("docs-writer", "repo-helper"):
    if need not in skills:
        raise SystemExit("inspect skills missing "+need+": "+repr(skills))
if d.get("config_dir") != sys.argv[2] or d.get("global_state_dir") != sys.argv[2]:
    raise SystemExit("inspect dirs are not the isolated CLAUDE_CONFIG_DIR")
' "$logs/inspect.json" "$CLAUDE_CONFIG_DIR"
pass "claude inspect (isolated config_dir, version $SUPPORTED_CLAUDE)"

rm -rf "$bundle"
if ! run_root --json agent-bundle export \
  --agent claude \
  --out "$bundle" \
  --skill docs-writer \
  --skill repo-helper \
  --include-executable repo-helper \
  --no-timestamp \
  >"$logs/export.json" 2>"$logs/export.err"; then
  fail "claude export (without --include-mcp)"
  cat "$logs/export.err" >&2 || true
  exit 1
fi
if [ ! -f "$bundle/manifest.json" ]; then
  fail "claude export: manifest.json missing"
  exit 1
fi
if [ "$(json_get "$logs/export.json" "adapter")" != "claude" ]; then
  fail "claude export: adapter mismatch"
  exit 1
fi
if [ "$(json_get "$logs/export.json" "source_agent_version")" != "$SUPPORTED_CLAUDE" ]; then
  fail "claude export: source_agent_version is not $SUPPORTED_CLAUDE"
  exit 1
fi
python3 -c 'import json,sys
m=json.load(open(sys.argv[1]))
model=sys.argv[2]
mcp=m.get("mcp") or {}
if mcp:
    raise SystemExit("exported MCP must be empty, got "+json.dumps(mcp))
settings=m.get("settings") or {}
if settings.get("model") != model:
    raise SystemExit("exported model is %r, expected %r" % (settings.get("model"), model))
for held in ("permissions", "hooks", "target_only_experimental"):
    if held in settings:
        raise SystemExit(held+" must not be exported")
rels=[f.get("rel") for f in (m.get("files") or [])]
for need in ("CLAUDE.md", "skills/docs-writer/SKILL.md", "repo-helper/run.sh"):
    if need not in rels:
        raise SystemExit("exported files missing "+need+": "+repr(rels))
' "$logs/export.json" "$SOURCE_MODEL"
pass "claude export (without --include-mcp; MCP empty, permissions held)"

mcp_out="$WORK/bundles/claude-mcp"
rm -rf "$mcp_out"
assert_held_error "export-include-mcp" agent-bundle export \
  --agent claude \
  --out "$mcp_out" \
  --skill docs-writer \
  --skill repo-helper \
  --include-executable repo-helper \
  --include-mcp "$MCP_ID" \
  --no-timestamp
if [ -e "$mcp_out" ]; then
  fail "claude export --include-mcp: created output dir $mcp_out"
  exit 1
else
  pass "claude export --include-mcp did not create a bundle"
fi

assert_held_error "enable-plan" agent-bundle enable-plan --agent claude --server "$MCP_ID"
assert_held_error "enable" agent-bundle enable \
  --agent claude \
  --server "$MCP_ID" \
  --plan-hash deadbeef \
  --approve deadbeef

write_target
t0_md="$logs/t0.CLAUDE.md"
t0_settings="$logs/t0.settings.json"
cp "$CLAUDE_CONFIG_DIR/CLAUDE.md" "$t0_md"
cp "$CLAUDE_CONFIG_DIR/settings.json" "$t0_settings"
cp "$CLAUDE_CONFIG_DIR/.claude.json" "$logs/t0.claude.json"
if ! cmp -s "$logs/canary.claude.json" "$logs/t0.claude.json"; then
  fail "target seed changed .claude.json canary before apply"
  exit 1
fi

plan_json="$logs/plan.json"
if ! run_root --json agent-bundle plan --bundle "$bundle" >"$plan_json" 2>"$logs/plan.err"; then
  fail "claude plan"
  cat "$logs/plan.err" >&2 || true
  exit 1
fi
plan_hash="$(json_get "$plan_json" "plan_hash")"
if [ -z "$plan_hash" ]; then
  fail "claude plan: empty plan_hash"
  exit 1
fi
python3 -c 'import json,sys
p=json.load(open(sys.argv[1]))
keys=list((p.get("target_preconditions") or {}).keys())
if any(".claude.json" in k for k in keys):
    raise SystemExit("plan target_preconditions mention .claude.json: "+repr(keys))
' "$plan_json"
pass "claude plan (hash $plan_hash)"

approve_args=()
while IFS= read -r hash; do
  [ -n "$hash" ] || continue
  approve_args+=(--approve "$hash")
done <<EOF
$(json_approvals "$plan_json")
EOF
if [ "${#approve_args[@]}" -eq 0 ]; then
  fail "claude apply: plan listed no --approve hashes (executable skill requires hash-bound approval)"
  exit 1
fi

apply_json="$logs/apply.json"
if ! run_root --json agent-bundle apply \
  --bundle "$bundle" \
  --apply \
  --plan-hash "$plan_hash" \
  "${approve_args[@]}" \
  >"$apply_json" 2>"$logs/apply.err"; then
  fail "claude apply"
  cat "$logs/apply.err" >&2 || true
  exit 1
fi
pass "claude apply (--apply --plan-hash + per-item --approve)"

if ! run_root --json agent-bundle verify --agent claude >"$logs/verify.json" 2>"$logs/verify.err"; then
  fail "claude verify"
  cat "$logs/verify.err" >&2 || true
  exit 1
fi
if [ "$(json_get "$logs/verify.json" "success")" != "true" ]; then
  fail "claude verify: success=false"
  exit 1
fi
pass "claude verify"

if ! assert_settings_patched "$CLAUDE_CONFIG_DIR/settings.json" "$SOURCE_MODEL"; then
  fail "claude apply: settings.json model not updated or permissions not preserved"
  exit 1
fi
pass "claude settings.json model updated ($SOURCE_MODEL) and permissions preserved"

if ! cmp -s "$CLAUDE_CONFIG_DIR/.claude.json" "$logs/canary.claude.json"; then
  fail "claude apply: .claude.json canary bytes changed"
  exit 1
fi
pass "claude .claude.json canary bytes unchanged"

if ! assert_no_claude_json_in_snapshots; then
  fail "claude agent-snapshots contain a .claude.json entry"
  exit 1
fi
pass "claude agent-snapshots have no .claude.json entry"

native_skill="$CLAUDE_CONFIG_DIR/skills/docs-writer/SKILL.md"
shared_skill="$HOME/.agents/skills/repo-helper/run.sh"
if [ ! -f "$native_skill" ]; then
  fail "claude apply: native skill docs-writer missing"
  exit 1
fi
if [ ! -f "$shared_skill" ]; then
  fail "claude apply: shared skill repo-helper/run.sh missing"
  exit 1
fi
if ! assert_mode_executable "$shared_skill"; then
  fail "claude apply: repo-helper/run.sh is not executable"
  exit 1
fi
pass "claude apply created native docs-writer and executable shared repo-helper"

if ! run_root --json agent-bundle rollback --last >"$logs/rollback.json" 2>"$logs/rollback.err"; then
  fail "claude rollback --last"
  cat "$logs/rollback.err" >&2 || true
  exit 1
fi
if ! cmp -s "$CLAUDE_CONFIG_DIR/CLAUDE.md" "$t0_md"; then
  fail "claude rollback: CLAUDE.md is not byte-identical to pre-apply"
  exit 1
fi
if ! cmp -s "$CLAUDE_CONFIG_DIR/settings.json" "$t0_settings"; then
  fail "claude rollback: settings.json is not byte-identical to pre-apply"
  exit 1
fi
if [ -e "$native_skill" ] || [ -e "$shared_skill" ]; then
  fail "claude rollback: created skills were not tombstoned"
  exit 1
fi
if ! cmp -s "$CLAUDE_CONFIG_DIR/.claude.json" "$logs/canary.claude.json"; then
  fail "claude rollback: .claude.json canary bytes changed"
  exit 1
fi
pass "claude rollback --last (byte-identical CLAUDE.md/settings.json, skills tombstoned)"

if [ "$FAILED" -ne 0 ]; then
  exit 1
fi
exit 0
