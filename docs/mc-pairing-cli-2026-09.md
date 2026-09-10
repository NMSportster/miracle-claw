# MC Pairing CLI — `miracle-claw-pair`

**Status:** Design draft (2026-09-09 19:35 MDT, Homebot request)
**Owner:** ABQShop Claw (taking the request from Homebot on steeler)
**Target release:** rc56.0 (Week 2.5, after launchpad ships)
**Requested by:** Homebot (steeler, 100.74.235.63) for MC Mobile
**Closes:** `mc_register_desktop` Tauri-only restriction — lets phone
simulators drive the full pairing handshake from curl

---

## What this is

A small Rust binary `miracle-claw-pair` (~3 MB) that runs the same
desktop-side pairing logic as the Tauri app's `mc_register_desktop`
command, but **without a WebView**. The pairing HTTP server starts
already-initialized, the identity persists to disk, and the binary
stays running until SIGINT — so a phone simulator (or any HTTP
client) can drive the full handshake end-to-end with `curl`.

This is the **mclaw pattern extended to the parent app**: a tiny
Rust binary that hits the same internal logic the Tauri app uses,
just without the GUI. `mclaw` talks to MAIC; `miracle-claw-pair`
talks to the local pairing listener that the Tauri app would
normally spin up.

## Why Homebot needs it

He's working on **MC Mobile** — the phone app that pairs with a
desktop instance. The pairing flow today is:

1. User opens Miracle Claw on desktop
2. `mc_register_desktop` Tauri command runs, generates an Ed25519
   keypair, starts a local HTTP server, returns a QR code
3. Phone scans QR, phone app does a handshake over HTTP

For testing, Homebot wants to drive the **phone side** of the
handshake from a phone simulator using `curl`. But step 2 requires
the Tauri app to be running, which means a WebView, which means a
display server — all unnecessary for the test. The CLI mode lets
him start the desktop side of the handshake from a terminal and
hit it with curl from the simulator.

## What it gives Homebot

| Capability | Today (Tauri only) | With `miracle-claw-pair` |
|---|---|---|
| Start pairing server from terminal | ❌ (must open app) | ✅ |
| Persist identity across restarts | ⚠️ (in Tauri app data dir) | ✅ (XDG-compliant path) |
| Drive handshake from curl | ❌ (Tauri required) | ✅ |
| Drive handshake from phone sim | ❌ | ✅ |
| `--daemon` mode (fork + exit) | ❌ | ✅ |
| `--port <N>` override | ❌ (runtime-allocated) | ✅ |
| `--keypair <PATH>` override | ❌ (in-memory) | ✅ |
| JSON output for CI / smoke tests | ❌ | ✅ |
| Bundle size | (whole Tauri app) | ~3 MB |
| Cross-build time | (full NSIS pipeline) | ~3 min via `cargo xwin` |

## Architecture

```
┌──────────────────────────┐         ┌──────────────────────┐
│  miracle-claw-pair       │         │  MC Mobile simulator │
│  (Rust, no GUI)          │         │  (any HTTP client)   │
│                          │         │                      │
│  ┌────────────────────┐  │   HTTP  │                      │
│  │  pairing_commands  │◄─┼─────────┤  curl / pairing / …  │
│  │  module (existing) │  │         │                      │
│  └────────────────────┘  │         └──────────────────────┘
│  ┌────────────────────┐  │
│  │  Ed25519 keypair   │  │
│  │  ~/.local/share/…  │  │
│  │  pairing_identity  │  │
│  └────────────────────┘  │
└──────────────────────────┘
```

The CLI reuses the existing `pairing_commands` module (which
`mc_register_desktop` calls) directly. The `#[tauri::command]`
attribute is purely a registration wrapper around the underlying
`fn register_desktop(...)` — calling the inner fn from `main()`
requires either:
- a `pub fn` re-export of the inner logic, OR
- constructing a minimal Tauri `App` / `State` shim

We prefer the **re-export** approach: it's a smaller change and
keeps the CLI independent of the Tauri runtime. The `#[tauri::command]`
decorated fn stays in `lib.rs`; we add a `pub(crate) fn
register_desktop_inner(...)` next to it that both call.

## CLI surface

```
$ miracle-claw-pair --help
miracle-claw-pair — headless pairing server for MC Mobile testing

Usage: miracle-claw-pair [OPTIONS]

Options:
  -p, --port <PORT>          Fixed port to bind (default: 0 = runtime-allocate)
      --keypair <PATH>       Path to persistent keypair (default: OS data dir)
      --daemon               Fork into background and exit (Unix only)
      --print-info           Print QR + port as JSON to stdout, then exit
      --info-only            Just print the existing identity (no server start)
  -v, --verbose              Increase log verbosity
  -h, --help                 Print help
  -V, --version              Print version
```

### Example: smoke test (foreground)

```bash
$ miracle-claw-pair --print-info
{"port":48291,"keypair":"/home/user/.local/share/com.milagrocloud.miracle-claw/pairing_identity.json","public_key":"ed25519:abc...","started_at":"2026-09-09T19:40:00Z"}
```

### Example: persistent server (production-ish)

```bash
$ miracle-claw-pair --port 9229 --keypair /var/lib/mc/pair.json
[2026-09-09 19:40:00] listening on 127.0.0.1:9229
[2026-09-09 19:40:00] keypair loaded from /var/lib/mc/pair.json
[2026-09-09 19:40:15] handshake from 127.0.0.1:51234 — token verified
[2026-09-09 19:40:15] mobile device registered (id=phone-abc)
^C
[2026-09-09 19:42:08] SIGINT received, shutting down
```

### Example: phone simulator end-to-end (Homebot's actual use case)

```bash
# Terminal 1 — start desktop side
$ miracle-claw-pair --port 9229 --keypair /tmp/pair.json
[2026-09-09 19:40:00] listening on 127.0.0.1:9229

# Terminal 2 — phone simulator side
$ curl http://127.0.0.1:9229/pairing/qr
{"qr_data":"milagro://pair?token=...&pubkey=ed25519:..."}

$ curl -X POST http://127.0.0.1:9229/pairing/handshake \
    -H "Content-Type: application/json" \
    -d '{"device_id":"phone-1","public_key":"ed25519:..."}'
{"status":"approved","device_token":"...","maic_url":"https://maicserver.com"}

$ curl -X POST http://127.0.0.1:9229/pairing/approve \
    -H "Content-Type: application/json" \
    -d '{"device_id":"phone-1"}'
{"status":"ok"}
```

## Build plan

### 1. Extract `pub(crate) fn register_desktop_inner`

In `src-tauri/src/lib.rs`, next to `#[tauri::command] fn mc_register_desktop()`:

```rust
pub(crate) fn register_desktop_inner(
    identity: &PairingIdentity,
    state: &PairingState,
) -> Result<RegisterDesktopResponse, String> {
    // existing logic from mc_register_desktop
}

#[tauri::command]
fn mc_register_desktop(
    state: tauri::State<'_, PairingState>,
) -> Result<RegisterDesktopResponse, String> {
    let identity = load_or_generate_identity()?;
    register_desktop_inner(&identity, &state)
}
```

Same pattern for `mc_pairing_qr_code` and `mc_pairing_approve`.

### 2. Create `src-tauri/src/bin/pair.rs`

```rust
use miracle_claw_lib::pairing_commands::{
    register_desktop_inner, pairing_qr_code_inner, pairing_approve_inner,
    PairingIdentity, PairingState,
};
// … CLI parsing with clap, then the same flow as the Tauri command
```

Wire as a `[[bin]]` target in `src-tauri/Cargo.toml`:

```toml
[[bin]]
name = "miracle-claw-pair"
path = "src/bin/pair.rs"
```

### 3. Cross-compile via `cargo xwin`

Same Docker image as `mclaw` (`messense/cargo-xwin:latest`, already
cached locally per Lesson 875). 3-minute build, output:

- `miracle-claw-pair_1.1.0-rc56.0_x86_64-pc-windows-msvc.exe` (3 MB)
- `miracle-claw-pair_1.1.0-rc56.0_x86_64-unknown-linux-gnu` (3 MB)
- macOS deferred (no osxcross in env, per existing `mclaw` posture)

### 4. Smoke test

- Generate keypair, verify file at OS data dir
- Bind on port 9229, hit /pairing/qr with curl, verify JSON shape
- POST /pairing/handshake, verify approval flow
- Restart, verify keypair persists (same public key)
- SIGINT, verify clean shutdown

## Open questions (need Homebot's input)

1. **OS coverage**: Linux + Windows, or just one? Default: both.
2. **Daemon mode**: foreground blocking (default) + `--daemon` flag,
   or true double-fork? Default: foreground + `--daemon` flag (simpler,
   matches `mclaw` posture).
3. **Port allocation**: runtime-allocated (current Tauri behavior,
   prints to stdout) or fixed (e.g. 9229 for dev parity)? Default:
   runtime with `--port` override.
4. **Keypair location on Windows**: AppData\Roaming\com.milagrocloud.miracle-claw\
   or the same `~/.local/share/…` path the Tauri app uses? Default:
   match the Tauri app's path (XDG-aware via `dirs` crate).
5. **Auth on the pairing endpoints**: local-only (127.0.0.1) or
   LAN-accessible (0.0.0.0) for testing on real phones? Default:
   local-only, with `--bind 0.0.0.0` override.

## Effort estimate

- Refactor `pairing_commands` to expose inner fns: 30 min
- Write `src/bin/pair.rs` with clap: 1 hour
- Cross-compile + smoke test: 30 min
- Total: **2 hours** (matches the "1-2h" estimate from the 19:35 MDT
  memory entry)

## Anti-pattern check

- **AP-launchpad-A**: don't rewrite the existing Tauri command. Add
  a thin `pub(crate) fn` and have both call it. Both code paths
  stay in sync.
- **AP-reveal-A**: this binary is build-but-don't-unveil. Don't ship
  to Hetzner downloads. Stage locally and let Homebot smoke test
  from `/tmp/mc-pair-dist/`.

## Relationship to existing work

- **rc55.26 `mclaw`**: same architecture (small Rust binary, hits
  internal API, no GUI). Reuses build pipeline.
- **Week 1 rc56.0 modules API**: orthogonal — pairing is a
  separate concern from modules. `miracle-claw-pair` is
  complementary infrastructure, not a module.
- **`launcher.rs` / `miracle-claw-launcher`**: orthogonal too —
  that's the launcher binary that finds and spawns the Tauri app.
  `miracle-claw-pair` is its own binary, registered separately.
