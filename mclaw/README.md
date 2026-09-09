# mclaw — Miracle Claw CLI

Ask MAIC from any shell, cron, or CI. A headless companion to the Tauri
desktop app.

## Install

Pre-built binaries are at <https://milagrocloud.com/downloads/>:

| Platform | File |
|---|---|
| Linux x86_64 | `mclaw_1.1.0-rc55.26_linux_x86_64.tar.gz` |
| Windows x86_64 | `mclaw_1.1.0-rc55.26_windows_x86_64.zip` |
| macOS aarch64 | `mclaw_1.1.0-rc55.26_macos_aarch64.tar.gz` |

```bash
# Linux: extract and put on PATH
tar xzf mclaw_1.1.0-rc55.26_linux_x86_64.tar.gz
sudo mv mclaw /usr/local/bin/

# Verify
mclaw --version
# mclaw 1.1.0-rc55.26
```

## Quick start

```bash
# 1. Log in (stores JWT in OS keychain)
mclaw login

# 2. Ask a question
mclaw ask "What is 2 + 2?"

# 3. JSON for scripts
mclaw ask "What is 2 + 2?" --json | jq -r .content
# "2 + 2 = 4."

# 4. Pipe a prompt from stdin
echo "Summarize the last 3 git commits" | mclaw ask

# 5. List available models
mclaw models

# 6. Interactive REPL
mclaw chat
```

## Headless / CI usage

In environments without a keychain (CI, Docker, headless servers), set
the `MAIC_JWT` env var instead of running `mclaw login`:

```bash
# In CI: store the JWT as a secret, inject at runtime
export MAIC_JWT="eyJhbGciOiJIUzI1NiIs..."
mclaw ask "Summarize the PR diff"
```

## Commands

| Command | Description |
|---|---|
| `mclaw ask "<prompt>"` | One-shot ask. Flags: `--json`, `--model=<id>`, `--no-tools` |
| `mclaw chat` | Interactive REPL (readline + history + multi-turn) |
| `mclaw login` | Log in to MAIC and store JWT in OS keychain |
| `mclaw logout` | Wipe JWT from keychain |
| `mclaw whoami` | Print current user, tier, plan |
| `mclaw usage` | Stub — coming in rc55.27 |
| `mclaw models` | List available models |
| `mclaw --version` | Print version |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Generic MAIC/network error |
| 2 | Auth error (run `mclaw login` or set `MAIC_JWT`) |
| 3 | Rate limited |
| 4 | Tool execution error |
| 64 | Bad CLI args |
| 130 | SIGINT (Ctrl-C) |

## Cron example

```bash
# /etc/cron.d/mclaw-daily-summary
0 7 * * *  david  /usr/local/bin/mclaw ask "Good morning! Give me a 3-bullet summary of the day" > /tmp/morning.txt 2>&1
```

## Build from source

Requires Rust 1.88+ (for keyring 4).

```bash
cd mclaw
cargo build --release
./target/release/mclaw --version
```

Cross-compile for Windows:

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Tests

```bash
cargo test                 # 3 config + 3 ask = 6 pass, 1 ignored
cargo test -- --ignored    # Also run the keychain round-trip (requires OS keychain)
```

## License

MIT
