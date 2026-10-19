# Milestone 3 — Executive Summary & Verdict: Evidence, Verification, and Store

Date: 2026-10-19 · Repo checkout: `f1218cc` + M3 working tree
Scope basis: audit's Milestone 3 (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*) under the recorded **feasibility-spike scope decision**
Implementation: two subagent phases (A: evidence/verifier/diff/taxonomy; B: SQLite store), independently CI-gated by the lead agent

## Verdict

**MILESTONE 3 COMPLETE — all five audit exit criteria pass, workspace CI green (`fmt --check` / `clippy -D warnings` / `test --all`: 1217 passed, 0 failed).**

## Deliverables

### Phase A — evidence, verifier, diff, taxonomy (`evidence.rs`, `diff.rs`, `verifier.rs`, `taxonomy.rs`)
- **ArtifactStore (CAS)**: `<evals_root>/artifacts/sha256/<shard>/<hex>`, atomic temp+rename writes with read-back hash verification, hash-verified reads, size caps, dedupe.
- **Redaction**: deterministic pattern redaction (secret-key assignments + JSON secret keys), `[REDACTED:<pattern>]` markers with per-pattern counts, idempotent, documented as a heuristic (not a secret manager).
- **EvidenceManifest seal**: canonical-JSON projection over every manifest field (domain `evidence-manifest.v1`) → `seal_sha256`; `verify_seal` recomputes (26 mutator tests incl. artifact reorder/count); `verify_artifacts` re-verifies artifact bytes on disk (same-length tampering caught). Integrity, not authenticity — documented scope honesty.
- **RepoDiff capture**: bounded `status --porcelain` + diff text + untracked list, hardened git env (`GIT_CONFIG_NOSYSTEM`, `GIT_CONFIG_GLOBAL=/dev/null`, `LC_ALL=C`), cap-truncation flag.
- **Verifier runner**: pre/post `tree_fingerprint` (content-only, sorted path+hash pairs, symlink/size/depth fail-closed); any post-verifier tree change → `mutation_detected` + `Invalid` regardless of exit codes — the "verifier cannot mutate scored artifacts unnoticed" criterion; timeouts/output-caps → `Invalid` with notes.
- **Failure taxonomy**: 11-class `TrialOutcomeClass` + `classify_trial` implementing the audit's failure/missing-data policy (agent failures attributable to trials; infrastructure invalid; provider separate; verifier mutation/timeout invalid).
- Supervisor extended additively: `raw_stdout`/`raw_stderr` bounded capture (required for sealing), `TerminationKind` tokens/serde.

### Phase B — eval store (`store.rs`)
- Separate versioned SQLite at `<evals_root>/<experiment_id>/state.db` (0700/0600), WAL + `foreign_keys=ON`, schema v1 with meta/experiments/trials/executions/verifications/evidence/artifacts/evidence_artifacts + indexes; refuses newer schemas (never partial-reads).
- `open_read_only` (`query_only=ON`) with every write path refused; explicit paths only — never reads env.
- Writes are per-transaction; `record_evidence` re-verifies the seal and cross-checks identities before storing; `set_trial_outcome` enforces invalid-reason coherence; trial `state` is recomputed on every write.
- `trial_full` join (trial + latest execution/verification/evidence + outcome class), `experiment_summary` counts — every execution/trial/config/task/revision/verifier relationship queryable.
- `verify_store_evidence(&ArtifactStore)`: re-seals every stored manifest + re-verifies every artifact; tampered artifact bytes and tampered `manifest_json` rows both detected by trial id.
- 16 store tests incl. failed-publication rollback (mid-transaction failure → zero partial rows), duplicate typed errors, missing-telemetry exact round-trip.

## Exit-criteria scorecard (audit M3)

| Criterion | Result | Evidence |
|---|---|---|
| Every execution/trial/config/task/revision/verifier relationship queryable | **PASS** | schema FKs + `trial_full`/`experiment_summary` joins; roundtrip tests over 2-trial experiment |
| Missing telemetry remains explicit | **PASS** | `missing_telemetry_json` stored/read verbatim; never coerced to empty |
| Tampering with any evidence artifact fails verification | **PASS** | seal mutation matrix + artifact byte-flip tests; store-level integrity check names the trial |
| Verifier cannot mutate scored artifacts unnoticed | **PASS** | pre/post fingerprint; write-then-exit-0 script still → `mutation_detected` + `Invalid` |
| Schema migration and failed-publication tests pass | **PASS** | fresh/reopen/refuse-newer matrix; rollback leaves 1/1/1 rows, failed seal `NotFound` |

## Recorded deviations
Supervisor additive extension (required for sealing); `VerifierSpec` uses per-command `expected_exit`; `run_verifier` returns outcomes (never `Err`) with notes; CAS `put` takes a logical name; backup guard unreachable at v1 (truth-table unit test); read-only open of an *older* schema also refused (stricter than required). Full lists in module docs and agent reports.

## Deliberately NOT in M3
Real-harness end-to-end evidence runs, scheduler/report (M4), CLI surface. Nothing phones home; all processes are local and bounded.
