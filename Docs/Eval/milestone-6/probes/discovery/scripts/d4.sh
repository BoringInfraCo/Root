#!/bin/sh
# M6 DISCOVERY - round 2:
#  (1) A-repeat : larger model-free sample of the duplicate-skill race (A1/A2/A3)
#  (2) m5       : AGENTS.md at two levels (instance dir INSIDE the git worktree)
#  (3) m6       : global XDG opencode/AGENTS.md AND ~/.claude/CLAUDE.md together
#                 (global first-match-wins check)
#  (4) m7       : which flag actually disables the .agents skill dirs
set -e
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
. "$D/lib.sh"
R=/Users/sergio/Documents/Developer/BoringInfraCo/Root/Docs/Eval/milestone-6/probes/discovery
OUT="$D/out/b3"
rm -rf "$OUT"; mkdir -p "$OUT"
mkdir -p "$R/task-b3"

gittree() {
  ( cd "$1" && git init -q . && git config user.email p@p && git config user.name p && \
    echo x > README && git add -A && git commit -qm init )
}
magic() { mkdir -p "$(dirname "$1")"; printf '# Rules\n\nThe magic passphrase is %s.\n' "$2" > "$1"; }

run_model() { # <workdir> <label> <idx> <trial> <env...> -- <prompt>
  WD="$1"; LABEL="$2"; I="$3"; T="$4"; shift 4
  ENVS=""; PROMPT=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --) shift; PROMPT="$*"; break ;;
      *) ENVS="$ENVS $1"; shift ;;
    esac
  done
  RUNDIR="$OUT/$LABEL/run-$I"; mkdir -p "$RUNDIR"
  ( cd "$WD" && $(trienv "$T" $ENVS) "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT" ) \
      > "$RUNDIR/out.ndjson" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/out.ndjson" "$R/task-b3/$LABEL.model.run-$I.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-b3/$LABEL.model.run-$I.err.txt"
}

MAGIC_PROMPT="Do not use any tool. If your instructions contain a magic passphrase, reply with that passphrase and nothing else. If your instructions contain no magic passphrase, reply with exactly NONE."
PATH_PROMPT="List every line in your instructions that starts with 'Instructions from:'. Output each such line verbatim on its own line. If there are none, output exactly NONE."

echo "############ (1) duplicate-skill race, model-free sample"
N=${N:-30}
race() { # <label> <setup-fn>
  LABEL="$1"; SETUP="$2"
  T=$(mktrial "race-$LABEL"); $SETUP "$T"
  i=1
  while [ "$i" -le "$N" ]; do
    ( cd "$T/cwd" && $(trienv "$T") "$BIN" debug skill ) > "$OUT/$LABEL.$i.json" 2>/dev/null || true
    i=$((i+1))
  done
  cp "$OUT/$LABEL."*.json "$R/task-b3/" 2>/dev/null || true
  echo "--- $LABEL ---"
  python3 - "$OUT" "$LABEL" "$T" <<'PY'
import json,sys,glob,re,collections
out,label,trial=sys.argv[1],sys.argv[2],sys.argv[3]
tal=collections.Counter()
for f in sorted(glob.glob(f"{out}/{label}.*.json")):
    try: d=json.load(open(f))
    except Exception: tal["PARSE-ERROR"]+=1; continue
    hit=[s for s in d if s.get("name")=="dup"]
    if not hit: tal["NO-DUP-SKILL"]+=1; continue
    s=hit[0]
    m=re.search(r"MARKER: (\S+)", s.get("content") or "")
    tal[m.group(1) if m else "?"]+=1
for k,v in tal.most_common(): print(f"  {k}: {v}")
PY
}

setup_a1() { gittree "$1/cwd"; skillmd "$1/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-SKILL; skillmd "$1/config/opencode/skills/dup/SKILL.md" dup "Duplicate test skill" XDG-CONFIG-SKILLS; }
setup_a2() { gittree "$1/cwd"; skillmd "$1/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-SKILL; skillmd "$1/home/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-CLAUDE-SKILLS; }
setup_a3() { mkdir -p "$1/worktree/app"; gittree "$1/worktree"; skillmd "$1/worktree/app/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" NEAREST-DOT-OPENCODE; skillmd "$1/worktree/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" FURTHEST-DOT-OPENCODE; }

race a1 setup_a1
race a2 setup_a2
# A3 runs from the nested instance dir
T=$(mktrial race-a3); setup_a3 "$T"
i=1; while [ "$i" -le "$N" ]; do
  ( cd "$T/worktree/app" && $(trienv "$T") "$BIN" debug skill ) > "$OUT/a3.$i.json" 2>/dev/null || true
  i=$((i+1))
done
cp "$OUT/a3."*.json "$R/task-b3/" 2>/dev/null || true
echo "--- a3 (from worktree/app) ---"
python3 - "$OUT" <<'PY'
import json,sys,glob,re,collections
tal=collections.Counter()
for f in sorted(glob.glob(f"{sys.argv[1]}/a3.*.json")):
    try: d=json.load(open(f))
    except Exception: tal["PARSE-ERROR"]+=1; continue
    hit=[s for s in d if s.get("name")=="dup"]
    if not hit: tal["NO-DUP-SKILL"]+=1; continue
    m=re.search(r"MARKER: (\S+)", hit[0].get("content") or "")
    tal[m.group(1) if m else "?"]+=1
for k,v in tal.most_common(): print(f"  {k}: {v}")
PY

echo
echo "############ (2) m5 AGENTS.md at two levels (instance dir = worktree/app)"
T=$(mktrial m5b); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
magic "$T/worktree/AGENTS.md"     MAGIC-FAR-AGENTS-7755
magic "$T/worktree/app/AGENTS.md" MAGIC-NEAR-AGENTS-8866
run_model "$T/worktree/app" m5b-none 1 "$T" -- "$MAGIC_PROMPT"
run_model "$T/worktree/app" m5b-paths 1 "$T" -- "$PATH_PROMPT"
run_model "$T/worktree/app" m5b-projectcfg 1 "$T" OPENCODE_DISABLE_PROJECT_CONFIG=1 -- "$MAGIC_PROMPT"

echo
echo "############ (3) m6 global XDG AGENTS.md + ~/.claude/CLAUDE.md together"
T=$(mktrial m6); gittree "$T/cwd"
magic "$T/config/opencode/AGENTS.md"    MAGIC-XDG-AGENTS-6644
magic "$T/home/.claude/CLAUDE.md"       MAGIC-GLOBAL-CLAUDE-4422
run_model "$T/cwd" m6-none 1 "$T" -- "$MAGIC_PROMPT"
run_model "$T/cwd" m6-paths 1 "$T" -- "$PATH_PROMPT"

echo
echo "############ (4) m7 which flag disables the .agents skill dirs"
T=$(mktrial m7); gittree "$T/cwd"
skillmd "$T/home/.claude/skills/claudeskill/SKILL.md" claudeskill "Claude global" CLAUDE-GLOBAL
skillmd "$T/home/.agents/skills/agentsskill/SKILL.md" agentsskill "Agents global" AGENTS-GLOBAL
skillmd "$T/cwd/.claude/skills/projclaudeskill/SKILL.md" projclaudeskill "Claude walkup" CLAUDE-WALKUP
skillmd "$T/cwd/.agents/skills/projagentsskill/SKILL.md" projagentsskill "Agents walkup" AGENTS-WALKUP
skillmd "$T/cwd/.opencode/skill/ocskill/SKILL.md" ocskill "Opencode config" OPENCODE-CONFIG
for FLAGS in "NONE" "OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1" "OPENCODE_DISABLE_EXTERNAL_SKILLS=1" "OPENCODE_DISABLE_CLAUDE_CODE=1" "OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 OPENCODE_DISABLE_EXTERNAL_SKILLS=1"; do
  SAFE=$(echo "$FLAGS" | tr ' =' '__')
  ( cd "$T/cwd" && $(trienv "$T" $FLAGS) "$BIN" debug skill ) > "$OUT/m7.$SAFE.json" 2>/dev/null || true
  cp "$OUT/m7.$SAFE.json" "$R/task-b3/m7.$SAFE.json"
  echo "--- flags: $FLAGS ---"
  python3 - "$OUT/m7.$SAFE.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
print("   skills:", sorted(s.get("name") for s in d))
PY
done

echo "ROUND2 DONE"
