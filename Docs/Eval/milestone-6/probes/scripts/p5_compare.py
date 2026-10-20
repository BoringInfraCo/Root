#!/usr/bin/env python3
"""P5 reproducibility comparison across the three treatment runs."""
import json
import os

M6 = "/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe"
RUNS = [
    ("treat#1 (P4)", f"{M6}/out/p4/treat.ndjson", f"{M6}/out/p4/audit.log.treat"),
    ("run1 (P5)", f"{M6}/out/p5/run1.ndjson", f"{M6}/out/p5/run1.audit.log"),
    ("run2 (P5)", f"{M6}/out/p5/run2.ndjson", f"{M6}/out/p5/run2.audit.log"),
]


def load(path):
    ev = []
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        o = json.loads(line)
        p = o.get("part") or {}
        st = p.get("state") or {}
        inp = st.get("input") if isinstance(st.get("input"), dict) else {}
        ev.append({
            "type": o.get("type"),
            "tool": p.get("tool"),
            "cmd": inp.get("command"),
            "status": st.get("status"),
            "text": p.get("text") if o.get("type") == "text" else None,
            "tokens": p.get("tokens") if o.get("type") == "step_finish" else None,
            "cost": p.get("cost") if o.get("type") == "step_finish" else None,
            "session": o.get("sessionID"),
            "ts": o.get("timestamp"),
        })
    return ev


runs = {}
for name, nd, al in RUNS:
    runs[name] = (load(nd), open(al).read() if os.path.exists(al) else "")

for name, (ev, al) in runs.items():
    print(f"### {name}")
    print(f"  lines={len(ev)}  session={ev[0]['session']}")
    print(f"  event_type_seq = {[e['type'] for e in ev]}")
    print(f"  tool_seq      = {[(e['tool'], e['status']) for e in ev if e['type'] == 'tool_use']}")
    print(f"  commands      = {[e['cmd'] for e in ev if e['type'] == 'tool_use' and e['cmd']]}")
    print(f"  text_events   = {[e['text'] for e in ev if e['type'] == 'text']}")
    print(f"  step_finish_costs = {[e['cost'] for e in ev if e['type'] == 'step_finish']}")
    print(f"  audit_log     = {al.splitlines()!r}")
    print()

names = list(runs)
print("=== pairwise ===")
for i in range(len(names)):
    for j in range(i + 1, len(names)):
        a, b = names[i], names[j]
        ea, eb = runs[a][0], runs[b][0]
        print(f"{a}  vs  {b}")
        print(f"  event_type_seq identical : {[e['type'] for e in ea] == [e['type'] for e in eb]}")
        print(f"  tool_seq identical       : {[(e['tool'], e['status']) for e in ea if e['type']=='tool_use'] == [(e['tool'], e['status']) for e in eb if e['type']=='tool_use']}")
        print(f"  command_seq identical    : {[e['cmd'] for e in ea if e['type']=='tool_use'] == [e['cmd'] for e in eb if e['type']=='tool_use']}")
        print(f"  audit_log identical      : {runs[a][1] == runs[b][1]}")
        print(f"  session_id identical     : {ea[0]['session'] == eb[0]['session']}")
        print(f"  text identical           : {[e['text'] for e in ea if e['text']] == [e['text'] for e in eb if e['text']]}")
        print(f"  tokens identical         : {[e['tokens'] for e in ea if e['tokens']] == [e['tokens'] for e in eb if e['tokens']]}")
        print(f"  has_skill_event          : {any(e['tool']=='skill' for e in ea)} / {any(e['tool']=='skill' for e in eb)}")
        print(f"  ends_with_text           : {ea[-1]['type']=='text'} / {eb[-1]['type']=='text'}")
        print(f"  last_event_type          : {ea[-1]['type']} / {eb[-1]['type']}")
        print()

print("=== protocol sequence present? (root doctor/plan/install/verify in order) ===")
for name, (ev, al) in runs.items():
    cmds = [e["cmd"] for e in ev if e["type"] == "tool_use" and e["cmd"]]
    order = []
    for c in cmds:
        if "root doctor" in c:
            order.append("doctor")
        elif "root plan" in c:
            order.append("plan")
        elif "root install" in c:
            order.append("install")
        elif "root verify" in c:
            order.append("verify")
    print(f"  {name}: {order}")
