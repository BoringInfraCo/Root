# 12 - A/B treatment shape

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 - `SKILL.md` files under a config
directory's `{skill,skills}` glob are discovered; a file with frontmatter
`name: root` is valid. The audit's Milestone 1 exit criterion requires that
the two condition manifests differ *only* by declared skill files/metadata.

Layout:
- `base/` - baseline condition (global + project instructions, skill `foo`);
- `treatment-added/` - byte-for-byte copy of `base/` plus exactly one delta
  dir: `project/.opencode/skill/root/SKILL.md`, copied byte-for-byte from the
  repository's `skills/claude/SKILL.md` (frontmatter `name: root`);
- `treatment-extra/` - the same delta plus an undeclared extra skill `other`;
- `treatment-modified/` - `base/` with the body of skill `foo` changed;
- `treatment-removed/` - `base/` with skill `foo` removed.

`overlay-add.json`, `overlay-modify.json`, and `overlay-remove.json` declare
the expected deltas; `expected.json` pins the manifest of every tree.
