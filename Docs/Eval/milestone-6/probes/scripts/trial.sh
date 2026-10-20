#!/bin/sh
# Shared trial-builder for P4/P5.
# A trial is a fully isolated env: HOME + 4 XDG dirs + TMPDIR, a PATH that
# contains only the fake `root` / `brew` shims (plus /bin:/usr/bin for the
# shell itself - no host brew/root/npm/node/git resolvable), a seeded project
# skill, and the pre-warmed npm cache + pre-seeded config/models templates from
# P3 so the trial performs ZERO registry fetches.
M6=/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m6-probe
REPO=/Users/sergio/Documents/Developer/BoringInfraCo/Root
SHARED="$M6/shared"

trial_bytes() { find "$1" -type f -exec stat -f '%z' {} + 2>/dev/null | awk '{s+=$1} END {print s+0}'; }

# mktrial4 <name> [with-skill|without-skill]
mktrial4() {
  T="$M6/trials/$1"
  SKILLMODE="${2:-with-skill}"
  rm -rf "$T"
  mkdir -p "$T/home" "$T/config" "$T/data" "$T/cache" "$T/state" "$T/tmp" "$T/cwd" "$T/bin" "$T/out"
  # pre-seeded config dir (node_modules/@opencode-ai/plugin present) + models.json template
  cp -R "$SHARED/config-template/opencode" "$T/config/opencode" 2>/dev/null || true
  mkdir -p "$T/cache/opencode"
  cp "$SHARED/models/models.json" "$T/cache/opencode/models.json" 2>/dev/null || true
  # fake shims
  cat > "$T/bin/root" <<'SH'
#!/bin/sh
printf '%s\n' "root $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' '{"success":true}'
exit 0
SH
  cat > "$T/bin/brew" <<'SH'
#!/bin/sh
printf '%s\n' "brew $*" >> "$TRIAL_AUDIT_LOG"
printf '%s\n' 'brew: fake shim: refusing to install (trial sandbox)' >&2
exit 1
SH
  chmod +x "$T/bin/root" "$T/bin/brew"
  # optional seeded project skill (byte-for-byte copy of the repo skill)
  if [ "$SKILLMODE" = "with-skill" ]; then
    mkdir -p "$T/cwd/.opencode/skill/root"
    cp "$REPO/skills/claude/SKILL.md" "$T/cwd/.opencode/skill/root/SKILL.md"
  fi
  : > "$T/audit.log"
  echo "$T"
}

# trialenv4 <trial> [extra env...]  -> `env -i ...` prefix
trialenv4() {
  T="$1"; shift
  printf '%s' "env -i HOME=$T/home XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache XDG_STATE_HOME=$T/state TMPDIR=$T/tmp PATH=$T/bin:/bin:/usr/bin TRIAL_AUDIT_LOG=$T/audit.log OPENCODE_DISABLE_AUTOUPDATE=1 npm_config_cache=$SHARED/npmcache"
  for kv in "$@"; do printf ' %s' "$kv"; done
}

# report <trial> <label> <ndjsonfile>
report() {
  T="$1"; LABEL="$2"; ND="$3"
  echo "===== $LABEL ====="
  echo "--- audit log ($T/audit.log) ---"
  if [ -s "$T/audit.log" ]; then cat "$T/audit.log"; else echo "(EMPTY - no shim invoked)"; fi
  echo "--- PATH sanity inside the trial env ---"
  ( cd "$T/cwd" && $(trialenv4 "$T") sh -c 'for t in root brew npm node git curl pip; do printf "%s -> %s\n" "$t" "$(command -v $t 2>/dev/null || echo NOTFOUND)"; done' )
  echo "--- NDJSON event types ---"
  python3 - "$ND" <<'PY'
import json,sys
for i,line in enumerate(open(sys.argv[1])):
    line=line.strip()
    if not line: continue
    o=json.loads(line)
    t=o.get("type"); p=o.get("part") or {}
    if t=="tool_use":
        st=p.get("state") or {}; md=st.get("metadata") or {}
        cmd=(st.get("input") or {}).get("command") if isinstance(st.get("input"),dict) else None
        print(f"  {i} tool_use tool={p.get('tool')} status={st.get('status')} callID={p.get('callID')} metadata={md} cmd={cmd!r}")
        if st.get("error"): print(f"      error={st['error'][:160]!r}")
    elif t=="text":
        print(f"  {i} text {p.get('text')!r}")
    elif t=="step_finish":
        print(f"  {i} step_finish reason={p.get('reason')} tokens={json.dumps(p.get('tokens'))} cost={p.get('cost')}")
    else:
        print(f"  {i} {t}")
PY
}
