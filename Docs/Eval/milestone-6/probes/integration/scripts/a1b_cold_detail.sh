#!/bin/sh
# A1b - COLD run, long observation: does the npm install land in config/opencode/node_modules
# AFTER the run exits?  Plus nettop network byte measurement.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a1b-cold-detail.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

CA=$M6I/shared/coldcacheB; rm -rf "$CA"; mkdir -p "$CA"
T=$(mktrial a1bcold)
say "trial=$T"
say "cold cache dir=$CA"
say "before: cache=$(dirsize "$CA") config=$(dirsize "$T/config") trial=$(dirsize "$T")"

# nettop sampler in background (matches npm/node/opencode process names)
python3 "$M6I/scripts/nettop_sampler.py" 60 "$OUT/a1b-nettop.json" npm node opencode > "$OUT/a1b-nettop.txt" 2>&1 &
NPID=$!

S=$(msnow)
( cd "$T/cwd" && $(trienv "$T" shims npm_config_cache="$CA") "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok" ) \
    > "$OUT/a1bcold.ndjson" 2> "$OUT/a1bcold.stderr.txt" || true
E=$(msnow)
say "run wall_ms=$((E-S))"
say "immediately after exit: cache=$(dirsize "$CA") config=$(dirsize "$T/config") trial=$(dirsize "$T")"

for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 5
  say "t+$((i*5))s: cache=$(dirsize "$CA") config=$(dirsize "$T/config") trial=$(dirsize "$T")"
done
say "final config tree:"
find "$T/config" -maxdepth 2 | sed "s|$T|\$T|" | tee -a "$LOG"
kill $NPID 2>/dev/null || true
wait $NPID 2>/dev/null || true
say "nettop sample:"
cat "$OUT/a1b-nettop.json" 2>/dev/null | tee -a "$LOG"
say "A1B DONE"
