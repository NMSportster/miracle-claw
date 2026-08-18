# Changelog

All notable changes to Miracle Claw are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — v1.0.1 polish queue

### Things planned (no rebuild required)
- Tag v1.0.1 once David confirms v5 installer fully loads.
- Formalize Lesson 427 in MEMORY.md (audit user-facing strings at v1.0.0 GA).
- Capture Lesson 428 (Tauri `bundle.resources` is explicit allowlist) — already in MEMORY.md.

### Things planned (require rebuild)
- Narrow `bundle.resources` to ship only the template subdirectories:
  - `resources/src/agents/templates` (1 file: HEARTBEAT.md)
  - `resources/docs/reference/templates` (13 files: AGENTS, SOUL, USER, IDENTITY, TOOLS, BOOTSTRAP, BOOT + .dev.md variants)
  - Saves ~1 MB on installer (was ~750 docs i18n files).
- Land the `capabilities/main.json` description change from branding cleanup (`aee2702`).
  - Current embedded text: "boot the OpenClaw gateway, host the OpenClaw WebChat"
  - Target: "boot the bundled chat gateway, host the chat UI"
- ADeal green branding (splash, theme, system tray icon).
- Code signing (Windows SmartScreen "Unknown publisher" → gone).
- Auto-updater (manual reinstall for now).
- Launcher → separate-crate refactor (currently a bin in the main crate).

## [v1.0.0 hotfix-2] — 2026-08-18 16:22 MDT (v5 installer)

### Fixed
- **Chat error: "Missing workspace template: AGENTS.md"** after the gateway started.
  - Root cause: `tauri.conf.json: bundle.resources` was an explicit allowlist that
    omitted `resources/src` and `resources/docs`. Tauri silently omitted those
    whole directories from the installer payload. v4 shipped 31,166 files instead
    of the 31,980 at source.
  - Fix: Added `"resources/src"` and `"resources/docs"` to `bundle.resources`.
  - Verified via 7z extraction: `resources/src/agents/templates/HEARTBEAT.md` and
    `resources/docs/reference/templates/AGENTS.md` (plus 12 other templates) are
    now present. Payload grew to 31,903 files (+737).
  - Lesson 428 captured.

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
- **Size:** 55 MB
- **MD5:** `49adfa6b198a5cb3906021ce32f2be08`
- **SHA256:** `dab5d0ed16ec0de5c9107a87eb4230aa89628a08ddd07dfd30a5d44daa66c48d`
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.0_x64-setup.exe`
- **Source commit:** `7b7cd8a` (root fix), `96a5fde` (STATUS update), `282ba41` (test doc)

## [v1.0.0 hotfix-1] — 2026-08-18 14:54 MDT (v4 installer)

### Fixed
- **`gateway.auth: Invalid input`** — openclaw 2026.7.1+ validates `gateway.auth`
  as a `.strict()` object with `mode` field (zod enum: `none|token|password|trusted-proxy`).
  v3 wrote flat string `"auth": "none"`; the schema rejected it.
- **Fix:** Pre-write nested object shape `"auth": { "mode": "none" }`.
- **Auto-migrate:** Added `migrate_legacy_mc_config()` that detects any existing
  flat-string config and rewrites it. Safe because MC is the only thing that
  would have written the legacy shape.
- Lesson 426 captured (verify schema from bundled validator, not memory).

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
- **Size:** 54 MB
- **MD5:** `7d82f00ea24b9c80f7ee7fa935ea84f8`
- **SHA256:** `6e9483a03fa5d505eaf8336929633197d7b0d7681638ccd3e92339fbe966d200`
- **Source commit:** `48f5810` (STATUS), `204bf39` (root fix)

### Verified by David
> "Bam. It loaded."

Chat rendered at `http://localhost:28789/`. The v1.0.0 tag was fired at this commit
(`48f5810`). However, **v4 was later found to miss the workspace templates** (see
v5 / hotfix-2 above). The tag is therefore pinned to a commit that does NOT
match the installable state. v5 is the installable state.

## [v1.0.0] — 2026-08-18 15:37 MDT (tagged)

### Added
- **First stable release** of Miracle Claw.
- Self-contained Tauri 2 desktop wrapper around OpenClaw WebChat + MAIC.
- Bundles Node 22.23.2 + openclaw@2026.7.1-2 (no system Node required).
- Bundled MAIC plugin with auto-install on first run.
- Isolated state dir: `~/.miracle-claw/` (Linux) / `%APPDATA%\MiracleClaw` (Windows).
- Loopback-only gateway on port 28789.
- First-run migration from legacy v3 config to nested object shape (auto).
- Belt-and-suspenders: `--allow-unconfigured` flag for headless fallback.

### Verified
- David: "Bam. It loaded." — chat rendered at `http://localhost:28789/`.
- **NOTE:** This tag was pinned to commit `48f5810` (v4 installer). v4 was
  later found to be missing workspace templates in the install. The v5
  hotfix-2 (above) is the installable state. Don't `git checkout v1.0.0`
  and reinstall — that'll give you the missing-templates bug.

### Installer history (full Day-2 arc)
| Ver | Size | MD5 | Result | Failure |
|---|---|---|---|---|
| v1 | 60 MB | `c2ebe2cc39030564b0760955858d4cd0` | FAILED | 0-byte `node.exe` (Linux tarball staged) |
| v2 | 54 MB | `30e1f5b126b559e6e5cf00f80cbabf59` | FAILED | openclaw exit 78 (Missing config) |
| v3 | 54 MB | `6f84425e3e944807c812616e8057f234` | FAILED | `gateway.auth: Invalid input` (flat string) |
| v4 | 54 MB | `7d82f00ea24b9c80f7ee7fa935ea84f8` | tagged but broken | gateway starts, but chat errors on missing templates |
| v5 | 55 MB | `49adfa6b198a5cb3906021ce32f2be08` | installs and runs | + bundle `resources/src` + `resources/docs` |

### Lessons captured this release
- **Lesson 423:** `bundle-runtime.sh` must be invoked with `--target windows --force` in Windows Docker build path.
- **Lesson 424:** Tauri `bundle.resources` bundler copies whatever's at the configured paths — does NOT validate executables.
- **Lesson 425:** Pre-write minimal config in your first-run code; don't rely on upstream `--allow-*` flag.
- **Lesson 426:** Verify schema shapes from the bundled validator (zod-schema-*.js), not from memory.
- **Lesson 428:** Tauri `bundle.resources` is an explicit allowlist, not a directory copy.

[unreleased]: https://github.com/adealauto/miracle-claw/compare/v1.0.0...HEAD
[v1.0.0]: https://github.com/adealauto/miracle-claw/releases/tag/v1.0.0
