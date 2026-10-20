#!/bin/sh
# M6 DISCOVERY - TASK A case 4 (discriminator).
#
# Layout (git worktree root = worktree, instance dir = worktree/app):
#   worktree/app/.agents/skills/dup/SKILL.md   NEAR-AGENTS
#   worktree/.claude/skills/dup/SKILL.md       FAR-CLAUDE
#
# crates/root-eval/src/context.rs scan order is PER LEVEL:
#   app/.claude, app/.agents, worktree/.claude, worktree/.agents
#   -> last present = FAR-CLAUDE  (resolver prediction)
#
# opencode 1.18.27 scan order is PER TARGET:
#   all .claude levels (app/.claude, worktree/.claude), then all .agents levels
#   (app/.agents, worktree/.agents)
#   -> last present = NEAR-AGENTS (upstream prediction)
set -e
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
. "$D/lib.sh"
R=/Users/sergio/Documents/Developer/BoringInfraCo/Root/Docs/Eval/milestone-6/probes/discovery
OUT="$D/out/a4"
rm -rf "$OUT"; mkdir -p "$OUT"

T=$(mktrial a4); mkdir -p "$T/worktree/app"
( cd "$T/worktree" && git init -q . && git config user.email p@p && git config user.name p && \
  echo x > README && git add -A && git commit -qm init )
skillmd "$T/worktree/app/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" NEAR-AGENTS
skillmd "$T/worktree/.claude/skills/dup/SKILL.md"     dup "Duplicate test skill" FAR-CLAUDE

N=${N:-30}
i=1
while [ "$i" -le "$N" ]; do
  ( cd "$T/worktree/app" && $(trienv "$T") "$BIN" debug skill ) > "$OUT/a4.$i.json" 2>/dev/null || true
  i=$((i+1))
done
# model-driven sample too
M=${M:-4}
i=1
while [ "$i" -le "$M" ]; do
  RUNDIR="$OUT/run-$i"; mkdir -p "$RUNDIR"
  ( cd "$T/worktree/app" && $(trienv "$T") "$BIN" run --format json --model opencode/big-pickle --pure "Use the dup skill and tell me its base directory" ) \
      > "$RUNDIR/out.ndjson" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/out.ndjson" "$R/task-a/a4.model.run-$i.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-a/a4.model.run-$i.err.txt"
  i=$((i+1))
done

python3 - "$OUT" <<'PY'
import json,sys,glob,re,collections
out=sys.argv[1]
tal=collections.Counter()
for f in sorted(glob.glob(f"{out}/a4.*.json")):
    try: d=json.load(open(f))
    except Exception: tal["PARSE-ERROR"]+=1; continue
    hit=[s for s in d if s.get("name")=="dup"]
    if not hit: tal["NO-DUP-SKILL"]+=1; continue
    m=re.search(r"MARKER: (\S+)", hit[0].get("content") or "")
    tal[m.group(1) if m else "?"]+=1
print("A4 model-free tally:")
for k,v in tal.most_common(): print(f"  {k}: {v}")
tal2=collections.Counter()
for f in sorted(glob.glob(f"{out}/run-*/out.ndjson")):
    for line in open(f):
        line=line.strip()
        if not line: continue
        try: o=json.loads(line)
        except: continue
        if o.get("type")=="tool_use":
            st=o.get("part",{}).get("state",{}) or {}
            md=st.get("metadata") or {}
            out_s=st.get("output") or ""
            m=re.search(r"MARKER: (\S+)", out_s)
            tal2[(m.group(1) if m else "?", (md.get("dir") or "").split("trials/")[-1])]+=1
print("A4 model-driven tally:")
for k,v in tal2.most_common(): print(f"  {k}: {v}")
PY
echo "A4 DONE"
