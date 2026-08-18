# Miracle Claw v1.0.0 — Build Status

**Scaffolded:** 2026-08-17 18:52 MDT
**First-run path shipped:** 2026-08-18 07:14 MDT
**Runtime bundled:** 2026-08-18 08:05 MDT
**State dir isolated:** 2026-08-18 08:13 MDT
**Branch:** `master`
**Release tag:** Not yet (clean-Windows .exe test is the v1.0.0 gate)

## What's done

✅ Scaffold (commit `680f002`)
✅ Sidecar binary `miracle-claw-launcher` — Rust, ~200 LoC, std-only, arg-validated
✅ `lib.rs` first-run path:
   - Resource resolution (production env var + dev fallback walk-up)
   - Idempotent MAIC plugin copy (SHA-256 manifest, skip-on-match)
   - `ensure_maic_manifest_compat` backfills `configSchema` on the plugin manifest after copy (openclaw 2026.7.1+ requires it)
   - Read-only `check_openclaw_json` (warn-only; openclaw 2026.7.1+ rejects the old `plugins.roots` and `pluginRoots` root keys — plugins auto-discover from `<stateDir>/extensions/`)
   - Tauri sidecar spawn via allowlist
   - Background log capture (stdout/stderr tagged and prefixed)
   - TCP connect poll for gateway readiness (15s cap, exponential backoff 100ms→1s)
   - RunEvent::ExitRequested child kill handler
✅ Capability allowlist `src-tauri/capabilities/main.json` — `core:default` + `shell:allow-execute` for the launcher with port-validator regex `^[0-9]{4,5}$`
✅ Build script `scripts/build-launcher-sidecar.sh` — handles tauri-build's pre-build sidecar validation via zero-byte placeholder
✅ Pinned Tauri =2.11.5, tauri-plugin-shell =2.3.5, tauri-build =2.6.3
✅ sha2 dependency added for the idempotent MAIC plugin manifest
✅ Smoke tested on Linux dev — full chain boots in 5 seconds, openclaw.json
   patched without overwriting user config
✅ **Runtime bundled** (commit `76d9be1`):
   - `scripts/bundle-runtime.sh` — idempotent, builds `src-tauri/resources/` from npm registry + Node 22.23.2 tarballs
   - Node 22.23.2 (matches openclaw `engines: >=22.22.3 <23`)
   - `openclaw@2026.7.1-2` + `node_modules` (270 packages, **flat layout via `pnpm --config.nodeLinker=hoisted`** — essential for Tauri's bundler, which strips pnpm's symlink-tree)
   - `resources/node` (copied binary) + `resources/node.exe` (Windows placeholder for tauri-build pre-flight)
   - `resources/maic-plugin/` (4 files) pre-staged so the SHA-checked copy actually fires on real installs
   - `resources/{package.json, openclaw.mjs, dist, docs, skills, scripts, patches, src, LICENSE, CHANGELOG, README, THIRD_PARTY_NOTICES, BUNDLE_VERSION}` — full bundled install
   - Total: ~678M on disk (Node 204M + node_modules 248M + dist 96M + rest 130M)
   - Verified: `nohup ./node openclaw.mjs gateway --port 28812 --bind loopback --auth none` boots clean, HTTP server listening, WebSocket connects, 9 manage-plugins loaded, MAIC detected by doctor as "managed npm plugin package"

## What's NOT done (next passes)

✅ Copy the MAIC plugin source into `resources/maic-plugin/`
✅ **State dir isolation** (commit `4b5e369`):
   - Linux: `~/.openclaw/` → `~/.miracle-claw/`
   - Windows: `%APPDATA%\MiracleClaw` (unchanged)
   - Smoke tested on Linux dev: gateway boots clean in isolated dir, system openclaw untouched
   - openclaw auto-migrates exec-approvals on first boot — existing users keep their history

⏳ **Linux dev full chain test** (after bundle, partial):
   - Webview → gateway ✅ (verified via curl to gateway port)
   - MAIC plugin load ✅ (doctor detects "1 managed npm plugin package")
   - End-to-end chat send 'hello' ⏳ (not yet — needs Tauri webview launch)

⏳ **Tomorrow's clean-Windows .exe test**:
   - Run full NSIS bundle in Docker (see skill: tauri-windows-cross-compile)
   - Copy installer to Windows desktop
   - Install on a clean Windows VM (no Node, no openclaw)
   - Confirm first-run path lands + chat works
   - **Only then** tag `v1.0.0`

⏳ **Branding pass**:
   - ADeal green theme
   - Splash screen
   - System tray icon
   - Currently using old MC lobster icons

## Build commands

```bash
# Build the launcher sidecar (auto-runs via beforeBuildCommand)
bash scripts/build-launcher-sidecar.sh

# Build main Tauri webview binary
cd src-tauri && cargo build --bin miracle-claw

# Linux dev with full hot-reload
cargo tauri dev

# Smoke-test the sidecar alone (Linux)
./src-tauri/binaries/miracle-claw-launcher-x86_64-unknown-linux-gnu --help
./src-tauri/binaries/miracle-claw-launcher-x86_64-unknown-linux-gnu --gateway-port 28789
```

## Smoke test result (2026-08-18 08:13 MDT, Linux, isolated state dir)

```bash
cd src-tauri/resources && \
  OPENCLAW_STATE_DIR=/home/adeal/.miracle-claw ./node openclaw.mjs gateway \
    --port 28814 --bind loopback --auth none --allow-unconfigured
# → "[state-migrations] Auto-migrated legacy state"
# → "[gateway] agent runtime plugins pre-warmed in 106ms"
# → http server listening on 28814
# → ~/.openclaw/ untouched, ~/.miracle-claw/ populated cleanly
```

## Smoke test result (2026-08-18 07:14 MDT, Linux)

```
$ ./target/debug/miracle-claw
[miracle-claw] setup: resources = .../target/debug/resources
[miracle-claw] maic plugin: present (no change)
[miracle-claw] openclaw.json: patched
[miracle-claw] spawning sidecar: miracle-claw-launcher --gateway-port 28789
[launcher.stderr] [miracle-claw-launcher] booting gateway on port 28789 (bind=loopback, auth=none)
[launcher.stderr] [miracle-claw-launcher] node=/usr/bin/node resources=...
[launcher.stderr] [miracle-claw-launcher] node openclaw pid=46105
[launcher.terminated] code=Some(0) signal=None
```

Verified after the smoke run:
- `~/.openclaw/openclaw.json` gained `pluginRoots: [.../extensions]` and `plugins.roots: [.../extensions]` (both keys, idempotent)
- `~/.openclaw/extensions/maic/` files untouched (source not bundled yet)
- Sidecar binary size: 4.8 MB ELF
- Main binary size: ~190 MB debug build (release will be ~10-20 MB)

## Architectural decisions locked

| Concern | Decision |
|---|---|
| Bundle strategy | (b) self-contained — `openclaw@2026.7.1-2` from npm registry + Node 22 LTS |
| Spawn shape | Rust sidecar (no Node escape hatch in allowlist) |
| Allowlist | `^[0-9]{4,5}$` port validator, no other args exposed |
| Bundle ID | `com.adealauto.miracle-claw` |
| State dir (Win) | `%APPDATA%\MiracleClaw\` |
| State dir (*nix) | `$HOME/.miracle-claw/` (isolated from system openclaw at `~/.openclaw/`) |
| Bind mode | loopback (per-user Tauri window, same box) |
| Auth mode | none (trusted local client) |

## Lessons applied

- **351:** All three version strings (Cargo.toml, tauri.conf.json, package.json) are `1.0.0`.
- **352:** Not `git add .` — staged only changed files explicitly.
- **353:** Sidecar build is fast (<30s warm). Tauri-build checks run every cargo invocation — placeholder is necessary.
- **169 (Memory):** `tool_execution` flag is irrelevant for native Tauri — MAIC plugin handles transport patching automatically.
- **395:** "When you find yourself repeating 'we need to add X' and X is already working in a different stack — adopt the working stack." OpenClaw WebChat is the working stack; we're wrapping it.
- **404:** Distinguish "old project artifacts" (move to old_mc_files) from sibling packages. Bundle strategy picks (b) — full prod npm install — based on registry unpacked size.
- **405:** Scaffold to a green light, not a working build. Today's work turned that green-light scaffold into a working first-run path. Two commits in this session, neither tagged v1.0.0.
- **406:** STATUS.md > TODO.md. This file is the orientation doc.

## Files in this session

| File | Status | LoC |
|---|---|---|
| `src-tauri/src/lib.rs` | new | 320 |
| `src-tauri/src/launcher.rs` | new | 320 |
| `src-tauri/src/launcher_info.rs` | new | 30 |
| `src-tauri/Cargo.toml` | updated | 30 |
| `src-tauri/tauri.conf.json` | updated | 60 |
| `src-tauri/capabilities/main.json` | new | 25 |
| `scripts/build-launcher-sidecar.sh` | new | 65 |
