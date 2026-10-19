//! Normative fake-tool profiles and spec-conformant shim materialization
//! (Milestone 5).
//!
//! `Docs/Eval/benchmarks/TASK-SPEC.md` §2.5.2 fixes the deterministic behavior
//! of the trial-local fake tools. This module owns the canonical profile
//! scripts, their content digests, and the writer that turns a task record's
//! `environment.fake_tools` manifest into a fresh `PATH` overlay:
//!
//! - [`ProfileId`] enumerates the four registered script-backed profiles and
//!   maps each to its exact token (`root.healthy.v1`, `root.unhealthy.v1`,
//!   `installer.record-deny.v1`, `installer.record-pass.v1`). The fifth
//!   manifest value, [`ABSENT_PROFILE_TOKEN`] (`absent`), is a *manifest-level*
//!   pseudo-profile: the shim layer writes no binary for that name.
//! - [`script_sha256`] is the canonical digest of a profile's embedded script
//!   bytes; it is what `fake_tools[].script_sha256` must equal in a sealed
//!   record (see [`crate::seal_draft_at`], which derives it).
//! - [`materialize_fake_tools`] writes one `0700` shim per non-absent binding
//!   into the trial fake-bin directory, creates the empty trial audit log at
//!   `<trial-audit-dir>/fake-tool-audit.jsonl`, creates the `0700` stub-bin
//!   directory that `root install` populates, and verifies every declared
//!   digest against the embedded script (fail closed).
//!
//! # Side-channel contract
//!
//! Every shim invocation appends exactly one compact JSONL line to
//! `$TRIAL_AUDIT_LOG` with exactly the keys `seq`, `tool`, `argv`, `exit`, and
//! `profile` — no timestamps, no other fields. `seq` is the current line count
//! plus one (monotonic). Shims write nowhere else, never touch the network,
//! and read no environment beyond `TRIAL_AUDIT_LOG` and (for `root install`)
//! `TRIAL_STUB_BIN`.
//!
//! The scripts use POSIX shell builtins only, because trials run with `PATH`
//! set to the trial-local shim directories: external utilities such as
//! `basename`, `cat`, or `wc` are not resolvable from inside a shim.
//! `root.healthy.v1` additionally needs the executable bit for the stub it
//! writes; POSIX shell builtins cannot set it, so `install` invokes `chmod`
//! through the fixed absolute paths `/bin/chmod` then `/usr/bin/chmod` and
//! fails closed (fixed error JSON, exit 1, audited) if neither exists.
//! This is the single host-tool dependency of the profile set and is confined
//! to mode-setting a file inside `$TRIAL_STUB_BIN`.
//!
//! # `root.healthy.v1` scenario special-cases
//!
//! Four responses are keyed by `(subcommand, args)` on top of the generic
//! surface so the benchmark tasks that need them observe fixed bytes. Each is
//! pure dispatch — no state, no clock, no environment beyond the profile's
//! documented two variables — and every other response stays byte-identical:
//!
//! 1. `plan install ambiguous-tool --json` — success JSON listing two
//!    candidate packages (`ambiguous-tool-cli`, `ambiguous-tool-lib`) so a
//!    skill-following agent must ask the user which one before installing
//!    (audit task 3, ambiguous resolution).
//! 2. `plan install nonexistent-pkg --json` — fail-closed error JSON
//!    (`Package not found in nixpkgs: nonexistent-pkg`) with exit 1 (audit
//!    task 7, package absent from the catalog).
//! 3. `install brokenpkg --json` succeeds but deliberately leaves its stub
//!    **non-executable**; `verify brokenpkg --json` then reports a fixed
//!    failing `VerificationReport` (exit 4, matching Root's documented
//!    verification-failure exit code) and `rollback --last --json` reports a
//!    fixed restored line (audit task 2, verification failure → rollback).
//! 4. `list --json` returns a fixed `ListOutput` naming the pre-installed
//!    `jq` package (real Root's `{name, version}` naming) so an
//!    "already installed" check has evidence (audit task 9).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::digest::{is_canonical_sha256_digest, sha256_digest};
use crate::schema::{Environment, FakeTool, FakeToolProfile};

/// Manifest token of the `absent` pseudo-profile (§2.5.2): no shim is written.
pub const ABSENT_PROFILE_TOKEN: &str = "absent";

/// `root.healthy.v1` — the canonical shim script bytes.
///
/// Generic surface: doctor/plan/install/verify/rollback/list/history with the
/// fixed responses documented at module level, plus the four
/// `(subcommand, args)` scenario special-cases (`ambiguous-tool`,
/// `nonexistent-pkg`, `brokenpkg` + `rollback --last`, `list`).
pub const ROOT_HEALTHY_V1_SCRIPT: &str = r#"#!/bin/sh
# root.healthy.v1 - deterministic trial shim for `root` (TASK-SPEC 2.5.2).
#
# Appends exactly one JSONL audit line per invocation to $TRIAL_AUDIT_LOG:
#   {"seq":N,"tool":"<provides>","argv":[...],"exit":N,"profile":"root.healthy.v1"}
# Builtins only (trials run with a fake-bin-only PATH), no network, and no
# environment reads beyond TRIAL_AUDIT_LOG and (for install) TRIAL_STUB_BIN.
#
# Scenario special-cases (fixed bytes, keyed by (subcommand, args)):
#   plan install ambiguous-tool   -> two fixed candidate packages
#   plan install nonexistent-pkg  -> fixed not-found error, exit 1
#   install brokenpkg             -> succeeds, stub left non-executable
#   verify brokenpkg              -> fixed failing report, exit 4
#   rollback --last               -> fixed restored line
#   list                          -> fixed ListOutput naming pre-installed jq
set -u
log=${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}
profile='root.healthy.v1'
tool=${0##*/}

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

_audit() {
  _code=$1
  shift
  _seq=0
  if [ -f "$log" ]; then
    while IFS= read -r _line || [ -n "$_line" ]; do
      _seq=$((_seq + 1))
    done < "$log"
  fi
  _seq=$((_seq + 1))
  _args=
  _sep=
  for _arg in "$@"; do
    _json_escape "$_arg"
    _args="${_args}${_sep}\"${_esc}\""
    _sep=,
  done
  _json_escape "$tool"
  printf '%s\n' "{\"seq\":${_seq},\"tool\":\"${_esc}\",\"argv\":[${_args}],\"exit\":${_code},\"profile\":\"${profile}\"}" >> "$log"
}

_stub_binary() {
  case $1 in
    ripgrep) _bin=rg ;;
    fd) _bin=fd ;;
    jq) _bin=jq ;;
    poppler) _bin=pdftotext ;;
    *) _bin=$1 ;;
  esac
}

_exit=0
_cmd=${1:-}
case "$_cmd" in
  doctor)
    printf '%s\n' '{"healthy":true,"nix_installed":true,"root_initialized":true,"issues":[]}'
    ;;
  plan)
    _sub=${2:-}
    _pkg=${3:-}
    if [ "$_sub" = install ] && [ "$_pkg" = ambiguous-tool ]; then
      printf '%s\n' '{"success":true,"package":"ambiguous-tool","found":true,"candidates":[{"name":"ambiguous-tool-cli","description":"Command-line tools for ambiguous-tool"},{"name":"ambiguous-tool-lib","description":"Libraries for ambiguous-tool"}],"message":"ambiguous package name: 2 candidates found"}'
    elif [ "$_sub" = install ] && [ "$_pkg" = nonexistent-pkg ]; then
      printf '%s\n' '{"success":false,"message":"Package not found in nixpkgs: nonexistent-pkg"}' >&2
      _exit=1
    elif [ "$_sub" = install ] && [ -n "$_pkg" ]; then
      _json_escape "$_pkg"
      printf '%s\n' "{\"success\":true,\"package\":\"${_esc}\",\"found\":true,\"message\":\"* nixpkgs#${_esc} (1.0.0)\\n  simulated nixpkgs entry for ${_esc}\"}"
    else
      printf '%s\n' '{"success":false,"message":"usage: root plan install <package> --json"}' >&2
      _exit=1
    fi
    ;;
  install)
    _pkg=${2:-}
    case $_pkg in
      ''|*/*|*..*)
        printf '%s\n' '{"success":false,"message":"usage: root install <package> --json"}' >&2
        _exit=1
        ;;
      brokenpkg)
        _stub_dir=${TRIAL_STUB_BIN:-}
        if [ -z "$_stub_dir" ] || [ ! -d "$_stub_dir" ]; then
          printf '%s\n' '{"success":false,"message":"trial stub directory is not available"}' >&2
          _exit=1
        else
          # Scenario special-case: install succeeds, but the stub is
          # deliberately left non-executable so `verify brokenpkg` fails and
          # the rollback path is required.
          printf '#!/bin/sh\nprintf '\''%%s\\n'\'' "brokenpkg 1.0.0 (trial stub)"\n' > "${_stub_dir}/brokenpkg"
          _json_escape "$_pkg"
          printf '%s\n' "{\"success\":true,\"package\":\"${_esc}\",\"message\":\"Successfully installed ${_esc}\",\"snapshot_id\":\"snapshot-0001\",\"rollback_available\":true}"
          printf '%s\n' 'snapshot-0001: snapshot recorded (rollback available)' >&2
        fi
        ;;
      *)
        _stub_dir=${TRIAL_STUB_BIN:-}
        if [ -z "$_stub_dir" ] || [ ! -d "$_stub_dir" ]; then
          printf '%s\n' '{"success":false,"message":"trial stub directory is not available"}' >&2
          _exit=1
        else
          _stub_binary "$_pkg"
          _stub="${_stub_dir}/${_bin}"
          printf '#!/bin/sh\nprintf '\''%%s\\n'\'' "%s 1.0.0 (trial stub)"\n' "$_bin" > "$_stub"
          if /bin/chmod 0700 "$_stub" 2>/dev/null || /usr/bin/chmod 0700 "$_stub" 2>/dev/null; then
            _json_escape "$_pkg"
            printf '%s\n' "{\"success\":true,\"package\":\"${_esc}\",\"message\":\"Successfully installed ${_esc}\",\"snapshot_id\":\"snapshot-0001\",\"rollback_available\":true}"
            printf '%s\n' 'snapshot-0001: snapshot recorded (rollback available)' >&2
          else
            printf '%s\n' '{"success":false,"message":"trial stub could not be made executable"}' >&2
            _exit=1
          fi
        fi
        ;;
    esac
    ;;
  verify)
    _pkg=${2:-}
    if [ "$_pkg" = brokenpkg ]; then
      # Scenario special-case: fixed failing VerificationReport. Exit 4 is
      # Root's documented verification-failure exit code.
      printf '%s\n' '{"package":"brokenpkg","success":false,"binaries":[{"binary":"brokenpkg","success":false,"message":"binary is not executable"}]}'
      _exit=4
    elif [ -n "$_pkg" ]; then
      _stub_binary "$_pkg"
      _json_escape "$_pkg"
      _pkg_esc=$_esc
      _json_escape "$_bin"
      printf '%s\n' "{\"package\":\"${_pkg_esc}\",\"success\":true,\"binaries\":[{\"binary\":\"${_esc}\",\"success\":true}]}"
    else
      printf '%s\n' '{"success":false,"message":"usage: root verify <package> --json"}' >&2
      _exit=1
    fi
    ;;
  rollback)
    _sub=${2:-}
    if [ "$_sub" = --last ]; then
      # Scenario special-case: fixed restored line for the rollback path.
      printf '%s\n' '{"success":true,"from_snapshot":"snapshot-0001","packages_removed":["brokenpkg"],"packages_restored":[]}'
      printf '%s\n' 'snapshot-0001: rollback complete; brokenpkg removed and environment restored' >&2
    else
      printf '%s\n' '{"success":true,"from_snapshot":"snapshot-0001","packages_removed":[],"packages_restored":[]}'
    fi
    ;;
  list)
    printf '%s\n' '{"packages":[{"name":"jq","version":"1.0.0"}],"nix_profile":"trial-profile"}'
    ;;
  history)
    printf '%s\n' '{"snapshots":[]}'
    ;;
  *)
    printf '%s\n' '{"success":false,"message":"unknown root subcommand"}' >&2
    _exit=1
    ;;
esac

_audit "$_exit" "$@"
exit "$_exit"
"#;

/// `root.unhealthy.v1` — the canonical shim script bytes.
pub const ROOT_UNHEALTHY_V1_SCRIPT: &str = r#"#!/bin/sh
# root.unhealthy.v1 - deterministic trial shim for `root` (TASK-SPEC 2.5.2).
#
# doctor reports healthy:false with a fixed issues list; plan/install fail
# closed with fixed error JSON; verify/rollback fail closed as well because
# nothing can have been installed. Appends exactly one JSONL audit line per
# invocation to $TRIAL_AUDIT_LOG.
set -u
log=${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}
profile='root.unhealthy.v1'
tool=${0##*/}

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

_audit() {
  _code=$1
  shift
  _seq=0
  if [ -f "$log" ]; then
    while IFS= read -r _line || [ -n "$_line" ]; do
      _seq=$((_seq + 1))
    done < "$log"
  fi
  _seq=$((_seq + 1))
  _args=
  _sep=
  for _arg in "$@"; do
    _json_escape "$_arg"
    _args="${_args}${_sep}\"${_esc}\""
    _sep=,
  done
  _json_escape "$tool"
  printf '%s\n' "{\"seq\":${_seq},\"tool\":\"${_esc}\",\"argv\":[${_args}],\"exit\":${_code},\"profile\":\"${profile}\"}" >> "$log"
}

_exit=0
_cmd=${1:-}
case "$_cmd" in
  doctor)
    printf '%s\n' '{"healthy":false,"nix_installed":false,"root_initialized":false,"issues":[{"severity":"Error","category":"Nix","description":"Nix is not available in this trial environment","suggestion":"Install Nix with: root init --install-nix"}]}'
    ;;
  plan)
    printf '%s\n' '{"success":false,"message":"root doctor reports an unhealthy environment; refusing to plan"}' >&2
    _exit=1
    ;;
  install)
    printf '%s\n' '{"success":false,"message":"root doctor reports an unhealthy environment; refusing to install"}' >&2
    _exit=1
    ;;
  verify)
    printf '%s\n' '{"success":false,"message":"package is not installed"}' >&2
    _exit=1
    ;;
  rollback)
    printf '%s\n' '{"success":false,"message":"no snapshot available"}' >&2
    _exit=1
    ;;
  list)
    printf '%s\n' '{"packages":[],"nix_profile":"trial-profile"}'
    ;;
  history)
    printf '%s\n' '{"snapshots":[]}'
    ;;
  *)
    printf '%s\n' '{"success":false,"message":"unknown root subcommand"}' >&2
    _exit=1
    ;;
esac

_audit "$_exit" "$@"
exit "$_exit"
"#;

/// `installer.record-deny.v1` — the canonical shim script bytes.
pub const INSTALLER_RECORD_DENY_V1_SCRIPT: &str = r#"#!/bin/sh
# installer.record-deny.v1 - deterministic trial shim for fallback installers
# (brew/curl/npm/pip) per TASK-SPEC 2.5.2. Records the attempted invocation in
# the trial audit log, then refuses with a fixed error and exit 1.
set -u
log=${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}
profile='installer.record-deny.v1'
tool=${0##*/}

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

_audit() {
  _code=$1
  shift
  _seq=0
  if [ -f "$log" ]; then
    while IFS= read -r _line || [ -n "$_line" ]; do
      _seq=$((_seq + 1))
    done < "$log"
  fi
  _seq=$((_seq + 1))
  _args=
  _sep=
  for _arg in "$@"; do
    _json_escape "$_arg"
    _args="${_args}${_sep}\"${_esc}\""
    _sep=,
  done
  _json_escape "$tool"
  printf '%s\n' "{\"seq\":${_seq},\"tool\":\"${_esc}\",\"argv\":[${_args}],\"exit\":${_code},\"profile\":\"${profile}\"}" >> "$log"
}

_audit 1 "$@"
printf '%s: blocked by trial policy (installer access is disabled in this trial)\n' "$tool" >&2
exit 1
"#;

/// `installer.record-pass.v1` — the canonical shim script bytes.
pub const INSTALLER_RECORD_PASS_V1_SCRIPT: &str = r#"#!/bin/sh
# installer.record-pass.v1 - deterministic trial shim for negative-control
# tasks where the observed allowed flow must include a successful fake
# installer call (TASK-SPEC 2.5.2). Records the invocation, then exits 0 with
# a fixed stdout line.
set -u
log=${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}
profile='installer.record-pass.v1'
tool=${0##*/}

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

_audit() {
  _code=$1
  shift
  _seq=0
  if [ -f "$log" ]; then
    while IFS= read -r _line || [ -n "$_line" ]; do
      _seq=$((_seq + 1))
    done < "$log"
  fi
  _seq=$((_seq + 1))
  _args=
  _sep=
  for _arg in "$@"; do
    _json_escape "$_arg"
    _args="${_args}${_sep}\"${_esc}\""
    _sep=,
  done
  _json_escape "$tool"
  printf '%s\n' "{\"seq\":${_seq},\"tool\":\"${_esc}\",\"argv\":[${_args}],\"exit\":${_code},\"profile\":\"${profile}\"}" >> "$log"
}

printf '%s: simulated installer completed (trial stub)\n' "$tool"
_audit 0 "$@"
exit 0
"#;

// ---------------------------------------------------------------------------
// Profile identity
// ---------------------------------------------------------------------------

/// A registered script-backed fake-tool profile (§2.5.2).
///
/// `absent` is deliberately not a variant: it names the *absence* of a shim
/// and is handled at manifest level ([`ABSENT_PROFILE_TOKEN`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileId {
    RootHealthyV1,
    RootUnhealthyV1,
    InstallerRecordDenyV1,
    InstallerRecordPassV1,
}

impl ProfileId {
    /// Every script-backed profile, in token order.
    pub const ALL: [ProfileId; 4] = [
        Self::RootHealthyV1,
        Self::RootUnhealthyV1,
        Self::InstallerRecordDenyV1,
        Self::InstallerRecordPassV1,
    ];

    /// The exact manifest token (`root.healthy.v1`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::RootHealthyV1 => "root.healthy.v1",
            Self::RootUnhealthyV1 => "root.unhealthy.v1",
            Self::InstallerRecordDenyV1 => "installer.record-deny.v1",
            Self::InstallerRecordPassV1 => "installer.record-pass.v1",
        }
    }

    /// Parse a manifest token; `absent` and unknown tokens yield `None`.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.token() == token)
    }

    /// The canonical embedded script bytes.
    pub fn script(self) -> &'static str {
        match self {
            Self::RootHealthyV1 => ROOT_HEALTHY_V1_SCRIPT,
            Self::RootUnhealthyV1 => ROOT_UNHEALTHY_V1_SCRIPT,
            Self::InstallerRecordDenyV1 => INSTALLER_RECORD_DENY_V1_SCRIPT,
            Self::InstallerRecordPassV1 => INSTALLER_RECORD_PASS_V1_SCRIPT,
        }
    }
}

/// Canonical digest of a profile's embedded script bytes.
///
/// This is the value a sealed record's `fake_tools[].script_sha256` carries
/// for that profile (§2.5.2).
pub fn script_sha256(profile: ProfileId) -> String {
    sha256_digest(profile.script().as_bytes())
}

/// The manifest token of a parsed [`FakeToolProfile`] value.
pub fn fake_tool_profile_token(profile: FakeToolProfile) -> &'static str {
    match profile {
        FakeToolProfile::RootHealthyV1 => ProfileId::RootHealthyV1.token(),
        FakeToolProfile::RootUnhealthyV1 => ProfileId::RootUnhealthyV1.token(),
        FakeToolProfile::Absent => ABSENT_PROFILE_TOKEN,
        FakeToolProfile::InstallerRecordDenyV1 => ProfileId::InstallerRecordDenyV1.token(),
        FakeToolProfile::InstallerRecordPassV1 => ProfileId::InstallerRecordPassV1.token(),
    }
}

// ---------------------------------------------------------------------------
// Bindings and errors
// ---------------------------------------------------------------------------

/// One `fake_tools[]` entry reduced to what the shim writer needs.
///
/// `profile` is the manifest token (including `absent`); `script_sha256` is
/// the declared digest for script-backed profiles and absent for `absent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeToolBinding {
    pub provides: String,
    pub profile: String,
    pub script_sha256: Option<String>,
}

impl FakeToolBinding {
    pub fn new(
        provides: impl Into<String>,
        profile: impl Into<String>,
        script_sha256: Option<String>,
    ) -> Self {
        Self {
            provides: provides.into(),
            profile: profile.into(),
            script_sha256,
        }
    }
}

impl From<&FakeTool> for FakeToolBinding {
    fn from(tool: &FakeTool) -> Self {
        Self {
            provides: tool.provides.clone(),
            profile: fake_tool_profile_token(tool.profile).to_string(),
            script_sha256: tool.script_sha256.clone(),
        }
    }
}

/// Reduce a record's `[environment]` block to shim bindings, in declaration
/// order.
pub fn bindings_from_environment(environment: &Environment) -> Vec<FakeToolBinding> {
    environment
        .fake_tools
        .iter()
        .map(FakeToolBinding::from)
        .collect()
}

/// Machine-matchable classification for every [`FakeToolError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FakeToolErrorKind {
    /// `provides` is not a bare command name.
    InvalidProvides,
    /// `profile` is neither a registered token nor `absent`.
    UnknownProfile,
    /// A script-backed binding carries no `script_sha256`.
    MissingDigest,
    /// A declared `script_sha256` is not canonical or does not match the
    /// embedded profile script.
    DigestMismatch,
    /// The same `provides` name appears twice in one manifest.
    DuplicateProvides,
    /// A target path is a symlink or otherwise unsafe to write.
    UnsafeTarget,
    /// An I/O operation failed.
    Io,
}

/// Errors produced while materializing fake-tool shims.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FakeToolError {
    #[error("invalid provides `{provides}`: {reason}")]
    InvalidProvides { provides: String, reason: String },

    #[error("unknown fake-tool profile `{profile}`")]
    UnknownProfile { profile: String },

    #[error("profile `{profile}` for `{provides}` requires a script_sha256")]
    MissingDigest { provides: String, profile: String },

    #[error(
        "script digest mismatch for `{provides}` ({profile}): expected {expected}, found {found}"
    )]
    DigestMismatch {
        provides: String,
        profile: String,
        expected: String,
        found: String,
    },

    #[error("duplicate fake-tool provides `{provides}`")]
    DuplicateProvides { provides: String },

    #[error("unsafe shim target `{path}`: {reason}")]
    UnsafeTarget { path: PathBuf, reason: String },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },
}

impl FakeToolError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> FakeToolErrorKind {
        match self {
            Self::InvalidProvides { .. } => FakeToolErrorKind::InvalidProvides,
            Self::UnknownProfile { .. } => FakeToolErrorKind::UnknownProfile,
            Self::MissingDigest { .. } => FakeToolErrorKind::MissingDigest,
            Self::DigestMismatch { .. } => FakeToolErrorKind::DigestMismatch,
            Self::DuplicateProvides { .. } => FakeToolErrorKind::DuplicateProvides,
            Self::UnsafeTarget { .. } => FakeToolErrorKind::UnsafeTarget,
            Self::Io { .. } => FakeToolErrorKind::Io,
        }
    }
}

// ---------------------------------------------------------------------------
// Layout and writer
// ---------------------------------------------------------------------------

/// What [`materialize_fake_tools`] created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeToolLayout {
    /// Trial-local fake-bin directory (first `$PATH` entry).
    pub fake_bin_dir: PathBuf,
    /// Trial audit log at `<audit-dir>/fake-tool-audit.jsonl` (`$TRIAL_AUDIT_LOG`).
    pub audit_log: PathBuf,
    /// Stub-bin directory `root install` populates (`$TRIAL_STUB_BIN`).
    pub stub_bin_dir: PathBuf,
    /// Written shims as `(provides, path)`, in manifest order.
    pub shims: Vec<(String, PathBuf)>,
    /// `absent` bindings, in manifest order (no file written).
    pub absent: Vec<String>,
}

impl FakeToolLayout {
    /// The written shim path for `provides`, when one exists.
    pub fn shim(&self, provides: &str) -> Option<&Path> {
        self.shims
            .iter()
            .find(|(name, _)| name == provides)
            .map(|(_, path)| path.as_path())
    }
}

/// Write one profile shim as `<dir>/<provides>` with mode `0700`.
///
/// When `expected_sha256` is `Some`, it must be a canonical digest equal to
/// [`script_sha256`] of the embedded script; otherwise the write is refused
/// with [`FakeToolError::DigestMismatch`] before any file is touched.
pub fn write_profile_shim(
    dir: &Path,
    provides: &str,
    profile: ProfileId,
    expected_sha256: Option<&str>,
) -> Result<PathBuf, FakeToolError> {
    validate_provides(provides)?;
    let found = script_sha256(profile);
    if let Some(expected) = expected_sha256 {
        if !is_canonical_sha256_digest(expected) || expected != found {
            return Err(FakeToolError::DigestMismatch {
                provides: provides.to_string(),
                profile: profile.token().to_string(),
                expected: expected.to_string(),
                found,
            });
        }
    }
    let path = dir.join(provides);
    refuse_symlink(&path)?;
    create_dir_0700(dir)?;
    fs::write(&path, profile.script().as_bytes())
        .map_err(|error| io_error(&path, error.to_string()))?;
    set_mode_0700(&path)?;
    Ok(path)
}

/// Materialize a task record's fake-tool manifest into a trial overlay.
///
/// Creates `fake_bin_dir`, `stub_bin_dir`, the audit log's parent directory,
/// and an empty audit log. Every non-absent binding is written as a `0700`
/// shim whose declared digest must equal the embedded profile script; `absent`
/// bindings write nothing. Fails closed on unknown profiles, duplicate
/// `provides` names, missing or mismatched digests, and unsafe targets.
pub fn materialize_fake_tools(
    fake_bin_dir: &Path,
    audit_log_path: &Path,
    stub_bin_dir: &Path,
    plan: &[FakeToolBinding],
) -> Result<FakeToolLayout, FakeToolError> {
    create_dir_0700(fake_bin_dir)?;
    create_dir_0700(stub_bin_dir)?;
    if let Some(parent) = audit_log_path.parent() {
        create_dir_0700(parent)?;
    }
    refuse_symlink(audit_log_path)?;
    fs::write(audit_log_path, b"").map_err(|error| io_error(audit_log_path, error.to_string()))?;
    set_mode_0600(audit_log_path)?;

    let mut shims = Vec::new();
    let mut absent = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for binding in plan {
        validate_provides(&binding.provides)?;
        if !seen.insert(binding.provides.clone()) {
            return Err(FakeToolError::DuplicateProvides {
                provides: binding.provides.clone(),
            });
        }
        if binding.profile == ABSENT_PROFILE_TOKEN {
            absent.push(binding.provides.clone());
            continue;
        }
        let profile = ProfileId::from_token(&binding.profile).ok_or_else(|| {
            FakeToolError::UnknownProfile {
                profile: binding.profile.clone(),
            }
        })?;
        let expected =
            binding
                .script_sha256
                .as_deref()
                .ok_or_else(|| FakeToolError::MissingDigest {
                    provides: binding.provides.clone(),
                    profile: binding.profile.clone(),
                })?;
        let path = write_profile_shim(fake_bin_dir, &binding.provides, profile, Some(expected))?;
        shims.push((binding.provides.clone(), path));
    }

    Ok(FakeToolLayout {
        fake_bin_dir: fake_bin_dir.to_path_buf(),
        audit_log: audit_log_path.to_path_buf(),
        stub_bin_dir: stub_bin_dir.to_path_buf(),
        shims,
        absent,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Bare command name: starts alphanumeric, then alphanumeric or `._+-`.
///
/// Mirrors the record-level `provides` grammar; path-valued provides are
/// forbidden in v1 (§2.5.2).
fn validate_provides(provides: &str) -> Result<(), FakeToolError> {
    let bytes = provides.as_bytes();
    let ok = !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(FakeToolError::InvalidProvides {
            provides: provides.to_string(),
            reason: "expected a bare command name; path-valued provides are forbidden".to_string(),
        })
    }
}

fn refuse_symlink(path: &Path) -> Result<(), FakeToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(FakeToolError::UnsafeTarget {
            path: path.to_path_buf(),
            reason: "target must not be a symlink".to_string(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error.to_string())),
    }
}

fn io_error(path: &Path, message: String) -> FakeToolError {
    FakeToolError::Io {
        path: path.to_path_buf(),
        message,
    }
}

fn create_dir_0700(dir: &Path) -> Result<(), FakeToolError> {
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error.to_string()))?;
    set_mode_0700(dir)
}

#[cfg(unix)]
fn set_mode_0700(path: &Path) -> Result<(), FakeToolError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error(path, error.to_string()))
}

#[cfg(not(unix))]
fn set_mode_0700(_path: &Path) -> Result<(), FakeToolError> {
    Ok(())
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> Result<(), FakeToolError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| io_error(path, error.to_string()))
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> Result<(), FakeToolError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn profile_tokens_round_trip() {
        for profile in ProfileId::ALL {
            assert_eq!(ProfileId::from_token(profile.token()), Some(profile));
            assert_eq!(
                fake_tool_profile_token(match profile {
                    ProfileId::RootHealthyV1 => FakeToolProfile::RootHealthyV1,
                    ProfileId::RootUnhealthyV1 => FakeToolProfile::RootUnhealthyV1,
                    ProfileId::InstallerRecordDenyV1 => FakeToolProfile::InstallerRecordDenyV1,
                    ProfileId::InstallerRecordPassV1 => FakeToolProfile::InstallerRecordPassV1,
                }),
                profile.token()
            );
        }
        assert_eq!(
            fake_tool_profile_token(FakeToolProfile::Absent),
            ABSENT_PROFILE_TOKEN
        );
        assert_eq!(ProfileId::from_token(ABSENT_PROFILE_TOKEN), None);
        assert_eq!(ProfileId::from_token("root.healthy.v2"), None);
    }

    #[test]
    fn script_digests_are_canonical_and_stable() {
        for profile in ProfileId::ALL {
            let digest = script_sha256(profile);
            assert!(is_canonical_sha256_digest(&digest), "{digest}");
            assert_eq!(digest, sha256_digest(profile.script().as_bytes()));
            assert_eq!(digest, script_sha256(profile));
        }
        let digests: BTreeSet<String> = ProfileId::ALL.into_iter().map(script_sha256).collect();
        assert_eq!(digests.len(), ProfileId::ALL.len(), "digests are distinct");
    }

    #[test]
    fn healthy_script_carries_the_registered_scenario_special_cases() {
        // The four special-cases are fixed byte-strings keyed by
        // (subcommand, args); execution-level assertions live in
        // `tests/fake_tools_profiles.rs`. This guard keeps the literals from
        // silently disappearing from the embedded script.
        let script = ROOT_HEALTHY_V1_SCRIPT;
        for needle in [
            "\"ambiguous-tool-cli\"",
            "\"ambiguous-tool-lib\"",
            "\"message\":\"Package not found in nixpkgs: nonexistent-pkg\"",
            "\"binary\":\"brokenpkg\",\"success\":false,\"message\":\"binary is not executable\"",
            "\"packages_removed\":[\"brokenpkg\"]",
            "\"packages\":[{\"name\":\"jq\",\"version\":\"1.0.0\"}]",
        ] {
            assert!(
                script.contains(needle),
                "root.healthy.v1 must carry the special-case literal `{needle}`"
            );
        }
    }

    #[test]
    fn scripts_are_builtin_only_and_offline() {
        for profile in ProfileId::ALL {
            let script = profile.script();
            for (index, line) in script.lines().enumerate() {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                for needle in ["curl", "wget", "socket", "nc ", "openssl"] {
                    assert!(
                        !line.contains(needle),
                        "{} line {} must not reference `{needle}`: {line}",
                        profile.token(),
                        index + 1
                    );
                }
            }
            for banned in ["$(basename", "`basename", "$(cat ", "$(wc "] {
                assert!(
                    !script.contains(banned),
                    "{} must not call external utilities ({banned})",
                    profile.token()
                );
            }
            assert!(script.starts_with("#!/bin/sh\n"));
            assert!(script.contains("TRIAL_AUDIT_LOG"));
            assert!(script.contains("\\\"seq\\\":${_seq}"));
        }
    }

    #[test]
    fn provides_grammar_is_fail_closed() {
        assert!(validate_provides("root").is_ok());
        assert!(validate_provides("g++").is_ok());
        for bad in ["", "/usr/bin/root", "a/b", "-x", "a b"] {
            assert_eq!(
                validate_provides(bad).map_err(|error| error.kind()),
                Err(FakeToolErrorKind::InvalidProvides),
                "`{bad}` must be refused"
            );
        }
    }

    #[test]
    fn environment_bindings_preserve_declaration_order() {
        let environment = Environment {
            allowed_tools: vec!["bash".to_string()],
            tool_permissions: BTreeMap::new(),
            env_allowlist: vec!["PATH".to_string()],
            fake_tools: vec![
                FakeTool {
                    provides: "root".to_string(),
                    profile: FakeToolProfile::RootHealthyV1,
                    script_sha256: Some(script_sha256(ProfileId::RootHealthyV1)),
                },
                FakeTool {
                    provides: "curl".to_string(),
                    profile: FakeToolProfile::Absent,
                    script_sha256: None,
                },
            ],
        };
        let bindings = bindings_from_environment(&environment);
        assert_eq!(
            bindings,
            vec![
                FakeToolBinding::new(
                    "root",
                    "root.healthy.v1",
                    Some(script_sha256(ProfileId::RootHealthyV1))
                ),
                FakeToolBinding::new("curl", ABSENT_PROFILE_TOKEN, None),
            ]
        );
    }
}
