#!/bin/sh
# P3 (part 2) - warm-cache, pre-seeded config dir, shared-cache reuse,
# and the models.json catalog snapshot.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p3b"
rm -rf "$OUT"; mkdir -p "$OUT"
SHARED="$M6/shared"
NPMDIR=/Users/sergio/.nvm/versions/node/v22.23.2/bin
REAL_NPM=$NPMDIR/npm
mkdir -p "$SHARED"

bytes() { find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'; }

mktrial3() {
  T="$M6/trials/$1"; rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd"
  echo "$T"
}

run3() { # run3 <trial> <label> <cachedir-to-watch> [env...]
  T="$1"; LABEL="$2"; WATCH="$3"; shift 3
  BEFORE=$(bytes "$WATCH"); BEFORE_TOTAL=$(bytes "$T")
  S=$(date +%s)
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" run --pure --format json --model opencode/big-pickle "Reply with exactly: ok" ) \
      > "$OUT/$LABEL.ndjson" 2> "$OUT/$LABEL.err" || true
  E=$(date +%s); sleep 3
  AFTER=$(bytes "$WATCH"); AFTER_TOTAL=$(bytes "$T")
  {
    echo "label=$LABEL"
    echo "wall_s=$((E-S))"
    echo "watched_cache=$WATCH"
    echo "cache_before=$BEFORE"
    echo "cache_after=$AFTER"
    echo "cache_delta=$((AFTER-BEFORE))"
    echo "trial_bytes_total=$AFTER_TOTAL"
    echo "models_json=$( [ -f "$T/cache/opencode/models.json" ] && bytes "$T/cache/opencode/models.json" || echo 0)"
    echo "config_dir_bytes=$(bytes "$T/config")"
    echo "config_node_modules=$( [ -d "$T/config/opencode/node_modules" ] && bytes "$T/config/opencode/node_modules" || echo 0)"
    echo "home_npm=$( [ -d "$T/home/.npm" ] && bytes "$T/home/.npm" || echo 0)"
  } > "$OUT/$LABEL.metrics"
  cat "$OUT/$LABEL.metrics"
}

echo "############ P3-4a PREWARM the shared npm cache (one-time cost)"
rm -rf "$SHARED/npmcache"; mkdir -p "$SHARED/npmcache"
T=$(mktrial3 p3warm)
run3 "$T" warm-prewarm "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"
echo "shared cache bytes = $(bytes "$SHARED/npmcache")"

echo
echo "############ P3-4b TRIAL A sharing the pre-warmed cache"
T=$(mktrial3 p3sharedA)
run3 "$T" shared-A "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"

echo
echo "############ P3-4c TRIAL B sharing the same pre-warmed cache (second sequential trial)"
T=$(mktrial3 p3sharedB)
run3 "$T" shared-B "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"

echo
echo "############ P3-5 PRE-SEEDED config dir (node_modules/@opencode-ai/plugin present)"
T=$(mktrial3 p3seeded)
mkdir -p "$T/config/opencode"
( cd "$T/config/opencode" && env -i HOME=$T/home PATH=$NPMDIR:/usr/bin:/bin npm_config_cache="$SHARED/npmcache" \
    "$REAL_NPM" install --no-audit --no-fund --no-package-lock @opencode-ai/plugin@1.18.27 ) > "$OUT/seed-npm.log" 2>&1 || true
echo "seeded node_modules bytes = $(bytes "$T/config/opencode/node_modules")"
run3 "$T" seeded "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"
echo "after run: config node_modules bytes = $(bytes "$T/config/opencode/node_modules")"
echo "after run: config dir listing:"; ls "$T/config/opencode" | head

echo
echo "############ P3-6 models.json: size + shared read-only reuse"
mkdir -p "$SHARED/models"
if [ ! -f "$SHARED/models/models.json" ]; then
  cp "$M6/trials/p3sharedA/cache/opencode/models.json" "$SHARED/models/models.json"
fi
echo "shared models.json bytes = $(bytes "$SHARED/models/models.json")"
chmod 444 "$SHARED/models/models.json"
T=$(mktrial3 p3models)
mkdir -p "$T/cache/opencode"
cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"
chmod 444 "$T/cache/opencode/models.json"
run3 "$T" models-ro "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"
echo "--- run stderr (models-ro) ---"; head -5 "$OUT/models-ro.err"
echo "--- run ok? ---"; python3 -c "
import json,sys
ok=0
for line in open('$OUT/models-ro.ndjson'):
    line=line.strip()
    if not line: continue
    o=json.loads(line)
    if o.get('type')=='text': ok=1; print('text:', (o.get('part') or {}).get('text'))
print('HAD_TEXT' if ok else 'NO_TEXT')
"
echo "P3B DONE"
