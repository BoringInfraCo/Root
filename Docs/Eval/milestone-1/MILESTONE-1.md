# Milestone 1 — Executive Summary & Verdict: Immutable Spec + Effective-Context Snapshot

Date: 2026-10-18 · Repo checkout: `d38935de` + M1 working tree
Scope basis: audit's Milestone 1 (`Docs/audits/skill-effectiveness-evaluation-audit.md`, *Implementation Roadmap*) under the recorded **feasibility-spike scope decision** (Review Addendum)
Implementation: two sequential subagent phases, both independently CI-gated by the lead agent

## Verdict

**MILESTONE 1 COMPLETE — all four audit exit criteria pass, workspace CI green (`build` / `fmt --check` / `clippy -D warnings` / `test --all`, zero failures).**

## Deliverables

### Phase A — `crates/root-eval`: strict task-record parser
- Full `TASK-SPEC.md` implementation: **65/65 schema fields**, 17 enums, six acceptance-payload types, budgets with armable caps ("missing is not zero"), `sha256:PENDING` sentinel refusal (drafts parse syntactically, refuse semantically), control-character rejection, fixed-point canonical hashing.
- Modules: `schema.rs`, `raw.rs` (two-layer strict parsing), `parse.rs` (version gate + per-level exact key sets), `validate.rs` (semantics), `canonical.rs`, `digest.rs`, `error.rs` (machine-matchable `ErrorKind`), `lib.rs`.
- **147 tests**: three spec examples verbatim (sentinel-refused as designed), a 30-field valid spine, determinism/fixed-point, a 104-case refusal matrix.

### Phase B — resolver + condition delta + plan hash
- `context.rs` (+`context/frontmatter.rs`): hermetic OpenCode 1.18.27 `EffectiveContextManifest` resolver — explicit roots + `DiscoveryFlags` only (no env reads, no process spawning, no implicit HOME), following the frozen `UPSTREAM-CONTRACT.md` §4 precedence literally.
- `condition.rs`: `ConditionOverlay` + `verify_declared_delta` — treatment may differ from baseline **only** by declared paths/digests; every undeclared/missing/mismatched difference errors with the offending path.
- `plan.rs`: minimal typed `ExperimentSpec` + `compute_plan_hash` (canonical JSON, task order canonically sorted, `hash_domain = "experiment-spec.v1"`, hashed via `root_lockfile::compute_sha256`).
- **Fixture corpus: 17 cases / 142 files** with pinned `expected.json` (30 manifest hashes, 53 file digests re-verified against `shasum`), including the A/B treatment shape seeding the **byte-for-byte `skills/claude/SKILL.md`** as `.opencode/skill/root/SKILL.md`.
- **68 new tests** (corpus replay 31, delta 7, plan 8, plus module tests); crate total **215**.

## Exit-criteria scorecard (audit M1)

| Criterion | Result | Evidence |
|---|---|---|
| Deterministic same inputs → same plan hash | **PASS** | repeated construction, task/condition order permutations, round-trips — all identical |
| A/B manifests differ only by declared skill files/metadata | **PASS** | `verify_declared_delta` Ok on the real-SKILL.md overlay; undeclared/missing/digest-mismatch each error by path |
| Nested cwd + all discovery paths have positive/negative tests | **PASS** | case 05 + dedicated walk-up boundary tests; nothing above the worktree root ever resolves |
| No trial or agent process starts during plan | **PASS** | grep-verified: no `process::Command`, no async, no `std::env` reads in library code |

## Recorded assumptions & deviations (full lists in crate docs + `TEST-PLAN.md` §7)

Most consequential for Milestone 2 and any later live confirmation:
1. **Scan-order totalization** — the contract lists discovery sources without an order; M1 adopts listed-order, walk-ups innermost-first, lexicographic within a root. Duplicate winners are pinned by cases 09/11/16/17 — re-verify live during M2 conformance.
2. **`OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` also skips `.agents`** (contract parenthetical ambiguous; conservative = fewer files).
3. **Config-reading subset** — only `opencode.json` in discovered config dirs (not `.jsonc`/`config.json`); remote `skills.urls` out of scope (runtime-network).
4. **Companion/lazy per-message instruction attachment not modeled** — manifest records primary global + project selections only.
5. Phase A's 12 deviations include deferrals that belong to later milestones by design (regex syntax validation, budget dominance, runner rules).

## Live-conformance results (2026-10-19)

The pinned harness (`opencode 1.18.27`) was probed live on 2026-10-19
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md`, raw evidence in
`Docs/Eval/milestone-6/probes/discovery/`). The M1 assumptions above now carry
these verdicts:

| # | M1 assumption | Verdict | Evidence |
|---|---|---|---|
| 1 | Scan-order totalization (listed order, walk-ups innermost-first, per-level cross-agent walk) | **CONFIRMED** for the *structure*; **REFUTED** for the *duplicate-name winner* (upstream registers skills concurrently — the winner flips between runs) | `CONFORMANCE-DISCOVERY.md` §2.2/§2.3 (A1–A4, 30 runs per layout) |
| 2 | `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` also skips `.agents` | **REFUTED** — that flag gates the `.claude` roots only; the `.agents` roots belong to `OPENCODE_DISABLE_EXTERNAL_SKILLS` (newly expressible) | `CONFORMANCE-DISCOVERY.md` §3.1 (`m7.*.json`) |
| 3 | Config-reading subset (`opencode.json` only; no `.jsonc`/`config.json`; no `skills.urls`) | **OPEN** — not exercised by the probe | `CONFORMANCE-DISCOVERY.md` §6 (O3) |
| 4 | Companion/lazy per-message instruction attachment not modeled | **OPEN** — still not modeled (out of the resolver's scope) | — |
| 5 | Cross-level name priority (name-first, then nearest file with that name) | **CONFIRMED** | `CONFORMANCE-DISCOVERY.md` §2.2 (fixture `04/cross-level`) |
| 6 | `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` leaves the project walk-up unaffected | **REFUTED** — it also drops the project-level `CLAUDE.md` candidate | `CONFORMANCE-DISCOVERY.md` §3.2 (`m3-*`) |
| 7 | `OPENCODE_DISABLE_PROJECT_CONFIG` leaves the project instruction walk-up active | **REFUTED** — it switches the project walk-up off entirely | `CONFORMANCE-DISCOVERY.md` §3.2 (`m1-*`) |
| 8 | One project instruction file per message | **REFUTED** — every level of the first-priority name attaches | `CONFORMANCE-DISCOVERY.md` §3.2 (`m5b-paths`) |

Consequences for the resolver (`crates/root-eval/src/context.rs`): BUG-1…BUG-5
of `CONFORMANCE-DISCOVERY.md` §4 were fixed on 2026-10-19. `DiscoveryFlags`
gained `disable_external_skills` (`OPENCODE_DISABLE_EXTERNAL_SKILLS`) and
`disable_claude_code` (the broad `OPENCODE_DISABLE_CLAUDE_CODE`); the manifest
gained `reproducible()` and an honest duplicate-name warning (candidate set
instead of a claimed winner); the manifest hash domain moved to
`effective-context.v2` (projection shape changed — **v1 hashes are not
comparable**). The corpus grew to 20 cases (`18`–`20` cover the corrected
flag semantics and the multi-level instruction attachment).

## Deliberately NOT in M1

Harness runner, trial materialization, execution, CLI surface (`root eval …`), scheduler, aggregation — all Milestone 2+ per the scope decision. M1 is pure specification: parse, resolve, hash — nothing runs.

## Next

Milestone 2 (isolated supervised runner) is the next approved slice. Its entry conditions are now met: frozen contract (M0), immutable inputs + plan hash (M1), and the M2-specific findings already queued from M0's live captures (process-group kill + tool-child reaping, `killed` vs `aborted` taxonomy, config-dir npm pre-warm).
