//! Root Continuity Engine.
//!
//! Sprint 006 scope: bind durable work state to observable repository and
//! environment state through immutable checkpoints. Sprint 007 adds
//! deterministic drift detection and the `resume` continuation package.

pub mod agent_env;
pub mod checkpoint;
pub mod drift;
pub mod environment;
pub mod git;
pub mod handoff;
pub mod recover;
pub mod restore;
pub mod resume;
pub mod snapshot;
pub mod summary;
pub mod with;

pub use agent_env::{capture as capture_agent_env, AgentEnvSummary, CaptureOutcome};
pub use checkpoint::{
    create, create_on_store, create_with_provenance, list, parse_snapshot, show, show_last,
};
pub use drift::DriftReport;
pub use environment::EnvironmentState;
pub use git::GitState;
pub use handoff::{handoff, render_handoff, HandoffCheckpoint, HandoffReport};
pub use recover::{
    build_recover, recover, render_recover, RecoverCheckpoint, RecoverReport, RecoverRepository,
    RecoverWorkState, RecoverWorkspace, NOT_RECOVERABLE,
};
pub use restore::{bind_workspace, rebind_workspace, WorkBindReport};
pub use resume::{resume, ResumeReport};
pub use snapshot::CheckpointSnapshot;
pub use with::{resume_with, ResumeWithReport, Step};

#[cfg(test)]
pub(crate) static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX.lock().unwrap_or_else(|error| error.into_inner())
}
