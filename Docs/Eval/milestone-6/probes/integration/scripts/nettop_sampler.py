#!/usr/bin/env python3
"""nettop sampler: per-process cumulative bytes_in/bytes_out.

nettop -P -L 1 -J bytes_in,bytes_out emits lines of the form
    "name.pid,bytes_in,bytes_out,"          (trailing empty field)
The process name is "name.pid"; counters are fields [1] and [2].

Usage: nettop_sampler.py <out.json> <duration_s> [name ...]
Keeps the MAX cumulative counters seen per name (nettop counters are
cumulative per process) and reports the totals.
"""
import json
import signal
import subprocess
import sys
import time

_best = {}


def _dump(outpath):
    result = {
        "per_name": {k: {"bytes_in": v[0], "bytes_out": v[1]} for k, v in sorted(_best.items())},
        "total_bytes_in": sum(v[0] for v in _best.values()),
        "total_bytes_out": sum(v[1] for v in _best.values()),
    }
    with open(outpath, "w") as fh:
        json.dump(result, fh, indent=2)
    return result


def main():
    outpath = sys.argv[1]
    duration = float(sys.argv[2])
    names = set(sys.argv[3:]) or None

    def _on_term(signum, frame):
        result = _dump(outpath)
        print(json.dumps(result, indent=2))
        sys.exit(0)

    signal.signal(signal.SIGTERM, _on_term)

    deadline = time.time() + duration
    while time.time() < deadline:
        try:
            p = subprocess.run(
                ["nettop", "-P", "-L", "1", "-J", "bytes_in,bytes_out"],
                capture_output=True, text=True, timeout=15,
            )
        except Exception:
            time.sleep(0.2)
            continue
        for line in p.stdout.splitlines():
            if not line.strip() or line.startswith(","):
                continue
            parts = line.split(",")
            if len(parts) < 3:
                continue
            raw = parts[0]
            name = raw.rsplit(".", 1)[0]
            try:
                bin_ = int(parts[1])
                bout = int(parts[2])
            except ValueError:
                continue
            if names and name not in names:
                continue
            prev = _best.get(name, (0, 0))
            _best[name] = (max(prev[0], bin_), max(prev[1], bout))
        time.sleep(0.1)

    result = _dump(outpath)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
