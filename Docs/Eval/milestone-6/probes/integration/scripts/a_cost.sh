#!/bin/sh
# TASK A - hermetic real-trial COST.
#
# A1 COLD      : fresh env, no npm cache, no config dir, no models.json
# A2 CACHE ENV : which env var redirects the npm cache (npm_config_cache vs NPM_CONFIG_CACHE)
# A3 PRESEED   : pre-seeded config dir -> does it skip the @opencode-ai/plugin npm install?
# A4 SHARED    : one shared pre-warmed npm cache reused across TWO sequential trials
# A5 MODELS    : models.json catalog size + read-only sharing
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a-cost.txt"
: > "$LOG"

say() { echo "$@" | tee -a "$LOG"; }

# ---------------------------------------------------------------- A1 COLD
say "############ A1 COLD (no npm cache, no config dir, no models.json)"
CA=$M6I/shared/coldcacheA; rm -rf "$CA"; mkdir -p "$CA"
T=$(mktrial a1cold)
S=$(msnow)
( cd "$T/cwd" && $(trienv "$T" shims npm_config_cache="$CA") "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
    > "$OUT/a1cold.ndjson" 2> "$OUT/a1cold.stderr.txt" || true
E=$(msnow)
sleep 2
{
  echo "label=a1cold"
  echo "wall_ms=$((E-S))"
  echo "cold_cache_bytes=$(dirsize "$CA")"
  echo "models_json_bytes=$( [ -f "$T/cache/opencode/models.json" ] && stat -f '%z' "$T/cache/opencode/models.json" || echo 0)"
  echo "config_bytes=$(dirsize "$T/config")"
  echo "config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  echo "data_bytes=$(dirsize "$T/data")"
  echo "tmp_bytes=$(dirsize "$T/tmp")"
  echo "trial_total_bytes=$(dirsize "$T")"
} | tee -a "$LOG"

# ------------------------------------------------- A2 CACHE ENV VAR (npm direct)
say ""
say "############ A2 npm cache env var honored (direct npm probe)"
NPM=/Users/sergio/.nvm/versions/node/v22.23.2/bin/npm
for variant in lowercase uppercase; do
  D="$M6I/out/a2-$variant"; rm -rf "$D"; mkdir -p "$D/home" "$D/cache" "$D/proj"
  echo '{"name":"t","version":"1.0.0"}' > "$D/proj/package.json"
  if [ "$variant" = "lowercase" ]; then E="npm_config_cache=$D/cache"; else E="NPM_CONFIG_CACHE=$D/cache"; fi
  RES=$(env -i HOME=$D/home PATH=/Users/sergio/.nvm/versions/node/v22.23.2/bin:/bin:/usr/bin $E $NPM config get cache 2>&1)
  say "  variant=$variant  npm config get cache -> $RES"
  say "    dedicated_cache_bytes=$(dirsize "$D/cache")  home_dot_npm_bytes=$(dirsize "$D/home/.npm")"
done

# --------------------------------------- A2b cache env var honored (via opencode)
say ""
say "############ A2b cache env var honored (via real opencode run)"
for variant in lowercase uppercase; do
  C=$M6I/out/a2b-$variant-cache; rm -rf "$C"; mkdir -p "$C"
  T=$(mktrial "a2b-$variant")
  S=$(msnow)
  if [ "$variant" = "lowercase" ]; then EV="npm_config_cache=$C"; else EV="NPM_CONFIG_CACHE=$C"; fi
  ( cd "$T/cwd" && $(trienv "$T" shims) env NPM_CONFIG_CACHE_UNUSED=1 "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
      > "$OUT/a2b-$variant.ndjson" 2> "$OUT/a2b-$variant.stderr.txt" || true
  E=$(msnow)
  sleep 2
  say "  variant=$variant wall_ms=$((E-S)) home_dot_npm_bytes=$(dirsize "$T/home/.npm") (dedicated dir unused - baseline)"
  rm -rf "$T"
done

say ""
say "A DONE"
