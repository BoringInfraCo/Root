#!/bin/sh
# Fake agent for M5 benchmark task 02-verify-fails-rollback (TASK-SPEC
# calibration). Behavior is selected by TRIAL_FAKE_AGENT_CONDITION
# ("baseline" | "treatment"); every action runs for real through the trial
# PATH so the fake-tool shims log to $TRIAL_AUDIT_LOG, and each action is
# mirrored as a golden-shaped NDJSON tool_use event on stdout. POSIX builtins
# only; no writes outside the trial cwd/$TMPDIR. The prompt file named by
# TRIAL_FAKE_AGENT_PROMPT is read for realism but never branched on.
set -u

condition=${TRIAL_FAKE_AGENT_CONDITION:?TRIAL_FAKE_AGENT_CONDITION is not set}
prompt_file=${TRIAL_FAKE_AGENT_PROMPT:?TRIAL_FAKE_AGENT_PROMPT is not set}
IFS= read -r _prompt_line < "$prompt_file" || :

session="ses_fake_02-verify-fails-rollback_${condition}"
seq=0
call=0

_json_escape() {
  _in=$1
  _esc=
  while [ -n "$_in" ]; do
    _ch=${_in%"${_in#?}"}
    _prev=$_in
    _in=${_in#?}
    [ "$_in" = "$_prev" ] && _in=
    case $_ch in
      '"') _esc="${_esc}\\\"" ;;
      '\') _esc="${_esc}\\\\" ;;
      [[:cntrl:]])
        _ord=$(printf '%d' "'$_ch")
        case $_ord in
          8) _esc="${_esc}\\b" ;;
          9) _esc="${_esc}\\t" ;;
          10) _esc="${_esc}\\n" ;;
          12) _esc="${_esc}\\f" ;;
          13) _esc="${_esc}\\r" ;;
          *) _esc="${_esc}\\u00$(printf '%02x' "$_ord")" ;;
        esac
        ;;
      *) _esc="${_esc}${_ch}" ;;
    esac
  done
}

_truncate() {
  # Keep at most 2048 characters of captured output (builtin-only loop).
  _t_in=$1
  _t_out=
  _t_n=0
  while [ -n "$_t_in" ] && [ "$_t_n" -lt 2048 ]; do
    _t_ch=${_t_in%"${_t_in#?}"}
    _t_prev=$_t_in
    _t_in=${_t_in#?}
    [ "$_t_in" = "$_t_prev" ] && _t_in=
    _t_out="${_t_out}${_t_ch}"
    _t_n=$((_t_n + 1))
  done
  if [ -n "$_t_in" ]; then
    _t_out="${_t_out}
...[output truncated]"
  fi
}

_emit_step_start() {
  seq=$((seq + 1))
  printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":$((1700000000000 + seq * 1000)),\"sessionID\":\"${session}\",\"part\":{\"type\":\"step-start\"}}"
}

_emit_tool() {
  # $1 = exact command line, $2 = exit code, $3 = captured output
  seq=$((seq + 1))
  call=$((call + 1))
  _json_escape "$1"
  _cmd_json=$_esc
  _truncate "$3"
  _json_escape "$_t_out"
  _out_json=$_esc
  _status=completed
  _error_json=
  if [ "$2" -ne 0 ]; then
    _status=error
    _error_json=",\"error\":\"command exited with status $2\""
  fi
  printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":$((1700000000000 + seq * 1000)),\"sessionID\":\"${session}\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_${call}\",\"state\":{\"status\":\"${_status}\",\"input\":{\"command\":\"${_cmd_json}\"},\"output\":\"${_out_json}\"${_error_json}}}}"
}

_emit_text() {
  seq=$((seq + 1))
  _json_escape "$1"
  printf '%s\n' "{\"type\":\"text\",\"timestamp\":$((1700000000000 + seq * 1000)),\"sessionID\":\"${session}\",\"part\":{\"type\":\"text\",\"text\":\"${_esc}\"}}"
}

_emit_finish() {
  seq=$((seq + 1))
  printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":$((1700000000000 + seq * 1000)),\"sessionID\":\"${session}\",\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\",\"tokens\":{\"total\":1200,\"input\":1100,\"output\":60,\"reasoning\":40,\"cache\":{\"write\":0,\"read\":0}},\"cost\":0.001}}"
}

_run() {
  # Execute the exact command line for real, capture merged output, mirror it
  # as a tool_use event, and return the command's exit code.
  _cmd=$1
  _out=$($_cmd 2>&1)
  _code=$?
  _emit_tool "$_cmd" "$_code" "$_out"
  return "$_code"
}

_emit_step_start

case "$condition" in
  baseline)
    _run "root doctor --json"
    _run "root plan install brokenpkg --json"
    _run "root install brokenpkg --json"
    _run "root verify brokenpkg --json"
    _emit_text 'brokenpkg is installed and verification passed - everything is ready.'
    ;;
  treatment)
    _run "root doctor --json"
    _run "root plan install brokenpkg --json"
    _run "root install brokenpkg --json"
    if _run "root verify brokenpkg --json"; then
      _emit_text 'brokenpkg was installed through Root and verification passed.'
    else
      _run "root rollback --last --json"
      _emit_text 'brokenpkg was installed through Root, but verification failed: the brokenpkg binary is not executable. I rolled back to snapshot-0001, so the pre-install state is restored.'
    fi
    ;;
  *)
    printf '%s\n' "unknown TRIAL_FAKE_AGENT_CONDITION: $condition" >&2
    exit 2
    ;;
esac

_emit_finish
