#!/bin/sh
# A1d - Does the COLD plugin install complete?  Is it detached from the CLI
# process (killed when the CLI exits)?
#
# A1c observed: npm_cache_delta=41MB, config node_modules=0, install never
# landed even after 120s of waiting.  Hypothesis: the install is a DETACHED
# background task that dies with the CLI process.
#
# Test: run a cold trial, then (a) look for a surviving npm/node child,
#       (b) watch node_modules for 60s after CLI exit.
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a1d-install-lifecycle.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

CA=$M6I/shared/a1d-cache; rm -rf "$CA"; mkdir -p "$CA"
T=$(mktrial a1d)
say "trial=$T  cache=$CA"

(
  cd "$T/cwd" && /usr/bin/env -i \
    HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
    XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
    PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    npm_config_cache=$CA \
    "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
) > "$OUT/a1d.ndjson" 2> "$OUT/a1d.stderr.txt" &
CLI=$!
say "cli pid=$CLI"

# while the CLI is alive, watch for the install target
i=0
while kill -0 $CLI 2>/dev/null && [ $i -lt 40 ]; do
  if [ -d "$T/config/opencode/node_modules/@opencode-ai/plugin" ]; then
    say "t=${i}s plugin dir PRESENT while cli alive"
    break
  fi
  sleep 1; i=$((i+1))
done
say "cli alive after ${i}s of polling"

wait $CLI 2>/dev/null || true
say "cli exited"
say "immediately after exit: cache=$(dirsize "$CA") config_nm=$(dirsize "$T/config/opencode/node_modules")"

say "--- child processes of the CLI right after exit ---"
ps -Ao pid,ppid,command | grep -E "npm|node|opencode" | grep -v grep | grep -E "a1d|$CA" | head -5 || echo "(none referencing this trial)"

for j in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
  sleep 2
  if [ -d "$T/config/opencode/node_modules/@opencode-ai/plugin" ]; then
    say "t=+$((j*2))s AFTER CLI exit: plugin dir PRESENT (install completed post-exit)"
    break
  fi
done
say "final: cache=$(dirsize "$CA") config_nm=$(dirsize "$T/config/opencode/node_modules")"
say "config tree:"
find "$T/config" -maxdepth 3 | sed "s|$T|\$T|" | tee -a "$LOG"
say "A1D DONE"
