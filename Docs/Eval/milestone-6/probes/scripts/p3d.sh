#!/bin/sh
# P3 (part 4) - DECISIVE isolation: seed the config dir with cache A, then run
# opencode against a COLD cache B. If B stays empty, the pre-seeded
# node_modules (not the warm cache) is what suppresses the plugin install.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p3d"
rm -rf "$OUT"; mkdir -p "$OUT"
SHARED="$M6/shared"
NPMDIR=/Users/sergio/.nvm/versions/node/v22.23.2/bin
REAL_NPM=$NPMDIR/npm

bytes() { find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'; }

CACHEA="$SHARED/cacheA"; COLD="$SHARED/cacheB"
rm -rf "$CACHEA" "$COLD"; mkdir -p "$CACHEA" "$COLD"

T="$M6/trials/p3iso"; rm -rf "$T"
mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd"
mkdir -p "$T/config/opencode"
( cd "$T/config/opencode" && env -i HOME=$T/home PATH=$NPMDIR:/usr/bin:/bin npm_config_cache="$CACHEA" \
    "$REAL_NPM" install --no-audit --no-fund --no-package-lock @opencode-ai/plugin@1.18.27 ) > "$OUT/seed.log" 2>&1 || true
echo "seeded node_modules bytes = $(bytes "$T/config/opencode/node_modules")"
echo "cacheA (seeding only) bytes = $(bytes "$CACHEA")"
echo "cacheB (cold, opencode-only) bytes before = $(bytes "$COLD")"

BEFORE=$(bytes "$COLD")
S=$(date +%s)
( cd "$T/cwd" && $(trienv "$T") npm_config_cache="$COLD" "$BIN" run --pure --format json --model opencode/big-pickle "Reply with exactly: ok" ) \
   > "$OUT/iso.ndjson" 2> "$OUT/iso.err" || true
E=$(date +%s); sleep 4
AFTER=$(bytes "$COLD")
echo "wall_s=$((E-S))"
echo "cacheB delta (bytes opencode itself fetched) = $((AFTER-BEFORE))"
echo "cacheB bytes after = $AFTER"
echo "config node_modules after = $(bytes "$T/config/opencode/node_modules")"
echo "--- text output ---"
python3 -c "
import json
for line in open('$OUT/iso.ndjson'):
    line=line.strip()
    if not line: continue
    o=json.loads(line)
    if o.get('type')=='text': print(repr((o.get('part') or {}).get('text')))
"
echo "P3D DONE"
