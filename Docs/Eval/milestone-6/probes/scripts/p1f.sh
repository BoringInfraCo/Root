#!/bin/sh
# P1 (part 6) - MODEL-FREE repeatability + total-order reconstruction.
# `opencode debug skill` prints the resolved skill list (name + location + content)
# without contacting any model. Each trial is run N times against the SAME
# layout; the winner of a duplicate name is tallied.
# Hypothesis under test (from the p1e model runs): duplicate-name resolution is
# RACY, not a deterministic total order.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p1f"
rm -rf "$OUT"; mkdir -p "$OUT"

N=${N:-10}

# tally <trialdir> <label> : run `debug skill` N times, print "MARKER location-suffix" per run
tally() {
  T="$1"; LABEL="$2"
  i=1
  while [ "$i" -le "$N" ]; do
    ( cd "$T/cwd" && $(trienv "$T") "$BIN" debug skill > "$OUT/$LABEL-$i.json" 2>"$OUT/$LABEL-$i.err" ) || true
    python3 - "$OUT/$LABEL-$i.json" "$T" <<'PY'
import json,sys,re
raw=open(sys.argv[1]).read()
trial=sys.argv[2]
try:
    data=json.loads(raw)
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

mk() { mktrial "$1"; }

### L01 project .opencode/skill/dup  vs  XDG config opencode/skills/dup
T=$(mk L01)
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-SKILL
skillmd "$T/config/opencode/skills/dup/SKILL.md" dup "Duplicate test skill" XDG-CONFIG-SKILLS
echo "### L01 project .opencode/skill vs XDG opencode/skills"
tally "$T" L01

### L02 project .opencode/skill/dup  vs  global ~/.claude/skills/dup
T=$(mk L02)
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-SKILL
skillmd "$T/home/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-CLAUDE-SKILLS
echo "### L02 project .opencode/skill vs ~/.claude/skills"
tally "$T" L02

### L03 near .opencode (app/) vs far .opencode (worktree root)   [git-bounded]
T=$(mk L03); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" NEAREST-DOT-OPENCODE
skillmd "$T/worktree/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" FURTHEST-DOT-OPENCODE
echo "### L03 near .opencode vs far .opencode (git root)"
tally "$T" L03

### L04 walk-up .claude/skills (cwd) vs global ~/.claude/skills
T=$(mk L04)
skillmd "$T/cwd/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" PROJECT-CLAUDE-WALKUP
skillmd "$T/home/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-CLAUDE-GLOBAL
echo "### L04 walk-up .claude/skills vs ~/.claude/skills"
tally "$T" L04

### L05 global ~/.claude/skills vs global ~/.agents/skills
T=$(mk L05)
skillmd "$T/home/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-CLAUDE-GLOBAL
skillmd "$T/home/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-AGENTS-GLOBAL
echo "### L05 ~/.claude/skills vs ~/.agents/skills"
tally "$T" L05

### L06 walk-up .claude/skills vs global ~/.agents/skills
T=$(mk L06)
skillmd "$T/cwd/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" WALKUP-CLAUDE
skillmd "$T/home/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-AGENTS-GLOBAL
echo "### L06 walk-up .claude/skills vs ~/.agents/skills"
tally "$T" L06

### L07 walk-up .agents/skills vs global ~/.claude/skills
T=$(mk L07)
skillmd "$T/cwd/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" WALKUP-AGENTS
skillmd "$T/home/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-CLAUDE-GLOBAL
echo "### L07 walk-up .agents/skills vs ~/.claude/skills"
tally "$T" L07

### L08 walk-up .agents/skills vs walk-up .claude/skills (same level)
T=$(mk L08)
skillmd "$T/cwd/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" WALKUP-AGENTS
skillmd "$T/cwd/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" WALKUP-CLAUDE
echo "### L08 walk-up .agents/skills vs walk-up .claude/skills"
tally "$T" L08

### L09 near walk-up .agents vs far walk-up .agents (git root)
T=$(mk L09); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-NEAR
skillmd "$T/worktree/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-FAR
echo "### L09 near .agents/skills vs far .agents/skills"
tally "$T" L09

### L10 near walk-up .claude vs far walk-up .claude (git root)
T=$(mk L10); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" CLAUDE-NEAR
skillmd "$T/worktree/.claude/skills/dup/SKILL.md" dup "Duplicate test skill" CLAUDE-FAR
echo "### L10 near .claude/skills vs far .claude/skills"
tally "$T" L10

### L11 walk-up .agents/skills vs global ~/.agents/skills
T=$(mk L11)
skillmd "$T/cwd/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-WALKUP
skillmd "$T/home/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-GLOBAL
echo "### L11 walk-up .agents/skills vs ~/.agents/skills"
tally "$T" L11

### L12 project .opencode/skill vs global ~/.agents/skills
T=$(mk L12)
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-OPENCODE-SKILL
skillmd "$T/home/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-GLOBAL
echo "### L12 project .opencode/skill vs ~/.agents/skills"
tally "$T" L12

### L13 XDG config opencode/skills vs global ~/.agents/skills
T=$(mk L13)
skillmd "$T/config/opencode/skills/dup/SKILL.md" dup "Duplicate test skill" XDG-CONFIG-SKILLS
skillmd "$T/home/.agents/skills/dup/SKILL.md" dup "Duplicate test skill" AGENTS-GLOBAL
echo "### L13 XDG opencode/skills vs ~/.agents/skills"
tally "$T" L13

### L14 project .opencode/skill vs global ~/.opencode/skills
T=$(mk L14)
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" PROJECT-OPENCODE-SKILL
skillmd "$T/home/.opencode/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-OPENCODE-SKILLS
echo "### L14 project .opencode/skill vs ~/.opencode/skills"
tally "$T" L14

### L15 XDG config opencode/skills vs global ~/.opencode/skills
T=$(mk L15)
skillmd "$T/config/opencode/skills/dup/SKILL.md" dup "Duplicate test skill" XDG-CONFIG-SKILLS
skillmd "$T/home/.opencode/skills/dup/SKILL.md" dup "Duplicate test skill" HOME-OPENCODE-SKILLS
echo "### L15 XDG opencode/skills vs ~/.opencode/skills"
tally "$T" L15

echo "P1F DONE"
