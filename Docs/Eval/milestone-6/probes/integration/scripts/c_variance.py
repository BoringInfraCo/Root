#!/usr/bin/env python3
"""TASK C - variance comparison between two treatment runs.

Reports which fields VARY across runs and which are STABLE.
Usage: c_variance.py <label1>=<ndjson1>:<audit1> <label2>=<ndjson2>:<audit2> ...
"""
import json
import sys


def load(nd_path, audit_path):
    events = []
    with open(nd_path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            o = json.loads(line)
            p = o.get("part") or {}
            st = p.get("state") or {}
            inp = st.get("input") if isinstance(st.get("input"), dict) else {}
            events.append({
                "type": o.get("type"),
                "session": o.get("sessionID"),
                "ts": o.get("timestamp"),
                "tool": p.get("tool"),
                "callID": p.get("callID"),
                "status": st.get("status"),
                "cmd": inp.get("command"),
                "skill_name": inp.get("name"),
                "text": p.get("text") if o.get("type") == "text" else None,
                "tokens": p.get("tokens") if o.get("type") == "step_finish" else None,
                "cost": p.get("cost") if o.get("type") == "step_finish" else None,
                "reason": p.get("reason") if o.get("type") == "step_finish" else None,
                "metadata": st.get("metadata") if o.get("type") == "tool_use" else None,
                "error": st.get("error") if o.get("type") == "tool_use" else None,
            })
    audit = []
    try:
        with open(audit_path) as fh:
            audit = [l.rstrip("\n") for l in fh if l.strip()]
    except FileNotFoundError:
        pass
    return events, audit


# The Root protocol, in order, as the skill prescribes it.
PROTOCOL = ["doctor", "plan", "install", "verify"]


def protocol_order(audit):
    """Extract the Root protocol step order from an audit log."""
    order = []
    for line in audit:
        parts = line.split()
        if len(parts) < 2 or parts[0] != "root":
            continue
        cmd = parts[1]
        if cmd in PROTOCOL and cmd not in order:
            order.append(cmd)
    return order


def normalize(text):
    """Strip trial-specific absolute paths so runs can be compared."""
    if text is None:
        return None
    out = []
    for chunk in str(text).split():
        # /private/var/.../trials/<name>/... -> <TRIAL>/...
        i = chunk.find("/trials/")
        if i != -1:
            j = chunk.find("/", i + len("/trials/"))
            if j == -1:
                chunk = "<TRIAL>"
            else:
                chunk = chunk[:i] + "/trials/<TRIAL>" + chunk[j:]
        out.append(chunk)
    return " ".join(out)


def main():
    specs = sys.argv[1:]
    runs = {}
    for spec in specs:
        label, paths = spec.split("=", 1)
        nd, audit = paths.split(":", 1)
        runs[label] = load(nd, audit)

    for label, (ev, audit) in runs.items():
        print(f"### {label}")
        print(f"  ndjson_lines={len(ev)}")
        print(f"  session_ids={sorted({e['session'] for e in ev})}")
        print(f"  envelope_type_seq={[e['type'] for e in ev]}")
        print(f"  tool_seq={[(e['tool'], e['status']) for e in ev if e['type'] == 'tool_use']}")
        print(f"  commands={[normalize(e['cmd']) for e in ev if e['type'] == 'tool_use' and e['cmd']]}")
        print(f"  skill_events={[(e['skill_name'], normalize((e['metadata'] or {}).get('dir'))) for e in ev if e['tool'] == 'skill']}")
        print(f"  text_events={[e['text'] for e in ev if e['type'] == 'text']}")
        print(f"  step_finish_reasons={[e['reason'] for e in ev if e['type'] == 'step_finish']}")
        print(f"  step_finish_tokens={[e['tokens'] for e in ev if e['type'] == 'step_finish']}")
        print(f"  step_finish_costs={[e['cost'] for e in ev if e['type'] == 'step_finish']}")
        print(f"  call_ids={[e['callID'] for e in ev if e['type'] == 'tool_use']}")
        print(f"  timestamps_first_last={ev[0]['ts'] if ev else None} .. {ev[-1]['ts'] if ev else None}")
        print(f"  audit_log={audit}")
        print(f"  protocol_order={protocol_order(audit)}")
        print()

    labels = list(runs)
    if len(labels) >= 2:
        print("=== VARIANCE (pairwise, first vs each later) ===")

        def cmp(a, b):
            ea, aa = runs[a]
            eb, ab = runs[b]
            out = {}
            out["session_id"] = (sorted({e["session"] for e in ea}) == sorted({e["session"] for e in eb}))
            out["timestamps"] = ([e["ts"] for e in ea] == [e["ts"] for e in eb])
            out["envelope_type_seq"] = ([e["type"] for e in ea] == [e["type"] for e in eb])
            out["tool_names+status"] = ([(e["tool"], e["status"]) for e in ea if e["type"] == "tool_use"]
                                         == [(e["tool"], e["status"]) for e in eb if e["type"] == "tool_use"])
            out["commands (path-normalized)"] = ([normalize(e["cmd"]) for e in ea if e["cmd"]]
                                                 == [normalize(e["cmd"]) for e in eb if e["cmd"]])
            out["skill_event_present"] = (any(e["tool"] == "skill" for e in ea) == any(e["tool"] == "skill" for e in eb))
            out["skill_metadata_dir (norm)"] = ([normalize((e["metadata"] or {}).get("dir")) for e in ea if e["tool"] == "skill"]
                                                 == [normalize((e["metadata"] or {}).get("dir")) for e in eb if e["tool"] == "skill"])
            out["skill_status"] = ([e["status"] for e in ea if e["tool"] == "skill"]
                                   == [e["status"] for e in eb if e["tool"] == "skill"])
            out["text_events"] = ([e["text"] for e in ea if e["text"]] == [e["text"] for e in eb if e["text"]])
            out["token_counts"] = ([e["tokens"] for e in ea if e["tokens"]] == [e["tokens"] for e in eb if e["tokens"]])
            out["costs (distinct values)"] = (sorted({e["cost"] for e in ea if e["cost"] is not None})
                                               == sorted({e["cost"] for e in eb if e["cost"] is not None}))
            out["all costs == 0"] = (all(e["cost"] == 0 for e in ea if e["cost"] is not None)
                                     and all(e["cost"] == 0 for e in eb if e["cost"] is not None))
            out["call_ids"] = ([e["callID"] for e in ea] == [e["callID"] for e in eb])
            out["audit_log (exact)"] = (aa == ab)
            out["audit_log (normalized)"] = ([normalize(x) for x in aa] == [normalize(x) for x in ab])
            out["protocol_order"] = (protocol_order(aa) == protocol_order(ab))
            out["protocol == doctor/plan/install/verify"] = (protocol_order(aa) == PROTOCOL and protocol_order(ab) == PROTOCOL)
            return out

        for i in range(len(labels)):
            for j in range(i + 1, len(labels)):
                a, b = labels[i], labels[j]
                print(f"  {a}  vs  {b}:")
                for k, same in cmp(a, b).items():
                    print(f"    {k:26s} {'STABLE' if same else 'VARIES'}")

        print()
        print("=== SUMMARY ===")
        first = labels[0]
        for k in cmp(first, labels[1]):
            allsame = all(cmp(labels[i], labels[j])[k] for i in range(len(labels)) for j in range(i + 1, len(labels)))
            print(f"  {k:26s} {'STABLE across all runs' if allsame else 'VARIES across runs'}")


if __name__ == "__main__":
    main()
