#!/bin/sh
# TASK A (consolidated) - hermetic real-trial COST.
#
# A1 COLD      : fresh cache + fresh config -> bytes fetched + wall time
# A2 CACHE ENV : npm_config_cache vs NPM_CONFIG_CACHE (which does opencode honor?)
# A3 PRESEED   : pre-seeded config dir -> does it skip the @opencode-ai/plugin install?
# A4 SHARED    : one shared pre-warmed npm cache reused across TWO sequential trials
# A5 MODELS    : models.json catalog size + read-only sharing
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a-cost.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

# wait_for_install <trial> <timeout_s>: poll for config/opencode/node_modules to appear
wait_for_install() {
  i=0
  while [ $i -lt "${2:-60}" ]; do
    [ -d "$1/config/opencode/node_modules" ] && return 0
    sleep 1; i=$((i+1))
  done
  return 1
}

# --------------------------------------------------------------- A1 COLD
say "############ A1 COLD (fresh npm cache + fresh config dir)"
CA=$M6I/shared/a1cache; rm -rf "$CA"; mkdir -p "$CA"
T=$(mktrial a1cold2)
say "trial=$T"
say "cache dir=$CA"
BEFORE=$(dirsize "$CA")
# nettop sampler over the opencode process (embedded arborist runs in-process)
python3 "$M6I/scripts/nettop_sampler.py" "$OUT/a1-nettop.json" 90 opencode > /dev/null 2>&1 &
NPID=$!
S=$(msnow)
( cd "$T/cwd" && $(trienv "$T" shims npm_config_cache="$CA") "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
    > "$OUT/a1cold.ndjson" 2> "$OUT/a1cold.stderr.txt" || true
E=$(msnow)
say "run wall_ms=$((E-S))  (run only; install continues in background)"
wait_for_install "$T" 90 && say "install landed: YES" || say "install landed: NO"
sleep 3
AFTER=$(dirsize "$CA")
kill $NPID 2>/dev/null || true; wait $NPID 2>/dev/null || true
{
  echo "label=a1cold"
  echo "wall_ms_run=$((E-S))"
  echo "npm_cache_before=$BEFORE"
  echo "npm_cache_after=$AFTER"
  echo "npm_cache_delta=$((AFTER-BEFORE))"
  echo "models_json_bytes=$( [ -f "$T/cache/opencode/models.json" ] && stat -f '%z' "$T/cache/opencode/models.json" || echo 0)"
  echo "config_bytes=$(dirsize "$T/config")"
  echo "config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  echo "data_bytes=$(dirsize "$T/data")"
  echo "tmp_bytes=$(dirsize "$T/tmp")"
  echo "trial_total_bytes=$(dirsize "$T")"
} | tee -a "$LOG"
say "nettop (opencode): $(cat "$OUT/a1-nettop.json" 2>/dev/null | tr -d '\n ')"
say ""

# --------------------------------------------------- A2 CACHE ENV VAR
say "############ A2 which npm cache env var does opencode honor?"
for variant in lowercase uppercase; do
  C=$M6I/shared/a2-$variant; rm -rf "$C"; mkdir -p "$C"
  T=$(mktrial "a2-$variant")
  say "--- variant=$variant cache_dir=$C"
  if [ "$variant" = "lowercase" ]; then EV="npm_config_cache=$C"; else EV="NPM_CONFIG_CACHE=$C"; fi
  S=$(msnow)
  ( cd "$T/cwd" && $(trienv "$T" shims) env "$EV" "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
      > "$OUT/a2-$variant.ndjson" 2> "$OUT/a2-$variant.stderr.txt" || true
  E=$(msnow)
  wait_for_install "$T" 60 || true
  sleep 3
  say "  wall_ms=$((E-S))  dedicated_cache_bytes=$(dirsize "$C")  home_dot_npm_bytes=$(dirsize "$T/home/.npm")  config_nm_bytes=$(dirsize "$T/config/opencode/node_modules")"
done
say ""

# ------------------------------------------------------------- A3 PRESEED
say "############ A3 pre-seeded config dir (node_modules already present)"
CP=$M6I/shared/a3cache; rm -rf "$CP"; mkdir -p "$CP"
T=$(mktrial a3preseed); seed_config "$T"; seed_models "$T" ro
BEFORE=$(dirsize "$CP")
S=$(msnow)
( cd "$T/cwd" && $(trienv "$T" shims npm_config_cache="$CP") "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
    > "$OUT/a3preseed.ndjson" 2> "$OUT/a3preseed.stderr.txt" || true
E=$(msnow)
sleep 8
AFTER=$(dirsize "$CP")
{
  echo "label=a3preseed"
  echo "wall_ms=$((E-S))"
  echo "npm_cache_before=$BEFORE"
  echo "npm_cache_after=$AFTER"
  echo "npm_cache_delta=$((AFTER-BEFORE))"
  echo "config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  echo "models_json_bytes=$(stat -f '%z' "$T/cache/opencode/models.json")"
  echo "trial_total_bytes=$(dirsize "$T")"
} | tee -a "$LOG"
say ""

say "A DONE"
