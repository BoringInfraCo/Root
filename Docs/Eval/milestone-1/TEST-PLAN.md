# Milestone 1 — Phase B Test Plan

**Scope.** Phase B of Milestone 1 ("Immutable spec and effective config
snapshot", audit lines 866–877): the OpenCode effective-context resolver with
its fixture corpus, condition-delta enforcement, and the canonical plan hash.
Behavioral source of truth: `Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md` §4
("Skills & Rules"), §5.2 (`OPENCODE_DISABLE_PROJECT_CONFIG`), and §4.3/§4.1
gate names, frozen for OpenCode `1.18.27`. No other behavior source is used.

**What this plan covers.** Deterministic, hermetic library behavior: the model
of discovery, the declared-delta audit, and hash determinism. **What it does
not cover.** Live harness behavior (env-var handling inside `opencode` itself,
network fetches for `skills.urls`, remote skill caching, session events) — those
stay runtime-gated and are listed as such in §5 rather than claimed by M1.

All tests: `cargo test -p root-eval` (integration tests
`context_corpus.rs`, `condition_delta.rs`, `plan_hash.rs`; unit tests in
`context.rs`, `context/frontmatter.rs`, `condition.rs`, `plan.rs`,
`canonical_json.rs`).

## 1. Corpus layout and `expected.json` schema

```
crates/root-eval/tests/fixtures/opencode-discovery/<case>/
  layout.txt      # tree listing + root mapping
  notes.md        # contract citation + what the case proves
  expected.json   # declarative scenarios
  home/...        # fake $HOME (config/ = fake XDG_CONFIG_HOME)
  project/...     # instance directory (often == worktree root)
  worktree/...    # walk-up boundary trees where relevant
```

`expected.json`:

```json
{
  "case": "<case-id>",
  "contract": "Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md",
  "scenarios": [{
    "name": "<scenario>",
    "roots": { "home": "home", "xdg_config_home": "home/config",
               "project_root": "project", "worktree_root": "project" },
    "flags": { "disable_claude_code_skills": false,
               "disable_claude_code_prompt": false,
               "disable_project_config": false,
               "disable_external_skills": false,
               "disable_claude_code": false,
               "config_dir": null },
    "expect": {
      "instructions": [{ "role", "source", "precedence", "file", "token",
                         "sha256", "size" }],
      "skills":       [{ "name", "source", "file", "token", "sha256",
                         "ambiguous" }],
      "discovery_errors":   [{ "file", "token", "kind", "sha256" }],
      "discovery_warnings": [{ "name",
                               "candidates": [{ "file", "token", "sha256" }] }],
      "reproducible": true,
      "manifest_sha256": "sha256:<64 lowercase hex>"
    }
  }]
}
```

`expect_error` replaces `expect` for refusal scenarios:
`{ "kind": "<ContextErrorKind>", "message_contains": "<substring>" }`.

`reproducible` is `false` for a layout with a duplicate skill name (upstream's
winner is a race — see §2). `ambiguous` marks the affected skill entry;
`discovery_warnings[].candidates` lists **every** candidate file of the
contested name, in scan order, and deliberately names no winner.

File digests/sizes were produced with `shasum -a 256`/`wc -c`; manifest hashes
were produced by the resolver itself and then pinned, so corpus replay is a
frozen-expectation test (a changed resolver fails loudly). The manifest hash is
computed over **root-relative tokens** (`home:`, `xdg:`, `project:`,
`worktree:`, `configdir:`, `abs:`), so it is machine-portable; the
`manifest_hash_projection_excludes_absolute_paths` test enforces that no
case-absolute path leaks into the hash input.

Every case directory must be replayed exactly once
(`every_corpus_case_is_replayed`) and no `TBD` placeholder may survive
(`every_manifest_hash_is_pinned`).

## 2. Documented scan order (duplicate names: a race upstream, a candidate set here)

The contract lists discovery sources but not a total order; the resolver fixes
and documents this total order (module docs of `context.rs`):

1. built-in `customize-opencode` (not on disk; recorded by name only, any disk
   skill of the same name wins);
2. `~/.claude/skills/**/SKILL.md` — dropped by the Claude-skills gate;
3. `~/.agents/skills/**/SKILL.md` — dropped by the external-skills gate;
4. walk-up from the instance directory to the worktree root (innermost first,
   worktree root included): `<dir>/.claude/skills`, `<dir>/.agents/skills`
   (each subject to its own gate);
5. config directories in the contract's listed order —
   `${XDG_CONFIG_HOME:-~/.config}/opencode`, walk-up `.opencode` dirs
   (innermost first), `~/.opencode`, `OPENCODE_CONFIG_DIR` — each with
   `skill/**/SKILL.md` then `skills/**/SKILL.md`;
6. explicit `skills.paths` entries, concatenated in config-directory order and,
   within a file, declared order.

Within one scanned root, candidates are visited in lexicographic path order;
scanned roots are de-duplicated by path (first occurrence). Final skill lists
are sorted by name; warnings by name; errors by `(token, kind)`.

**Duplicate names (changed 2026-10-19).** Upstream registers the discovered
skills with unbounded concurrency, so the file that wins a contested name is
decided by async completion order and flips between runs (measured live: 5–10
winner flips per 30 runs across four layouts). The resolver therefore does
**not** model "last-scanned wins" as upstream behavior:

* every contested name produces one `DuplicateSkillName` warning carrying the
  **candidate set** (path, token, digest, in scan order) — no winner is named;
* the affected skill entry is flagged `ambiguous`;
* the manifest reports `reproducible() == false`, and its canonical hash pins
  the candidate set rather than a winner.

The resolver itself stays deterministic (the same tree always yields the same
manifest and hash), but such a manifest must not be read as a prediction of the
harness's effective file.

## 3. Case matrix

| Case | Behavior under test | Assertions |
| --- | --- | --- |
| `01-global-agents-only` | Global `${XDG}/opencode/AGENTS.md` attaches alone | 1 instruction (role global, source `global-config-agents`, precedence 0, token `xdg:opencode/AGENTS.md`, sha256 + size); empty skills/errors/warnings; pinned hash |
| `02-global-claude-fallback` | Global fallback `~/.claude/CLAUDE.md`; first-match-wins; `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` | default: 1 fallback instruction; flagged: 0 instructions; hashes differ; dedicated flag test |
| `03-project-walkup-nearest-agents` | Walk-up attaches **every** level of the winning name (instance dir → worktree root) | 2 project instructions: `worktree/applications/AGENTS.md` (precedence 0) and `worktree/AGENTS.md` (precedence 1) |
| `04-level-priority` | Name priority `AGENTS.md` > `CLAUDE.md` > `CONTEXT.md`, same level and cross-level | 4 scenarios: `all`→AGENTS, `claude-context`→CLAUDE, `context-only`→CONTEXT, `cross-level`→farther AGENTS beats nearer CLAUDE (literal "first name in priority order" reading) |
| `05-worktree-boundary` | Nothing above the worktree root is scanned | `inside`: root-level file resolves; `outside-not-scanned`: 0 instructions although `outside/AGENTS.md` exists above; token/path absent from canonical bytes |
| `06-project-and-global` | Global and project instructions attach independently | 2 instructions, precedence 0 global / 1 project; global AGENTS.md suppresses the CLAUDE fallback |
| `07-opencode-singular-skill` | `{skill}` config-dir glob | 1 skill, source `opencode-config-dir`, token `project:.opencode/skill/root-singular/SKILL.md` |
| `08-opencode-plural-skills` | `{skills}` config-dir glob | 1 skill under `.opencode/skills/…`, token `project:.opencode/skills/plural-skill/SKILL.md` |
| `09-cross-agent-roots` | `~/.claude/skills/**` + `~/.agents/skills/**` (global and walk-up), recursion | 5 skills with sources `claude-global`/`agents-global`/`claude-walkup`/`agents-walkup`, incl. a nested path; `disable-claude-skills` scenario keeps the two `.agents` skills (the gate removes the `.claude` roots only) |
| `10-invalid-frontmatter` | Invalid frontmatter is excluded with a recorded error | 1 valid skill survives; 5 errors with kinds `invalid-frontmatter-line`, `invalid-frontmatter-value`, `missing-frontmatter`, `missing-name`, `unterminated-frontmatter`; each error carries the file sha256 and non-empty detail |
| `11-duplicate-name` | Duplicate names are a **race** upstream: the manifest records the candidate set and is not reproducible | One `duplicate-skill-name` warning with the three candidate files (claude-global → xdg → project, in scan order); the effective skill is flagged `ambiguous`; `reproducible: false` |
| `12-ab-treatment` | A/B shape + delta semantics | 5 tree scenarios with pinned hashes; `treatment-added` is byte-identical to `base` except one added directory; delta tests: declared add OK; undeclared extra errs naming `.opencode/skill/other/SKILL.md`; declared-but-absent errs; modify/remove OK only when declared |
| `13-empty-nil` | Empty context is valid | 0 instructions/skills/errors/warnings; builtin `["customize-opencode"]`; pinned hash |
| `14-skills-paths` | Explicit `skills.paths`: `~/`, relative, absolute entries | default: home-relative + instance-relative skills (`explicit-path`); absolute-but-missing entry is accepted and discovers nothing; `disable-project-config` resolves 0; `OPENCODE_CONFIG_DIR` override adds `configdir:`-token skill + explicit path |
| `15-unsafe-explicit-path` | `..` traversal is refused | resolve fails with `ContextErrorKind::UnsafePath`, message names `../escape` |
| `16-flags-project-config` | `OPENCODE_DISABLE_PROJECT_CONFIG` skips project `.opencode`, keeps `~/.opencode` | default: 2 skills; flagged: only `global-skill` |
| `17-opencode-walkup-global` | Walk-up `.opencode` at the worktree root + `~/.opencode` | default: `walkup-skill` (token `worktree:…`) + `global-plural` (token `home:…`); flagged: only `global-plural` |
| `18-flag-gates-external-skills` | The Claude-skills gate, the external-skills gate and the broad Claude-Code gate are three distinct gates | default: 5 skills; `disable-claude-skills`: 3 (the `.claude` skills gone, the `.agents` skills kept); `disable-external-skills`: 1 (`.claude` **and** `.agents` gone); `disable-claude-code` (broad): 3 (same set as the direct skills gate, different hash); both gates: 1 |
| `19-flag-project-config-instructions` | The project-config gate kills the project walk-up; the prompt gate drops the project `CLAUDE.md` | `agents-only`: global + project `AGENTS.md`; `agents-only-disable-project-config`: global only; `claude-only`: global + project `CLAUDE.md`; `claude-only-disable-claude-prompt`: global only |
| `20-project-instruction-multilevel` | Every level of the winning project instruction name attaches | 2 project instructions (`worktree/app/AGENTS.md`, then `worktree/AGENTS.md`); `disable-project-config`: none |

Additional non-corpus tests:

- `walkup_boundary_positive/negative` — dedicated nested-cwd boundary pair
  (also asserted in `05`).
- `manifest_replay_is_byte_deterministic` — same tree → same hash and canonical
  bytes across repeated resolution.
- `missing_root_is_a_typed_error_not_a_panic`, `disjoint_roots_are_a_typed_error`
  — no-IO-safety and root-containment refusals.
- `ab_treatment_root_skill_is_byte_identical_to_the_repository_skill` — the
  delta skill is a byte-for-byte copy of `skills/claude/SKILL.md`.
- `ab_trees_differ_only_by_the_declared_delta_directory` — filesystem-level A/B
  shape (added-file set, removals, shared-file bytes).
- `flags_are_part_of_the_manifest_hash_even_when_resolution_is_unchanged` —
  gate changes are context changes even when the resolved file set is equal.
- Plan tests: repeated construction; permuted task/condition order; changed
  condition manifest; task version/content bump; harness version/binary,
  repository, overlay-presence, `created_at` changes; `verify_plan`
  accept/reject; validation refusals (`tasks` empty, non-`opencode` harness,
  pending digest).
- Frontmatter unit tests: repo-shape parse, missing/unterminated blocks,
  missing vs empty name, duplicate keys, quoted scalars, YAML-indicator values,
  nested lines, blank/commented lines, unknown keys.

## 4. Flag sensitivity (runtime-gated)

The resolver never reads the environment; callers map gates into
`DiscoveryFlags`. The table records the modeled effect, its corpus coverage,
and the live-conformance verdict of 2026-10-19
(`Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md`; upstream runtime flags in
`packages/core/src/flag/flag.ts`).

| Gate | Modeled effect | Corpus coverage | Live verdict |
| --- | --- | --- | --- |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` | Skip the **`.claude`** skill roots only (`~/.claude/skills` and the walk-up `<level>/.claude/skills`); the `.agents` roots stay | `09` scenario `disable-claude-skills`; `18`; `claude_skills_flag_removes_only_the_claude_roots` | **CONFIRMED** (the M1 reading that it also skipped `.agents` was **REFUTED**) |
| `OPENCODE_DISABLE_EXTERNAL_SKILLS` | Skip **every** external skill root — the `.claude` and the `.agents` roots, global and walk-up | `18` scenario `disable-external-skills` | **CONFIRMED** (newly expressible; upstream gates `.agents` with this flag) |
| `OPENCODE_DISABLE_CLAUDE_CODE` (broad) | Implies the Claude-skills gate **and** the Claude-prompt gate; never the external-skills gate | `18` scenario `disable-claude-code` | **CONFIRMED** (newly expressible) |
| `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT` | Suppress the global `~/.claude/CLAUDE.md` fallback **and** drop the project-level `CLAUDE.md` candidate | `02` scenario `disable-claude-prompt`; `19`; `claude_prompt_flag_removes_the_global_fallback_and_the_project_claude_file` | **CONFIRMED** (the M1 reading that the project walk-up was unaffected was **REFUTED**) |
| `OPENCODE_DISABLE_PROJECT_CONFIG` | Skip project `.opencode` config dirs (skill globs **and** their `opencode.json`), skip `skills.paths` from project config, **and** skip the project instruction walk-up; keep `${XDG}/opencode`, `~/.opencode`, `OPENCODE_CONFIG_DIR` | `14`, `16`, `17` flagged scenarios; `19`; `20`; `project_config_flag_removes_the_project_instruction_walk_up` | **CONFIRMED** (the M1 reading that the instruction walk-up was unaffected was **REFUTED**) |
| `OPENCODE_CONFIG_DIR` | Appended as a config directory (scanned last, so highest duplicate precedence); its `opencode.json` contributes `skills.paths` | `14` scenario `config-dir-override` | **OPEN** — code-level only, not exercised live |

Flag values are hashed into the manifest projection: flipping a gate always
changes the manifest hash, even when the resolved file set is unchanged. The
projection carries the *effective* upstream runtime flags
(`disableClaudeCodeSkills`, `disableClaudeCodePrompt`, `disableExternalSkills`,
`OPENCODE_DISABLE_PROJECT_CONFIG`), so two different environment spellings
with the same effect (e.g. the broad gate versus the two direct ones) hash
alike.

## 5. Condition-delta semantics

`ConditionOverlay` = per canonical token: `add`/`modify` (require
`sha256:<64 lowercase hex>`) or `remove` (optional digest pinning the base).
`apply_overlay(base, overlay)` validates the declaration against the base
manifest and produces a `DeltaExpectation`; `verify_declared_delta(base,
treatment, overlay)` additionally enforces:

1. **Only declared differences.** Base and treatment are flattened into
   identity-keyed records (`instruction|<token>`, `skill|<token>`,
   `error|<token>`, `warning|<name>|<overridden>|<winner>`, plus `flags` and
   `builtin_skill_names`). Every differing record must reference a declared
   token; otherwise `UndeclaredDifference` names the offending path(s).
2. **Every declared change happened.** A declared token with no difference is
   `MissingDeclaredChange`.
3. **Digests match.** Add/modify must equal the treatment record's file digest;
   a record without a digest is an outcome mismatch. Remove requires absence
   from the treatment.
4. **Declaration sanity** (at `apply` time): `add` of an already-referenced
   path, `modify`/`remove` of an unreferenced path, `modify` whose digest
   equals the base (no-op), stale remove digests, duplicate tokens, and
   non-canonical tokens are all refused.

`precedence` (a pure order index) is excluded from record comparison while the
instruction *set* remains compared, so selection ripples still surface as
undeclared differences. This is the mechanical form of the audit criterion
"A/B manifests differ only by declared skill files/metadata".

## 6. Plan hash inputs and canonical ordering

`ExperimentSpec` (M1 scope): `schema_version`, `experiment_id`,
`tasks[] = {task_id, version, content_sha256}`, `conditions[] = {id,
expected_manifest_sha256, overlay_present}`, `harness = {name:"opencode",
version, binary_sha256}`, `repository = {commit, tree_sha256}`, `created_at`.

`compute_plan_hash`:

1. validates the spec (supported schema version, non-empty identities and
   collections, canonical digests, `opencode` harness, no duplicate
   `(task_id, version)` or condition ids);
2. canonical JSON projection: tasks sorted by `(task_id, version)`, conditions
   sorted by `id`, keys sorted at every level, compact output, constant
   `hash_domain = "experiment-spec.v1"`;
3. `sha256:<hex>` via `root_lockfile::compute_sha256`.

Nothing environment-, clock-, or filesystem-dependent is read: `created_at` is
an explicit caller field and is hashed. `verify_plan(spec, expected)` rejects
malformed expected digests (`DigestInvalid`) and mismatches
(`PlanHashMismatch`).

## 7. Interpretations, deviations, and known gaps

Recorded because the frozen contract is silent or the M1 surface is
deliberately bounded (conservative = fewer files discovered). Items marked
**REFUTED (2026-10-19)** were live-measured against the pinned harness and
have been corrected in the resolver and the corpus.

1. **Scan-order totalization** (§2 above) — the contract lists sources without
   an order; the list order is adopted, walk-ups are innermost-first. The
   *structure* is **CONFIRMED** live; the deterministic duplicate *winner* is
   **REFUTED** — upstream registers skills concurrently, so the manifest now
   records the candidate set and reports `reproducible: false`.
2. **Cross-level name priority** — "first name in priority order … that matches
   anywhere in that span" is read as name-first, then nearest-with-that-name
   (fixture `04/cross-level`). **CONFIRMED** live.
3. **`.agents` under the Claude-skills gate** — **REFUTED (2026-10-19)**. The
   gate skips the `.claude` roots only; `OPENCODE_DISABLE_EXTERNAL_SKILLS`
   skips both families. The resolver's `DiscoveryFlags` now carries that flag
   (and the broad `OPENCODE_DISABLE_CLAUDE_CODE`).
4. **`OPENCODE_DISABLE_PROJECT_CONFIG` leaves the project instruction walk-up
   active** — **REFUTED (2026-10-19)**. The flag switches the project
   instruction walk-up off entirely; only the global instruction survives.
5. **Companion/lazy instruction attachment not modeled** — the contract's
   "companion files above a read file are also attached lazily per message" is
   per-message path resolution; the manifest records the global and the
   project-level selections only. **Still open.** The primary project
   selection itself changed on 2026-10-19: **every** level of the
   first-priority name attaches (the contract's "one file per message" was
   **REFUTED**).
6. **`opencode.json` coverage** — only `opencode.json` inside the discovered
   config directories is read; `opencode.jsonc`, `config.json`, and a
   project-root `opencode.json` (outside `.opencode`) are not read in M1.
   **Still open** (not exercised by the 2026-10-19 probe).
7. **Remote `skills.urls`** and the skill pull cache are out of scope (hermetic,
   no network). **Still open.**
8. **Built-in skill body unavailable** — `customize-opencode` is recorded by
   name; its compiled body is not hashed.
9. **Symlinks are refused** wherever a file would be consumed (instructions,
   `SKILL.md`, config, roots); other symlinks inside scanned trees are skipped.
   Scan depth is bounded (32) and file reads are bounded (1 MiB); both bound
   violations/exclusions are documented resolver limits, not contract behavior.
10. **`abs:` tokens** (files outside every declared root) are deterministic but
    machine-specific; fixtures avoid them so every pinned hash is portable.
11. **Manifest hash domain** — `effective-context.v2` since 2026-10-19 (the
    projection gained the effective runtime flags, the skill ambiguity flag
    with its candidate set, and the `reproducible` verdict). **`v1` hashes are
    not comparable.**

## 8. Audit Milestone-1 criteria mapping

| Exit criterion (audit) | Evidence |
| --- | --- |
| deterministic same inputs → same plan hash | `plan_hash.rs` (repeated construction, order permutations) + unit tests |
| A/B manifests differ only by declared skill files/metadata | `condition_delta.rs` on `12-ab-treatment`; declaration validation matrix |
| nested cwd and exact-version discovery paths have positive/negative tests | `05` + dedicated boundary tests; 17-case corpus for every §4.1 source class |
| no trial or agent process starts during plan | library contains no process spawning, no async, no env reads; resolver reads explicit roots only (review + typed-input API) |
