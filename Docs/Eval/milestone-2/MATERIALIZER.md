# Milestone 2, Track A — Isolated Trial Materialization

Date: 2026-10-18 · Scope: `crates/root-eval` only (`trial.rs`, `materializer.rs`,
`isolation.rs`, `tests/trial_materializer.rs`). No CLI, no `root-cli` changes.

## What it does

`materialize_trial(spec, base_dir)` lays down one hermetic trial tree
(`TrialLayout`) under `base_dir/<trial_id>/`:

1. Fresh per-trial directories with `0700` permissions: `repo/`, `home/`,
   `xdg-config/`, `xdg-data/`, `xdg-cache/`, `xdg-state/`, `tmp/`, `fake-bin/`.
2. Detached repository checkout: `git clone --no-checkout --no-hardlinks
   <source> repo` then `git checkout --detach <commit>`. Post-checkout
   `HEAD` must equal the locked commit and `git status --porcelain` must be
   empty — fail closed otherwise. Non-git sources are copied verbatim
   (symlinks refused, 1 MiB file cap); the tree hash is *recorded, not
   verified* on that path, so git sources are required when commit pinning
   matters.
3. Treatment skill files: every manifest skill with a `project:` token is
   written from its resolved source bytes into the trial repo
   (digest-checked against both the manifest record and the declared overlay
   digest). A baseline manifest carries no `project:` skills, so **no skill
   directory appears** — treatment-ness is data-driven, never sniffed from
   `condition_id`. Trial files are *untracked* by design and stay visible to
   later `git diff` inspection.
4. Hermetic harness config: project `.opencode/opencode.json` with
   `{"permission":{"*":{"*":"ask"}}}`. Per the M0 live correction
   (`GOLDEN-CAPTURES` 05b/05c), a `deny` rule removes the tool from the
   model's toolset with no observable event, so denial fixtures must use
   `ask` (auto-reject emits the `state.status:"error"` denial shape).
5. Offline fake tools in `fake-bin/`: a fake `root` script emitting
   deterministic JSON for `doctor`/`plan`/`install`/`verify`/`rollback`
   (unknown subcommands exit 1), plus log-and-fail `brew`/`curl`/`npm`/`pip`
   shims. Every invocation appends to `$TRIAL_AUDIT_LOG`. Script bodies
   contain no `curl`/`wget`/`socket` references (grep-verified by test).
6. Allowlist environment: `trial.env` plus `allowed_env()`/`scrub_env()`.
   `PATH` holds only the fake bin; `OPENCODE_DISABLE_AUTOUPDATE=1` and
   `OPENCODE_DB=:memory:` encode the M0 hermetic policy.

## Isolation guarantees

- `verify_isolation(a, b)`: canonicalized roots must be disjoint; every entry
  under both roots is canonicalized (symlinks resolve), so a symlink escape
  into the sibling trial is `EscapeDetected`, and any shared canonical path
  is `SharedFiles`.
- `residue_scan(base_dir)`: live `TrialDir` entries, `*.lock` /
  `*heartbeat*` markers as `StaleLock`, and non-`none` `pids.txt` content as
  `LeftoverProcessMarker`. Track A spawns no long-lived processes, so a
  clean trial's `pids.txt` reads `none`.
- `cleanup_trial(layout)`: recursive removal + existence verification;
  idempotent.
- `check_no_leak(layout, before, after)`: any new path outside the canonical
  trial root is `LeakDetected`.

## Secret policy

The parent environment is never inherited and never read by the library
(callers pass an explicit snapshot to `scrub_env` for audit). The trial env
is built from empty with the allowlist only. Blocked patterns —
documented in `secret_pattern_hit` and enforced fail-closed in both
`scrub_env` and the env-file writer: `AWS_*`, `GITHUB_*`, `OPENAI_*`,
`ANTHROPIC_*` prefixes and `*_TOKEN`, `*_SECRET`, `*_KEY` suffixes.

## Deviations and known limits

1. **Sole process-spawning exception.** M1 forbids spawning in this crate;
   detached checkouts require `git clone`/`checkout`, so `materialize_trial`
   (via the private `run_git` helper only) may spawn bounded `git` children
   with `env_clear` + captured stderr. Nothing else ever spawns.
2. **Skill bytes come from manifest source paths.** The manifest records
   path+digest, not bytes; the materializer reads bytes from the recorded
   source file (fail-closed: no symlinks, size-bounded, UTF-8, digest must
   match). A content-addressed skill store is later work.
3. **Non-`project:` skill tokens are refused** (`UnsupportedSkillToken`).
   Trial-local home skills are out of scope for Track A.
4. **Copy-path tree hash is recorded, not verified** (no canonical directory
   hash exists at this milestone).
5. **No network namespace / OS sandbox.** Trials are offline by construction
   (fake tools, no credentials) but share the host kernel; OS-level
   containment is a later milestone.

## Tests

`tests/trial_materializer.rs` (8 tests): disjoint trials + repo invisibility,
baseline/treatment delta (manifest delta via `verify_declared_delta` +
materialized trees differ only by the skill file), env scrub with a live
parent secret, cleanup + residue scan, offline fake-tool grep, fail-closed
validation (bad ids, existing dirs, unknown commits), non-git copy, and
overlapping-root isolation refusal.
