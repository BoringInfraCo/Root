//! Portable Agent Environments — S1 Codex + S2 OpenCode + S3 Claude.
//!
//! Explicit bundle transfer only (NOT Root restore; no Rootfile/lock
//! integration). Same-agent only, macOS/Linux, exact live-tested versions.
//! No cross-agent translation. Claude MCP is held in v0.4.1 on Claude Code
//! 2.1.260 (no disable-until-enable; nonempty `mcp` is invalid).
//!
//! Security contract (bundle v1):
//! - Manifest (`manifest.json`) + content-addressed blobs (`blobs/<sha256>`).
//! - Constrained targets `{scope, rel}`; interpolated `$CODEX_HOME` strings
//!   are forbidden.
//! - All symlinks rejected.
//! - Unknown source fields held, never exported; target unknowns preserved
//!   via `toml_edit` (Codex) or `serde_json::Value` (OpenCode) patching.
//! - Executable content requires per-item hash-bound `--approve <sha256>`;
//!   global boolean approval is forbidden.
//! - MCP declarations apply `enabled = false`; `enable` is a separate
//!   protected mutation requiring secret references in the environment.
//! - Known secret locations excluded; selected prompt/skill files copied
//!   verbatim (see `SECRET_DISCLOSURE`) — bundles are NOT claimed secret-free.

pub mod apply;
pub mod blob;
pub mod claude;
pub mod codex;
pub mod export;
pub mod journal;
pub mod lock;
pub mod manifest;
pub mod opencode;
pub mod plan;
pub mod scope;
pub mod snapshot;
pub mod verify;

pub use manifest::{
    ADAPTER_ID, ADAPTER_SCHEMA_VERSION, BUNDLE_VERSION, CLAUDE_ADAPTER_ID, OPENCODE_ADAPTER_ID,
    SECRET_DISCLOSURE, SUPPORTED_CLAUDE_VERSIONS, SUPPORTED_CODEX_VERSIONS,
    SUPPORTED_OPENCODE_VERSIONS,
};
