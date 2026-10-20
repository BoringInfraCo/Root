#!/bin/sh
# M6 discovery probe library (subagent: scan-order + flags).
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
M6=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe
BIN="$M6/bin/opencode"
SHARED="$M6/shared"
EXPECTED_SHA=f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6

mktrial() {
  T="$D/trials/$1"
  rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd" "$T/out"
  # pre-seed config dir (plugin node_modules) + models catalog to avoid npm registry work
  cp -R "$SHARED/config-template/opencode" "$T/config/opencode"
  mkdir -p "$T/cache/opencode"
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json"
  echo "$T"
}

trienv() {
  T="$1"; shift
  printf '%s' "env -i HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp PATH=$M6/syspath OPENCODE_DISABLE_AUTOUPDATE=1 npm_config_cache=$SHARED/npmcache"
  for kv in "$@"; do printf ' %s' "$kv"; done
}

skillmd() {
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<EOT
---
name: $2
description: $3
---

# Dup skill sourced from $4

MARKER: $4
EOT
}

instruct() {
  mkdir -p "$(dirname "$1")"
  printf '# Project rules\n\nMARKER-INSTR: %s\n' "$2" > "$1"
}
