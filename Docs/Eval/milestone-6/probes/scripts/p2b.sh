#!/bin/sh
# P2 (part 2) - skills.paths semantics + OPENCODE_DISABLE_PROJECT_CONFIG,
# model-free via `opencode debug skill`.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p2b"
rm -rf "$OUT"; mkdir -p "$OUT"

T=$(mktrial p2b)
# project-root opencode.json -> relative skills.paths (resolved against instance dir?)
mkdir -p "$T/cwd/.opencode" "$T/home/.opencode" "$T/config/opencode"
skillmd "$T/cwd/proj-paths/sp_proj/SKILL.md"  sp_proj  "proj"  PROJ-ROOT-PATHS
# .opencode/opencode.json -> relative skills.paths
skillmd "$T/cwd/dot-paths/sp_dot/SKILL.md"    sp_dot   "dot"   DOT-OPENCODE-PATHS
# global ~/.opencode/opencode.json -> absolute skills.paths
skillmd "$T/home/gpaths/sp_gpath/SKILL.md"    sp_gpath "gpath" HOME-OPENCODE-PATHS
# global XDG config opencode.json -> absolute skills.paths
skillmd "$T/config/opencode/xpaths/sp_xpath/SKILL.md" sp_xpath "xpath" XDG-OPENCODE-PATHS

cat > "$T/cwd/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["proj-paths"] } }
JSON
cat > "$T/cwd/.opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["dot-paths"] } }
JSON
cat > "$T/home/.opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["$T/home/gpaths"] } }
JSON
cat > "$T/config/opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["$T/config/opencode/xpaths"] } }
JSON

filetree "$T" > "$OUT/layout.tree.txt"

probe() {
  LABEL="$1"; shift
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" debug skill > "$OUT/$LABEL.json" 2>"$OUT/$LABEL.err" ) || true
  python3 - "$OUT/$LABEL.json" "$LABEL" <<'PY'
import json,sys
label=sys.argv[2]; raw=open(sys.argv[1]).read()
try: data=json.loads(raw)
except Exception as e:
    print(f"{label}: PARSE-ERROR {e}"); raise SystemExit
names=sorted(s["name"] for s in data if s["name"]!="customize-opencode")
print(f"{label}: {','.join(names)}")
PY
}
cfg() {
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" debug config 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin); print('   resolved skills.paths =', json.dumps((d.get('skills') or {}).get('paths')))
" )
}

echo "== baseline =="; probe base; cfg
echo "== OPENCODE_DISABLE_PROJECT_CONFIG=1 =="; probe flag-project-config OPENCODE_DISABLE_PROJECT_CONFIG=1; cfg OPENCODE_DISABLE_PROJECT_CONFIG=1
echo "P2B DONE"
