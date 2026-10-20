#!/bin/sh
# M6 live-conformance probe library.
# All state lives under $M6 (disposable). No host writes.
M6=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe
BIN="$M6/bin/opencode"
EXPECTED_SHA=f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6

# mktrial <name> -> creates $M6/trials/<name> with the 6 isolated dirs + cwd
mktrial() {
  T="$M6/trials/$1"
  rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd" "$T/out" "$T/shims"
  echo "$T"
}

# trienv <trialdir> [extra env assignments...] -> prints `env -i ...` prefix
trienv() {
  T="$1"; shift
  printf '%s' "env -i HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp PATH=$M6/syspath OPENCODE_DISABLE_AUTOUPDATE=1"
  for kv in "$@"; do printf ' %s' "$kv"; done
}

# dirsize <dir> -> total bytes
dirsize() {
  if [ -d "$1" ]; then
    find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'
  else
    echo 0
  fi
}

# filetree <dir> -> sorted relative path listing
filetree() {
  ( cd "$1" 2>/dev/null && find . -type f | sort ) || true
}

# skillmd <path> <name> <description> <marker>
skillmd() {
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<EOF
---
name: $2
description: $3
---

# Dup skill sourced from $4

MARKER: $4
EOF
}

# run_oc <trialdir> <workdir> <outprefix> [args...] -- runs opencode, captures ndjson/stderr/exit/wall
run_oc() {
  T="$1"; WD="$2"; PFX="$3"; shift 3
  START=$(date +%s)
  ( cd "$WD" && $(trienv "$T") "$BIN" "$@" ) \
    > "$PFX.stdout.ndjson" 2> "$PFX.stderr.txt"
  RC=$?
  END=$(date +%s)
  echo "$RC" > "$PFX.exit"
  echo $(( (END - START) * 1000 )) > "$PFX.wall_ms"
  return 0
}

# summarize <ndjsonfile> -> prints one line per event: seq type tool= status dir= marker=
summarize() {
  python3 - "$1" <<'PY'
import json,sys
for i,line in enumerate(open(sys.argv[1])):
    line=line.strip()
    if not line: continue
    try: o=json.loads(line)
    except Exception as e:
        print(f"{i}\tPARSE-ERROR\t{e}"); continue
    t=o.get("type")
    if t=="tool_use":
        p=o.get("part",{}); st=p.get("state",{})
        md=st.get("metadata") or {}
        out=st.get("output") or ""
        marker=""
        for ln in out.splitlines():
            if ln.startswith("MARKER:"): marker=ln.strip()
        print(f"{i}\t{t}\ttool={p.get('tool')}\tstatus={st.get('status')}\tdir={md.get('dir')}\tname={md.get('name')}\t{marker}")
    elif t=="text":
        p=o.get("part",{})
        txt=(p.get("text") or "").replace("\n","\\n")
        print(f"{i}\t{t}\t{txt[:400]}")
    elif t=="step_finish":
        p=o.get("part",{})
        print(f"{i}\t{t}\treason={p.get('reason')}\ttokens={json.dumps(p.get('tokens'))}\tcost={p.get('cost')}")
    elif t=="error":
        print(f"{i}\t{t}\t{json.dumps(o.get('error'))[:300]}")
    else:
        print(f"{i}\t{t}")
PY
}
