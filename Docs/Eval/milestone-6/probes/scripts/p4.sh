#!/bin/sh
# P4 - REAL TRIAL INTEGRATION.
# Treatment: project skill `.opencode/skill/root/SKILL.md` (byte-for-byte copy of
#          skills/claude/SKILL.md) present.
# Baseline:  identical env, skill ABSENT.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/trial.sh
OUT="$M6/out/p4"
rm -rf "$OUT"; mkdir -p "$OUT"

PROMPT="Install ripgrep on this machine."

run_trial() {  # run_trial <label> <trialdir>
  NAME="$1"; T="$2"
  S=$(date +%s)
  ( cd "$T/cwd" && $(trialenv4 "$T") "$BIN" run --format json --pure --model opencode/big-pickle "$PROMPT" ) \
      > "$OUT/$NAME.ndjson" 2> "$OUT/$NAME.stderr.txt" || true
  E=$(date +%s)
  echo $(( (E-S)*1000 )) > "$OUT/$NAME.wall_ms"
  echo "$NAME wall_ms=$(cat "$OUT/$NAME.wall_ms")"
  report "$T" "$NAME" "$OUT/$NAME.ndjson" | tee "$OUT/$NAME.report.txt"
  echo
}

echo "################ TREATMENT (skill present)"
T=$(mktrial4 p4treat with-skill)
echo "seeded skill sha256: $(shasum -a 256 "$T/cwd/.opencode/skill/root/SKILL.md" | cut -d' ' -f1)"
echo "repo    skill sha256: $(shasum -a 256 "$REPO/skills/claude/SKILL.md" | cut -d' ' -f1)"
run_trial treat "$T"

echo "################ BASELINE (no skill)"
T2=$(mktrial4 p4base without-skill)
run_trial base "$T2"

echo "P4 DONE"
