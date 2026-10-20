#!/bin/sh
# A7 - VALIDATE the recommended per-trial setup.
#
# Recommended setup (per trial):
#   - isolated HOME + XDG_CONFIG/DATA/CACHE/STATE + TMPDIR
#   - PATH = ONLY the fake root/brew shims
#   - $XDG_CONFIG_HOME/opencode/node_modules  PRE-SEEDED (global plugin target)
#   - <cwd>/.opencode/node_modules            PRE-SEEDED (project plugin target)
#   - <cwd>/.opencode/skill/root/SKILL.md     seeded (treatment only)
#   - $XDG_CACHE_HOME/opencode/models.json    PRE-SEEDED (read-only)
#   - $XDG_CACHE_HOME/opencode/bin/rg         PRE-SEEDED (so `skill` is hermetic)
#   - npm_config_cache = SHARED pre-warmed cache
#
# Question: with a COLD (empty) shared npm cache, does the recommended setup
# pull ANY registry bytes?  (i.e. is the pre-seeding sufficient on its own?)
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a7-recommended-setup.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

COLD=$M6I/shared/a7-cold-cache; rm -rf "$COLD"; mkdir -p "$COLD"

T=$(mktrial a7-recommended)
seed_config "$T"
seed_models "$T" ro
seed_rg "$T"
mkdir -p "$T/cwd/.opencode"
cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
seed_skill "$T" > /dev/null

say "############ A7 recommended setup, COLD shared npm cache"
say "  pre-seeded global nm bytes=$(dirsize "$T/config/opencode/node_modules")"
say "  pre-seeded project nm bytes=$(dirsize "$T/cwd/.opencode/node_modules")"
say "  pre-seeded models.json bytes=$(stat -f '%z' "$T/cache/opencode/models.json")"
say "  pre-seeded rg bytes=$(stat -f '%z' "$T/cache/opencode/bin/rg")"
BEFORE=$(dirsize "$COLD")
S=$(msnow)
(
  cd "$T/cwd" && /usr/bin/env -i \
    HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
    XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
    PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    npm_config_cache=$COLD \
    "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
) > "$OUT/a7-recommended.ndjson" 2> "$OUT/a7-recommended.stderr.txt" || true
E=$(msnow)
sleep 10
AFTER=$(dirsize "$COLD")
say "  wall_ms=$((E-S))"
say "  shared_npmcache_delta=$((AFTER-BEFORE))   <-- registry bytes pulled"
say "  project_nm_bytes=$(dirsize "$T/cwd/.opencode/node_modules")"
say "  global_nm_bytes=$(dirsize "$T/config/opencode/node_modules")"
say "  cwd_bytes=$(dirsize "$T/cwd")"
say "  data_bytes=$(dirsize "$T/data")"
say "  tmp_bytes=$(dirsize "$T/tmp")"
say "  trial_total_bytes=$(dirsize "$T")"
say "  audit_log:"
sed 's/^/    /' "$T/audit.log"
say "  rg downloaded? $([ -f "$T/cache/opencode/bin/ripgrep-15.1.0-aarch64-apple-darwin.tar.gz" ] && echo YES || echo NO)"
say "A7 DONE"
