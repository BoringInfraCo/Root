#!/bin/sh
# A9 - FINAL COST MEASUREMENT of the recommended per-trial setup.
#   - fully-warm shared npm cache (pre-warmed once)
#   - global + project plugin node_modules pre-seeded
#   - models.json pre-seeded (read-only)
#   - rg pre-seeded
#   - strict shims-only PATH
# Measures: wall time, registry bytes (npm cache delta), network bytes (nettop),
#           disk bytes materialized in the trial, tokens, cost.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a9-final-cost.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

WARM=$M6I/shared/a6cache
say "############ A9 recommended per-trial setup"
say "  shared warm npm cache = $WARM ($(dirsize "$WARM") bytes)"

for N in 1 2 3; do
  T=$(mktrial "a9trial$N")
  seed_config "$T"
  seed_models "$T" ro
  seed_rg "$T"
  mkdir -p "$T/cwd/.opencode"
  cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
  seed_skill "$T" > /dev/null

  BEFORE=$(dirsize "$WARM")
  python3 "$M6I/scripts/nettop_sampler.py" "$OUT/a9trial$N.nettop.json" 300 opencode node npm > /dev/null 2>&1 &
  NPID=$!
  sleep 1
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$WARM \
      "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
  ) > "$OUT/a9trial$N.ndjson" 2> "$OUT/a9trial$N.stderr.txt" || true
  E=$(msnow)
  sleep 2
  kill -TERM $NPID 2>/dev/null || true; wait $NPID 2>/dev/null || true
  AFTER=$(dirsize "$WARM")
  TOK=$(python3 - "$OUT/a9trial$N.ndjson" <<'PY'
import json,sys
ti=to=tr=0; cost=0.0
for l in open(sys.argv[1]):
    l=l.strip()
    if not l: continue
    o=json.loads(l); p=o.get('part') or {}
    if o['type']=='step_finish':
        t=p.get('tokens') or {}
        ti+=t.get('input',0); to+=t.get('output',0); tr+=t.get('reasoning',0)
        cost+=p.get('cost') or 0
print(f"input={ti} output={to} reasoning={tr} cost={cost}")
PY
)
  say "--- A9 trial $N"
  say "    wall_ms=$((E-S))"
  say "    registry_bytes_pulled=$((AFTER-BEFORE))"
  say "    tokens: $TOK"
  say "    network: $(python3 -c "import json;d=json.load(open('$OUT/a9trial$N.nettop.json'));print(json.dumps(d.get('per_name',{})))" 2>/dev/null)"
  say "    trial_disk_bytes=$(dirsize "$T")"
  say "    audit_log=$(tr '\n' '|' < "$T/audit.log")"
  say ""
done
say "A9 DONE"
