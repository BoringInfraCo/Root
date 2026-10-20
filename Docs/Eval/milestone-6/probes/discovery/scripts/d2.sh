#!/bin/sh
# M6 DISCOVERY - TASK B: OPENCODE_DISABLE_* flag semantics.
#
# B1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS : cross-agent skill dirs
#      (~/.claude/skills, ~/.agents/skills, walk-up .claude/.agents variants)
# B2 OPENCODE_DISABLE_PROJECT_CONFIG     : project .opencode config + skills,
#                                          and the project instruction walk-up
# B3 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT : global ~/.claude/CLAUDE.md fallback
# B4 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT : project-level CLAUDE.md
#
# Observables:
#   * skills      -> `opencode debug skill` (model-free list) + native `skill`
#                    tool metadata.dir from the model-driven run
#   * instructions-> the model reports the verbatim "Instructions from: <path>"
#                    lines that are injected into its system prompt
set -e
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
. "$D/lib.sh"
R=/Users/sergio/Documents/Developer/BoringInfraCo/Root/Docs/Eval/milestone-6/probes/discovery
OUT="$D/out/b"
rm -rf "$OUT"; mkdir -p "$OUT"
mkdir -p "$R/task-b"

gittree() {
  ( cd "$1" && git init -q . && git config user.email p@p && git config user.name p && \
    echo x > README && git add -A && git commit -qm init )
}

run_model() { # <trial> <label> <idx> <env...> -- <prompt>
  T="$1"; LABEL="$2"; I="$3"; shift 3
  ENVS=""; PROMPT=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --) shift; PROMPT="$*"; break ;;
      *) ENVS="$ENVS $1"; shift ;;
    esac
  done
  RUNDIR="$OUT/$LABEL/run-$I"; mkdir -p "$RUNDIR"
  ( cd "$T/cwd" && $(trienv "$T" $ENVS) "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT" ) \
      > "$RUNDIR/out.ndjson" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/out.ndjson" "$R/task-b/$LABEL.model.run-$I.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-b/$LABEL.model.run-$I.err.txt"
}

run_debug_skill() { # <trial> <label> <idx> <env...>
  T="$1"; LABEL="$2"; I="$3"; shift 3
  RUNDIR="$OUT/$LABEL/debug-$I"; mkdir -p "$RUNDIR"
  ( cd "$T/cwd" && $(trienv "$T" "$@") "$BIN" debug skill ) > "$RUNDIR/skills.json" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/skills.json" "$R/task-b/$LABEL.debug.run-$I.json"
}

run_debug_config() { # <trial> <label> <idx> <env...>
  T="$1"; LABEL="$2"; I="$3"; shift 3
  RUNDIR="$OUT/$LABEL/cfg-$I"; mkdir -p "$RUNDIR"
  ( cd "$T/cwd" && $(trienv "$T" "$@") "$BIN" debug config ) > "$RUNDIR/config.json" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/config.json" "$R/task-b/$LABEL.debug.config-$I.json"
}

INSTR_PROMPT="List every line in your instructions that starts with 'Instructions from:'. Output each such line verbatim on its own line. If there are none, output exactly NONE."
SKILL_PROMPT="Use the claudeskill skill, the agentsskill skill, the projclaudeskill skill and the projagentsskill skill. For each one that exists, report its base directory. For each one that does not exist, write NOTFOUND. Also use the ocskill skill if it exists and report its base directory."

### --------------------------------------------------------------- B0 control
# no instruction files at all -> the method must answer NONE
T=$(mktrial b0); gittree "$T/cwd"
echo "### B0 control: no instruction files"
run_model "$T" b0-control 1 -- "$INSTR_PROMPT"

### --------------------------------------------------------------- B1 skills flag
T=$(mktrial b1); gittree "$T/cwd"
skillmd "$T/home/.claude/skills/claudeskill/SKILL.md"     claudeskill     "Cross agent claude skill" CLAUDE-GLOBAL
skillmd "$T/home/.agents/skills/agentsskill/SKILL.md"     agentsskill     "Cross agent agents skill" AGENTS-GLOBAL
skillmd "$T/cwd/.claude/skills/projclaudeskill/SKILL.md" projclaudeskill "Project claude skill"      CLAUDE-WALKUP
skillmd "$T/cwd/.agents/skills/projagentsskill/SKILL.md" projagentsskill "Project agents skill"      AGENTS-WALKUP
skillmd "$T/cwd/.opencode/skill/ocskill/SKILL.md"        ocskill        "Opencode config skill"     OPENCODE-CONFIG
echo "### B1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS off"
run_debug_skill "$T" b1-off 1
run_model "$T" b1-off 1 -- "$SKILL_PROMPT"
echo "### B1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS on"
run_debug_skill "$T" b1-on 1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1
run_model "$T" b1-on 1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 -- "$SKILL_PROMPT"

### ------------------------------------------------------- B2 project config flag
T=$(mktrial b2); gittree "$T/cwd"
instruct "$T/cwd/AGENTS.md" PROJECT-AGENTS-MARKER
skillmd "$T/cwd/.opencode/skill/projskill/SKILL.md"    projskill    "Project opencode skill"  PROJECT-OPENCODE-SKILL
mkdir -p "$T/cwd/.opencode/explicit"
skillmd "$T/cwd/.opencode/explicit/SKILL.md"           explicitskill "Explicit path skill"   PROJECT-EXPLICIT-SKILL
cat > "$T/cwd/.opencode/opencode.json" <<EOF
{ "\$schema": "https://opencode.ai/config.json", "skills": { "paths": ["./.opencode/explicit"] } }
EOF
echo "### B2 OPENCODE_DISABLE_PROJECT_CONFIG off"
run_debug_skill "$T" b2-off 1
run_debug_config "$T" b2-off 1
run_model "$T" b2-off 1 -- "$INSTR_PROMPT"
run_model "$T" b2-off 2 -- "Use the projskill skill and the explicitskill skill and report their base directories."
echo "### B2 OPENCODE_DISABLE_PROJECT_CONFIG on"
run_debug_skill "$T" b2-on 1 OPENCODE_DISABLE_PROJECT_CONFIG=1
run_debug_config "$T" b2-on 1 OPENCODE_DISABLE_PROJECT_CONFIG=1
run_model "$T" b2-on 1 OPENCODE_DISABLE_PROJECT_CONFIG=1 -- "$INSTR_PROMPT"
run_model "$T" b2-on 2 OPENCODE_DISABLE_PROJECT_CONFIG=1 -- "Use the projskill skill and the explicitskill skill and report their base directories."

### ------------------------------------------------------------- B3 prompt flag
# global ~/.claude/CLAUDE.md only (no ${XDG_CONFIG_HOME}/opencode/AGENTS.md)
T=$(mktrial b3); gittree "$T/cwd"
instruct "$T/home/.claude/CLAUDE.md" HOME-CLAUDE-MARKER
echo "### B3 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT off (global ~/.claude/CLAUDE.md)"
run_model "$T" b3-off 1 -- "$INSTR_PROMPT"
echo "### B3 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT on"
run_model "$T" b3-on 1 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1 -- "$INSTR_PROMPT"

### -------------------------------------------------- B4 prompt flag vs project CLAUDE.md
T=$(mktrial b4); gittree "$T/cwd"
instruct "$T/cwd/CLAUDE.md" PROJECT-CLAUDE-MARKER
echo "### B4 project-level CLAUDE.md, flag off"
run_model "$T" b4-off 1 -- "$INSTR_PROMPT"
echo "### B4 project-level CLAUDE.md, OPENCODE_DISABLE_CLAUDE_CODE_PROMPT on"
run_model "$T" b4-on 1 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1 -- "$INSTR_PROMPT"

echo "TASK-B DONE"
