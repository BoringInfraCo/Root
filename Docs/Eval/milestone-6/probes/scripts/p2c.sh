#!/bin/sh
# P2 (part 3) - INSTRUCTION-file (rules) flag semantics. These need a real model
# turn, because the resolved instruction files are injected into the system
# prompt and are not visible in the NDJSON stream.
# Each instruction file carries a unique PROBE_TOKEN=<value>; the model is asked
# to echo the token(s) it can see. A missing token => the file was not attached.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
OUT="$M6/out/p2c"
rm -rf "$OUT"; mkdir -p "$OUT"

PROMPT='Instruction probe. Look at the instruction text attached to your context. If it contains one or more lines that begin with PROBE_TOKEN=, reply with only those values, separated by commas, and nothing else. If it contains no such line, reply with exactly: NONE'

instr() {  # instr <path> <token>
  mkdir -p "$(dirname "$1")"
  printf '# Instruction probe file\n\nThis file is attached as an instruction file.\nPROBE_TOKEN=%s\n' "$2" > "$1"
}

runcase() {  # runcase <label> <trialdir> [env...]
  NAME="$1"; T="$2"; shift 2
  WD="${CWD:-$T/cwd}"
  ( cd "$WD" && $(trienv "$T") "$@" "$BIN" run --format json --pure --model opencode/big-pickle "$PROMPT" ) \
     > "$OUT/$NAME.stdout.ndjson" 2> "$OUT/$NAME.stderr.txt" || true
  python3 - "$OUT/$NAME.stdout.ndjson" "$NAME" <<'PY'
import json,sys,re
name=sys.argv[2]; texts=[]; tools=[]; errs=[]
for line in open(sys.argv[1]):
    line=line.strip()
    if not line: continue
    try: o=json.loads(line)
    except Exception: continue
    t=o.get("type")
    if t=="text": texts.append((o.get("part") or {}).get("text") or "")
    elif t=="error": errs.append(json.dumps(o.get("error"))[:200])
    elif t=="tool_use":
        p=o.get("part") or {}; tools.append(f"{p.get('tool')}:{(p.get('state') or {}).get('status')}")
ans=" ".join(texts)
toks=re.findall(r"PROBE_TOKEN[=:]\s*([A-Z0-9]+)", ans)
bare=re.findall(r"\b([A-Z]{3}\d[A-Z0-9]{3})\b", ans)
print(f"{name}: tokens={sorted(set(toks)) or '-'} bare={sorted(set(bare)) or '-'} tools={tools} errs={errs}")
print(f"   answer: {' '.join(texts).strip()[:300]!r}")
PY
}

### I1 project AGENTS.md only
T=$(mktrial I1); instr "$T/cwd/AGENTS.md" PRJ7A2F
echo "### I1 project AGENTS.md, no flags"; runcase I1 "$T"

### I2 project AGENTS.md + OPENCODE_DISABLE_PROJECT_CONFIG
echo "### I2 project AGENTS.md + OPENCODE_DISABLE_PROJECT_CONFIG"; runcase I2 "$T" OPENCODE_DISABLE_PROJECT_CONFIG=1

### I3 global ~/.claude/CLAUDE.md fallback only
T=$(mktrial I3); instr "$T/home/.claude/CLAUDE.md" CLB9C4D
echo "### I3 global ~/.claude/CLAUDE.md fallback"; runcase I3 "$T"

### I4 global ~/.claude/CLAUDE.md + OPENCODE_DISABLE_CLAUDE_CODE_PROMPT
echo "### I4 global CLAUDE.md fallback + OPENCODE_DISABLE_CLAUDE_CODE_PROMPT"; runcase I4 "$T" OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1

### I5 global XDG AGENTS.md + global ~/.claude/CLAUDE.md (first match wins?)
T=$(mktrial I5); instr "$T/config/opencode/AGENTS.md" XDG3E8B; instr "$T/home/.claude/CLAUDE.md" CLB9C4D
echo "### I5 XDG AGENTS.md + ~/.claude/CLAUDE.md"; runcase I5 "$T"

### I6 same + OPENCODE_DISABLE_CLAUDE_CODE_PROMPT
echo "### I6 XDG AGENTS.md + ~/.claude/CLAUDE.md + DISABLE_CLAUDE_CODE_PROMPT"; runcase I6 "$T" OPENCODE_DISABLE_CLAUDE_CODE_PROMPT=1

### I7 project AGENTS.md + global ~/.claude/CLAUDE.md (both levels attach?)
T=$(mktrial I7); instr "$T/cwd/AGENTS.md" PRJ7A2F; instr "$T/home/.claude/CLAUDE.md" CLB9C4D
echo "### I7 project AGENTS.md + global CLAUDE.md fallback"; runcase I7 "$T"

### I8 project CLAUDE.md only (name priority within the project level)
T=$(mktrial I8); instr "$T/cwd/CLAUDE.md" PCLA6H1
echo "### I8 project CLAUDE.md only"; runcase I8 "$T"

### I9 project AGENTS.md + project CLAUDE.md (name priority AGENTS > CLAUDE)
T=$(mktrial I9); instr "$T/cwd/AGENTS.md" PRJ7A2F; instr "$T/cwd/CLAUDE.md" PCLA6H1
echo "### I9 project AGENTS.md + project CLAUDE.md"; runcase I9 "$T"

### I10 project AGENTS.md in a SUBDIR (walk-up from a nested instance dir)
T=$(mktrial I10); instr "$T/cwd/AGENTS.md" PRJ7A2F
mkdir -p "$T/cwd/sub/deep"
echo "### I10 project AGENTS.md, instance dir = cwd/sub/deep"; CWD="$T/cwd/sub/deep" runcase I10 "$T"

echo "P2C DONE"
