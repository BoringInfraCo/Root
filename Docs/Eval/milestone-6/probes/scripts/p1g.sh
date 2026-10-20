#!/bin/sh
# P1 (part 6b) - git-bounded near/far .opencode and .claude/.agents walk-up cases.
# (The p1f.sh tally helper always cd'd into $T/cwd; these layouts put the
#  instance dir at $T/worktree/app, so they are re-run here with the correct cwd.)
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p1f"
mkdir -p "$OUT"
N=${N:-10}

tally_wd() {
  T="$1"; WD="$2"; LABEL="$3"
  i=1
  while [ "$i" -le "$N" ]; do
    ( cd "$WD" && $(trienv "$T") "$BIN" debug skill > "$OUT/$LABEL-$i.json" 2>"$OUT/$LABEL-$i.err" ) || true
    python3 - "$OUT/$LABEL-$i.json" "$T" <<'PY'
import json,sys,re
raw=open(sys.argv[1]).read(); trial=sys.argv[2]
try: data=json.loads(raw)
except Exception as e:
    print(f"PARSE-ERROR {e}"); raise SystemExit
hits=[]
for s in data:
    if s.get("name")=="dup":
        loc=(s.get("location") or "").replace(trial,"")
        m=re.search(r"MARKER: (\S+)", s.get("content") or "")
        hits.append((m.group(1) if m else "?", loc))
print(" | ".join(f"{a} {b}" for a,b in hits) if hits else "NO-DUP-SKILL")
PY
    i=$((i+1))
  done
}

gittree() {
  ( cd "$1" && git init -q . && git config user.email p@p && git config user.name p && \
    echo x > README && git add -A && git commit -qm init )
}

### L03 near .opencode (app/) vs far .opencode (worktree root)   [git-bounded]
T=$(mktrial L03); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" NEAREST-DOT-OPENCODE
skillmd "$T/worktree/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" FURTHEST-DOT-OPENCODE
echo "### L03 near .opencode vs far .opencode (git root)  [instance=$T/worktree/app]"
tally_wd "$T" "$T/worktree/app" L03

### L09 near walk-up .agents vs far walk-up .agents (git root)
T=$(mktrial L09); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-NEAR
skillmd "$T/worktree/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-FAR
echo "### L09 near .agents/skills vs far .agents/skills"
tally_wd "$T" "$T/worktree/app" L09

### L10 near walk-up .claude vs far walk-up .claude (git root)
T=$(mktrial L10); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" CLAUDE-NEAR
skillmd "$T/worktree/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" CLAUDE-FAR
echo "### L10 near .claude/skills vs far .claude/skills"
tally_wd "$T" "$T/worktree/app" L10

### L16 CONTROL: dup ONLY at the far .opencode (git root) - is the outer dir in span?
T=$(mktrial L16); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" FURTHEST-DOT-OPENCODE
echo "### L16 CONTROL only far .opencode present"
tally_wd "$T" "$T/worktree/app" L16

### L17 CONTROL: dup ONLY above the git root -> must NOT be discovered
T=$(mktrial L17); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" ABOVE-GIT-ROOT
echo "### L17 CONTROL only .opencode ABOVE the git root"
tally_wd "$T" "$T/worktree/app" L17

### L18 CONTROL: dup ONLY at near .opencode (inside git root)
T=$(mktrial L18); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" NEAREST-DOT-OPENCODE
echo "### L18 CONTROL only near .opencode present"
tally_wd "$T" "$T/worktree/app" L18

### L19 CONTROL: no git repo at all, dup only at the outer dir
T=$(mktrial L19); mkdir -p "$T/worktree/app"
skillmd "$T/worktree/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" OUTER-NO-GIT
echo "### L19 CONTROL no git repo, only outer .opencode"
tally_wd "$T" "$T/worktree/app" L19

echo "P1F-B DONE"
