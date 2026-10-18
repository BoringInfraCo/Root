# 15 - path traversal in an explicit `skills.paths` entry

Root conventions (and the phase mandate) require rejecting path traversal.
A `..` component in an explicit entry is refused as a typed
`ContextError::UnsafePath`, even if the normalized target would stay inside
the project. The fixture expects the resolution to fail; no manifest is
produced.
