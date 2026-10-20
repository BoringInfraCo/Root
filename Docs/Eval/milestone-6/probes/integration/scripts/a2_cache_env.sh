#!/bin/sh
# A2 (retry) - which npm cache env var does opencode 1.18.27 honor?
#   lowercase npm_config_cache vs uppercase NPM_CONFIG_CACHE
# Each variant gets its OWN dedicated cache dir; the trial's HOME is separate so
# $HOME/.npm is a distinct third location. We then see which dir grows.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a2-cache-env.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

wait_for_install() {
  i=0
  while [ $i -lt "${2:-90}" ]; do
    [ -d "$1/config/opencode/node_modules" ] && return 0
    sleep 1; i=$((i+1))
  done
  return 1
}

for variant in lowercase uppercase; do
  C=$M6I/shared/a2b-$variant; rm -rf "$C"; mkdir -p "$C"
  T=$(mktrial "a2b-$variant")
  say "--- variant=$variant"
  say "    dedicated cache dir = $C"
  if [ "$variant" = "lowercase" ]; then EV="npm_config_cache=$C"; else EV="NPM_CONFIG_CACHE=$C"; fi
  S=$(msnow)
  ( cd "$T/cwd" && $(trienv "$T" shims "$EV") "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
      > "$OUT/a2b-$variant.ndjson" 2> "$OUT/a2b-$variant.stderr.txt" || true
  E=$(msnow)
  wait_for_install "$T" 90 && INST=YES || INST=NO
  sleep 3
  say "    wall_ms=$((E-S)) install_landed=$INST"
  say "    dedicated_cache_bytes=$(dirsize "$C")"
  say "    home_dot_npm_bytes=$(dirsize "$T/home/.npm")"
  say "    config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  say "    stderr: $(head -c 200 "$OUT/a2b-$variant.stderr.txt" | tr '\n' ' ')"
  say ""
done

say "############ A2c direct-npm cross-check (same two env vars, host npm)"
NPM=/Users/sergio/.nvm/versions/node/v22.23.2/bin/npm
for variant in lowercase uppercase; do
  D=$M6I/out/a2c-$variant; rm -rf "$D"; mkdir -p "$D/home" "$D/cache" "$D/proj"
  echo '{"name":"t","version":"1.0.0"}' > "$D/proj/package.json"
  if [ "$variant" = "lowercase" ]; then E="npm_config_cache=$D/cache"; else E="NPM_CONFIG_CACHE=$D/cache"; fi
  R=$(/usr/bin/env -i HOME=$D/home PATH=/Users/sergio/.nvm/versions/node/v22.23.2/bin:/bin:/usr/bin $E $NPM config get cache 2>&1)
  say "  host-npm variant=$variant -> npm config get cache = $R"
done

say "A2 DONE"
