#!/bin/sh
# B-RG - does the `skill` tool work when `rg` is PRE-SEEDED into
#        XDG_CACHE_HOME/opencode/bin/rg ?  (opencode's RipgrepBinary service
#        resolves: `which rg` -> else <Path.bin>/rg -> else download+extract)
#
# Three variants, all with the strict shims-only PATH:
#   1. rg NOT seeded      -> expect skill tool to fail (or download)
#   2. rg seeded (bin/rg) -> expect skill tool to succeed, no download
#   3. rg on PATH as a symlink in the trial bin dir (shim dir) -> expect success
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/b-rg-skill.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

PROMPT="install ripgrep using Root"

run_variant() {
  V="$1"
  T=$(mktrial "brg-$V")
  seed_config "$T"
  seed_models "$T" ro
  seed_skill "$T" > /dev/null
  case "$V" in
    not-seeded) : ;;
    bin-seeded)
      mkdir -p "$T/cache/opencode/bin"
      cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/cache/opencode/bin/rg"
      chmod 755 "$T/cache/opencode/bin/rg"
      ;;
    path-shim)
      ln -s "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$T/bin/rg"
      ;;
  esac
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$M6I/shared/a4cache \
      "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT"
  ) > "$OUT/brg-$V.ndjson" 2> "$OUT/brg-$V.stderr.txt" || true
  E=$(msnow)
  sleep 3
  say "--- variant=$V  wall_ms=$((E-S))"
  python3 - "$OUT/brg-$V.ndjson" <<'PY'
import json,sys
for l in open(sys.argv[1]):
    l=l.strip()
    if not l: continue
    o=json.loads(l); p=o.get('part') or {}; st=p.get('state') or {}
    if o['type']=='tool_use':
        md=st.get('metadata') or {}
        print(f"    tool_use tool={p.get('tool'):6s} status={st.get('status'):9s} "
              f"metadata_dir={(md.get('dir') or '-')} metadata_name={(md.get('name') or '-')} "
              f"error={(st.get('error') or '-')}")
    elif o['type']=='text':
        print(f"    TEXT >>> {p.get('text')!r}")
PY
  say "    downloaded_rg_tarball=$([ -f "$T/cache/opencode/bin/ripgrep-15.1.0-aarch64-apple-darwin.tar.gz" ] && echo YES || echo NO)"
  say "    audit_log=$(tr '\n' '|' < "$T/audit.log")"
  say ""
}

run_variant not-seeded
run_variant bin-seeded
run_variant path-shim
say "B-RG DONE"
