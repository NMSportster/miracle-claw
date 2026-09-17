# MC Option A — `mc` Headless Companion Binary

**Status:** Design draft (2026-09-09 11:25 MDT, David approved)
**Owner:** ABQShop Claw
**Target release:** rc55.26 (1-2 days after sign-off)
**Closes:** Competitive-gap-analysis "Headless / non-interactive mode" (Sep 8) — 6 of 6 use cases (cron, CI, pipes, exit codes, JSON, shell invocation)

---

## What this is

A single Rust binary `mc` (~5 MB) that lets you invoke MAIC from any shell, cron, or CI. It's a **thin HTTP wrapper** around the same MAIC endpoints the Tauri app already calls — no TUI, no local tools, no bundled openclaw runtime. The Tauri app stays the interactive surface; `mc` is the automation surface.

Both code paths share the same MAIC backend, the same JWT, the same module catalog. One product, two surfaces.

## What it gives us vs the full headless build

| Capability | Option A | Full headless (1-2 weeks) |
|---|---|---|
| `mc ask "what's the weather"` from any shell | ✅ | ✅ |
| `mc ask "..." \| less` (stdout piping) | ✅ | ✅ |
| `if mc check tests; then deploy; fi` (exit code) | ✅ | ✅ |
| Cron / scheduled tasks | ✅ | ✅ |
| GitHub Actions | ✅ | ✅ |
| JSON output for tool chains | ✅ | ✅ |
| TUI / interactive REPL | ❌ (use Tauri Terminal tile) | ✅ |
| Local tools (bash_run, read_file) | ❌ (MAIC runs them server-side) | ✅ |
| Works offline without MAIC | ❌ | ✅ |
| Bundle size | ~5 MB | ~60-200 MB |
| Build time | 1-2 days | 1-2 weeks |
| Code maintenance | Single HTTP client, ~500 lines | Full TUI + tool registry + auth flow |

**Trade-off**: no interactive standalone TUI. But the Tauri app already has that surface, so it's not lost — just in a different binary.

## CLI surface

```
mc ask "<prompt>"                    # one-shot ask, prints response, exits 0
mc ask "<prompt>" --json             # same, but JSON output
mc ask "<prompt>" --model=<id>       # pick model (default: user default)
mc ask "<prompt>" --stream           # stream tokens to stderr, summary to stdout
mc chat                              # REPL (readline, history, multi-turn)
mc login                             # prompts for email/password, stores JWT in OS keychain
mc logout                            # wipes keychain
mc whoami                            # prints current user + tier + quota
mc usage                             # calls /v1/dashboard/usage, prints table
mc models                            # calls /v1/models, prints table
mc --version                         # 1.1.0-rc55.26
mc --help                            # full help
```

## File structure (one Cargo workspace, 4-6 files)

```
mc-headless/
├── Cargo.toml                # single-binary crate, deps: reqwest, serde, serde_json, tokio, keyring, clap, dialoguer, dirs
├── README.md
├── src/
│   ├── main.rs               # clap CLI, dispatch
│   ├── config.rs             # MAIC URL, user config path (~/.config/mc/config.toml)
│   ├── auth.rs               # JWT in OS keychain (reuse auto_relogin.rs pattern)
│   ├── client.rs             # MAIC HTTP client (reqwest + JSON streaming)
│   ├── ask.rs                # one-shot ask command
│   ├── chat.rs               # REPL (readline + history)
│   ├── output.rs             # plain text / JSON output formatters
│   └── whoami.rs             # whoami + usage + models commands
└── tests/
    ├── ask_test.rs           # mock MAIC server, verify request shape + output
    ├── auth_test.rs          # keychain round-trip
    └── json_output_test.rs   # --json flag shape
```

~500 lines of Rust total. Reuses the `auto_relogin.rs` pattern from MC for JWT keychain storage (well-tested, Lesson 169).

## MAIC endpoints it calls

| Endpoint | Used by |
|---|---|
| `POST /v1/auth/login` | `mc login` |
| `POST /v1/auth/refresh` | silent JWT refresh on 401 |
| `GET  /v1/auth/me` | `mc whoami` |
| `GET  /v1/models` | `mc models` |
| `GET  /v1/dashboard/usage` | `mc usage` |
| `POST /v1/chat/completions` | `mc ask`, `mc chat` (with `tool_execution: "server"` so MAIC runs tools) |

**No MAIC code changes required.** All endpoints already exist. The only MAIC-side work is verifying that `POST /v1/chat/completions` with a non-streaming request from a non-Tauri user-agent works (it does — that's how the Tauri app already calls it).

## Auth flow

1. **First run**: `mc login` prompts for email + password (using `dialoguer` for mask), calls `POST /v1/auth/login`, stores JWT in OS keychain under `mc-cli-jwt`.
2. **Subsequent runs**: `mc ask` reads JWT from keychain, attaches as `Authorization: Bearer <jwt>`.
3. **On 401**: tries `POST /v1/auth/refresh` (uses refresh token if available). On success, updates keychain. On failure, prompts re-login.
4. **Logout**: deletes the keychain entry.

Reuses the exact pattern from `src-tauri/src/auto_relogin.rs` (Lesson 169 — process-global `tokio::sync::Mutex`, AES-256-GCM for the email+password stash, 7 crypto tests).

## Tool behavior

**Default mode: `tool_execution: "server"`** (MAIC runs server-side tools — weather, web_search, get_current_time, calculate, quotas). This is what 95% of headless use cases want.

**`mc ask --no-tools`** flag: passes `tool_execution: "client"` with an empty `tools: []` array. The model responds with text only, no tool calls. Useful for "summarize this" or "explain this" in CI scripts.

**No client-side tool execution** (no `bash_run`, no `read_file`). That's a future enhancement if we want to ship a real "headless" later.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Generic error (network, parse, MAIC 5xx) |
| 2 | Auth error (no JWT, 401 after refresh attempt) |
| 3 | User rate-limited (MAIC 429) |
| 4 | Tool execution error (MAIC returned tool error) |
| 64 | Bad CLI args (matches `sysexits.h`) |
| 130 | Ctrl-C during REPL (matches POSIX SIGINT) |

## JSON output shape (for `mc ask --json`)

```json
{
  "model": "milagro-dev",
  "content": "It's 72°F and sunny in Albuquerque.",
  "tool_calls": [
    {"name": "get_weather", "args": {"location": "Albuquerque"}, "result": "72°F sunny"}
  ],
  "usage": {
    "prompt_tokens": 42,
    "completion_tokens": 18,
    "total_tokens": 60
  },
  "duration_ms": 1234
}
```

`mc ask --json | jq '.content'` works as expected.

## Bundling

Single binary `mc` (~5 MB), distributed alongside the Tauri installers on `milagrocloud.com/downloads/`. Two new files:

- `mc-linux-x86_64` (5 MB) — for Linux/Mac/CI users
- `mc-windows-x86_64.exe` (5 MB) — for Windows users
- (Optional) `mc-macos-aarch64` (5 MB) — for Apple Silicon

Built with `cargo build --release --target x86_64-unknown-linux-musl` for the static binary, no runtime deps. Same approach as our existing launchers.

The Tauri installer does **not** bundle `mc` — it's a separate download for users who want CLI access. The Tauri Terminal tile covers interactive use for everyone else.

## Acceptance tests (the "Definition of Done")

1. **Build**: `cargo build --release` produces 5 MB binary on Linux.
2. **Smoke**: `mc ask "say hello"` prints a non-empty response.
3. **JSON**: `mc ask "say hello" --json | jq .content` prints the response.
4. **Exit code**: `mc ask "exit 0"; echo $?` prints 0.
5. **Login flow**: `mc logout; mc ask "..."` fails with auth error; `mc login` + retry succeeds.
6. **REPL**: `mc chat` opens interactive REPL, multi-turn works, history saved.
7. **Piping**: `mc ask 'list 3 colors' | head -1` returns the first color only.
8. **Cron-ready**: `mc ask 'date' > /tmp/out` works from a cron job (no TTY required).
9. **CI-ready**: a GitHub Actions workflow that runs `mc ask "explain this diff"` and uses the result.
10. **Tests**: 8+ unit tests + 3 integration tests with mock MAIC server, all green.
11. **No MAIC code changes**: deploy only requires scp of the binary, no MAIC restart.

## Out of scope (deliberately)

- TUI / interactive prompts beyond the simple `mc login` (use Tauri Terminal tile)
- Client-side tool execution (no `bash_run`, `read_file` from CLI)
- Local model support (MC must reach MAIC)
- Streaming token output (just summary to stdout; full stream is a future enhancement)
- Slash commands (no `/model`, `/new` — keep CLI surface small)

## Risks

1. **JWT keychain parity across OS**: macOS Keychain, Windows Credential Manager, Linux Secret Service. The `keyring` crate handles this but Linux needs `gnome-keyring` or `kwallet` running. Document the fallback to `MAIC_JWT` env var for headless servers.
2. **MAIC rate limits from CLI users**: same as Tauri users. No new vector.
3. **CI users without interactive login**: handled via `MAIC_JWT` env var fallback (CI sets it as a secret).

## Build sequence

1. Cargo workspace + 4-6 stub files (1 hour)
2. `mc login` + keychain (1-2 hours, reuses `auto_relogin.rs` pattern)
3. `mc whoami` + `mc usage` + `mc models` (1 hour)
4. `mc ask` plain text (2 hours)
5. `mc ask --json` + tests (2 hours)
6. `mc chat` REPL with readline (2 hours)
7. Cross-compile Linux + Windows binaries (1 hour, ~10 min each via existing build scripts)
8. Smoke test on David's Windows + macOS (1 hour)
9. scp to `/opt/maic/dist/` + add download links to Hetzner page (30 min, Lesson 869 auto-derives)

**Total: 1-2 days, can ship as rc55.26.**

## Why this release number

rc55.25 is the public stable. The next meaningful change is rc55.26. The freeze policy (Lesson in MEMORY.md) says: new feature work, breaking API changes, install path = new release. This is a new feature (new binary, new download), so it gets its own release number.
