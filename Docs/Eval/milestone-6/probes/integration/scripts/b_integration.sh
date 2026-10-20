#!/bin/sh
# TASK B - INTEGRATION (real harness driving Root's fake tools).
#
# One trial-like env per run:
#   - PATH = ONLY the two fake shims (root, brew). No host PATH leakage.
#   - project skill at <cwd>/.opencode/skill/root/SKILL.md
#     (byte-for-byte copy of skills/claude/SKILL.md)
#   - prompt: install ripgrep using Root
#
# Usage: b_integration.sh <label> <with-skill|without-skill>
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"

LABEL="$1"; MODE="$2"
PROMPT="install ripgrep using Root"

T=$(mktrial "$LABEL")
seed_config "$T"
seed_models "$T" ro
if [ "$MODE" = "with-skill" ]; then
  SEEDED_SHA=$(seed_skill "$T")
  say_sha="seeded_skill_sha256=$SEEDED_SHA"
else
  say_sha="seeded_skill_sha256=NONE (skill absent)"
fi
REPO_SHA=$(shasum -a 256 "$REPO/skills/claude/SKILL.md" | cut -d' ' -f1)

# PATH sanity inside the trial env (uses /bin/sh by absolute path)
( cd "$T/cwd" && /usr/bin/env -i HOME=$T/home PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    /bin/sh -c 'for t in root brew npm node git curl nix cargo rg python3 sed awk; do printf "%s -> %s\n" "$t" "$(command -v $t 2>/dev/null || echo NOTFOUND)"; done' ) > "$OUT/$LABEL.pathsanity.txt" 2>&1

# network byte sampler (host-wide, filtered to opencode/npm/node)
python3 "$M6I/scripts/nettop_sampler.py" "$OUT/$LABEL.nettop.json" 120 opencode npm node > /dev/null 2>&1 &
NPID=$!
sleep 1

S=$(msnow)
(
  cd "$T/cwd" && /usr/bin/env -i \
    HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
    XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
    PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    npm_config_cache=$M6I/shared/a4cache \
    "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT"
) > "$OUT/$LABEL.ndjson" 2> "$OUT/$LABEL.stderr.txt"
RC=$?
E=$(msnow)
sleep 2
kill $NPID 2>/dev/null || true; wait $NPID 2>/dev/null || true

echo $((E-S)) > "$OUT/$LABEL.wall_ms"
{
  echo "label=$LABEL"
  echo "mode=$MODE"
  echo "prompt=$PROMPT"
  echo "rc=$RC"
  echo "wall_ms=$((E-S))"
  echo "$say_sha"
  echo "repo_skill_sha256=$REPO_SHA"
  echo "audit_log_begin"
  cat "$T/audit.log"
  echo "audit_log_end"
} > "$OUT/$LABEL.meta.txt"

echo "=== $LABEL ($MODE) rc=$RC wall_ms=$((E-S)) ==="
cat "$OUT/$LABEL.meta.txt"
echo "--- path sanity ---"
cat "$OUT/$LABEL.pathsanity.txt"
echo "--- nettop ---"
cat "$OUT/$LABEL.nettop.json" 2>/dev/null
