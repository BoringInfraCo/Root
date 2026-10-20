#!/bin/sh
# A8 - DECISIVE: fully-warm shared npm cache + ALL targets pre-seeded.
# Does the recommended per-trial setup pull ZERO registry bytes?
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a8-warm-zero-fetch.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

# Build the canonical warm shared cache from a scratch dir, then measure trials
# against it. Use the already-warm a6cache (populated by A6's four variants).
WARM=$M6I/shared/a6cache
say "############ A8 warm shared cache = $WARM ($(dirsize "$WARM") bytes)"

for N in 1 2; do
  T=$(mktrial "a8trial$N")
  seed_config "$T"
  seed_models "$T" ro
  seed_rg "$T"
  mkdir -p "$T/cwd/.opencode"
  cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
  seed_skill "$T" > /dev/null
  BEFORE=$(dirsize "$WARM")
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$WARM \
      "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
  ) > "$OUT/a8trial$N.ndjson" 2> "$OUT/a8trial$N.stderr.txt" || true
  E=$(msnow)
  sleep 12
  AFTER=$(dirsize "$WARM")
  say "--- A8 trial $N"
  say "    wall_ms=$((E-S))"
  say "    shared_npmcache_delta=$((AFTER-BEFORE))   <-- registry bytes pulled"
  say "    project_nm_bytes=$(dirsize "$T/cwd/.opencode/node_modules")"
  say "    global_nm_bytes=$(dirsize "$T/config/opencode/node_modules")"
  say "    trial_total_bytes=$(dirsize "$T")"
  say "    audit_log=$(tr '\n' '|' < "$T/audit.log")"
  say ""
done
say "A8 DONE"
