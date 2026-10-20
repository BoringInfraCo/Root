#!/bin/sh
# M6 DISCOVERY - TASK B2: instruction-attachment matrix.
#
# Method: content-based. Each planted instruction file contains a unique magic
# passphrase. The model is asked for the passphrase. A passphrase can only be
# produced if the file was actually injected into the system prompt, so this
# is immune to path hallucination (a control run with no instruction files
# must answer NONE).
#
# Trials (one instruction file each, so exactly one attachment can occur):
#   m1 project AGENTS.md          (cwd/AGENTS.md)
#   m2 global ~/.claude/CLAUDE.md
#   m3 project CLAUDE.md          (cwd/CLAUDE.md)
#   m4 global ${XDG_CONFIG_HOME}/opencode/AGENTS.md
#   m5 AGENTS.md at TWO levels    (worktree/AGENTS.md + worktree/app/AGENTS.md)
# Conditions: no flag / DISABLE_CLAUDE_CODE_SKILLS / DISABLE_PROJECT_CONFIG /
#             DISABLE_CLAUDE_CODE_PROMPT / DISABLE_CLAUDE_CODE (broad)
set -e
D=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/discovery
. "$D/lib.sh"
R=/Users/sergio/Documents/Developer/BoringInfraCo/Root/Docs/Eval/milestone-6/probes/discovery
OUT="$D/out/b2"
rm -rf "$OUT"; mkdir -p "$OUT"
mkdir -p "$R/task-b2"

gittree() {
  ( cd "$1" && git init -q . && git config user.email p@p && git config user.name p && \
    echo x > README && git add -A && git commit -qm init )
}

magic() { # <path> <passphrase>
  mkdir -p "$(dirname "$1")"
  printf '# Rules\n\nThe magic passphrase is %s.\n' "$2" > "$1"
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
  cp "$RUNDIR/out.ndjson" "$R/task-b2/$LABEL.model.run-$I.ndjson"
  cp "$RUNDIR/err.txt"    "$R/task-b2/$LABEL.model.run-$I.err.txt"
}

MAGIC_PROMPT="Do not use any tool. If your instructions contain a magic passphrase, reply with that passphrase and nothing else. If your instructions contain no magic passphrase, reply with exactly NONE."

REPS=${REPS:-2}

matrix() { # <label> <trial>
  LABEL="$1"; T="$2"
  run_model "$T" "$LABEL-none"        1 -- "$MAGIC_PROMPT"
  run_model "$T" "$LABEL-skills"      1 OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1  -- "$MAGIC_PROMPT"
  run_model "$T" "$LABEL-projectcfg"  1 OPENCODE_DISABLE_PROJECT_CONFIG=1      -- "$MAGIC_PROMPT"
  run_model "$T" "$LABEL-prompt"      1 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1  -- "$MAGIC_PROMPT"
  run_model "$T" "$LABEL-broad"       1 OPENCODE_DISABLE_CLAUDE_CODE=1          -- "$MAGIC_PROMPT"
  if [ "$REPS" -gt 1 ]; then
    run_model "$T" "$LABEL-none"        2 -- "$MAGIC_PROMPT"
    run_model "$T" "$LABEL-projectcfg"  2 OPENCODE_DISABLE_PROJECT_CONFIG=1      -- "$MAGIC_PROMPT"
    run_model "$T" "$LABEL-prompt"      2 OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1  -- "$MAGIC_PROMPT"
  fi
}

### m1 project AGENTS.md
T=$(mktrial m1); gittree "$T/cwd"
magic "$T/cwd/AGENTS.md" MAGIC-PROJECT-AGENTS-3311
echo "### m1 project AGENTS.md"
matrix m1 "$T"

### m2 global ~/.claude/CLAUDE.md
T=$(mktrial m2); gittree "$T/cwd"
magic "$T/home/.claude/CLAUDE.md" MAGIC-GLOBAL-CLAUDE-4422
echo "### m2 global ~/.claude/CLAUDE.md"
matrix m2 "$T"

### m3 project CLAUDE.md
T=$(mktrial m3); gittree "$T/cwd"
magic "$T/cwd/CLAUDE.md" MAGIC-PROJECT-CLAUDE-5533
echo "### m3 project CLAUDE.md"
matrix m3 "$T"

### m4 global ${XDG_CONFIG_HOME}/opencode/AGENTS.md
T=$(mktrial m4); gittree "$T/cwd"
magic "$T/config/opencode/AGENTS.md" MAGIC-XDG-AGENTS-6644
echo "### m4 global XDG opencode/AGENTS.md"
matrix m4 "$T"

### m5 AGENTS.md at two levels (walk-up)
T=$(mktrial m5); mkdir -p "$T/worktree/app"; gittree "$T/worktree"
magic "$T/worktree/AGENTS.md"     MAGIC-FAR-AGENTS-7755
magic "$T/worktree/app/AGENTS.md" MAGIC-NEAR-AGENTS-8866
echo "### m5 AGENTS.md at two levels"
run_model "$T" m5-none       1 -- "$MAGIC_PROMPT"
run_model "$T" m5-projectcfg 1 OPENCODE_DISABLE_PROJECT_CONFIG=1 -- "$MAGIC_PROMPT"
# path listing for the two-level case (does the binary attach BOTH levels?)
run_model "$T" m5-paths      1 -- "List every line in your instructions that starts with 'Instructions from:'. Output each such line verbatim on its own line. If there are none, output exactly NONE."

echo "TASK-B2 DONE"
