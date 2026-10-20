#!/bin/sh
# A10 - FINAL: per-trial cost with PID-accurate network accounting, and
#        APFS-clone disk cost (cp -c) instead of full copies.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a10-final-pid-cost.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

WARM=$M6I/shared/a6cache
say "############ A10 recommended setup, PID-accurate nettop, APFS-clone seeding"

for N in 1 2; do
  T=$(mktrial "a10trial$N")
  # APFS clone: copy-on-write, near-zero disk cost, same bytes visible
  mkdir -p "$T/config/opencode" "$T/cwd/.opencode" "$T/cache/opencode/bin"
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/config/opencode/node_modules" 2>/dev/null \
    || cp -R "$SHARED/config-template/opencode/node_modules" "$T/config/opencode/node_modules"
  cp "$SHARED/config-template/opencode/opencode.jsonc" "$T/config/opencode/opencode.jsonc" 2>/dev/null || true
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules" 2>/dev/null \
    || cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"
  chmod 444 "$T/cache/opencode/models.json"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
  chmod 755 "$T/cache/opencode/bin/rg"
  seed_skill "$T" > /dev/null

  BEFORE=$(dirsize "$WARM")
  # spawn opencode ourselves so we know its pid
  (
    cd "$T/cwd" && exec /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$WARM \
      "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
  ) > "$OUT/a10trial$N.ndjson" 2> "$OUT/a10trial$N.stderr.txt" &
  OPID=$!
  # find the real opencode child pid (env execs into the binary, so $! is it)
  python3 "$M6I/scripts/nettop_pid.py" "$OUT/a10trial$N.nettop.json" "$OPID" 300 > /dev/null 2>&1 &
  NPID=$!
  S=$(msnow)
  wait $OPID 2>/dev/null || true
  E=$(msnow)
  sleep 2
  kill -TERM $NPID 2>/dev/null || true; wait $NPID 2>/dev/null || true
  AFTER=$(dirsize "$WARM")
  say "--- A10 trial $N (opencode pid=$OPID)"
  say "    wall_ms=$((E-S))"
  say "    registry_bytes_pulled=$((AFTER-BEFORE))"
  say "    nettop(this pid only): $(cat "$OUT/a10trial$N.nettop.json" 2>/dev/null | tr -d '\n ' )"
  say "    apparent_trial_bytes=$(dirsize "$T")"
  say "    actual_trial_disk_kb=$(du -sk "$T" | cut -f1)   (APFS clone = shared blocks)"
  say "    audit_log=$(tr '\n' '|' < "$T/audit.log")"
  say ""
done
say "A10 DONE"
