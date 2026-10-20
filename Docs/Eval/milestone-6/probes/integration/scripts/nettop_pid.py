#!/usr/bin/env python3
"""nettop sampler, PID-aware: measures only the trial's own opencode process.

nettop -P -L 1 -J bytes_in,bytes_out emits "name.pid,bytes_in,bytes_out,".
We filter to the exact PID we spawned so the host's own long-running
`opencode serve` is NOT counted.

Usage: nettop_pid.py <out.json> <pid> [duration_s]
"""
import json
import signal
import subprocess
import sys
import time

_best = {}


def _dump(outpath):
    result = {
        "per_pid": {k: {"bytes_in": v[0], "bytes_out": v[1]} for k, v in sorted(_best.items())},
        "total_bytes_in": sum(v[0] for v in _best.values()),
        "total_bytes_out": sum(v[1] for v in _best.values()),
    }
    with open(outpath, "w") as fh:
        json.dump(result, fh, indent=2)
    return result


def main():
    outpath = sys.argv[1]
    pid = sys.argv[2]
    duration = float(sys.argv[3]) if len(sys.argv) > 3 else 300.0

    def _on_term(signum, frame):
        _dump(outpath)
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
            name, _, pid_s = raw.rpartition(".")
            if pid_s != pid:
                continue
            try:
                bin_, bout = int(parts[1]), int(parts[2])
            except ValueError:
                continue
            prev = _best.get(name, (0, 0))
            _best[name] = (max(prev[0], bin_), max(prev[1], bout))
        time.sleep(0.1)
    _dump(outpath)


if __name__ == "__main__":
    main()
