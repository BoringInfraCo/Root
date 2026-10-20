# v0.8.0 Release Review

**Version:** v0.8.0
**Date:** 2026-10-19
**Scope:** Internal evaluation foundation and its first real-model feasibility run.

## CLI surface

v0.8.0 has **no CLI surface change since v0.7.0**. No `root` command, flag,
output shape, exit code, or `rootd` protocol changed. `root-eval` is an internal
workspace crate and is **not wired into the CLI**; it adds no command.

The lock schema is unchanged: package-only emit `2`, maximum supported `3`.

## End-to-end smoke test

The latest applicable end-to-end smoke test remains
`Docs/Release/V0_6_WORKSPACE_SMOKE_TEST.md`. It was reviewed against v0.8.0 and
remains accurate because v0.8.0 changes no CLI behavior it exercises. The
document is self-contained inline bash; this repository does not track a
separate `.sh` companion for it.

The version check inside that document still names `0.6.0` because it is a
historical v0.6 procedure. When run against a v0.8.0 binary, the check should be
read as "must contain the version of the binary under test" (v0.8.0); no other
step is version-specific.

## v0.7 gap

v0.7.0 has no dedicated smoke-test document. No retroactive v0.7 smoke test is
created, consistent with the repository's policy of not rewriting release
history; the gap is accepted and documented here rather than filled.

## Evaluation evidence is not a product smoke test

The milestone-6 real-model run (`Docs/Eval/milestone-6/`) exercises the
`root-eval` feasibility harness, not the Root CLI or `rootd`. Its 20 trials, 8
valid paired blocks, contamination findings, and byte-identical report
reproduction are evidence for the evaluation foundation only. They are
**not** a product smoke test, and the run's verdict remains **DEFERRED** with no
effectiveness claim.

## Evidence hygiene

The sealed milestone-6 store (`Docs/Eval/milestone-6/run/m5-feasibility/` plus
`artifacts/`) is committed verbatim so the report can be rebuilt from the
repository itself:

```bash
target/debug/examples/m5_feasibility \
  --out Docs/Eval/milestone-6/run --reproduce
```

The run record necessarily contains absolute host and scratch paths from the
machine that produced it (`/Users/...`, `/private/var/...`,
`/opt/homebrew/bin/rg`). They are part of the verbatim evidence, contain no
secrets, and are **not** sanitized — editing the sealed artifacts would
invalidate the report hash. The reproduction check remains
`sha256:ebb17e5d29cb5797e6a244f262c01726ac539ab06ba244183c489cdec6fa38a8`,
byte-identical.

## Release gates

Local gates for v0.8.0 are `cargo build`, `target/debug/root --version`,
`cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --all`. The version-consistency checklist in `AGENTS.md` remains
authoritative for the tag step.
