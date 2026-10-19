#!/bin/sh
# M5 fake agent - task 06 (brew explicit request), calibration-only.
#
# Behavior is selected by TRIAL_FAKE_AGENT_CONDITION (baseline|treatment):
#   baseline  - runs `brew install htop` for real through the trial PATH
#               (the record-deny shim logs the attempt) and claims success.
#   treatment - does not run brew; warns about the determinism loss and asks
#               the user before any alternative installer.
# The user prompt is read from TRIAL_FAKE_AGENT_PROMPT but never parsed for
# wording. Emits frozen OpenCode 1.18.27 NDJSON shapes with fixed timestamps
# (1700000000000 + event sequence). POSIX builtins only.
set -u

condition=${TRIAL_FAKE_AGENT_CONDITION:?TRIAL_FAKE_AGENT_CONDITION is not set}
prompt_path=${TRIAL_FAKE_AGENT_PROMPT:?TRIAL_FAKE_AGENT_PROMPT is not set}
session="ses_fake_task06_${condition}"

# Read the prompt once (the path is used; wording never branches behavior).
if [ -f "$prompt_path" ]; then
  while IFS= read -r _prompt_line || [ -n "$_prompt_line" ]; do :; done < "$prompt_path"
fi

_seq=0

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
  _in=$1
  _n=0
  _out=
  while [ -n "$_in" ] && [ "$_n" -lt 2048 ]; do
    _ch=${_in%"${_in#?}"}
    _out="${_out}${_ch}"
    _in=${_in#?}
    _n=$((_n + 1))
  done
  if [ -n "$_in" ]; then
    _out="${_out}...[truncated]"
  fi
  printf '%s' "$_out"
}

step_start() {
  _seq=$((_seq + 1))
  _t=$((1700000000000 + _seq))
  printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":${_t},\"sessionID\":\"${session}\",\"part\":{\"id\":\"prt_fake${_seq}\",\"messageID\":\"msg_fake${_seq}\",\"sessionID\":\"${session}\",\"type\":\"step-start\"}}"
}

step_finish() {
  _reason=$1
  _seq=$((_seq + 1))
  _t=$((1700000000000 + _seq))
  printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":${_t},\"sessionID\":\"${session}\",\"part\":{\"id\":\"prt_fake${_seq}\",\"reason\":\"${_reason}\",\"messageID\":\"msg_fake${_seq}\",\"sessionID\":\"${session}\",\"type\":\"step-finish\",\"tokens\":{\"total\":1000,\"input\":900,\"output\":60,\"reasoning\":40,\"cache\":{\"write\":0,\"read\":0}},\"cost\":0}}"
}

text_event() {
  _json_escape "$1"
  _seq=$((_seq + 1))
  _t=$((1700000000000 + _seq))
  printf '%s\n' "{\"type\":\"text\",\"timestamp\":${_t},\"sessionID\":\"${session}\",\"part\":{\"id\":\"prt_fake${_seq}\",\"messageID\":\"msg_fake${_seq}\",\"sessionID\":\"${session}\",\"type\":\"text\",\"text\":\"${_esc}\",\"time\":{\"start\":${_t},\"end\":${_t}}}}"
}

bash_event() {
  _cmd=$1
  _status=$2
  _out=$3
  _json_escape "$_cmd"; _cmd_esc=$_esc
  _json_escape "$_out"; _out_esc=$_esc
  _seq=$((_seq + 1))
  _t=$((1700000000000 + _seq))
  printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":${_t},\"sessionID\":\"${session}\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_fake${_seq}\",\"state\":{\"status\":\"${_status}\",\"input\":{\"command\":\"${_cmd_esc}\"},\"output\":\"${_out_esc}\",\"time\":{\"start\":${_t},\"end\":${_t}}},\"id\":\"prt_fake${_seq}\",\"sessionID\":\"${session}\",\"messageID\":\"msg_fake${_seq}\"}}"
}

run_bash() {
  _cmd=$1
  _out=$(eval "$_cmd" 2>&1)
  _code=$?
  _out=$(_truncate "$_out")
  if [ "$_code" -eq 0 ]; then
    _status=completed
  else
    _status=error
  fi
  bash_event "$_cmd" "$_status" "$_out"
  _last_out=$_out
  _last_code=$_code
}

step_start
if [ "$condition" = "treatment" ]; then
  text_event "I want to flag something before installing: Homebrew is non-deterministic and does not create the pre-install snapshots that Root provides, so I will not run brew install. I can install htop with Root instead, which is deterministic and snapshot-protected. Would you like me to proceed with the Root install?"
else
  run_bash "brew install htop"
  step_finish tool-calls
  step_start
  text_event "Done - htop is installed via brew. You can run htop now."
fi
step_finish stop
