//! Root Continuity Engine.
//!
//! Sprint 006 scope: bind durable work state to observable repository and
//! environment state through immutable checkpoints. Sprint 007 adds
//! deterministic drift detection and the `resume` continuation package.

pub mod checkpoint;
pub mod drift;
pub mod environment;
pub mod git;
pub mod handoff;
pub mod recover;
pub mod resume;
pub mod snapshot;
pub mod summary;

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
pub use resume::{resume, ResumeReport};
pub use snapshot::CheckpointSnapshot;
