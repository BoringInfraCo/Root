#!/bin/sh
# P2 (part 2b) - isolated skills.paths + OPENCODE_DISABLE_PROJECT_CONFIG, and
# config-file merge precedence for skills.paths. Model-free.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p2b2"
rm -rf "$OUT"; mkdir -p "$OUT"

probe_trial() {
  T="$1"; LABEL="$2"; shift 2
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" debug skill > "$OUT/$LABEL.json" 2>"$OUT/$LABEL.err" ) || true
  python3 - "$OUT/$LABEL.json" "$LABEL" <<'PY'
import json,sys
label=sys.argv[2]; raw=open(sys.argv[1]).read()
try: data=json.loads(raw)
except Exception as e:
    print(f"{label}: PARSE-ERROR {e}"); raise SystemExit
names=sorted(s["name"] for s in data if s["name"]!="customize-opencode")
print(f"{label}: {','.join(names) if names else '(none)'}")
PY
}

# --- A: ONLY a project-root opencode.json declares skills.paths
T=$(mktrial a); mkdir -p "$T/cwd"
skillmd "$T/cwd/proj-paths/sp_proj/SKILL.md" sp_proj "proj" PROJ-ROOT-PATHS
cat > "$T/cwd/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["proj-paths"] } }
JSON
echo "== A: project opencode.json skills.paths only =="
probe_trial "$T" A-base
probe_trial "$T" A-disable-project-config OPENCODE_DISABLE_PROJECT_CONFIG=1

# --- B: ONLY .opencode/opencode.json declares skills.paths
T=$(mktrial b); mkdir -p "$T/cwd/.opencode"
skillmd "$T/cwd/dot-paths/sp_dot/SKILL.md" sp_dot "dot" DOT-OPENCODE-PATHS
cat > "$T/cwd/.opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["dot-paths"] } }
JSON
echo "== B: .opencode/opencode.json skills.paths only =="
probe_trial "$T" B-base
probe_trial "$T" B-disable-project-config OPENCODE_DISABLE_PROJECT_CONFIG=1

# --- C: project opencode.json vs XDG global opencode.json (both declare paths)
T=$(mktrial c); mkdir -p "$T/cwd" "$T/config/opencode"
skillmd "$T/cwd/proj-paths/sp_proj/SKILL.md" sp_proj "proj" PROJ-ROOT-PATHS
skillmd "$T/config/opencode/xpaths/sp_xpath/SKILL.md" sp_xpath "xpath" XDG-OPENCODE-PATHS
cat > "$T/cwd/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["proj-paths"] } }
JSON
cat > "$T/config/opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["$T/config/opencode/xpaths"] } }
JSON
echo "== C: project opencode.json vs XDG global opencode.json =="
probe_trial "$T" C-base

# --- D: project opencode.json vs ~/.opencode/opencode.json
T=$(mktrial d); mkdir -p "$T/cwd" "$T/home/.opencode"
skillmd "$T/cwd/proj-paths/sp_proj/SKILL.md" sp_proj "proj" PROJ-ROOT-PATHS
skillmd "$T/home/hpaths/sp_hpath/SKILL.md" sp_hpath "hpath" HOME-OPENCODE-PATHS
cat > "$T/cwd/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["proj-paths"] } }
JSON
cat > "$T/home/.opencode/opencode.json" <<JSON
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["$T/home/hpaths"] } }
JSON
echo "== D: project opencode.json vs ~/.opencode/opencode.json =="
probe_trial "$T" D-base

echo "P2B2 DONE"
