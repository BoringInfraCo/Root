#!/bin/sh
# A11 - DISK SCALING: how much disk does a 60-trial batch actually consume?
#   (a) with APFS clones (cp -Rc) of the pre-seeded node_modules
#   (b) the shared assets that must be created ONCE
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a11-disk-scaling.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

BATCH=$M6I/shared/a11-batch; rm -rf "$BATCH"; mkdir -p "$BATCH"

say "############ A11 disk scaling for a 60-trial batch (NO opencode runs)"
say ""
say "  -- shared assets created ONCE --"
say "     shared warm npm cache (a6cache)   : $(du -sk "$M6I/shared/a6cache" | cut -f1) KB"
say "     shared config template (node_modules): $(du -sk "$SHARED/config-template" | cut -f1) KB"
say "     shared models.json                  : $(du -sk "$SHARED/models" | cut -f1) KB"
say "     shared rg binary                    : $(du -sk "$SHARED/rg" | cut -f1) KB"
say "     TOTAL shared                        : $(du -sk "$M6I/shared/a6cache" "$SHARED/config-template" "$SHARED/models" "$SHARED/rg" | awk '{s+=$1} END {print s}') KB"

say ""
say "  -- per-trial materialized bytes (APFS clone) --"
T=$M6I/trials/a11probe
mkdir -p "$T/config/opencode" "$T/cwd/.opencode" "$T/cache/opencode/bin" "$T/bin" "$T/home" "$T/data" "$T/state" "$T/tmp"
cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/config/opencode/node_modules"
cp -Rc "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"
cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"; chmod 444 "$T/cache/opencode/models.json"
cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill-tmp" 2>/dev/null || true
mkdir -p "$T/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill/root/SKILL.md"
say "     apparent bytes (stat sum)           : $(dirsize "$T")"
say "     du -sk of ONE trial                 : $(du -sk "$T" | cut -f1) KB"

say ""
say "  -- 60 such trials materialized, then du the batch --"
for i in $(seq 1 60); do
  D=$BATCH/t$i
  mkdir -p "$D/config/opencode" "$D/cwd/.opencode" "$D/cache/opencode/bin"
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$D/config/opencode/node_modules"
  cp -Rc "$SHARED/config-template/opencode/node_modules" "$D/cwd/.opencode/node_modules"
  cp "$SHARED/models/models.json" "$D/cache/opencode/models.json"; chmod 444 "$D/cache/opencode/models.json"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$D/cache/opencode/bin/rg"
  mkdir -p "$D/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$D/cwd/.opencode/skill/root/SKILL.md"
done
say "     batch du -sk (60 trials, APFS clones): $(du -sk "$BATCH" | cut -f1) KB"
say "     batch apparent bytes (stat sum)      : $(dirsize "$BATCH")"
say "     => per-trial marginal disk (batch/60): $(( $(du -sk "$BATCH" | cut -f1) / 60 )) KB"

say ""
say "  -- 60 trials materialized WITHOUT clones (full copies) for comparison --"
BATCH2=$M6I/shared/a11-batch2; rm -rf "$BATCH2"; mkdir -p "$BATCH2"
for i in 1 2 3; do
  D=$BATCH2/t$i
  mkdir -p "$D/config/opencode" "$D/cwd/.opencode"
  cp -R "$SHARED/config-template/opencode/node_modules" "$D/config/opencode/node_modules"
  cp -R "$SHARED/config-template/opencode/node_modules" "$D/cwd/.opencode/node_modules"
done
say "     3 trials full-copy du -sk            : $(du -sk "$BATCH2" | cut -f1) KB  => per-trial $(( $(du -sk "$BATCH2" | cut -f1) / 3 )) KB"
say "     => extrapolated 60 trials full copy  : $(( $(du -sk "$BATCH2" | cut -f1) / 3 * 60 )) KB"

rm -rf "$BATCH" "$BATCH2"
say "A11 DONE"
