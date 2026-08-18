# Changelog

All notable changes to Miracle Claw are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — v1.0.1 polish queue

### Things planned (no rebuild required)
- David: pick a workaround for the `missing-provider-auth` error (Lesson 431):
  - **Option A**: from `C:\Program Files\MiracleClaw\resources\`, run
    `openclaw agents add main` and answer the prompts (provider = `maic`,
    paste MAIC API key).
  - **Option B**: copy the portable static auth profiles from system openclaw at
    `C:\Users\Adeal\AppData\Roaming\openclaw\agents\main\agent\` (NOT the
    sqlite) into MC's isolated agentDir at
    `C:\Users\Adeal\AppData\Roaming\MiracleClaw\agents\main\agent\`.
  - Confirm chat sends/receives without errors.
- Tag v1.0.1 once David confirms chat roundtrip works on v6.
- Lessons 431 (provider config gap) and 432 (chat roundtrip is the release gate)
  captured in MEMORY.md.
- Unified 8-step pre-flight checklist (Lesson 432 corollary) committed to
  MEMORY.md; future MC releases must run it before tagging.

### Things planned (require rebuild)
- **Wire MAIC as the agent provider in first-run (Lesson 431).**
  - Symptom: chat errors with `No API key found for provider "openai"`
    (provider defaults to `openai` because we don't write `agents.providers.maic`).
  - Root cause: MC's setup() installs the MAIC plugin but doesn't write the
    agent provider config. Plugin registry and provider config are independent.
  - Fix: setup() gains a third config-write step — write
    `agents.providers.maic = {endpoint, defaultModel: 'milagro-dev', apiKey}`
    via the same deepMerge helper used for gateway config. Source API key from
    system openclaw's existing auth profile (auto-import with consent), env var
    `MAIC_API_KEY`, or first-run UI prompt.
- **Installer should kill running gateway before overwriting node.exe (Lesson 430).**
  - Symptom: NSIS error "Error opening file for writing: node.exe" when installing
    a hotfix over a running prior gateway. Root cause: Windows file lock on
    `node.exe` while the gateway is still up.
  - Fix: NSIS pre-step does `taskkill /F /IM miracle-claw.exe` then
    `taskkill /F /IM node.exe` before the file copy. Show a clear error if the
    kill fails.
- ADeal green branding (splash, theme, system tray icon).
  - Tray icon also solves Lesson 430 by giving the user a "Quit" affordance.
- Code signing (Windows SmartScreen "Unknown publisher" → gone).
- Auto-updater (manual reinstall for now).
- Launcher → separate-crate refactor (currently a bin in the main crate).

## [v1.0.0 hotfix-3] — 2026-08-18 16:53 MDT (v6 installer)

### Changed
- **Narrowed `bundle.resources` to only ship runtime-needed templates.**
  - Reason: v5 shipped 31,903 files because we listed whole `resources/src` and
    `resources/docs` directories. The runtime only needs `src/agents/templates/`
    (1 file: HEARTBEAT.md) and `docs/reference/templates/` (13 files: AGENTS/SOUL/
    USER/IDENTITY/TOOLS/BOOTSTRAP/BOOT + .dev.md variants).
  - v6 lists `resources/src/agents/templates` and `resources/docs/reference/templates`
    explicitly. Saves 723 files (~1 MB).
  - Same template files, fewer attachments. Strictly better than v5.

### Replaces
- **v5 (hotfix-2)** is superseded. v5 binary is gone (only v6 exists in dist-installers/).

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
- **Size:** 53 MB
- **MD5:** `7fa3a977fb1517c2d9d10fdd7b4cdb36`
- **SHA256:** `6f9f66f785b2613b21ea37baaef53dc337a51a6fe1121b015ada262b68dff89c`
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.0_x64-setup.exe`
- **Source commit:** `428900d` (bundle.resources narrowing), `5062ac0` (CHANGELOG)

## [v1.0.0 hotfix-2] — 2026-08-18 16:22 MDT (v5 installer, SUPERSEDED)

### Fixed
- **Chat error: "Missing workspace template: AGENTS.md"** after the gateway started.
  - Root cause: `tauri.conf.json: bundle.resources` was an explicit allowlist that
    omitted `resources/src` and `resources/docs`. Tauri silently omitted those
    whole directories from the installer payload. v4 shipped 31,166 files instead
    of the 31,980 at source.
  - Fix (v5): Added `"resources/src"` and `"resources/docs"` to `bundle.resources`.
  - Verified via 7z extraction: `resources/src/agents/templates/HEARTBEAT.md` and
    `resources/docs/reference/templates/AGENTS.md` (plus 12 other templates) are
    now present. Payload grew to 31,903 files (+737).
  - Lesson 428 captured.
- **Re-fix (v6):** narrowed `bundle.resources` to `resources/src/agents/templates`
  and `resources/docs/reference/templates` so we ship only the 14 files the
  runtime actually needs. Saves 723 files.

### Installer (v5 was, before being superseded)
- **File:** `dist-installers/windows/MiracleClaw_1.0.0_x64-setup.exe`
- **Size:** 55 MB
- **MD5:** `49adfa6b198a5cb3906021ce32f2be08`
- **SHA256:** `dab5d0ed16ec0de5c9107a87eb4230aa89628a08ddd07dfd30a5d44daa66c48d`
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
| v5 | 55 MB | `49adfa6b198a5cb3906021ce32f2be08` | **SUPERSEDED** | + bundle `resources/src` + `resources/docs` (Lesson 428); shipped 737 unused files |
| v6 | 53 MB | `7fa3a977fb1517c2d9d10fdd7b4cdb36` | **INSTALLED** (17:09 MDT) | narrowed bundle.resources; install successful; chat hits `missing-provider-auth` (Lesson 431) |

### Lessons captured this release
- **Lesson 423:** `bundle-runtime.sh` must be invoked with `--target windows --force` in Windows Docker build path.
- **Lesson 424:** Tauri `bundle.resources` bundler copies whatever's at the configured paths — does NOT validate executables.
- **Lesson 425:** Pre-write minimal config in your first-run code; don't rely on upstream `--allow-*` flag.
- **Lesson 426:** Verify schema shapes from the bundled validator (zod-schema-*.js), not from memory.
- **Lesson 428:** Tauri `bundle.resources` is an explicit allowlist, not a directory copy.

[unreleased]: https://github.com/adealauto/miracle-claw/compare/v1.0.0...HEAD
[v1.0.0]: https://github.com/adealauto/miracle-claw/releases/tag/v1.0.0
