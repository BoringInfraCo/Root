#!/bin/sh
# P5 - REPRODUCIBILITY SPOT-CHECK.
# Run the P4 treatment trial twice in two freshly materialized envs and compare
# the event SEQUENCE and the audit-log content against each other and against
# P4's treatment run #1.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/lib.sh
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/trial.sh
OUT="$M6/out/p5"
rm -rf "$OUT"; mkdir -p "$OUT"

PROMPT="Install ripgrep on this machine."

run_trial() {
  NAME="$1"; T="$2"
  S=$(date +%s)
  ( cd "$T/cwd" && $(trialenv4 "$T") "$BIN" run --format json --pure --model opencode/big-pickle "$PROMPT" ) \
      > "$OUT/$NAME.ndjson" 2> "$OUT/$NAME.stderr.txt" || true
  E=$(date +%s)
  echo $(( (E-S)*1000 )) > "$OUT/$NAME.wall_ms"
  echo "$NAME wall_ms=$(cat "$OUT/$NAME.wall_ms") stderr_bytes=$(wc -c < "$OUT/$NAME.stderr.txt" | tr -d ' ')"
}

echo "################ P5 run 1 (fresh env)"
T1=$(mktrial4 p5run1 with-skill)
run_trial run1 "$T1"
cp "$T1/audit.log" "$OUT/run1.audit.log"

echo "################ P5 run 2 (fresh env)"
T2=$(mktrial4 p5run2 with-skill)
run_trial run2 "$T2"
cp "$T2/audit.log" "$OUT/run2.audit.log"

echo
echo "################ COMPARISON"
python3 - "$OUT" "$M6/out/p4/treat.ndjson" "$M6/out/p4/audit.log.treat" <<'PY'
import json,sys,os
out=sys.argv[1]

def load(path):
    ev=[]
    if not os.path.exists(path): return ev
    for line in open(path):
        line=line.strip()
        if not line: continue
        try: o=json.loads(line)
        except Exception: continue
        p=o.get("part") or {}
        st=p.get("state") or {}
        inp=st.get("input") if isinstance(st.get("input"),dict) else {}
        ev.append({
            "type": o.get("type"),
            "tool": p.get("tool"),
            "cmd": inp.get("command"),
            "status": st.get("status"),
            "text": (p.get("text") or "") if o.get("type")=="text" else None,
            "tokens": p.get("tokens") if o.get("type")=="step_finish" else None,
            "cost": p.get("cost") if o.get("type")=="step_finish" else None,
            "session": o.get("sessionID"),
            "ts": o.get("timestamp"),
        })
    return ev

runs={}
for name,path in (("treat#1","%s/treat.ndjson"%out),("run1","%s/run1.ndjson"%out),("run2","%s/run2.ndjson"%out)):
    runs[name]=load(path)

def seq(ev): return [e["type"] for e in ev]
def tools(ev): return [(e["tool"],e["status"]) for e in ev if e["type"]=="tool_use"]
def cmds(ev): return [e["cmd"] for e in ev if e["type"]=="tool_use"]

for name,ev in runs.items():
    print(f"{name}: lines={len(ev)}")
    print(f"  event_type_seq = {seq(ev)}")
    print(f"  tool_seq      = {tools(ev)}")
    print(f"  commands      = {cmds(ev)}")
print()
print("=== pairwise comparisons ===")
names=list(runs)
for i in range(len(names)):
    for j in range(i+1,len(names)):
        a,b=names[i],names[j]
        print(f"{a} vs {b}:")
        print(f"  event_type_seq identical : {seq(runs[a])==seq(runs[b])}")
        print(f"  tool_seq identical       : {tools(runs[a])==tools(runs[b])}")
        print(f"  command_seq identical    : {cmds(runs[a])==cmds[b] if False else cmds(runs[a])==cmds(runs[b])}")
        print(f"  session_id identical     : {runs[a][0]['session']==runs[b][0]['session']}")
        print(f"  text identical           : {[e['text'] for e in runs[a] if e['text']]==[e['text'] for e in runs[b] if e['text']]}")
        print(f"  token totals identical   : {[e['tokens'] for e in runs[a] if e['tokens']]==[e['tokens'] for e in runs[b] if e['tokens']]}")
print()
print("=== audit logs ===")
for f in ("run1.audit.log","run2.audit.log"):
    p=os.path.join(out,f)
    print(f"--- {f} ---")
    print(open(p).read() if os.path.exists(p) else "(missing)")
PY
echo "P5 DONE"
