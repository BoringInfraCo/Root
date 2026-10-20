#!/bin/sh
# P3 - HERMETIC TRIAL COST.
# Measures what a real `opencode run` needs in a fresh trial env:
#   (a) cold: no npm cache, no config dir
#   (b) warm npm cache: which env var the binary honours (npm_config_cache vs
#       NPM_CONFIG_CACHE) and whether pre-warming removes the registry fetch
#   (c) pre-seeded config dir: does an existing plugin install skip the fetch
#   (d) a SHARED pre-warmed cache reused across two sequential trials
#   (e) the models.json catalog snapshot: size + read-only sharing
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p3"
rm -rf "$OUT"; mkdir -p "$OUT"
SHARED="$M6/shared"
mkdir -p "$SHARED"

bytes() { find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'; }

# mktrial3 <name> <npmcache-path-or-empty>
mktrial3() {
  T="$M6/trials/$1"
  rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd"
  echo "$T"
}

# run3 <trial> <label> [env...]  -> measures
run3() {
  T="$1"; LABEL="$2"; shift 2
  CACHE_HOME_DIR="$T/home/.npm"
  BEFORE_NPM=0; BEFORE_TOTAL=0
  [ -d "$CACHE_HOME_DIR" ] && BEFORE_NPM=$(bytes "$CACHE_HOME_DIR")
  BEFORE_TOTAL=$(bytes "$T")
  S=$(date +%s)
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" run --pure --format json --model opencode/big-pickle "Reply with exactly: ok" ) \
      > "$OUT/$LABEL.ndjson" 2> "$OUT/$LABEL.err" || true
  E=$(date +%s)
  # let background work settle
  sleep 3
  AFTER_NPM=$(bytes "$CACHE_HOME_DIR")
  AFTER_TOTAL=$(bytes "$T")
  {
    echo "label=$LABEL"
    echo "wall_s=$((E-S))"
    echo "npm_cache_dir=$CACHE_HOME_DIR"
    echo "npm_cache_before=$BEFORE_NPM"
    echo "npm_cache_after=$AFTER_NPM"
    echo "npm_cache_delta=$((AFTER_NPM-BEFORE_NPM))"
    echo "trial_bytes_total=$AFTER_TOTAL"
    echo "models_json=$( [ -f "$T/cache/opencode/models.json" ] && bytes "$T/cache/opencode/models.json" || echo 0)"
    echo "config_dir_bytes=$(bytes "$T/config")"
    echo "data_dir_bytes=$(bytes "$T/data")"
    echo "tmp_dir_bytes=$(bytes "$T/tmp")"
    echo "exit=$(cat "$OUT/$LABEL.err" >/dev/null 2>&1; echo n/a)"
  } > "$OUT/$LABEL.metrics"
  cat "$OUT/$LABEL.metrics"
}

echo "############ P3-1 COLD (empty HOME, no npm cache, no config dir)"
T=$(mktrial3 p3cold)
run3 "$T" cold

echo
echo "############ P3-2 npm_config_cache (lowercase) -> dedicated cache dir"
T=$(mktrial3 p3lower)
mkdir -p "$T/npmcache"
run3 "$T" lower npm_config_cache="$T/npmcache"
echo "--- where did the cache land? ---"
echo "trials/p3lower/npmcache bytes = $(bytes "$T/npmcache")"
echo "trials/p3lower/home/.npm bytes = $(bytes "$T/home/.npm")"

echo
echo "############ P3-3 NPM_CONFIG_CACHE (uppercase) -> dedicated cache dir"
T=$(mktrial3 p3upper)
mkdir -p "$T/npmcache"
run3 "$T" upper NPM_CONFIG_CACHE="$T/npmcache"
echo "--- where did the cache land? ---"
echo "trials/p3upper/npmcache bytes = $(bytes "$T/npmcache")"
echo "trials/p3upper/home/.npm bytes = $(bytes "$T/home/.npm")"

echo "P3 DONE"
