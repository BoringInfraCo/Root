#!/bin/sh
# Integration probe library (milestone-6 INTEGRATION-PROBE).
# All state lives under $M6I (disposable). No host writes outside $M6I.
M6I=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration
M6=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe
REPO=/Users/sergio/Documents/Developer/BoringInfraCo/Root
BIN="$M6I/bin/opencode"
SHARED="$M6I/shared"
EXPECTED_SHA=f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6

# dirsize <dir> -> total bytes of regular files under dir
dirsize() {
  if [ -d "$1" ]; then
    find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'
  else
    echo 0
  fi
}

# msnow -> epoch milliseconds
msnow() { python3 -c 'import time;print(int(time.time()*1000))'; }

# mktrial <name> -> creates $M6I/trials/<name> with isolated dirs
mktrial() {
  T="$M6I/trials/$1"
  rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd" "$T/bin" "$T/out"
  # fake shims (byte-identical protocol to the task brief)
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
  chmod +x "$T/bin/root" "$T/bin/brew"
  : > "$T/audit.log"
  echo "$T"
}

# seed_skill <trial> -> copies skills/claude/SKILL.md byte-for-byte to
# <trial>/cwd/.opencode/skill/root/SKILL.md
seed_skill() {
  mkdir -p "$1/cwd/.opencode/skill/root"
  cp "$REPO/skills/claude/SKILL.md" "$1/cwd/.opencode/skill/root/SKILL.md"
  shasum -a 256 "$1/cwd/.opencode/skill/root/SKILL.md" | cut -d' ' -f1
}

# seed_config <trial> -> pre-seed XDG_CONFIG_HOME/opencode with the plugin tree
seed_config() {
  mkdir -p "$1/config"
  cp -R "$SHARED/config-template/opencode" "$1/config/opencode"
}

# seed_models <trial> [mode]  mode=ro (default) copies the shared models.json
# read-only; mode=rw copies it writable.
seed_models() {
  mkdir -p "$1/cache/opencode"
  cp "$SHARED/models/models.json" "$1/cache/opencode/models.json"
  if [ "${2:-ro}" = "ro" ]; then chmod 444 "$1/cache/opencode/models.json"; fi
}

# seed_rg <trial> - pre-seed ripgrep into XDG_CACHE_HOME/opencode/bin/rg.
# The `skill` tool resolves ripgrep via: `which rg` -> <XDG_CACHE_HOME>/opencode/bin/rg
# -> else download+extract from GitHub (needs `tar`, absent from a shims-only PATH).
# Pre-seeding makes the skill tool hermetic and offline.
seed_rg() {
  mkdir -p "$1/cache/opencode/bin"
  cp "$SHARED/rg/ripgrep-15.1.0-aarch64-apple-darwin/rg" "$1/cache/opencode/bin/rg"
  chmod 755 "$1/cache/opencode/bin/rg"
}

# trienv <trial> <pathspec> [extra env...]
#   pathspec: "shims"  -> PATH=<trial>/bin            (strict: only the 2 shims)
#              "shims+sys" -> PATH=<trial>/bin:/bin:/usr/bin
#   extra env kv pairs are appended AFTER the defaults, so a later
#   "npm_config_cache=X" / "NPM_CONFIG_CACHE=X" overrides the default.
#   NOTE: "env" itself is resolved by the CALLING shell (absolute /usr/bin/env),
#   never from the trial PATH.
trienv() {
  T="$1"; P="$2"; shift 2
  case "$P" in
    shims)    PP="$T/bin" ;;
    shims+sys) PP="$T/bin:/bin:/usr/bin" ;;
    *) echo "bad pathspec" >&2; exit 9 ;;
  esac
  printf '%s' "/usr/bin/env -i HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp PATH=$PP TRIAL_AUDIT_LOG=$T/audit.log OPENCODE_DISABLE_AUTOUPDATE=1 npm_config_cache=$SHARED/npmcache"
  for kv in "$@"; do printf ' %s' "$kv"; done
}

# run_trial <trial> <label> <pathspec> <prompt> [extra opencode args...]
#   -> writes $M6I/out/<label>.{ndjson,stderr,metrics}
run_trial() {
  T="$1"; LABEL="$2"; P="$3"; PROMPT="$4"; shift 4
  OUT="$M6I/out"
  mkdir -p "$OUT"
  CACHE_BEFORE=$(dirsize "$SHARED/npmcache")
  S=$(msnow)
  ( cd "$T/cwd" && $(trienv "$T" "$P") "$BIN" run --format json --model opencode/big-pickle "$@" "$PROMPT" ) \
      > "$OUT/$LABEL.ndjson" 2> "$OUT/$LABEL.stderr.txt"
  RC=$?
  E=$(msnow)
  CACHE_AFTER=$(dirsize "$SHARED/npmcache")
  {
    echo "label=$LABEL"
    echo "rc=$RC"
    echo "wall_ms=$((E-S))"
    echo "shared_npmcache_before=$CACHE_BEFORE"
    echo "shared_npmcache_after=$CACHE_AFTER"
    echo "shared_npmcache_delta=$((CACHE_AFTER-CACHE_BEFORE))"
    echo "trial_total_bytes=$(dirsize "$T")"
    echo "home_npm_bytes=$(dirsize "$T/home/.npm")"
    echo "config_bytes=$(dirsize "$T/config")"
    echo "config_node_modules_bytes=$(dirsize "$T/config/opencode/node_modules")"
    echo "data_bytes=$(dirsize "$T/data")"
    echo "cache_bytes=$(dirsize "$T/cache")"
    echo "models_json_bytes=$( [ -f "$T/cache/opencode/models.json" ] && stat -f '%z' "$T/cache/opencode/models.json" || echo 0)"
    echo "tmp_bytes=$(dirsize "$T/tmp")"
  } > "$OUT/$LABEL.metrics"
  cat "$OUT/$LABEL.metrics"
}
