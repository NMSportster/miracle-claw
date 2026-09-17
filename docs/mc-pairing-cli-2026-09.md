# MC Pairing CLI — `miracle-claw-pair`

**Status:** Design draft (2026-09-09 19:35 MDT), office bot answers integrated (19:45 MDT)
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

## CLI surface (office-bot-revised, 19:45 MDT)

After Homebot's office bot answered the 5 open questions, the CLI
shape locks in as follows. Cross-platform; Linux is the test
environment, Windows is the priority target (David's box).

```
$ miracle-claw-pair --help
miracle-claw-pair — headless pairing server for MC Mobile testing

Usage: miracle-claw-pair [OPTIONS]

Options:
      --port <N>             Pairing HTTP server port
                             (default 0 = OS-assigned, or $MIRACLE_CLAW_PAIR_PORT)
      --bind <mode>          Bind mode: loopback|lan|tailnet
                             (default loopback; mirrors miracle-claw-launcher)
      --identity-file <path> Override the pairing identity file location
                             (default: same path the GUI uses, XDG-aware)
      --ephemeral-identity   Generate an in-memory identity, do not write to disk
      --foreground (default) Block in foreground; log to stdout
      --detach               Detach from terminal, write PID to
                             /tmp/miracle-claw.pair.pid (Unix only)
      --log-level <level>    silent|info|debug (default info)
  -h, --help                 Print help
  -V, --version              Print version

On startup:
  1. Initialize pairing_commands (X25519 keypair if not already on disk)
  2. Bind the pairing HTTP server on <bind>:<port>
  3. Print the heartbeat URL + bound port + instance_id to stdout
  4. Start the heartbeat loop (existing pairing_heartbeat.rs)
  5. Block (or detach)
```

### Resolved design decisions (from office bot's answers)

1. **OS coverage**: implement cross-platform with `cfg(target_os)`.
   Linux is the test environment; Windows is first-class (David's
   machine); macOS ships too (no `#ifdef` walls).
2. **Daemon mode**: **(a) stay-in-foreground** is the default.
   `--detach` flag is available for users who need background
   behavior, but the default mirrors what the launcher already
   does for the gateway. **No systemd unit in v1** — that can
   come later if there's demand.
3. **Port allocation**: `--port <N>` with **default 0 (OS-assigned)**.
   `MIRACLE_CLAW_PAIR_PORT` env var is the fallback if the flag
   is absent. The bound port is printed to stdout (or via the
   existing heartbeat URL) so callers can discover it.
4. **Keypair path**: **default = same path the GUI uses** (XDG-aware
   via `dirs` crate: `~/.local/share/com.milagrocloud.miracle-claw/pairing_identity.json`
   on Linux, `AppData\Roaming\com.milagrocloud.miracle-claw\`
   on Windows). `--identity-file <path>` overrides for tests.
   `--ephemeral-identity` skips persistence entirely (phone-sim
   scenario).
5. **Bind address**: **default = loopback** (safe; doesn't accidentally
   expose pairing to LAN on a coffee-shop wifi). `--bind lan` or
   `--bind tailnet` exposes the right interface for the actual
   phone-pairing scenario, matching the launcher's flag set.

### Example: smoke test (foreground)

```bash
$ miracle-claw-pair --print-info
{"port":48291,"keypair":"/home/user/.local/share/com.milagrocloud.miracle-claw/pairing_identity.json","public_key":"ed25519:abc...","started_at":"2026-09-09T19:40:00Z"}
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

## Open questions — RESOLVED 2026-09-09 19:45 MDT

All 5 open questions answered by Homebot's office bot. The choices
above are now locked.

| # | Question | Resolution |
|---|---|---|
| 1 | OS coverage | Cross-platform via `cfg(target_os)`. Linux is test env; Windows is priority target; macOS is first-class. |
| 2 | Daemon mode | (a) foreground (default) + `--detach` flag. No systemd unit in v1. |
| 3 | Port allocation | `--port <N>` flag, default 0 (OS-assigned), `MIRACLE_CLAW_PAIR_PORT` env var fallback. |
| 4 | Keypair path | Default = same as GUI (XDG via `dirs` crate). `--identity-file <PATH>` override. `--ephemeral-identity` skips persistence. |
| 5 | Bind address | `--bind <mode>` flag (loopback\|lan\|tailnet), default **loopback** (safe). |

The office bot also offered to draft the actual Rust arg parsing
("I have a clear picture of what pairing_commands.rs exposes from
the symbol table") — accepting that would shrink our 2h estimate
slightly. We'll wait to see if they ship it.

## Effort estimate

- Refactor `pairing_commands` to expose inner fns: 30 min
- Write `src/bin/pair.rs` with clap (foreground + loopback default,
  --detach, --port, --bind, --identity-file, --ephemeral-identity,
  --log-level, --print-info, --info-only, --version): 1 hour
- Cross-compile (Linux + Windows via cached `cargo xwin` image) +
  smoke test: 30 min
- Total: **2 hours** (matches the "1-2h" estimate from the 19:35 MDT
  memory entry)

## Build policy — build-but-don't-unveil

Per David's 17:36 MDT direction (2026-09-09): "we leave the rc55.25
for now as the downloadable option, and continue with the builds
but not make them available until we get to a point we are ready
to unveil it."

`miracle-claw-pair` falls under that policy. The flow is:

1. **Build locally** in `/tmp/mc-pair-dist/` (Linux + Windows via
   the cached `cargo xwin` image).
2. **Homebot smoke-tests** the binaries on steeler (his box is
   Linux glibc, so the Linux build is the immediate target).
3. **Hold the build** at `/tmp/mc-pair-dist/` until David calls
   unveil. Do **not** `scp` to Hetzner `/opt/maic/dist/`, do
   **not** update `milagrocloud.com/downloads/`.
4. **When David calls "ship it" / "unveil rc56.0"**: same ship
   procedure as the rest of rc56.0 (see "When unveil happens" in
   `memory/2026-09-09.md`).

Homebot's use case (drive the handshake from a phone simulator)
works fine with the local build — he doesn't need it on the
downloads page. He can `scp` directly from main-adeal to steeler.

## Anti-pattern check

- **AP-launchpad-A**: don't rewrite the existing Tauri command. Add
  a thin `pub(crate) fn` and have both call it. Both code paths
  stay in sync.
- **AP-unveil-A**: build but don't deploy. Stage locally, smoke
  test locally, hold at `/tmp/mc-pair-dist/` until unveil.
- **AP-launchpad-B** (new): don't add a new HTTP route for
  `/pairing/handshake` just for the test scenario. The existing
  `/pairing/qr` and `/pairing/approve` Tauri commands are
  sufficient; if Homebot needs a new endpoint, that's a separate
  design conversation, not a CLI hack.

## Relationship to existing work

- **rc55.26 `mclaw`**: same architecture (small Rust binary, hits
  internal API, no GUI). Reuses build pipeline.
- **Week 1 rc56.0 modules API**: orthogonal — pairing is a
  separate concern from modules. `miracle-claw-pair` is
  complementary infrastructure, not a module.
- **`launcher.rs` / `miracle-claw-launcher`**: orthogonal too —
  that's the launcher binary that finds and spawns the Tauri app.
  `miracle-claw-pair` is its own binary, registered separately.
