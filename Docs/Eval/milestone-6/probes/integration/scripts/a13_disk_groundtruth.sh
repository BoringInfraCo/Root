#!/bin/sh
# A13 - GROUND-TRUTH disk cost (df free-space delta) + contamination check.
#   strategy 1: cp -Rc   (APFS copy-on-write clone)
#   strategy 2: cp -al   (hardlink)
#   strategy 3: cp -R    (full copy)
# And: does a trial run MUTATE the shared template under each strategy?
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a13-disk-groundtruth.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

FREE() { df -k /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/ | awk 'NR==2{print $4}'; }

build_batch() {  # build_batch <dir> <strategy>
  B="$1"; S="$2"; rm -rf "$B"; mkdir -p "$B"
  for i in $(seq 1 60); do
    D=$B/t$i
    mkdir -p "$D/config/opencode" "$D/cwd/.opencode" "$D/cache/opencode/bin"
    case "$S" in
      clone) cp -Rc "$SHARED/config-template/opencode/node_modules" "$D/config/opencode/node_modules"
             cp -Rc "$SHARED/config-template/opencode/node_modules" "$D/cwd/.opencode/node_modules" ;;
      hard) cp -al "$SHARED/config-template/opencode/node_modules" "$D/config/opencode/node_modules"
             cp -al "$SHARED/config-template/opencode/node_modules" "$D/cwd/.opencode/node_modules" ;;
      copy) cp -R  "$SHARED/config-template/opencode/node_modules" "$D/config/opencode/node_modules"
             cp -R  "$SHARED/config-template/opencode/node_modules" "$D/cwd/.opencode/node_modules" ;;
    esac
    cp "$SHARED/models/models.json" "$D/cache/opencode/models.json"; chmod 444 "$D/cache/opencode/models.json"
    cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$D/cache/opencode/bin/rg"
    mkdir -p "$D/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$D/cwd/.opencode/skill/root/SKILL.md"
  done
}

for S in clone hard copy; do
  B=$M6I/shared/a13-$S
  F1=$(FREE)
  build_batch "$B" "$S"
  F2=$(FREE)
  say "strategy=$S : df free-space delta for 60 trials = $(( (F1-F2) )) KB  => $(( (F1-F2)/60 )) KB/trial"
  say "             du -sk batch = $(du -sk "$B" | cut -f1) KB (apparent)"
  rm -rf "$B"
done
say ""

# ------------------------------------------------------ contamination check
say "############ A13 contamination: does a trial run mutate the shared template?"
for S in clone hard; do
  TMPL=$M6I/shared/a13t-$S
  rm -rf "$TMPL"; mkdir -p "$TMPL/global" "$TMPL/project"
  cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/global/node_modules"
  cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/project/node_modules"
  BEFORE=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s+0}')
  CNT=$(find "$TMPL" -type f | wc -l | tr -d ' ')

  T=$M6I/trials/a13run-$S
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
  case "$S" in
    clone) cp -Rc "$TMPL/global/node_modules" "$T/config/opencode/node_modules"
           cp -Rc "$TMPL/project/node_modules" "$T/cwd/.opencode/node_modules" ;;
    hard) cp -al "$TMPL/global/node_modules" "$T/config/opencode/node_modules"
           cp -al "$TMPL/project/node_modules" "$T/cwd/.opencode/node_modules" ;;
  esac
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"; chmod 444 "$T/cache/opencode/models.json"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
  mkdir -p "$T/cwd/.opencode/skill/root"; cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill/root/SKILL.md"
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$M6I/shared/a6cache \
      "$BIN" run --format json --model opencode/big-pickle --pure "install ripgrep using Root"
  ) > "$OUT/a13run-$S.ndjson" 2> "$OUT/a13run-$S.stderr.txt" || true
  sleep 8
  AFTER=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s+0}')
  CNT2=$(find "$TMPL" -type f | wc -l | tr -d ' ')
  say "  strategy=$S: template bytes before=$BEFORE after=$AFTER  files $CNT -> $CNT2  MUTATED=$([ "$BEFORE" != "$AFTER" ] && echo YES || echo NO)"
  say "    trial audit_log=$(tr '\n' '|' < "$T/audit.log")"
  rm -rf "$TMPL"
done
say "A13 DONE"
