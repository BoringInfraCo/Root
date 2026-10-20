#!/bin/sh
# A2 (decisive) - which npm cache env var does opencode 1.18.27 honor?
#
# Design: build the env list MANUALLY so exactly ONE cache var is set per run.
#   The default shared cache var is NOT included.
# Each variant gets its own dedicated cache dir; the trial HOME is separate so
# $HOME/.npm is a distinct third location. We observe which dir grows.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a2-cache-env-decisive.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

wait_for_install() {
  i=0
  while [ $i -lt "${2:-70}" ]; do
    [ -d "$1/config/opencode/node_modules/@opencode-ai/plugin" ] && return 0
    sleep 1; i=$((i+1))
  done
  return 1
}

run_variant() {
  VARIANT="$1"
  C=$M6I/shared/a2c-$VARIANT; rm -rf "$C"; mkdir -p "$C"
  T=$(mktrial "a2c-$VARIANT")
  say "--- variant=$VARIANT"
  say "    dedicated cache dir = $C"
  case "$VARIANT" in
    lowercase) CACHEVAR="npm_config_cache=$C" ;;
    uppercase) CACHEVAR="NPM_CONFIG_CACHE=$C" ;;
    none)      CACHEVAR="" ;;
  esac
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home \
      XDG_CONFIG_HOME=$T/config \
      XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache \
      XDG_STATE_HOME=$T/state \
      TMPDIR=$T/tmp \
      PATH=$T/bin \
      TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      $CACHEVAR \
      "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
  ) > "$OUT/a2c-$VARIANT.ndjson" 2> "$OUT/a2c-$VARIANT.stderr.txt" || true
  E=$(msnow)
  wait_for_install "$T" 70 && INST=YES || INST=NO
  sleep 3
  say "    wall_ms=$((E-S))  plugin_install_landed=$INST"
  say "    dedicated_cache_bytes=$(dirsize "$C")"
  say "    home_dot_npm_bytes=$(dirsize "$T/home/.npm")"
  say "    config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  say "    stderr: $(head -c 200 "$OUT/a2c-$VARIANT.stderr.txt" | tr '\n' ' ')"
  say ""
}

run_variant lowercase
run_variant uppercase
run_variant none

say "############ A2d host-npm cross-check (same two env vars, host npm)"
NPM=/Users/sergio/.nvm/versions/node/v22.23.2/bin/npm
for variant in lowercase uppercase; do
  D=$M6I/out/a2d-$variant; rm -rf "$D"; mkdir -p "$D/home" "$D/cache" "$D/proj"
  echo '{"name":"t","version":"1.0.0"}' > "$D/proj/package.json"
  if [ "$variant" = "lowercase" ]; then E="npm_config_cache=$D/cache"; else E="NPM_CONFIG_CACHE=$D/cache"; fi
  R=$(/usr/bin/env -i HOME=$D/home PATH=/Users/sergio/.nvm/versions/node/v22.23.2/bin:/bin:/usr/bin $E $NPM config get cache 2>&1)
  say "  host-npm variant=$variant -> npm config get cache = $R"
done

say "A2 DECISIVE DONE"
