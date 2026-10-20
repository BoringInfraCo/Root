#!/bin/sh
# M6 DISCOVERY - TASK A: skill scan order (duplicate frontmatter name `dup`).
# For each case two SKILL.md files share the frontmatter name `dup`; we record
# which file wins (opencode's native `skill` tool metadata.dir + model-free
# `opencode debug skill` location).
#
# Predictions come from crates/root-eval/src/context.rs:
#   skill_scan_roots() order (later scan overrides earlier by name):
#     1 ~/.claude/skills
#     2 ~/.agents/skills
#     3 <level>/.claude/skills then <level>/.agents/skills   (level chain: instance dir -> worktree root, INNERMOST FIRST)
#     4 config dirs: ${XDG_CONFIG_HOME}/opencode, then each <level>/.opencode (innermost first),
#       then ~/.opencode, then OPENCODE_CONFIG_DIR; per config dir: <dir>/skill/** then <dir>/skills/**
#     5 explicit skills.paths
set -e
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
. "$D/lib.sh"
R=/Users/sergio/Documents/Developer/BoringInfraCo/Root/Docs/Eval/milestone-6/probes/discovery
OUT="$D/out/a"
rm -rf "$OUT"; mkdir -p "$OUT"
mkdir -p "$R/task-a"

NMODEL=${NMODEL:-6}       # model-driven repeats (skill tool call)
NDEBUG=${NDEBUG:-10}      # model-free repeats (opencode debug skill)

gittree() {
  ( cd "$1" && git init -q . && git config user.email p@p && git config user.name p && \
    echo x > README && git add -A && git commit -qm init )
}

# ---- model-driven run: force the native skill tool, capture metadata.dir ----
model_run() {  # <trial> <label> <runidx> <prompt>
  T="$1"; LABEL="$2"; I="$3"; PROMPT="$4"
  RUNDIR="$OUT/$LABEL/run-$I"; mkdir -p "$RUNDIR"
  ( cd "$T/cwd" && $(trienv "$T") "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT" ) \
      > "$RUNDIR/out.ndjson" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/out.ndjson" "$R/task-a/$LABEL.model.run-$I.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-a/$LABEL.model.run-$I.err.txt"
}

# ---- model-free run: opencode debug skill ----
debug_run() { # <trial> <label> <runidx>
  T="$1"; LABEL="$2"; I="$3"
  RUNDIR="$OUT/$LABEL/debug-$I"; mkdir -p "$RUNDIR"
  ( cd "$T/cwd" && $(trienv "$T") "$BIN" debug skill ) > "$RUNDIR/skills.json" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/skills.json" "$R/task-a/$LABEL.debug.run-$I.json"
}

PROMPT="Use the dup skill and tell me its base directory"

### ------------------------------------------------------------------ A1
# project .opencode/skill/dup  vs  ${XDG_CONFIG_HOME}/opencode/skills/dup
# resolver: config dirs = [xdg/opencode, <chain>/.opencode ...]; within a config
# dir `skill` is scanned before `skills`; the project .opencode dir is scanned
# AFTER xdg/opencode -> project wins.
T=$(mktrial a1); gittree "$T/cwd"
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md"       dup "Duplicate test skill" PROJECT-SKILL
skillmd "$T/config/opencode/skills/dup/SKILL.md"     dup "Duplicate test skill" XDG-CONFIG-SKILLS
echo "### A1 project .opencode/skill/dup  vs  XDG opencode/skills/dup"
i=1; while [ "$i" -le "$NMODEL" ]; do model_run "$T" a1 "$i" "$PROMPT"; i=$((i+1)); done
i=1; while [ "$i" -le "$NDEBUG" ]; do debug_run "$T" a1 "$i"; i=$((i+1)); done

### ------------------------------------------------------------------ A2
# project .opencode/skill/dup  vs  ~/.claude/skills/dup
# resolver: ~/.claude/skills is scan root #1, project .opencode/skill is a
# config-dir root scanned later -> project wins.
T=$(mktrial a2); gittree "$T/cwd"
skillmd "$T/cwd/.opencode/skill/dup/SKILL.md"   dup "Duplicate test skill" PROJECT-SKILL
skillmd "$T/home/.claude/skills/dup/SKILL.md"   dup "Duplicate test skill" HOME-CLAUDE-SKILLS
echo "### A2 project .opencode/skill/dup  vs  ~/.claude/skills/dup"
i=1; while [ "$i" -le "$NMODEL" ]; do model_run "$T" a2 "$i" "$PROMPT"; i=$((i+1)); done
i=1; while [ "$i" -le "$NDEBUG" ]; do debug_run "$T" a2 "$i"; i=$((i+1)); done

### ------------------------------------------------------------------ A3
# two .opencode dirs at different depths (git-bounded worktree)
# resolver: chain is instance-dir -> worktree-root (innermost first), so the
# OUTERMOST .opencode is scanned last -> outermost wins.
T=$(mktrial a3); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
skillmd "$T/worktree/app/.opencode/skill/dup/SKILL.md" dup "Duplicate test skill" NEAREST-DOT-OPENCODE
skillmd "$T/worktree/.opencode/skill/dup/SKILL.md"     dup "Duplicate test skill" FURTHEST-DOT-OPENCODE
echo "### A3 near .opencode/skill/dup  vs  far .opencode/skill/dup (git root)"
# instance dir must be worktree/app: run from there
i=1; while [ "$i" -le "$NMODEL" ]; do
  RUNDIR="$OUT/a3/run-$i"; mkdir -p "$RUNDIR"
  ( cd "$T/worktree/app" && $(trienv "$T") "$BIN" run --format json --model opencode/big-pickle --pure "$PROMPT" ) \
      > "$RUNDIR/out.ndjson" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/out.ndjson" "$R/task-a/a3.model.run-$i.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-a/a3.model.run-$i.err.txt"
  i=$((i+1))
done
i=1; while [ "$i" -le "$NDEBUG" ]; do
  RUNDIR="$OUT/a3/debug-$i"; mkdir -p "$RUNDIR"
  ( cd "$T/worktree/app" && $(trienv "$T") "$BIN" debug skill ) > "$RUNDIR/skills.json" 2> "$RUNDIR/err.txt" || true
  cp "$RUNDIR/skills.json" "$R/task-a/a3.debug.run-$i.json"
  i=$((i+1))
done

echo "TASK-A DONE"
