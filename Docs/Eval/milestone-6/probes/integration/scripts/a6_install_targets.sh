#!/bin/sh
# A6 - WHERE does the @opencode-ai/plugin install land, and what does it cost?
#
# opencode installs the plugin into EVERY ".opencode" config dir it discovers:
#   - the global config dir  ($XDG_CONFIG_HOME/opencode)
#   - the PROJECT config dir (<cwd>/.opencode)   <-- exists iff the project has one
# A project skill at <cwd>/.opencode/skill/root/SKILL.md CREATES <cwd>/.opencode,
# so the trial gets a SECOND install target.
#
# This measures, per variant:
#   - shared npm cache delta (registry bytes pulled)
#   - bytes written into <cwd>/.opencode/node_modules
#   - bytes written into $XDG_CONFIG_HOME/opencode/node_modules
set -e
. /private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe/integration/scripts/lib.sh
OUT="$M6I/out"; mkdir -p "$OUT"
LOG="$OUT/a6-install-targets.txt"
: > "$LOG"
say() { echo "$@" | tee -a "$LOG"; }

SHAREDCACHE=$M6I/shared/a6cache; rm -rf "$SHAREDCACHE"; mkdir -p "$SHAREDCACHE"

variant() {
  V="$1"; SEED_GLOBAL="$2"; SEED_PROJECT="$3"; WITH_SKILL="$4"
  T=$(mktrial "a6-$V")
  [ "$SEED_GLOBAL" = "yes" ] && seed_config "$T"
  [ "$SEED_PROJECT" = "yes" ] && { mkdir -p "$T/cwd/.opencode"; cp -R "$SHARED/config-template/opencode/node_modules" "$T/cwd/.opencode/node_modules"; }
  [ "$WITH_SKILL" = "yes" ] && seed_skill "$T" > /dev/null
  seed_models "$T" ro
  seed_rg "$T"
  BEFORE=$(dirsize "$SHAREDCACHE")
  S=$(msnow)
  (
    cd "$T/cwd" && /usr/bin/env -i \
      HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data \
      XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp \
      PATH=$T/bin TRIAL_AUDIT_LOG=$T/audit.log \
      OPENCODE_DISABLE_AUTOUPDATE=1 \
      npm_config_cache=$SHAREDCACHE \
      "$BIN" run --format json --model opencode/big-pickle --pure "Reply with exactly: ok"
  ) > "$OUT/a6-$V.ndjson" 2> "$OUT/a6-$V.stderr.txt" || true
  E=$(msnow)
  sleep 12
  AFTER=$(dirsize "$SHAREDCACHE")
  say "--- variant=$V (seed_global=$SEED_GLOBAL seed_project_nm=$SEED_PROJECT skill=$WITH_SKILL)"
  say "    wall_ms=$((E-S))"
  say "    shared_npmcache_delta=$((AFTER-BEFORE))"
  say "    project_nm_bytes=$(dirsize "$T/cwd/.opencode/node_modules")"
  say "    global_nm_bytes=$(dirsize "$T/config/opencode/node_modules")"
  say "    cwd_bytes=$(dirsize "$T/cwd")"
  say "    trial_total_bytes=$(dirsize "$T")"
  say ""
}

say "############ A6-1 COLD shared cache, no skill (only global target), global NOT seeded"
variant cold-noskill-globalunseeded no no no
say "############ A6-2 COLD shared cache, WITH skill (global + project targets), neither seeded"
variant cold-skill-bothunseeded no no yes
say "############ A6-3 WARM shared cache (now populated), WITH skill, global seeded, project NOT seeded"
variant warm-skill-projunseeded yes no yes
say "############ A6-4 WARM shared cache, WITH skill, global seeded, project node_modules ALSO seeded"
variant warm-skill-projseeded yes yes yes
say "A6 DONE"
