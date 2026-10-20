#!/bin/sh
# TASK C (final) - VARIANCE with the RECOMMENDED per-trial setup.
# Two treatment trials in two FRESH envs; then compare.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
PROMPT="install ripgrep using Root"
WARM=$M6I/shared/a6cache

mktrial_recommended() {
  T=$(mktrial "$1")
  mkdir -p "$T/config/opencode" "$T/cwd/.opencode" "$T/cache/opencode/bin"
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/config/opencode/node_modules"
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
  cp "$SHARED/config-template/opencode/opencode.jsonc" "$T/config/opencode/opencode.jsonc" 2>/dev/null || true
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"; chmod 444 "$T/cache/opencode/models.json"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
  chmod 755 "$T/cache/opencode/bin/rg"
  echo "$T"
}

for N in 1 2; do
  T=$(mktrial_recommended "C_run$N")
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
      "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT"
  ) > "$OUT/C_run$N.ndjson" 2> "$OUT/C_run$N.stderr.txt" || true
  E=$(msnow)
  sleep 8
  AFTER=$(dirsize "$WARM")
  cp "$T/audit.log" "$OUT/C_run$N.audit.log"
  {
    echo "label=C_run$N"
    echo "wall_ms=$((E-S))"
    echo "registry_bytes_pulled=$((AFTER-BEFORE))"
    echo "audit_log_begin"
    cat "$T/audit.log"
    echo "audit_log_end"
  } > "$OUT/C_run$N.meta.txt"
  echo "C_run$N wall_ms=$((E-S)) registry_bytes=$((AFTER-BEFORE))"
done

python3 "$M6I/scripts/c_variance.py" \
  "C_run1=$OUT/C_run1.ndjson:$OUT/C_run1.audit.log" \
  "C_run2=$OUT/C_run2.ndjson:$OUT/C_run2.audit.log" \
  | tee "$OUT/c-variance.txt"
echo "C DONE"
