# Milestone 0 — Host Probe Report (opencode 1.18.27)

Date: 2026-10-18 · Platform: macOS darwin/arm64 · Sandbox: `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m0/` (disposable)

## 1. Version identity

| Item | Value |
|---|---|
| Resolved npm package | `opencode-ai@1.18.27` (+ platform package `opencode-darwin-arm64@1.18.27`) |
| npm tarball URLs | `https://registry.npmjs.org/opencode-ai/-/opencode-ai-1.18.27.tgz`, `https://registry.npmjs.org/opencode-darwin-arm64/-/opencode-darwin-arm64-1.18.27.tgz` |
| GitHub release | `https://github.com/anomalyco/opencode/releases/tag/v1.18.27` (repo is `anomalyco/opencode`, published 2026-09-02; note: not `sst/opencode`) |
| GitHub asset | `https://github.com/anomalyco/opencode/releases/download/v1.18.27/opencode-darwin-arm64.zip` (46,207,675 B) |
| Binary sha256 (darwin/arm64 CLI) | `f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6` (144,074,210 B, Mach-O 64-bit arm64) |
| Cross-channel identity | npm platform-package binary == assembled wrapper binary == GitHub zip binary (identical sha256) |
| `--version` output (pinned binary) | `1.18.27`, exit 0 |
| Host opencode (context, untouched) | `opencode v2.0.32` at `/Users/sergio/.opencode/bin/opencode` |

Integrity: both npm tarballs matched registry `dist.shasum` (sha1) and `dist.integrity` (sha512). `1.18.27` is present in the npm versions list (`npm view opencode-ai versions --json | grep -c '"1.18.27"'` → 1) but is **not** any dist-tag (`latest` = `1.19.3`) — it exists only as an exact-version tarball. Root's exact pin therefore cannot be expressed as `^`/`~` and must use exact-version install semantics.

Install shape: `opencode-ai` ships a tiny POSIX shim plus `postinstall.mjs` whose only side effect is copying the platform-package binary over `bin/opencode.exe`. The probe replicated that copy deterministically (no postinstall executed, no npm run) and created the `.bin`-style PATH entry as a symlink.

## 2. Flag surface at 1.18.27

Confirmed PRESENT (from `opencode run --help` and `opencode --help`):

| Flag | `opencode run` | top level |
|---|---|---|
| `--format` (`json` allowed) | ✅ choices `default`/`json`, default `default` | ❌ (run-level only) |
| `--pure` | ✅ | ✅ |
| `--dir` | ✅ ("directory to run in, path on remote server if attaching") | ❌ (top-level project is a positional) |
| `--model` / `-m` | ✅ | ✅ |
| `--print-logs` | ✅ | ✅ |

Also present and relevant: `--share`, `--agent`, `--title`, `--command`, `-f/--file`, `--attach`/`-u`/`-p` (server attach), `--variant`, `--thinking`, `--log-level`, `--port`, `--hostname`/`--mdns`/`--cors` (serve), `--auto`. Full raw help in `fixtures/04-help-surface.txt` and `fixtures/05-run-help.txt`.

## 3. Unauthenticated run shape — **no error, run succeeds**

With zero auth configured (fresh HOME/XDG, no auth.json, no env keys):

```
$ opencode run "Reply with exactly: ok"    → exit=0, stdout: "ok"
stderr banner: "build · big-pickle"
```

Log evidence: `llm.runtime=ai-sdk llm.provider=opencode llm.model=big-pickle`; session created with `cost=0`. The unauth path in 1.18.27 silently provisions opencode's built-in free default provider (`opencode`/`big-pickle`). There is **no provider-missing error** to fixture at this version.

Deviation from plan: the brief predicted a provider-missing failure that could be captured as a fixture; instead the run auto-selected the free default and issued a network model call that returned successfully (the watchdog/kill guard never triggered because the run self-completed in seconds; no credentials existed to leak, no auth was configured). No additional runs were attempted. If Root's harness needs an error-shaped fixture it must force provider selection (explicit provider/model or a locked config) — out of scope here to avoid more network calls.

## 4. Filesystem write map

Per-command census: `--version`, `--help`, and `run --help` write **nothing**. Only `opencode run` writes:

- `XDG_CONFIG_HOME/opencode/` → `opencode.jsonc` (only `{"$schema": "https://opencode.ai/config.json"}`) + `.gitignore`
- `XDG_DATA_HOME/opencode/` → `log/opencode.log` (4044 B), `opencode.db` + `-shm`/`-wal` (SQLite session store)
- `XDG_CACHE_HOME/opencode/models.json` → ~5.6 MB provider/model catalog snapshot
- `XDG_STATE_HOME/opencode/locks/<hash>.lock/{heartbeat,meta.json}` → **persists after exit** (not cleaned up)
- `$HOME/.npm/_cacache/` → ~83 MB: `opencode run` fetches its ai-sdk/runtime npm deps through npm **at run time**
- `$TMPDIR/` → `node-compile-cache/` (bulk of 463 env-dir files), one generated `dylib`, `tmp/opencode/`
- Project cwd → **nothing** (no `.opencode/` project dir created by the run)

Implication for Root packaging: first real use writes config, a SQLite db, a models cache, node-compile caches, and npm-caches ~83 MB into HOME — a "pure"/off-network first-run contract cannot be satisfied simply by pointing XDG dirs elsewhere if the error-free default-provider path is taken.

Host isolation verdict: **no writes outside the m0 sandbox**. `/Users/sergio/.opencode` byte-identical pre/post; `/Users/sergio/.config/opencode` had zero files newer than the probe window; `/Users/sergio/Library/Application Support/opencode` never came to exist. One disclosed deviation: the first `npm view` call preceded the cache redirect and may have touched npm's own host cache (npm cache, not opencode).

## 5. Probe failures / caveats

- `pi --version`: `command not found` (exit 127) — recorded as-is, not chased.
- No GNU `timeout` on macOS — implemented a shell watchdog (TERM at 30 s, KILL at 32 s).
- `npm view opencode-ai@... repository/homepage --json` produced no output via `npm view multi-field` — extracted fields from the full `--json` metadata dump instead (both fields are undefined in the published `package.json`).
- An `opencode` pid seen in `pgrep` (26392) is the session-harness process (`/Users/sergio/.opencode/bin/opencode`, started 00:09 local, predates the probe); verified via `lsof` txt path. No probe-spawned processes remain.

## 6. Artifacts

- `fixtures/01-host-probe.txt` — host tool inventory
- `fixtures/02-npm-resolution.txt` — channel resolution + asset inventory
- `fixtures/03-binary-hash.txt` — sha256/sha1/sha512 identity evidence
- `fixtures/04-help-surface.txt` — `opencode --help` raw
- `fixtures/05-run-help.txt` — `opencode run --help` raw
- `fixtures/06-unauth-run.txt` — unauth run capture + log excerpt + disclosure
- `fixtures/07-fs-writes.txt` — fs write map + host-isolation verification

Disposable probe tree retained at `/private/var/folders/b5/wj2djqx569x96bjvf5wclxf80000gn/T/opencode/m0/` (downloads, extracted npm layout, hashes, raw command scripts + captures). Host default install untouched; nothing committed to the repo.
