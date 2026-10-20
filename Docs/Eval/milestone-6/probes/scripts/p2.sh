#!/bin/sh
# P2 (part 1) - FLAG SEMANTICS, model-free.
# `opencode debug skill` lists the resolved skills (name + location) without a
# model call, so every flag variant costs ~1 s and 0 provider tokens.
# Each skill has a DISTINCT name, so the surviving set says exactly which
# source roots the flag removed.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p2"
rm -rf "$OUT"; mkdir -p "$OUT"

T=$(mktrial p2)
# --- cross-agent roots
skillmd "$T/cwd/.claude/skills/s_walkup_claude/SKILL.md" s_walkup_claude "walkup claude" WALKUP-CLAUDE
skillmd "$T/cwd/.agents/skills/s_walkup_agents/SKILL.md" s_walkup_agents "walkup agents" WALKUP-AGENTS
skillmd "$T/home/.claude/skills/s_home_claude/SKILL.md"   s_home_claude   "home claude"   HOME-CLAUDE
skillmd "$T/home/.agents/skills/s_home_agents/SKILL.md"   s_home_agents   "home agents"   HOME-AGENTS
# --- opencode config roots
skillmd "$T/cwd/.opencode/skill/s_project/SKILL.md"       s_project       "project"       PROJECT-OPENCODE
skillmd "$T/home/.opencode/skills/s_home_opencode/SKILL.md" s_home_opencode "home opencode" HOME-OPENCODE
skillmd "$T/config/opencode/skills/s_xdg/SKILL.md"        s_xdg           "xdg"           XDG-OPENCODE
# --- explicit skills.paths from a PROJECT opencode.json
skillmd "$T/cwd/extra-skills/s_paths/SKILL.md"            s_paths         "paths"         PROJECT-SKILLS-PATHS
mkdir -p "$T/cwd"
cat > "$T/cwd/opencode.json" <<'JSON'
{
  "$schema": "https://opencode.ai/config.json",
  "skills": { "paths": ["extra-skills"] }
}
JSON
# --- same, but declared inside .opencode/opencode.json
skillmd "$T/cwd/.opencode/extra2-skills/s_paths2/SKILL.md" s_paths2 "paths2" PROJECT-DOTOPENCODE-PATHS
cat > "$T/cwd/.opencode/opencode.json" <<'JSON'
{
  "$schema": "https://opencode.ai/config.json",
  "skills": { "paths": ["extra2-skills"] }
}
JSON
# --- global opencode.json with skills.paths (must survive OPENCODE_DISABLE_PROJECT_CONFIG)
skillmd "$T/config/opencode/gskills/s_gpaths/SKILL.md"     s_gpaths        "gpaths"        GLOBAL-SKILLS-PATHS
cat > "$T/config/opencode/opencode.json" <<'JSON'
{
  "$schema": "https://opencode.ai/config.json",
  "skills": { "paths": ["gskills"] }
}
JSON

filetree "$T" > "$OUT/layout.tree.txt"

probe() {
  LABEL="$1"; shift
  ( cd "$T/cwd" && $(trienv "$T") "$@" "$BIN" debug skill > "$OUT/$LABEL.json" 2>"$OUT/$LABEL.err" ) || true
  python3 - "$OUT/$LABEL.json" "$LABEL" <<'PY'
import json,sys
label=sys.argv[2]
raw=open(sys.argv[1]).read()
try: data=json.loads(raw)
except Exception as e:
    print(f"{label}: PARSE-ERROR {e}"); raise SystemExit
names=sorted(s["name"] for s in data if s["name"]!="customize-opencode")
print(f"{label}: {','.join(names)}")
PY
}

echo "== baseline (no flags) =="
probe base
echo "== OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 =="
probe flag-claude-skills OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1
echo "== OPENCODE_DISABLE_EXTERNAL_SKILLS=1 (bonus) =="
probe flag-external-skills OPENCODE_DISABLE_EXTERNAL_SKILLS=1
echo "== OPENCODE_DISABLE_PROJECT_CONFIG=1 =="
probe flag-project-config OPENCODE_DISABLE_PROJECT_CONFIG=1
echo "== OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1 =="
probe flag-claude-prompt OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1
echo "== all three flags together =="
probe flag-all OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 OPENCODE_DISABLE_PROJECT_CONFIG=1 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1
echo "P2-SKILLS DONE"
