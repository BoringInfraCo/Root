#!/bin/sh
# A12 - disk seeding strategy: hardlink vs APFS-clone vs full-copy.
# Also verifies the shared template is NOT mutated by a trial run.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a12-disk-seeding.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

say "############ A12 does cp -c (APFS clone) actually work on this volume?"
T=$M6I/trials/a12c; rm -rf "$T"; mkdir -p "$T/d"
if cp -c "$SHARED/config-template/opencode/node_modules/zod" "$T/d/zod" 2>/dev/null; then
  say "  cp -c SUPPORTED"
  say "  src inode=$(stat -f '%i' "$SHARED/config-template/opencode/node_modules/zod")  clone inode=$(stat -f '%i' "$T/d/zod")"
  say "  src du -sk=$(du -sk "$SHARED/config-template/opencode/node_modules/zod" | cut -f1)  clone du -sk=$(du -sk "$T/d/zod" | cut -f1)"
else
  say "  cp -c NOT supported -> $(cp -c "$SHARED/config-template/opencode/node_modules/zod" "$T/d/zod" 2>&1 | head -1)"
fi

say ""
say "############ A12 hardlink seeding: 60 trials, ZERO marginal bytes?"
B=$M6I/shared/a12-hard; rm -rf "$B"; mkdir -p "$B"
TMPL=$M6I/shared/a12-template
rm -rf "$TMPL"; mkdir -p "$TMPL/global" "$TMPL/project"
cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/global/node_modules"
cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/project/node_modules"
say "  template global nm du -sk = $(du -sk "$TMPL/global" | cut -f1)"
say "  template project nm du -sk = $(du -sk "$TMPL/project" | cut -f1)"
for i in $(seq 1 60); do
  D=$B/t$i
  mkdir -p "$D/config/opencode" "$D/cwd/.opencode" "$D/cache/opencode/bin"
  cp -al "$TMPL/global/node_modules" "$D/config/opencode/node_modules" 2>/dev/null \
    || cp -R "$TMPL/global/node_modules" "$D/config/opencode/node_modules"
  cp -al "$TMPL/project/node_modules" "$D/cwd/.opencode/node_modules" 2>/dev/null \
    || cp -R "$TMPL/project/node_modules" "$D/cwd/.opencode/node_modules"
  cp "$SHARED/models/models.json" "$D/cache/opencode/models.json"; chmod 444 "$D/cache/opencode/models.json"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$D/cache/opencode/bin/rg"
  mkdir -p "$D/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$D/cwd/.opencode/skill/root/SKILL.md"
done
say "  60-trial hardlink batch du -sk = $(du -sk "$B" | cut -f1) KB"
say "  => per-trial marginal disk    = $(( $(du -sk "$B" | cut -f1) / 60 )) KB"
say "  sample inode check: template=$(stat -f '%i' "$TMPL/global/node_modules/zod") trial=$(stat -f '%i' "$B/t1/config/opencode/node_modules/zod")"

say ""
say "############ A12 hardlink batch: run one trial and verify the TEMPLATE is unchanged"
TPL_SUM_BEFORE=$(dirsize "$TMPL")
TPL_NM_BEFORE=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s}')
TPL_CNT_BEFORE=$(find "$TMPL" -type f | wc -l | tr -d ' ')
T=$M6I/trials/a12run
rm -rf "$T"; mkdir -p "$T/config/opencode" "$T/cwd/.opencode" "$T/cache/opencode/bin" "$T/home" "$T/data" "$T/state" "$T/tmp" "$T/bin"
cat > "$T/bin/root" <<'SH'
#!/bin/sh
printf '%s\n' "root $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' '{"success":true}'
exit 0
SH
cat > "$T/bin/brew" <<'SH'
#!/bin/sh
printf '%s\n' "brew $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' 'brew: fake shim: refusing to install (trial sandbox)' >&2
exit 1
SH
chmod +x "$T/bin/root" "$T/bin/brew"; : > "$T/audit.log"
cp -al "$TMPL/global/node_modules" "$T/config/opencode/node_modules" 2>/dev/null || cp -R "$TMPL/global/node_modules" "$T/config/opencode/node_modules"
cp -al "$TMPL/project/node_modules" "$T/cwd/.opencode/node_modules" 2>/dev/null || cp -R "$TMPL/project/node_modules" "$T/cwd/.opencode/node_modules"
cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"; chmod 444 "$T/cache/opencode/models.json"
cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
mkdir -p "$T/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill/root/SKILL.md"
S=$(msnow)
(
  cd "$T/cwd" && /usr/bin/env -i \
    HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
    XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
    PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    npm_config_cache=$M6I/shared/a6cache \
    "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
) > "$OUT/a12run.ndjson" 2> "$OUT/a12run.stderr.txt" || true
E=$(msnow)
sleep 8
TPL_SUM_AFTER=$(dirsize "$TMPL")
TPL_NM_AFTER=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s}')
TPL_CNT_AFTER=$(find "$TMPL" -type f | wc -l | tr -d ' ')
say "  wall_ms=$((E-S))"
say "  template bytes before=$TPL_NM_BEFORE after=$TPL_NM_AFTER   files before=$TPL_CNT_BEFORE after=$TPL_CNT_AFTER"
say "  TEMPLATE UNCHANGED? $([ "$TPL_NM_BEFORE" = "$TPL_NM_AFTER" ] && [ "$TPL_CNT_BEFORE" = "$TPL_CNT_AFTER" ] && echo YES || echo NO)"
say "  audit_log=$(tr '\n' '|' < "$T/audit.log")"
say "  trial project nm bytes=$(dirsize "$T/cwd/.opencode/node_modules")"

rm -rf "$B"
say "A12 DONE"
