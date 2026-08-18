# Clean-Windows Install Test — Miracle Claw v1.0.0

This is the **release gate** for v1.0.0. The installer must install cleanly
on a Windows box with **no prerequisites** (no Node.js, no Rust, no OpenClaw
preinstalled) and produce a working desktop app.

## Installer

- **File:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
- **Size:** ~60 MB
- **MD5:** `c2ebe2cc39030564b0760955858d4cd0`
- **Type:** NSIS self-extracting installer (Nullsoft v3.11-1), 7 sections,
  requires admin elevation
- **Bundled in installer:**
  - `miracle-claw.exe` (~11 MB Tauri webview host)
  - `miracle-claw-launcher.exe` (Tauri sidecar that boots OpenClaw gateway)
  - Node.js 22.23.2 (`node.exe`)
  - `openclaw@2026.7.1-2` (full runtime with all deps)
  - MAIC plugin v0.1.0 (4 files: index.js, openclaw.plugin.json, package.json, test_plugin.js)
- **Installed to:** `C:\Program Files\MiracleClaw\` (default NSIS path)
- **State dir:** `%APPDATA%\MiracleClaw\` (ISOLATED from system OpenClaw at
  `%APPDATA%\openclaw\`)

## Test Environment

Target box: clean Windows 10/11 VM with no Node.js, no Rust, no OpenClaw
installed. David runs this test today.

## Test Steps

1. **Copy installer to Windows box:**
   ```
   cp /home/adeal/.openclaw/workspace/projects/miracle-claw/dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe \
      /mnt/c/Users/Adeal/Desktop/
   ```
   (Already done — installer is on David's desktop.)

2. **Verify file:**
   - Right-click → Properties → Digital Signatures (none expected — unsigned
     dev build; see Lesson 213 for production signing plan).
   - File size: 60 MB.
   - MD5: should match `c2ebe2cc39030564b0760955858d4cd0` (right-click →
     `certutil -hashfile MiracleClaw_1.0.0_x64-setup.exe MD5` in cmd).

3. **Run installer:**
   - Double-click `MiracleClaw_1.0.0_x64-setup.exe`.
   - UAC prompt → Yes (requires admin elevation).
   - NSIS install wizard:
     - Welcome → Next
     - Install location → default `C:\Program Files\MiracleClaw\` → Install
     - Wait ~10s for file copy + registry entries
     - Finish → launches app (or check "Run Miracle Claw" + Finish).

4. **First-run path verification:**
   - Window opens, title says "MiracleClaw", 1200x820 default size.
   - webview loads `http://localhost:28789/` (the bundled OpenClaw gateway).
   - Tauri main process spawns `miracle-claw-launcher.exe` as a sidecar.
   - Launcher:
     - Reads bundled resources (node.exe, openclaw.mjs, package.json,
       node_modules/, dist/, maic-plugin/).
     - Spawns `node openclaw.mjs gateway --port 28789 --bind loopback --auth
       none --allow-unconfigured` (first-run, empty state dir).
     - Waits for gateway to listen on 127.0.0.1:28789.
   - Gateway bootstraps:
     - Creates `%APPDATA%\MiracleClaw\` state dir.
     - Creates `%APPDATA%\MiracleClaw\extensions\` dir.
     - Copies MAIC plugin files from resources to extensions dir.
     - Adds MAIC plugin to discovered plugins list.
     - Selects default model: `maic/milagro-oc-minimax` (cloud MiniMax-M3).
     - Logs go to `%APPDATA%\MiracleClaw\logs\gateway.log`.

5. **Smoke test chat:**
   - In the webview, type a simple message ("hello") and send.
   - Expect: streamed response from MAIC (`milagro-oc-minimax`).
   - If the message gets "I don't have tools" response, the model
     tier didn't pick up the server-side tools — check the gateway log.
   - If "MAIC returned 401" appears, MAIC API key is stale — re-run
     `~/.config/secrets/bin/maic_self_heal.sh` on main-adeal to refresh.

6. **State dir isolation check:**
   - Open `%APPDATA%\` in File Explorer.
   - Confirm `MiracleClaw\` folder exists.
   - Confirm `openclaw\` (system install) is **NOT** created.
   - Confirm `MiracleClaw\extensions\` has the 4 MAIC plugin files.

7. **Clean uninstall:**
   - Settings → Apps → Installed apps → MiracleClaw → Uninstall.
   - Or: re-run the installer → it detects the install → offers Uninstall.
   - Confirm `C:\Program Files\MiracleClaw\` removed.
   - Confirm `%APPDATA%\MiracleClaw\` NOT auto-removed (Tauri's NSIS
     config doesn't have a `deleteAppDataOnUninstall` flag — by design,
     we keep state across installs).

## Pass Criteria

All of these must be true to tag v1.0.0:

- [ ] Installer runs without errors on clean Windows.
- [ ] App window opens, webview loads.
- [ ] Chat returns a streamed MAIC response.
- [ ] State dir at `%APPDATA%\MiracleClaw\` (NOT `%APPDATA%\openclaw\`).
- [ ] MAIC plugin discovered (4 files in `%APPDATA%\MiracleClaw\extensions\`).
- [ ] Uninstall cleanly removes `C:\Program Files\MiracleClaw\`.
- [ ] No crash logs in `%APPDATA%\MiracleClaw\logs\`.

## Known Limitations (v1.0.0)

- **No code signing** — Windows SmartScreen will warn "Unknown publisher".
  Click "More info" → "Run anyway" to proceed. (Production signing deferred
  per Lesson 213.)
- **No auto-update** — Tauri updater not configured in v1.0.0. Manual
  reinstall to upgrade.
- **No system tray** — close button exits the app. (Branding pass deferred.)
- **Default theme** — ADeal green theme not applied yet. Uses bundled
  frontend defaults.
- **No installer icon polish** — uses generic Tauri icon. (Branding pass
  deferred.)

## If It Fails

- **UAC elevation denied** → re-run as Administrator.
- **Antivirus quarantine** → add exclusion for `%LOCALAPPDATA%\Programs\MiracleClaw`
  and `C:\Program Files\MiracleClaw\`.
- **"VCRUNTIME140.dll not found"** → install MSVC redist (rare — Tauri
  build should bundle it; only fails if installer stripped manifests).
- **"api.maicserver.com connection refused"** → check the gateway log
  for the actual error. Likely MAIC JWT expired; run `maic_self_heal.sh`.
- **Window opens but webview blank** → port 28789 not listening. Check
  `%APPDATA%\MiracleClaw\logs\launcher.log` and `gateway.log`.

## Status

- [ ] Test passed on David's clean Windows box.
- [ ] v1.0.0 tagged (`git tag -a v1.0.0 -m "..."`).
- [ ] STATUS.md updated with "v1.0.0 released" entry.

## Build Provenance

- Built: 2026-08-18 ~09:37 MDT
- Image: `miracle-claw-build:latest` (7.54 GB Docker image with Rust 1.89 +
  Node 20 + cargo-xwin + NSIS + GTK dev headers)
- Cross-compile: cargo-xwin → x86_64-pc-windows-msvc
- Build host: Main-Adeal (WSL2 Debian trixie)
- Bundled runtime: `openclaw@2026.7.1-2` + Node 22.23.2 (see Lesson 351
  for why we bundle full latest stable)
- Bundle script: `scripts/bundle-runtime.sh` (Lesson 414 — uses isolated
  `~/.miracle-claw/` / `%APPDATA%\MiracleClaw` state dir)