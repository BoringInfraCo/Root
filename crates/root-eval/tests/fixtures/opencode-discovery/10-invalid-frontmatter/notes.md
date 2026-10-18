# 10 - invalid frontmatter is excluded with a recorded error

Contract: `UPSTREAM-CONTRACT.md` Sec.4.1 - "Frontmatter must contain string
`name`; `description` optional string. Invalid frontmatter -> the load emits a
`session.error` event and the skill is skipped."

Each invalid file is read (so its SHA-256 is recorded) and excluded with a
`DiscoveryError`: missing block, unterminated block, missing name, invalid
line, unsupported value. The valid sibling still resolves, proving one bad
file does not abort discovery.
