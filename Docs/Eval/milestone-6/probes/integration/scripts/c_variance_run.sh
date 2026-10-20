#!/bin/sh
# TASK C - VARIANCE. Two treatment trials in two FRESH envs, then compare.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
PROMPT="install ripgrep using Root"
WARM=$M6I/shared/a6cache

for N in 1 2; do
  T=$(mktrial "C_run$N")
  seed_config "$T"
  seed_models "$T" ro
  seed_rg "$T"
  mkdir -p "$T/cwd/.opencode"
  cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
  seed_skill "$T" > /dev/null
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$WARM \
      "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT"
  ) > "$OUT/C_run$N.ndjson" 2> "$OUT/C_run$N.stderr.txt" || true
  E=$(msnow)
  cp "$T/audit.log" "$OUT/C_run$N.audit.log"
  {
    echo "label=C_run$N"
    echo "rc=$?"
    echo "wall_ms=$((E-S))"
    echo "audit_log_begin"
    cat "$T/audit.log"
    echo "audit_log_end"
  } > "$OUT/C_run$N.meta.txt"
  echo "C_run$N wall_ms=$((E-S))"
done

python3 "$M6I/scripts/c_variance.py" \
  "C_run1=$OUT/C_run1.ndjson:$OUT/C_run1.audit.log" \
  "C_run2=$OUT/C_run2.ndjson:$OUT/C_run2.audit.log" \
  | tee "$OUT/c-variance.txt"
echo "C DONE"
