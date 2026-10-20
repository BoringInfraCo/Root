#!/bin/sh
# P3 (part 3) - isolate the pre-seeded config dir against a COLD cache, test
# whether the plugin fetch can be avoided at all, and measure a fully
# pre-seeded (template-copied) trial.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p3c"
rm -rf "$OUT"; mkdir -p "$OUT"
SHARED="$M6/shared"
NPMDIR=/Users/sergio/.nvm/versions/node/v22.23.2/bin
REAL_NPM=$NPMDIR/npm

bytes() { find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'; }
mktrial3() { T="$M6/trials/$1"; rm -rf "$T"; mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd"; echo "$T"; }

run3() {
  T="$1"; LABEL="$2"; WATCH="$3"; shift 3
  BEFORE=$(bytes "$WATCH"); BEFORE_TOTAL=$(bytes "$T")
  S=$(date +%s)
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" run --pure --format json --model opencode/big-pickle "Reply with exactly: ok" ) \
      > "$OUT/$LABEL.ndjson" 2> "$OUT/$LABEL.err" || true
  E=$(date +%s); sleep 4
  AFTER=$(bytes "$WATCH"); AFTER_TOTAL=$(bytes "$T")
  { echo "label=$LABEL"; echo "wall_s=$((E-S))"
    echo "cache_delta=$((AFTER-BEFORE))"
    echo "trial_bytes_total=$AFTER_TOTAL"
    echo "config_node_modules=$( [ -d "$T/config/opencode/node_modules" ] && bytes "$T/config/opencode/node_modules" || echo 0)"
    echo "models_json=$( [ -f "$T/cache/opencode/models.json" ] && bytes "$T/cache/opencode/models.json" || echo 0)"
    echo "tmp=$(bytes "$T/tmp")"; echo "data=$(bytes "$T/data")"
  } > "$OUT/$LABEL.metrics"
  cat "$OUT/$LABEL.metrics"
}

echo "############ P3-5' PRE-SEEDED config dir + COLD cache (isolated)"
COLD="$SHARED/coldcache"; rm -rf "$COLD"; mkdir -p "$COLD"
T=$(mktrial3 p3seededcold)
mkdir -p "$T/config/opencode"
( cd "$T/config/opencode" && env -i HOME=$T/home PATH=$NPMDIR:/usr/bin:/bin npm_config_cache="$COLD" \
    "$REAL_NPM" install --no-audit --no-fund --no-package-lock @opencode-ai/plugin@1.18.27 ) > "$OUT/seed-cold.log" 2>&1 || true
echo "seeded node_modules bytes = $(bytes "$T/config/opencode/node_modules")"
echo "cold cache bytes after seeding = $(bytes "$COLD")"
run3 "$T" seeded-cold "$COLD" npm_config_cache="$COLD"

echo
echo "############ P3-7 can the plugin fetch be avoided? (OPENCODE_PURE + DISABLE_DEFAULT_PLUGINS, COLD cache)"
COLD2="$SHARED/coldcache2"; rm -rf "$COLD2"; mkdir -p "$COLD2"
T=$(mktrial3 p3pure)
run3 "$T" pure-and-noplugins "$COLD2" npm_config_cache="$COLD2" OPENCODE_PURE=1 OPENCODE_DISABLE_DEFAULT_PLUGINS=1

echo
echo "############ P3-8 FULLY PRE-SEEDED trial (config-dir template + models.json template + warm cache)"
TPL="$SHARED/config-template"; rm -rf "$TPL"; mkdir -p "$TPL"
cp -R "$M6/trials/p3sharedA/config/opencode" "$TPL/opencode"
echo "config template bytes = $(bytes "$TPL")"
for N in 1 2; do
  T=$(mktrial3 "p3tpl$N")
  cp -R "$TPL/opencode" "$T/config/opencode"
  mkdir -p "$T/cache/opencode"
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"
  run3 "$T" "tpl-$N" "$SHARED/npmcache" npm_config_cache="$SHARED/npmcache"
done
echo "P3C DONE"
