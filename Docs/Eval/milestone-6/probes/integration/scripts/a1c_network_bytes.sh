#!/bin/sh
# A1c - COLD run with precise network byte accounting.
#   - nettop sampler over the whole host during the run
#   - npm cache delta (bytes written to disk by the registry fetch)
#   - models.json size (catalog download)
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a1c-network-bytes.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

CA=$M6I/shared/a1c-cache; rm -rf "$CA"; mkdir -p "$CA"
T=$(mktrial a1ccold)
say "trial=$T"
say "cold cache dir=$CA"

python3 "$M6I/scripts/nettop_sampler.py" "$OUT/a1c-nettop.json" 120 > /dev/null 2>&1 &
NPID=$!
sleep 1
BEFORE=$(dirsize "$CA")
S=$(msnow)
(
  cd "$T/cwd" && /usr/bin/env -i \
    HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
    XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
    PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    npm_config_cache=$CA \
    "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
) > "$OUT/a1ccold.ndjson" 2> "$OUT/a1ccold.stderr.txt" || true
E=$(msnow)
say "run wall_ms=$((E-S))"
# let the background plugin install finish
i=0; while [ $i -lt 120 ]; do
  [ -d "$T/config/opencode/node_modules/@opencode-ai/plugin" ] && break
  sleep 1; i=$((i+1))
done
say "plugin_install_landed_after=$((i))s"
sleep 2
AFTER=$(dirsize "$CA")
kill $NPID 2>/dev/null || true; wait $NPID 2>/dev/null || true
{
  echo "label=a1ccold"
  echo "wall_ms_run=$((E-S))"
  echo "npm_cache_delta_bytes=$((AFTER-BEFORE))"
  echo "models_json_bytes=$(stat -f '%z' "$T/cache/opencode/models.json")"
  echo "config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  echo "data_bytes=$(dirsize "$T/data")"
  echo "tmp_bytes=$(dirsize "$T/tmp")"
  echo "trial_total_bytes=$(dirsize "$T")"
} | tee -a "$LOG"
say "nettop (all processes, host-wide during the run window):"
cat "$OUT/a1c-nettop.json" | tee -a "$LOG"
say ""
say "A1C DONE"
