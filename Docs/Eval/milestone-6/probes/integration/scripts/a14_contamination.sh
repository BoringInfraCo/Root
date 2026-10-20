#!/bin/sh
# A14 - contamination: does a trial run MUTATE the shared pre-seeded template
#       when the trial's node_modules are APFS clones (cp -Rc) vs hardlinks?
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a14-contamination.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

for S in clone hard; do
  TMPL=$M6I/shared/a14t-$S
  rm -rf "$TMPL"; mkdir -p "$TMPL/global" "$TMPL/project"
  cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/global/node_modules"
  cp -R "$SHARED/config-template/opencode/node_modules" "$TMPL/project/node_modules"
  BEFORE=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s+0}')
  CNT=$(find "$TMPL" -type f | wc -l | tr -d ' ')
  BEFORE_LOCK=$(stat -f '%z' "$TMPL/project/node_modules/.package-lock.json" 2>/dev/null || echo NA)

  T=$M6I/trials/a14run-$S
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
  ) > "$OUT/a14run-$S.ndjson" 2> "$OUT/a14run-$S.stderr.txt" || true
  sleep 10
  AFTER=$(find "$TMPL" -type f -exec stat -f '%z' {} + | awk '{s+=$1}END{print s+0}')
  CNT2=$(find "$TMPL" -type f | wc -l | tr -d ' ')
  AFTER_LOCK=$(stat -f '%z' "$TMPL/project/node_modules/.package-lock.json" 2>/dev/null || echo NA)
  say "strategy=$S"
  say "  template bytes   before=$BEFORE after=$AFTER"
  say "  template files   before=$CNT after=$CNT2"
  say "  .package-lock.json size before=$BEFORE_LOCK after=$AFTER_LOCK"
  say "  TEMPLATE MUTATED? $([ "$BEFORE" != "$AFTER" ] && echo YES || echo NO)"
  say "  trial audit_log=$(tr '\n' '|' < "$T/audit.log")"
  say "  trial skill event present? $(grep -c '"tool":"skill"' "$OUT/a14run-$S.ndjson")"
  say ""
  rm -rf "$TMPL"
done
say "A14 DONE"
