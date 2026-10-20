#!/bin/sh
# TASK A (part 2) - A4 shared cache reuse, A5 models.json read-only sharing.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a4-a5-shared.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

# ------------------------------------------------------------------ A4
say "############ A4 SHARED pre-warmed npm cache reused across TWO sequential trials"
SHAREDCACHE=$M6I/shared/a4cache; rm -rf "$SHAREDCACHE"; mkdir -p "$SHAREDCACHE"

for N in 1 2; do
  T=$(mktrial "a4trial$N")
  seed_config "$T"     # pre-seeded config dir (plugin present)
  seed_models "$T" ro
  BEFORE=$(dirsize "$SHAREDCACHE")
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$SHAREDCACHE \
      "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
  ) > "$OUT/a4trial$N.ndjson" 2> "$OUT/a4trial$N.stderr.txt" || true
  E=$(msnow)
  sleep 5
  AFTER=$(dirsize "$SHAREDCACHE")
  say "--- A4 trial $N"
  say "    wall_ms=$((E-S))"
  say "    shared_cache_before=$BEFORE"
  say "    shared_cache_after=$AFTER"
  say "    shared_cache_delta=$((AFTER-BEFORE))"
  say "    config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
  say "    models_json_bytes=$(stat -f '%z' "$T/cache/opencode/models.json")"
  say "    trial_total_bytes=$(dirsize "$T")"
  say "    ndjson_types=$(grep -o '"type":"[a-z_]*"' "$OUT/a4trial$N.ndjson" | sort -u | tr '\n' ' ')"
  say "    text=$(grep -o '"type":"text"[^}]*"text":"[^"]*"' "$OUT/a4trial$N.ndjson" | sed 's/.*"text":"//;s/"$//' | tr '\n' '|')"
  say ""
done

# ------------------------------------------------------------------ A5
say "############ A5 models.json: size + SHARED READ-ONLY reuse (one shared file, many trials)"
SHAREDMODELS=$M6I/shared/a5models.json
rm -f "$SHAREDMODELS"
cp "$SHARED/models/models.json" "$SHAREDMODELS"
chmod 444 "$SHAREDMODELS"
say "  shared models.json bytes=$(stat -f '%z' "$SHAREDMODELS")  mode=$(stat -f '%Sp' "$SHAREDMODELS")"

for N in 1 2; do
  T=$(mktrial "a5trial$N")
  seed_config "$T"
  # point XDG_CACHE_HOME at a per-trial dir whose models.json is a HARDLINK to the
  # shared read-only file -> one physical copy, still read-only
  mkdir -p "$T/cache/opencode"
  ln "$SHAREDMODELS" "$T/cache/opencode/models.json" 2>/dev/null \
    || cp "$SHAREDMODELS" "$T/cache/opencode/models.json"
  BEFORE=$(stat -f '%z' "$T/cache/opencode/models.json")
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$M6I/shared/a4cache \
      "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
  ) > "$OUT/a5trial$N.ndjson" 2> "$OUT/a5trial$N.stderr.txt" || true
  E=$(msnow)
  sleep 3
  say "--- A5 trial $N (hardlinked shared read-only models.json)"
  say "    wall_ms=$((E-S))"
  say "    models_json_bytes_before=$BEFORE after=$(stat -f '%z' "$T/cache/opencode/models.json")"
  say "    models_json_mode=$(stat -f '%Sp' "$T/cache/opencode/models.json")"
  say "    shared_file_mode=$(stat -f '%Sp' "$SHAREDMODELS")"
  say "    inode_shared=$(stat -f '%i' "$T/cache/opencode/models.json") inode_shared_file=$(stat -f '%i' "$SHAREDMODELS")"
  say "    text=$(grep -o '"type":"text"[^}]*"text":"[^"]*"' "$OUT/a5trial$N.ndjson" | sed 's/.*"text":"//;s/"$//' | tr '\n' '|')"
  say "    stderr=$(head -c 300 "$OUT/a5trial$N.stderr.txt" | tr '\n' ' ')"
  say ""
done
say "  shared models.json mode after both trials: $(stat -f '%Sp' "$SHAREDMODELS")"
say "A4/A5 DONE"
