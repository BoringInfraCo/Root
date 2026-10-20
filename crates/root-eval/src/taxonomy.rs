//! Trial outcome classification (Milestone 3, Phase A).
//!
//! [`classify_trial`] folds a supervised harness termination
//! ([`crate::TerminationKind`]) plus an optional verifier result
//! ([`crate::VerifierOutcome`]) into one machine-matchable
//! [`TrialOutcomeClass`], implementing the audit's failure and missing-data
//! policy:
//!
//! - **Agent failure, permission denial, or timeout** count as task failure
//!   when attributable to the configured trial: [`TerminationKind::TimeoutKilled`]
//!   → [`TrialOutcomeClass::TimeoutKilled`], `CancelledKilled` → `Cancelled`,
//!   `Denied` → `Denied`, `OutputCapExceeded` → `OutputCapExceeded`.
//! - **Provider outage/rate limit** is classified separately
//!   ([`TrialOutcomeClass::ProviderError`]) so balanced retry scheduling can
//!   treat it without ever counting it as agent failure.
//! - **Infrastructure failure before the agent starts** is invalid:
//!   `SpawnFailed` → [`TrialOutcomeClass::SpawnFailed`].
//! - **Host contamination** is invalid: a trial whose tree carries host state
//!   (a symlink out of the trial root, or a host-sized file in the trial's
//!   stub directories — see [`crate::leakage`]) measures the host, not the
//!   agent, so it is [`TrialOutcomeClass::HostContamination`]: never a pass
//!   and never a fail.
//! - **Verifier mutation or unreliability** invalidates the trial:
//!   [`VerifierOutcome::mutation_detected`] →
//!   [`TrialOutcomeClass::VerifierMutation`]; a verifier timeout →
//!   [`TrialOutcomeClass::VerifierFailed`]; any other `Invalid` verdict
//!   (fingerprint failure, empty spec, spawn failure, cap kill) →
//!   [`TrialOutcomeClass::InvalidInfrastructure`].
//! - **Completed runs** are decided by the verifier:
//!   [`VerifierVerdict::Pass`] → [`TrialOutcomeClass::CompletedPassed`],
//!   [`VerifierVerdict::Fail`] → [`TrialOutcomeClass::CompletedFailed`]. A
//!   completed run with **no** verifier result cannot establish task success
//!   and is classified [`TrialOutcomeClass::CompletedFailed`] per the
//!   incomplete-task rule (aggregation may still exclude it as missing
//!   primary outcome).
//!
//! # Verifier overlay precedence
//!
//! Verifier invalidity **overrides** the agent termination class: a trial whose
//! verifier mutated the scored tree is [`TrialOutcomeClass::VerifierMutation`]
//! even when the harness itself completed cleanly (or was killed). The agent
//! termination class otherwise wins over a verifier `Pass`/`Fail`, because a
//! killed or denied agent is a task failure regardless of what a later
//! verifier observed.
//!
//! # Invalid vs. task failure
//!
//! [`TrialOutcomeClass::is_invalid`] marks classes that must not be scored as
//! agent failure (invalid attempts are retained as evidence and retried only
//! under a predeclared capped rule); [`TrialOutcomeClass::is_task_failure`]
//! marks classes that count as failures of the configured trial;
//! [`TrialOutcomeClass::is_provider_error`] is deliberately neither.

use serde::{Deserialize, Serialize};

use crate::supervisor::TerminationKind;
use crate::verifier::{VerifierOutcome, VerifierVerdict};

/// One machine-matchable trial outcome class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialOutcomeClass {
    /// Harness completed and the verifier passed.
    CompletedPassed,
    /// Harness completed but the verifier failed, or no verifier result
    /// exists to establish task success.
    CompletedFailed,
    /// The wall timeout killed the harness; counts as task failure.
    TimeoutKilled,
    /// Cancellation killed the harness; counts as task failure.
    Cancelled,
    /// An ask-mode permission denial; counts as task failure.
    Denied,
    /// A provider error event or nonzero natural exit; classified separately
    /// from agent failure so retries stay balanced across conditions.
    ProviderError,
    /// The harness process could not be spawned: invalid attempt.
    SpawnFailed,
    /// Harness stdout breached the output cap and the run was killed; counts
    /// as task failure.
    OutputCapExceeded,
    /// The verifier itself was unreliable (timeout): invalid trial.
    VerifierFailed,
    /// The verifier mutated the scored tree: invalid trial.
    VerifierMutation,
    /// Infrastructure failure before a trustworthy verifier result
    /// (fingerprint failure, empty spec, verifier spawn failure, cap kill):
    /// invalid trial.
    InvalidInfrastructure,
    /// The trial tree carries host state (a symlink out of the trial root, or
    /// a host-sized file in the trial's stub directories): the measurement is
    /// compromised, so the trial is invalid — never a pass and never a fail.
    HostContamination,
}

impl TrialOutcomeClass {
    /// Stable snake-case token used by eval storage and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompletedPassed => "completed_passed",
            Self::CompletedFailed => "completed_failed",
            Self::TimeoutKilled => "timeout_killed",
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
            Self::ProviderError => "provider_error",
            Self::SpawnFailed => "spawn_failed",
            Self::OutputCapExceeded => "output_cap_exceeded",
            Self::VerifierFailed => "verifier_failed",
            Self::VerifierMutation => "verifier_mutation",
            Self::InvalidInfrastructure => "invalid_infrastructure",
            Self::HostContamination => "host_contamination",
        }
    }

    /// Parse the stable token produced by [`TrialOutcomeClass::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "completed_passed" => Some(Self::CompletedPassed),
            "completed_failed" => Some(Self::CompletedFailed),
            "timeout_killed" => Some(Self::TimeoutKilled),
            "cancelled" => Some(Self::Cancelled),
            "denied" => Some(Self::Denied),
            "provider_error" => Some(Self::ProviderError),
            "spawn_failed" => Some(Self::SpawnFailed),
            "output_cap_exceeded" => Some(Self::OutputCapExceeded),
            "verifier_failed" => Some(Self::VerifierFailed),
            "verifier_mutation" => Some(Self::VerifierMutation),
            "invalid_infrastructure" => Some(Self::InvalidInfrastructure),
            "host_contamination" => Some(Self::HostContamination),
            _ => None,
        }
    }

    /// True when this class is the primary task success.
    pub fn is_completed_pass(self) -> bool {
        matches!(self, Self::CompletedPassed)
    }

    /// True when this class counts as a failure of the configured trial.
    pub fn is_task_failure(self) -> bool {
        matches!(
            self,
            Self::CompletedFailed
                | Self::TimeoutKilled
                | Self::Cancelled
                | Self::Denied
                | Self::OutputCapExceeded
        )
    }

    /// True when this class invalidates the trial (retain evidence; never
    /// score as agent failure; retry only under a predeclared capped rule).
    pub fn is_invalid(self) -> bool {
        matches!(
            self,
            Self::SpawnFailed
                | Self::VerifierFailed
                | Self::VerifierMutation
                | Self::InvalidInfrastructure
                | Self::HostContamination
        )
    }

    /// True for the separately classified provider outage/rate-limit class,
    /// which is neither a task failure nor an invalid trial.
    pub fn is_provider_error(self) -> bool {
        matches!(self, Self::ProviderError)
    }
}

/// Classify one trial from its harness termination and verifier result.
///
/// See the module docs for the full policy table and verifier-overlay
/// precedence. `verifier = None` means no verifier result exists.
pub fn classify_trial(
    termination: &TerminationKind,
    verifier: Option<&VerifierOutcome>,
) -> TrialOutcomeClass {
    if let Some(outcome) = verifier {
        // Verifier-owned invalidity overrides everything: a verifier that
        // mutated the scored tree or could not be trusted makes the trial
        // invalid no matter what the harness did.
        if outcome.mutation_detected {
            return TrialOutcomeClass::VerifierMutation;
        }
        if outcome.timed_out {
            return TrialOutcomeClass::VerifierFailed;
        }
        if outcome.verdict == VerifierVerdict::Invalid {
            return TrialOutcomeClass::InvalidInfrastructure;
        }
    }

    match termination {
        TerminationKind::Completed => match verifier.map(|outcome| outcome.verdict) {
            Some(VerifierVerdict::Pass) => TrialOutcomeClass::CompletedPassed,
            // Fail, or no verifier result: task success cannot be established.
            _ => TrialOutcomeClass::CompletedFailed,
        },
        TerminationKind::TimeoutKilled => TrialOutcomeClass::TimeoutKilled,
        TerminationKind::CancelledKilled => TrialOutcomeClass::Cancelled,
        TerminationKind::Denied => TrialOutcomeClass::Denied,
        TerminationKind::ProviderError => TrialOutcomeClass::ProviderError,
        TerminationKind::SpawnFailed => TrialOutcomeClass::SpawnFailed,
        TerminationKind::OutputCapExceeded => TrialOutcomeClass::OutputCapExceeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(verdict: VerifierVerdict, mutation: bool, timed_out: bool) -> VerifierOutcome {
        VerifierOutcome {
            verifier_id: "verifier-test".to_string(),
            verdict,
            exit_codes: vec![Some(0)],
            stdout_sha256: "sha256:stdout".to_string(),
            stderr_sha256: "sha256:stderr".to_string(),
            stdout_bytes_len: 0,
            stderr_bytes_len: 0,
            pre_tree_sha256: "sha256:tree".to_string(),
            post_tree_sha256: "sha256:tree".to_string(),
            mutation_detected: mutation,
            wall_ms: 1,
            timed_out,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            notes: Vec::new(),
        }
    }

    #[test]
    fn full_mapping_table() {
        // Completed runs are decided by the verifier.
        assert_eq!(
            classify_trial(&TerminationKind::Completed, None),
            TrialOutcomeClass::CompletedFailed
        );
        assert_eq!(
            classify_trial(
                &TerminationKind::Completed,
                Some(&outcome(VerifierVerdict::Pass, false, false))
            ),
            TrialOutcomeClass::CompletedPassed
        );
        assert_eq!(
            classify_trial(
                &TerminationKind::Completed,
                Some(&outcome(VerifierVerdict::Fail, false, false))
            ),
            TrialOutcomeClass::CompletedFailed
        );
        assert_eq!(
            classify_trial(
                &TerminationKind::Completed,
                Some(&outcome(VerifierVerdict::Invalid, false, false))
            ),
            TrialOutcomeClass::InvalidInfrastructure
        );
        assert_eq!(
            classify_trial(
                &TerminationKind::Completed,
                Some(&outcome(VerifierVerdict::Invalid, false, true))
            ),
            TrialOutcomeClass::VerifierFailed
        );
        assert_eq!(
            classify_trial(
                &TerminationKind::Completed,
                Some(&outcome(VerifierVerdict::Invalid, true, false))
            ),
            TrialOutcomeClass::VerifierMutation
        );

        // Agent failure classes win over verifier pass/fail.
        for (termination, expected) in [
            (
                TerminationKind::TimeoutKilled,
                TrialOutcomeClass::TimeoutKilled,
            ),
            (
                TerminationKind::CancelledKilled,
                TrialOutcomeClass::Cancelled,
            ),
            (TerminationKind::Denied, TrialOutcomeClass::Denied),
            (
                TerminationKind::ProviderError,
                TrialOutcomeClass::ProviderError,
            ),
            (TerminationKind::SpawnFailed, TrialOutcomeClass::SpawnFailed),
            (
                TerminationKind::OutputCapExceeded,
                TrialOutcomeClass::OutputCapExceeded,
            ),
        ] {
            assert_eq!(classify_trial(&termination, None), expected);
            assert_eq!(
                classify_trial(
                    &termination,
                    Some(&outcome(VerifierVerdict::Fail, false, false))
                ),
                expected,
                "{termination:?} must keep its class over a verifier fail"
            );
        }
    }

    #[test]
    fn verifier_invalidity_overrides_every_termination() {
        for termination in [
            TerminationKind::Completed,
            TerminationKind::TimeoutKilled,
            TerminationKind::CancelledKilled,
            TerminationKind::Denied,
            TerminationKind::ProviderError,
            TerminationKind::SpawnFailed,
            TerminationKind::OutputCapExceeded,
        ] {
            assert_eq!(
                classify_trial(
                    &termination,
                    Some(&outcome(VerifierVerdict::Pass, true, false))
                ),
                TrialOutcomeClass::VerifierMutation,
                "mutation must override {termination:?}"
            );
            assert_eq!(
                classify_trial(
                    &termination,
                    Some(&outcome(VerifierVerdict::Fail, false, true))
                ),
                TrialOutcomeClass::VerifierFailed,
                "verifier timeout must override {termination:?}"
            );
            assert_eq!(
                classify_trial(
                    &termination,
                    Some(&outcome(VerifierVerdict::Invalid, false, false))
                ),
                TrialOutcomeClass::InvalidInfrastructure,
                "verifier infrastructure failure must override {termination:?}"
            );
        }
    }

    #[test]
    fn predicates_partition_the_classes() {
        let all = [
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedFailed,
            TrialOutcomeClass::TimeoutKilled,
            TrialOutcomeClass::Cancelled,
            TrialOutcomeClass::Denied,
            TrialOutcomeClass::ProviderError,
            TrialOutcomeClass::SpawnFailed,
            TrialOutcomeClass::OutputCapExceeded,
            TrialOutcomeClass::VerifierFailed,
            TrialOutcomeClass::VerifierMutation,
            TrialOutcomeClass::InvalidInfrastructure,
            TrialOutcomeClass::HostContamination,
        ];
        for class in all {
            let buckets = [
                class.is_completed_pass(),
                class.is_task_failure(),
                class.is_invalid(),
                class.is_provider_error(),
            ];
            assert_eq!(
                buckets.iter().filter(|bucket| **bucket).count(),
                1,
                "{class:?} must fall into exactly one bucket"
            );
            assert_eq!(TrialOutcomeClass::from_token(class.as_str()), Some(class));
        }
    }

    #[test]
    fn tokens_match_serde_representation() {
        for class in [
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedFailed,
            TrialOutcomeClass::TimeoutKilled,
            TrialOutcomeClass::Cancelled,
            TrialOutcomeClass::Denied,
            TrialOutcomeClass::ProviderError,
            TrialOutcomeClass::SpawnFailed,
            TrialOutcomeClass::OutputCapExceeded,
            TrialOutcomeClass::VerifierFailed,
            TrialOutcomeClass::VerifierMutation,
            TrialOutcomeClass::InvalidInfrastructure,
            TrialOutcomeClass::HostContamination,
        ] {
            let serialized = serde_json::to_value(class).expect("serializable");
            assert_eq!(
                serialized,
                serde_json::Value::String(class.as_str().to_string())
            );
        }
        assert_eq!(TrialOutcomeClass::from_token("nope"), None);
    }
}
