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

## MAIC plugin updates

- 2026-08-18 — bumped to 0.1.0 (verified on HomeBot gateway: agent model `maic/milagro-oc-minimax`, 9 plugins loaded incl. MAIC, `/v1/models` 200 OK)
  - depot/maic-plugin/VERSION = 0.1.0
  - depot/maic-plugin/SHA256SUMS = 4 files
  - /home/steeler/.openclaw/extensions/maic/ on steeler, in place
  - Handoff note: /home/steeler/notes/miracle-claw-handoff/README.md

## v1.0.0 installer built — release gate pending

- 2026-08-18 ~09:37 MDT — **REBUILT 13:24 MDT** after David's release-gate fail
  - **v1 installer (FAILED):** 60 MB, MD5 `c2ebe2cc39030564b0760955858d4cd0`
    - Bug: shipped 0-byte `node.exe` placeholder + 124 MB Linux ELF
    - Symptom on Windows: launcher spawn fails with os error 193 (not a valid Win32 application)
    - Root cause: `bundle-runtime.sh` was invoked with default `--target host` (Linux); the script's `touch "$RESOURCES_DIR/node.exe"` placeholder was what got shipped
    - See Lesson 423
  - **v2 installer (FAILED second-run):** 54 MB, MD5 `30e1f5b126b559e6e5cf00f80cbabf59`, SHA256 `fd316b091fc6680ec52e1f53acb2b61ca2aa001c9e08b270c80153d8ff9d9611`
    - Bug: openclaw's gateway refused to start with exit code 78 ("Missing config. Run openclaw setup or set gateway.mode=local (or pass --allow-unconfigured)")
    - First-run on a fresh Windows box has no openclaw.json yet, so the gateway bails
    - Root cause: MC's setup() didn't pre-write an openclaw.json, and the launcher didn't pass `--allow-unconfigured`
  - **v3 installer (FAILED third-run):** 54 MB, MD5 `6f84425e3e944807c812616e8057f234`, SHA256 `c9fbf5fbdb8541da2538b07a926edcd5d35b8c345686e5b1093e208891a17ada`
    - Bug: openclaw rejected `gateway.auth: "none"` (flat string) with "Invalid input"
    - openclaw 2026.7.1+ validates `gateway.auth` as a `.strict()` object with a `mode` field; the flat string form is no longer accepted
    - Root cause: I wrote `"auth": "none"` when the schema wants `"auth": { "mode": "none" }`
  - **v4 installer (CURRENT):** 54 MB, MD5 `7d82f00ea24b9c80f7ee7fa935ea84f8`, SHA256 `6e9483a03fa5d505eaf8336929633197d7b0d7681638ccd3e92339fbe966d200`
    - **Two fixes:**
      - New minimal config uses nested object shape: `gateway: { mode: 'local', bind: 'loopback', auth: { mode: 'none' } }`
      - Added `migrate_legacy_mc_config()` that auto-rewrites any existing v3-style flat-string config to the new shape. MC is the only thing that would have written the legacy shape, so the rewrite is safe.
    - **Verified payload:**
      - `resources/node.exe` = PE32+ Windows x86-64, 87 MB ✓
      - `resources/node` = 0-byte placeholder ✓
      - `miracle-claw.exe` = PE32+ x86-64, 11 MB ✓
      - `miracle-claw-launcher.exe` = PE32+ x86-64, 325 KB, contains `--allow-unconfigured` ✓
      - migrate string present: `[miracle-claw] migrated legacy openclaw.json ...` ✓
    - **Type:** PE32 i386, requires admin elevation
    - **Path:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
    - **Copied to:** `/mnt/c/Users/Adeal/Desktop/MiracleClaw_1.0.0_x64-setup.exe`
    - **Extracted node.exe check:** PE32+ executable for MS Windows 6.00 (console), x86-64, 83 MB — real Node 22.23.2 ✓
    - **No node-dist/:** Linux ELF extracted tree removed (saves ~150 MB) ✓
    - **Built by:** `scripts/build-windows-docker.sh` via Docker image `miracle-claw-build:latest` (cargo-xwin + NSIS + GTK dev headers)
    - **Cross-compile:** `cargo-xwin --target x86_64-pc-windows-msvc` → `miracle-claw.exe` (11 MB) + `miracle-claw-launcher.exe` (318 KB) + bundled Node 22.23.2 Win32 + `openclaw@2026.7.1-2` + MAIC plugin v0.1.0
    - **Build time:** ~22 min (rebuild — Tauri-build re-validated bundle.resources and forced cargo re-run after bundle-runtime.sh wiped + re-staged resources/)
    - **productName aligned:** "MiracleClaw" (no space) per David's v1.7.x convention

- **Release gate:** clean-Windows install test (see `docs/CLEAN-WINDOWS-INSTALL-TEST.md`)
  - Test steps: 7-step verification (installer runs → app opens → chat works → state dir isolated → MAIC plugin found → uninstall clean)
  - **First run (failed):** Node died on install (David reported); fix landed in commits `e66b8ad` + `b5b7eab`
  - **Re-run:** David to install `C:\Users\Adeal\Desktop\MiracleClaw_1.0.0_x64-setup.exe` (v2, MD5 `30e1f5b126b559e6e5cf00f80cbabf59`) and report back each of the 7 steps
  - **Tag trigger:** v1.0.0 tagged once David confirms all 7 pass criteria green

## Lessons added this session (Day 2 — installer build)

- **Lesson 419:** Cross-compile the Tauri sidecar for the **target** triple, not the host. `cargo build --bin launcher --target x86_64-pc-windows-msvc` fails with `linker link.exe not found`; you need `cargo xwin build --bin launcher --target x86_64-pc-windows-msvc` for the MSVC SDK wrapper. Also: tauri-build validates `externalBin` against the host triple too (because the lib build.rs runs even when cross-compiling the bin), so stage BOTH a host-triple placeholder AND a target-triple placeholder before `beforeBuildCommand` runs.
- **Lesson 420:** File extension on the launcher placeholder is determined by the **target triple**, not the host. When cross-compiling for Windows from a Linux Docker container, the placeholder must be `miracle-claw-launcher-x86_64-pc-windows-msvc.exe` (with `.exe`), not just `...-msvc` (without).
- **Lesson 421:** When the launcher's `[dependencies]` is shared with the main binary's tauri dep tree (cargo workspace has only one Cargo.toml), the launcher's Linux-host build pulls in GTK headers too (tauri → webkit → gtk → gdk). Install `libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev` in the Docker image even though the launcher is conceptually std-only. Proper fix would be to split the launcher into its own crate; libs are cheaper.
- **Lesson 422:** `scripts/build-windows-docker.sh`'s "auto-copy to Desktop" path silently no-ops when `chat.rs` doesn't exist (was reading `const APP_VERSION` for the `+N` build suffix). Drop the suffix lookup when shipping clean `MAJOR.MINOR.PATCH` tags.
- **Lesson 423:** `scripts/bundle-runtime.sh`'s default `--target host` is a footgun for cross-compile builds. If you run it once on Linux dev, `resources/node.exe` is a 0-byte placeholder via `touch`, and that placeholder gets shipped in the Windows installer. The script's comment says "placeholder so tauri-build's pre-flight validation passes" — but Tauri's pre-flight is the only thing that placeholder is good for. **Fix:** `scripts/build-windows-docker.sh` must invoke `bundle-runtime.sh --target windows --force` BEFORE the `npm run tauri -- build` runs (which we now do). Without `--force`, the Linux-tarball cache makes the script skip downloading the Windows Node. Without `--target windows`, the Linux ELF gets copied as `resources/node` (124 MB). Also: when `--target windows`, don't ship the Linux `node-dist/` subtree (~150 MB dead weight) and use `python3 -m zipfile` as a fallback when `unzip` isn't installed (some Linux sandboxes / Docker base images omit it).
