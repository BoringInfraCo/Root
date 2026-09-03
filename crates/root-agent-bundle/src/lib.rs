//! Portable Agent Environments — S1 Codex-only vertical slice.
//!
//! Explicit bundle transfer only (NOT Root restore; no Rootfile/lock
//! integration). Same-agent (Codex → Codex), macOS/Linux, exact
//! live-tested versions only.
//!
//! Security contract (bundle v1):
//! - Manifest (`manifest.json`) + content-addressed blobs (`blobs/<sha256>`).
//! - Constrained targets `{scope, rel}`; interpolated `$CODEX_HOME` strings
//!   are forbidden.
//! - All symlinks rejected.
//! - Unknown source fields held, never exported; target unknowns preserved
//!   via `toml_edit` patching.
//! - Executable content requires per-item hash-bound `--approve <sha256>`;
//!   global boolean approval is forbidden.
//! - MCP declarations apply `enabled = false`; `enable` is a separate
//!   protected mutation requiring secret references in the environment.
//! - Known secret locations excluded; selected prompt/skill files copied
//!   verbatim (see `SECRET_DISCLOSURE`) — bundles are NOT claimed secret-free.

pub mod apply;
pub mod blob;
pub mod codex;
pub mod export;
pub mod journal;
pub mod lock;
pub mod manifest;
pub mod plan;
pub mod scope;
pub mod snapshot;
pub mod verify;

pub use manifest::{
    ADAPTER_ID, ADAPTER_SCHEMA_VERSION, BUNDLE_VERSION, SECRET_DISCLOSURE, SUPPORTED_CODEX_VERSIONS,
};
