#!/usr/bin/env python3
"""Summarize the M6 discovery probes into machine-readable tally files.

Reads the raw captures under Docs/Eval/milestone-6/probes/discovery/ and writes
one summary per task next to them.
"""
import collections
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TRIAL_PREFIX = "/m6-probe/discovery/trials/"
MAGIC = re.compile(r"MAGIC-[A-Z]+-[A-Z]+-\d+")


def events(path):
    """Yield (type, part) for every NDJSON event in path."""
    with open(path, errors="replace") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            yield obj.get("type"), obj.get("part", {}) or {}


def skill_tool_calls(path):
    out = []
    for kind, part in events(path):
        if kind != "tool_use":
            continue
        state = part.get("state", {}) or {}
        meta = state.get("metadata") or {}
        body = state.get("output") or ""
        marker = re.search(r"MARKER: (\S+)", body)
        out.append(
            {
                "tool": part.get("tool"),
                "status": state.get("status"),
                "name": meta.get("name"),
                "dir": meta.get("dir"),
                "marker": marker.group(1) if marker else None,
                "error": state.get("error"),
            }
        )
    return out


def text_of(path):
    chunks = []
    for kind, part in events(path):
        if kind == "text":
            chunks.append(part.get("text") or "")
    return "\n".join(chunks)


def shorten(path):
    return path.replace(TRIAL_PREFIX, "") if path else path


def tally_a():
    lines = ["# TASK A - duplicate skill name resolution (winner per run)", ""]
    for case in ("a1", "a2", "a3", "a4"):
        model = collections.Counter()
        for path in sorted(glob.glob(f"{ROOT}/task-a/{case}.model.run-*.ndjson")):
            for call in skill_tool_calls(path):
                if call["tool"] == "skill":
                    model[call["marker"] or "?"] += 1
        debug = collections.Counter()
        for path in sorted(glob.glob(f"{ROOT}/task-a/{case}.debug.run-*.json")):
            try:
                data = json.load(open(path))
            except json.JSONDecodeError:
                debug["PARSE-ERROR"] += 1
                continue
            hits = [s for s in data if s.get("name") == "dup"]
            if not hits:
                debug["NO-DUP-SKILL"] += 1
                continue
            marker = re.search(r"MARKER: (\S+)", hits[0].get("content") or "")
            debug[marker.group(1) if marker else "?"] += 1
        big = collections.Counter()
        for path in sorted(glob.glob(f"{ROOT}/task-b3/{case}.*.json")):
            try:
                data = json.load(open(path))
            except json.JSONDecodeError:
                big["PARSE-ERROR"] += 1
                continue
            hits = [s for s in data if s.get("name") == "dup"]
            if not hits:
                big["NO-DUP-SKILL"] += 1
                continue
            marker = re.search(r"MARKER: (\S+)", hits[0].get("content") or "")
            big[marker.group(1) if marker else "?"] += 1
        lines.append(f"## {case}")
        lines.append(f"  model-driven (`opencode run`, skill tool metadata.dir): {dict(model)}")
        lines.append(f"  model-free  (`opencode debug skill`, location):        {dict(debug)}")
        if big:
            lines.append(f"  model-free, 30-sample set:                            {dict(big)}")
        lines.append("")
    return "\n".join(lines)


def tally_b():
    lines = ["# TASK B - skill visibility under the OPENCODE_DISABLE_* flags", ""]
    for path in sorted(glob.glob(f"{ROOT}/task-b3/m7.*.json")):
        flags = os.path.basename(path)[3:-5]
        try:
            data = json.load(open(path))
            names = sorted(s.get("name") for s in data)
        except json.JSONDecodeError:
            names = ["PARSE-ERROR"]
        lines.append(f"  {flags:55s} {names}")
    lines.append("")
    lines.append("# TASK B - skills under OPENCODE_DISABLE_CLAUDE_CODE_SKILLS (model-driven run)")
    for label in ("b1-off", "b1-on"):
        for path in sorted(glob.glob(f"{ROOT}/task-b/{label}.model.run-*.ndjson")):
            calls = [c for c in skill_tool_calls(path) if c["tool"] == "skill"]
            lines.append(
                f"  {label}: "
                + ", ".join(
                    f"{c['name']}={c['status']}({shorten(c['dir'])})" for c in calls
                )
            )
    lines.append("")
    lines.append("# TASK B - project .opencode discovery under OPENCODE_DISABLE_PROJECT_CONFIG")
    for label in ("b2-off", "b2-on"):
        for path in sorted(glob.glob(f"{ROOT}/task-b/{label}.model.run-*.ndjson")):
            calls = [c for c in skill_tool_calls(path) if c["tool"] == "skill"]
            lines.append(
                f"  {label}: "
                + ", ".join(
                    f"{c['name']}={c['status']}({shorten(c['dir'])})" for c in calls
                )
                + " | "
                + " / ".join(
                    line.strip()
                    for line in text_of(path).splitlines()
                    if line.strip()
                )[:160]
            )
    lines.append("")
    return "\n".join(lines)


def tally_b2():
    """Instruction attachment matrix, decoded by call order (labels are cumulative)."""
    order = [
        ("none", []),
        ("skills", ["OPENCODE_DISABLE_CLAUDE_CODE_SKILLS"]),
        ("projectcfg", ["OPENCODE_DISABLE_PROJECT_CONFIG"]),
        ("prompt", ["OPENCODE_DISABLE_CLAUDE_CODE_PROMPT"]),
        ("broad", ["OPENCODE_DISABLE_CLAUDE_CODE"]),
    ]
    lines = [
        "# TASK B2 - instruction attachment matrix (magic-passphrase method)",
        "",
        "Each trial plants exactly ONE instruction file containing a unique magic",
        "passphrase. PRESENT = the passphrase was echoed back (file was injected).",
        "NONE = no passphrase available (file was not injected).",
        "",
    ]
    trials = {
        "m1": "project AGENTS.md (cwd/AGENTS.md)",
        "m2": "global ~/.claude/CLAUDE.md",
        "m3": "project CLAUDE.md (cwd/CLAUDE.md)",
        "m4": "global ${XDG_CONFIG_HOME}/opencode/AGENTS.md",
    }
    for trial, desc in trials.items():
        lines.append(f"## {trial} - {desc}")
        # labels are cumulative because the probe reused one shell variable;
        # recover the true flag set from the label suffix chain.
        files = sorted(glob.glob(f"{ROOT}/task-b2/{trial}-*.model.run-*.ndjson"))
        parsed = []
        for path in files:
            label = os.path.basename(path).split(".model.run-")[0]
            suffix = label[len(trial) + 1 :]
            last = suffix.split("-")[-1]
            flags = dict((name, env) for name, env in order).get(last, [])
            match = re.search(r"-run-(\d+)\.ndjson$", path)
            reps = match.group(1) if match else "?"
            text = text_of(path)
            magic = sorted(set(MAGIC.findall(text)))
            verdict = "PRESENT:" + ",".join(magic) if magic else (
                "NONE" if "NONE" in text.upper() else "OTHER"
            )
            parsed.append((flags, reps, verdict))
        for flags, reps, verdict in parsed:
            lines.append(
                f"  flags={'+'.join(flags) if flags else '<none>':45s} "
                f"rep={reps} -> {verdict}"
            )
        lines.append("")
    lines.append("## race evidence - `duplicate skill name` WARN pairs (model-driven runs)")
    lines.append("")
    lines.append("`existing` = the file registered first, `duplicate` = the file that overwrote it.")
    lines.append("A swap between runs proves the winner is decided by async completion order.")
    for case in ("a1", "a2", "a3", "a4"):
        path = f"{ROOT}/task-a/{case}.duplicate-skill-warnings.log"
        if not os.path.exists(path):
            continue
        first = collections.Counter()
        for line in open(path):
            match = re.search(r"existing=(\S+)", line)
            if match:
                first[match.group(1)] += 1
        lines.append(f"  {case}: {dict(first)}")
    lines.append("")
    lines.append("## m5 - AGENTS.md at two levels (instance dir = worktree/app)")
    for path in sorted(glob.glob(f"{ROOT}/task-b3/m5b-*.model.run-*.ndjson")):
        lines.append(f"  {os.path.basename(path)}: {text_of(path).strip()[:400]}")
    lines.append("")
    lines.append("## m6 - global XDG AGENTS.md + ~/.claude/CLAUDE.md together")
    for path in sorted(glob.glob(f"{ROOT}/task-b3/m6-*.model.run-*.ndjson")):
        lines.append(f"  {os.path.basename(path)}: {text_of(path).strip()[:400]}")
    lines.append("")
    return "\n".join(lines)


def main():
    for path, body in (
        (f"{ROOT}/task-a/summary.txt", tally_a()),
        (f"{ROOT}/task-b/summary.txt", tally_b()),
        (f"{ROOT}/task-b2/summary.txt", tally_b2()),
    ):
        with open(path, "w") as handle:
            handle.write(body)
    print("wrote summary files")


if __name__ == "__main__":
    sys.exit(main())
