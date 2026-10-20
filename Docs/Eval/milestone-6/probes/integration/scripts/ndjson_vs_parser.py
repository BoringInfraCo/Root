#!/usr/bin/env python3
"""Compare a real OpenCode 1.18.27 NDJSON stream against crates/root-eval/src/events.rs.

The Rust parser (read from the repo) accepts exactly five envelope types:
  step_start | text | step_finish | tool_use | error
and requires:
  - envelope: type (str), sessionID (str), timestamp (int ms)
  - step_start : part must be an object
  - text       : part.text must be a string
  - step_finish: part.tokens.{input,output,reasoning,cache.read,cache.write} must be
                 non-negative integers; part.cost (if present) must be a number
  - tool_use   : part.tool (str), part.callID (str), part.state (object)
  - error      : error.name (str), error.data (any)

This script re-implements those rules in Python and reports every line that the
Rust parser would REJECT (typed error), plus any field the Rust parser silently
drops (e.g. part.state.metadata).
"""
import json
import sys

RUST_TYPES = {"step_start", "text", "step_finish", "tool_use", "error"}


def parse_line(line, seq):
    """Mirror of root-eval::events::parse_ndjson_line. Returns (event, error)."""
    try:
        value = json.loads(line)
    except Exception as exc:
        return None, ("InvalidJson", str(exc))
    if not isinstance(value, dict):
        return None, ("InvalidShape", "top level must be a JSON object")
    for field, want in (("type", str), ("sessionID", str), ("timestamp", int)):
        if field not in value:
            return None, ("MissingField", f"missing `{field}`")
        if want is int:
            if not isinstance(value[field], int) or isinstance(value[field], bool):
                return None, ("MissingField", f"`{field}` must be an integer")
        elif not isinstance(value[field], str):
            return None, ("MissingField", f"`{field}` must be a string")
    etype = value["type"]
    part = value.get("part")
    if etype == "step_start":
        if not isinstance(part, dict):
            return None, ("InvalidShape", "`part` must be an object")
    elif etype == "text":
        if not isinstance(part, dict):
            return None, ("InvalidShape", "`part` must be an object")
        if not isinstance(part.get("text"), str):
            return None, ("InvalidShape", "`part.text` must be a string")
    elif etype == "step_finish":
        if not isinstance(part, dict):
            return None, ("InvalidShape", "`part` must be an object")
        tokens = part.get("tokens")
        if tokens is not None:
            if not isinstance(tokens, dict):
                return None, ("InvalidShape", "`part.tokens` must be an object")
            for name in ("input", "output", "reasoning"):
                if not isinstance(tokens.get(name), int) or isinstance(tokens.get(name), bool):
                    return None, ("InvalidShape", f"`part.tokens.{name}` must be a non-negative integer")
            cache = tokens.get("cache")
            if not isinstance(cache, dict):
                return None, ("InvalidShape", "`part.tokens.cache` must be an object")
            for name in ("read", "write"):
                if not isinstance(cache.get(name), int) or isinstance(cache.get(name), bool):
                    return None, ("InvalidShape", f"`part.tokens.cache.{name}` must be a non-negative integer")
        cost = part.get("cost")
        if cost is not None and not isinstance(cost, (int, float)) or isinstance(cost, bool):
            return None, ("InvalidShape", "`part.cost` must be a number")
    elif etype == "tool_use":
        if not isinstance(part, dict):
            return None, ("InvalidShape", "`part` must be an object")
        if not isinstance(part.get("tool"), str):
            return None, ("InvalidShape", "`part.tool` must be a string")
        if not isinstance(part.get("callID"), str):
            return None, ("InvalidShape", "`part.callID` must be a string")
        state = part.get("state")
        if not isinstance(state, dict):
            return None, ("InvalidShape", "`part.state` must be an object")
    elif etype == "error":
        err = value.get("error")
        if not isinstance(err, dict):
            return None, ("InvalidShape", "`error` must be an object with `name` and `data`")
        if not isinstance(err.get("name"), str):
            return None, ("InvalidShape", "`error.name` must be a string")
    else:
        return None, ("UnknownEventType", etype)
    return value, None


def main():
    path = sys.argv[1]
    lines = [l.rstrip("\n") for l in open(path) if l.strip()]
    ok = 0
    errors = []
    type_hist = {}
    part_type_hist = {}
    dropped_fields = {}
    for seq, line in enumerate(lines):
        ev, err = parse_line(line, seq)
        if err:
            errors.append((seq, err[0], err[1]))
            continue
        ok += 1
        type_hist[ev["type"]] = type_hist.get(ev["type"], 0) + 1
        part = ev.get("part") or {}
        pt = part.get("type")
        part_type_hist[(ev["type"], pt)] = part_type_hist.get((ev["type"], pt), 0) + 1
        # Fields the Rust parser does NOT read
        rust_reads = {"step_start": set(), "text": {"text"}, "step_finish": {"tokens", "cost"},
                      "tool_use": {"tool", "callID", "state"}, "error": set()}
        extra = set(part.keys()) - rust_reads.get(ev["type"], set()) - {"type", "id", "messageID", "sessionID"}
        if ev["type"] == "tool_use":
            state = part.get("state") or {}
            extra -= {"input", "output", "status", "error"}
            extra |= set(state.keys()) - {"input", "output", "status", "error"}
        for f in extra:
            dropped_fields.setdefault((ev["type"], f), 0)
            dropped_fields[(ev["type"], f)] += 1

    print(f"file: {path}")
    print(f"total non-empty lines: {len(lines)}")
    print(f"lines the Rust parser ACCEPTS: {ok}")
    print(f"lines the Rust parser REJECTS: {len(errors)}")
    for seq, kind, reason in errors:
        print(f"  REJECT line {seq}: {kind}: {reason}")
    print(f"envelope type histogram: {type_hist}")
    print(f"(envelope type, part.type) histogram:")
    for k, v in sorted(part_type_hist.items(), key=lambda x: str(x[0])):
        print(f"    {k}: {v}")
    print("part fields NOT READ by crates/root-eval/src/events.rs (silently dropped):")
    for (et, f), n in sorted(dropped_fields.items()):
        print(f"    {et}.{f}  ({n} occurrence(s))")
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
