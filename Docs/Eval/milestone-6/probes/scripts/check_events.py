#!/usr/bin/env python3
"""Field-by-field conformance check of an `opencode run --format json` NDJSON
stream against the rules encoded in crates/root-eval/src/events.rs.

Mirrors, exactly:
  parse_ndjson_line -> envelope {type, sessionID, timestamp}
  SOURCE_STEP_START  -> part must be an object
  SOURCE_TEXT        -> part.text must be a string
  SOURCE_STEP_FINISH -> part.tokens.{input,output,reasoning,cache.read,cache.write}
                        must be non-negative integers; part.cost must be a number
  SOURCE_TOOL_USE    -> part.tool string, part.callID string, part.state object
  SOURCE_ERROR       -> error object with error.name string
  anything else      -> UnknownEventType (a hard error in Root's parser)
"""
import json
import sys

KINDS = {"step_start", "text", "step_finish", "tool_use", "error"}
U64 = int


def is_u64(v):
    return isinstance(v, int) and not isinstance(v, bool) and v >= 0


def check_line(line, seq, errs):
    try:
        o = json.loads(line)
    except Exception as e:
        errs.append((seq, "InvalidJson", str(e)))
        return None
    if not isinstance(o, dict):
        errs.append((seq, "InvalidShape", "top level must be a JSON object"))
        return None
    t = o.get("type")
    if not isinstance(t, str):
        errs.append((seq, "MissingField", "type"))
        return None
    if not isinstance(o.get("sessionID"), str):
        errs.append((seq, "MissingField", "sessionID"))
        return None
    ts = o.get("timestamp")
    if not isinstance(ts, int) or isinstance(ts, bool):
        errs.append((seq, "MissingField", "timestamp"))
        return None
    if t not in KINDS:
        errs.append((seq, "UnknownEventType", t))
        return None
    if t == "error":
        e = o.get("error")
        if not isinstance(e, dict):
            errs.append((seq, "InvalidShape", "error must be an object"))
        elif not isinstance(e.get("name"), str):
            errs.append((seq, "InvalidShape", "error.name must be a string"))
        return o
    p = o.get("part")
    if not isinstance(p, dict):
        errs.append((seq, "InvalidShape", "part must be an object"))
        return o
    if t == "text":
        if not isinstance(p.get("text"), str):
            errs.append((seq, "InvalidShape", "part.text must be a string"))
    elif t == "step_finish":
        tk = p.get("tokens")
        if tk is not None:
            if not isinstance(tk, dict):
                errs.append((seq, "InvalidShape", "part.tokens must be an object"))
            else:
                for k in ("input", "output", "reasoning"):
                    if not is_u64(tk.get(k)):
                        errs.append((seq, "InvalidShape", f"part.tokens.{k} must be a non-negative integer"))
                c = tk.get("cache")
                if not isinstance(c, dict):
                    errs.append((seq, "InvalidShape", "part.tokens.cache must be an object"))
                else:
                    for k in ("read", "write"):
                        if not is_u64(c.get(k)):
                            errs.append((seq, "InvalidShape", f"part.tokens.cache.{k} must be a non-negative integer"))
        if "cost" in p and not isinstance(p["cost"], (int, float)):
            errs.append((seq, "InvalidShape", "part.cost must be a number"))
    elif t == "tool_use":
        if not isinstance(p.get("tool"), str):
            errs.append((seq, "InvalidShape", "part.tool must be a string"))
        if not isinstance(p.get("callID"), str):
            errs.append((seq, "InvalidShape", "part.callID must be a string"))
        st = p.get("state")
        if not isinstance(st, dict):
            errs.append((seq, "InvalidShape", "part.state must be an object"))
        else:
            out = st.get("output")
            if out is not None and not isinstance(out, str):
                errs.append((seq, "InvalidShape", "part.state.output must be a string or absent"))
            if "error" in st and not isinstance(st["error"], str):
                errs.append((seq, "InvalidShape", "part.state.error must be a string"))
    return o


def main():
    path = sys.argv[1]
    errs = []
    types = []
    tools = []
    cmds = []
    sess = set()
    tokens = []
    costs = []
    for seq, line in enumerate(open(path)):
        line = line.rstrip("\n")
        if not line.strip():
            continue
        o = check_line(line, seq, errs)
        if o is None:
            continue
        types.append(o["type"])
        sess.add(o["sessionID"])
        p = o.get("part") or {}
        if o["type"] == "tool_use":
            tools.append(p.get("tool"))
            st = p.get("state") or {}
            inp = st.get("input")
            if isinstance(inp, dict):
                cmds.append(inp.get("command"))
            if st.get("status") == "error":
                errs.append((seq, "ASK_REJECT", st.get("error")))
        if o["type"] == "step_finish":
            tokens.append(p.get("tokens"))
            costs.append(p.get("cost"))
    print(f"file: {path}")
    print(f"  lines={len(types)} sessions={len(sess)} session_id={sorted(sess)}")
    print(f"  event_types={types}")
    print(f"  tool_sequence={tools}")
    print(f"  commands={[c for c in cmds if c is not None]}")
    print(f"  step_finish_count={len(tokens)} all_costs={costs}")
    if errs:
        print(f"  PARSER ERRORS ({len(errs)}):")
        for e in errs:
            print(f"    line {e[0]}: [{e[1]}] {e[2]}")
    else:
        print("  PARSER ERRORS: none - fully conformant with crates/root-eval/src/events.rs")


if __name__ == "__main__":
    main()
