# Changelog

All notable changes to Miracle Claw are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — v1.0.1 polish queue

### Things planned (no rebuild required)

- **Lesson 534 (pending rc28 rebuild): MAIC plugin's `tool_execution: "client"` patch
  was being dropped on the floor by openclaw 2026.7.1's `buildOpenAICompletionsParams`.
  Steeler-era (openclaw 2026.6.8) had a 5-line compat block at the end of
  `buildOpenAICompletionsParams` that promoted `model.params.tool_execution` onto the
  request body. openclaw 2026.7.1 dropped that block during the function rewrite.
  Net effect on MC: maic plugin's `extraParamsForTransport` returns
  `{tool_execution: "client"}` correctly, `createStreamFnWithExtraParams` merges it
  into `effectiveExtraParams` correctly — but then whitelists only
  temperature/topP/maxTokens/responseFormat/etc. through to the underlying stream
  options. `tool_execution` never reached the wire. MAIC defaulted to server-side
  tool execution, model only saw 4-5 server tools (weather, web_search, time,
  calculate, describe_image), 7 client tools invisible.
  Fix: re-add the steeler compat block via the patch system. New
  `depot/openclaw-patches/dist/openai-transport-stream-B0WkSqXp.js.insert` carries
  the 5-line patch. `scripts/patch-openclaw-dist.sh` learned a new `*.js.insert`
  patch mode that splices content before a sentinel line (preserves tabs).
  `node --check` confirms the patched file parses. With rc28, MAIC requests will
  carry `tool_execution: "client"`, model will see 11 tools (4 server + 7 client).
  When openclaw upstream restores the compat, this patch can be deleted.**
  Files: `scripts/patch-openclaw-dist.sh` (new `*.js.insert` mode), `depot/openclaw-patches/dist/openai-transport-stream-B0WkSqXp.js.insert` (new patch file), `depot/openclaw-patches/MANIFEST.md` (inventory update).

### Things built this session (WIP — pending rc25 with Lesson 528 banner)

- **Lesson 528 (WIP, pending rc25 rebuild): Sidecar EXEs identify themselves in the log.**
  - Symptom (Lesson 528 rc24 bug): build script's pre-step `cargo xwin build` produced
    a stale `miracle-claw.exe` that NSIS bundled into the installer. We shipped an rc24
    installer containing the rc23 leftover EXE (md5 `8d1045855b...`, timestamp 14:02)
    inside a `MiracleClaw_1.0.9-rc24_x64-setup.exe` shell (md5 `1cd25d9...`). Build
    script's "Finished release profile" log line was a lie — NSIS read the EXE BEFORE
    tauri-cli's own cargo invocation overwrote it.
  - Fix 1: drop the wasted pre-step from `scripts/build-windows-docker.sh`. tauri-cli
    handles the miracle-claw.exe build + NSIS bundling in one pass; we no longer
    double-invoke cargo and race the EXE write.
  - Fix 2: add `BUILD_TIMESTAMP` to `src-tauri/build.rs` (pure-std, no chrono dep).
    Format: `YYYY-MM-DD-HHMM UTC`.
  - Fix 3: every EXE prints its own banner on startup:
    - `miracle-claw.exe` → `[miracle-claw] MiracleClaw v1.0.9-rc24 (build 2026-08-21-1530 UTC, install=C:\Program Files\MiracleClaw\resources)`
    - `miracle-claw-launcher.exe` → `[miracle-claw-launcher] miracle-claw-launcher v1.0.9-rc24 (build 2026-08-21-1530 UTC)`
    - `miracle-claw-tools.exe` → `[miracle-claw-tools] miracle-claw-tools v1.0.9-rc24 (build 2026-08-21-1530 UTC)`
  - With this banner in the side-car log, we can verify which build is actually
    running from line 1 of the log file. No more "did the installer ship the right
    binary?" guessing.
  - Verification (rc24 mid-session rebuild, before banner was added):
    - New installer md5 `a500bfa4910df3b5a3dff9fc6f816621` (vs old `1cd25d9...`)
    - target/release miracle-claw.exe md5 `f0943f39...` (vs old `80ecc643...`)
    - Installer bundle contains EXE md5 `521f418ad...` (different from both target/
      and pre-build, because cargo embeds build path + timestamp on each invocation)
    - Lesson: binary byte-content comparison across builds of the same source produces
      3.5M byte differences from timestamps alone. Strings comparison shows nothing
      because strip removes constants. **Only reliable verification is a runtime
      log banner that identifies the build.**

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

### Things built this session (WIP — not yet committed as rc2)

- **Lesson 444 (WIP, ready to commit): First-run MAIC login UI.**
  - Symptom (Lesson 431 v2 leftover): `MAIC_API_KEY` not set on a fresh
    customer machine → `models.providers.maic.apiKey = SecretRef` resolves
    to "secret not found in env" at request time → user sees an opaque
    error and doesn't know how to proceed.
  - Fix: MC's first-run window shows a login form when no MAIC key is
    available. Customer enters email/password → MC POSTs to
    `https://maicserver.com/v1/auth/login` → JWT returned → MC sets
    `MAIC_API_KEY` in the process env + bakes the JWT as a literal string
    in `openclaw.json` + the chat UI unblocks and redirects to the
    openclaw gateway at `http://localhost:28789/`.
  - **Frontend (NEW)**: `index.html` (Vite root), `src/main.js` (entry
    logic — calls `invoke('first_run_report')`, conditionally renders
    login form, redirects on success), `src/styles.css` (vanilla CSS,
    no framework).
  - **Backend (NEW)**: `#[tauri::command] fn maic_login(email, password)`
    POSTs via `ureq` (sync, ships with rustls-tls). Sets
    `MAIC_API_KEY=token` in `process::env` so re-running
    `ensure_maic_provider_config()` hits the literal-key branch and
    writes the provider entry to disk. Returns `MaicLoginInfo { token,
    email, tier, endpoint }`.
  - **New variant**: `MaicKeySource::LoginRequired` (replaces the dead
    `EnvRef` fallback path). When no key is found, the bootstrap
    function early-returns BEFORE writing to disk, so the customer's
    `openclaw.json` stays clean — no half-populated provider entry, no
    `secrets.providers.default` placeholder that would fail at request
    time.
  - **New helper**: `needs_maic_login_from_state()` — reads the live
    `openclaw.json` to decide whether the login modal should fire on
    next boot. Honors already-baked literal keys, treats SecretRef as
    "needs login" since the env-var fallback is unreliable.
  - **`tauri.conf.json`**: `build.frontendDist = "../dist"`, `windows[0].url =
    "index.html"` (Tauri asset protocol — MC's bundled HTML loads first,
    then redirects to `localhost:28789` after login).
  - **`Cargo.toml`**: `ureq = { version = "2.10", default-features = false,
    features = ["tls", "json"] }` — sync HTTP, no tokio weight.
  - **Test coverage**: `tests::no_env_var_returns_login_required_and_writes_no_provider`
    asserts the early-return path correctly skips the provider write
    AND does NOT register `secrets.providers.default`.
  - **Verified**: `cargo check` clean, all 6 unit tests pass.
  - **Pending**: build v9 installer on Windows, verify chat roundtrip
    through the login UI, then tag v1.0.1 at this commit.

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
  - Fix: Custom NSIS template (`src-tauri/installer.nsi`) wired in via
    `tauri.conf.json: bundle.windows.nsis.template`. The `customInstall`
    macro does:
    ```
    nsExec::ExecToLog 'taskkill /F /IM miracle-claw.exe /T'
    nsExec::ExecToLog 'taskkill /F /IM node.exe /T'
    Sleep 2000
    ```
    If the kill fails (no process running), the install proceeds
    anyway — `nsExec::ExecToLog` swallows the error code.
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

---

## [v1.0.1-rc1] — 2026-08-18 18:46 MDT (commit `a5a5ed6`)

### Fixed
- **Lesson 431 v2 — MAIC provider config uses openclaw's native SecretRef + SecretProvider schema.** When `MAIC_API_KEY` is NOT set on the user's machine, `models.providers.maic.apiKey` is now written as a SecretRef object `{source: "env", provider: "default", id: "MAIC_API_KEY"}` and a `secrets.providers.default` env provider is registered with `allowlist: ["MAIC_API_KEY"]`. openclaw resolves the SecretRef at request time, so the user just needs to set `MAIC_API_KEY` in their environment and restart — no per-install keystore entry, no opaque `missing-provider-auth` error. Verified end-to-end against `openclaw@2026.7.1-2`'s `resolveSecretRefString` runtime.
- **Lesson 428 — `resources/node` 0-byte stub removed from Windows installer.** Tauri bundler couldn't tell the Linux-portable-Node dir from a regular file path, so it created a 0-byte `resources/node` stub at `C:\Program Files\MiracleClaw\resources\node` on every install. Benign, but pollutes the install. Removed from `tauri.conf.json` `bundle.resources`.

### Added
- 6 unit tests for `ensure_maic_provider_config()` covering env-var-key path, SecretRef fallback, idempotency, existing-entry preservation, `tool_execution` pinning, and `is_empty_api_key` helper. All tests pass.
- `[dev-dependencies] tempfile = "3"` for the test suite (test-only, never linked into the production binary).

### Verified
- Run `cargo test --bin miracle-claw --lib`: 6/6 pass.
- Run `node openclaw.mjs --version` against v8's bundled node: `OpenClaw 2026.7.1-2 (0790d9f)`.
- Run end-to-end SecretRef resolution against the v8-bundled openclaw.mjs:
  - With `MAIC_API_KEY="test-v8-key-abc"` set: `resolveSecretRefString` returns `test-v8-key-abc` ✓
  - Without env var: clear error `Environment variable "MAIC_API_KEY" is missing or empty.` ✓
- Run zod schema validation against v8-bundled `zod-schema.core-DviqqtPj.js`:
  - SecretRefSchema, SecretProviderSchema, SecretInputSchema, SecretsConfigSchema, ModelsConfigSchema all accept the v1.0.1 config shape ✓

### Installer
- MD5: `6762024031c9d9cd677ad9a92478fff3`
- SHA256: `13d33848c4d08ab941091a1ec730c9a7a7475913e2b97b593326f4f0b26b6633`
- Size: 56,064,824 bytes (+10,716 vs v7)
- File count: 31,188 (+8 vs v7)
- Format: PE32 GUI NSIS, 7 sections

### Test plan (per Lesson 432 — chat roundtrip is the release gate)
1. Kill v6/v7: `taskkill /F /IM miracle-claw.exe /T`
2. Run v8 installer
3. Verify no `resources/node` 0-byte stub at `C:\Program Files\MiracleClaw\resources\`
4. Verify `openclaw.json` contains SecretRef + secrets.providers.default
5. Open Miracle Claw, send a chat → expect clear error (env var not set)
6. Set `MAIC_API_KEY`, restart, send chat → expect chat to work
7. If 6 passes, **tag v1.0.1 at commit a5a5ed6**

[v1.0.1-rc1]: https://github.com/adealauto/miracle-claw/compare/v1.0.0...a5a5ed6

---

## [v1.0.1-rc2] — 2026-08-18 20:25 MDT (commit `47640b1`)

### Fixed
- **Lesson 430 (NSIS file lock) — `MiracleClaw_*_x64-setup.exe` overwrites an existing MC install without an Abort/Retry/Ignore dialog.** Symptom (David 19:01 MDT): trying to install v9 over v8 while MC was running gave NSIS error "Error opening file for writing: node.exe" — Tauri killed `miracle-claw.exe` via `CheckIfAppIsRunning`, but the orphaned `node.exe` child kept its file handle. Fix: custom NSIS template wired in via `tauri.conf.json: bundle.windows.nsis.template`. The `NSIS_HOOK_PREINSTALL` macro (recognized by Tauri's installer.nsi at line 641) runs BEFORE `CheckIfAppIsRunning` and force-kills both `node.exe` and `miracle-claw.exe`. `nsExec::ExecToLog` swallows error codes so installs without a running MC proceed normally. `Sleep 2000` gives the OS time to release file handles.

- **Lesson 444 (first-run login UI) — opaque `missing-provider-auth` error on a fresh customer machine.** Symptom: MAIC provider config was wired to a `SecretRef` for `MAIC_API_KEY`, but on a fresh install the env var isn't set and the customer has no idea how to proceed. Fix: when no MAIC key is found at bootstrap time, `ensure_maic_provider_config()` returns `provider_configured: false` + `api_key_source: LoginRequired` + early-returns BEFORE `fs::write()` (critical Lesson 444 bug fix — initial impl had the early-return AFTER `fs::write()`, which would have written a half-populated provider entry to disk). The customer's `openclaw.json` stays clean. MC's frontend detects `needs_maic_login: true` via `first_run_report` and renders an email/password form. On submit, `maic_login(email, password)` POSTs to `https://maicserver.com/v1/users/login` (via `ureq`, sync, ships with rustls-tls — no tokio weight), parses the JWT, sets `process.env.MAIC_API_KEY = token`, RE-RUNS `ensure_maic_provider_config()` to bake the JWT as a literal in `openclaw.json`, and the frontend redirects to `http://localhost:28789/`.

### Added
- **Frontend stack (vanilla HTML/JS, no framework):**
  - `index.html` — Vite root, ~600 bytes built
  - `src/main.js` — entry logic: `invoke('first_run_report')` → render form OR redirect
  - `src/styles.css` — vanilla CSS, dark/light via `prefers-color-scheme`, ADeal auto-repair green `#22c55e` accent
  - `vite.config.js` — port 1420, `strictPort: true`
  - Total Vite output: ~7 KB
- **Backend:**
  - `MaicKeySource::LoginRequired` enum variant (replaces dead `EnvRef` variant)
  - `#[tauri::command] fn maic_login(email, password) -> Result<MaicLoginInfo, String>`
  - `MaicLoginInfo { token, email, tier, endpoint }` response struct
  - `FirstRunReport { ..., needs_maic_login: bool }` extended
  - `needs_maic_login_from_state()` helper
  - `http_post_json_with_tls_fallback()` helper (ureq-based, 10s timeout, HTTPS first)
  - `ENV_VAR_NAME` and `DEFAULT_ENDPOINT` constants hoisted to module scope (Lesson 444 scoping gotcha)
  - `ensure_secrets_default_env_provider()` retained as `#[allow(dead_code)]` utility
  - `read_maic_root()` made defensive (returns `serde_json::json!({})` if file not found)
- **NSIS template (`src-tauri/installer.nsi`)** — Tauri 2's full default installer template with my `NSIS_HOOK_PREINSTALL` macro injected at line 68 (before the `!define` block).
- **`Cargo.toml`**: `ureq = { version = "2.10", default-features = false, features = ["tls", "json"] }` — sync HTTP, rustls-tls, no tokio.
- **`tauri.conf.json`**:
  - `build.frontendDist = "../dist"`, `build.devUrl = "http://localhost:1420"`
  - `windows[0].url = "index.html"` (was `http://localhost:28789/`) — Tauri asset protocol (`tauri://localhost`) serves MC's bundled HTML
  - `security.csp` extended with `frame-src http://localhost:28789;` for post-login redirect
  - `bundle.windows.nsis.template = "installer.nsi"`
  - Bumped `version` to `1.0.1`

### Tests
- `tests::no_env_var_returns_login_required_and_writes_no_provider` — asserts the early-return path correctly skips the provider write AND does NOT register `secrets.providers.default`.
- All 6 unit tests pass.

### Verified
- `cargo check --bin miracle-claw --lib` — clean, no warnings
- `cargo test --bin miracle-claw --lib` — 6/6 pass
- `npx vite build` — dist/index.html (0.60 KB), dist/assets/index-*.css (2.84 KB), dist/assets/index-*.js (3.39 KB)
- Docker cross-compile (`scripts/build-windows-docker.sh`) — full build completed in ~14 min (slow because makensis LZMA-compresses 31,185 resource files over a WSL→Windows 9P mount)
- v9 installer extracted via 7z — `miracle-claw.exe` contains `maic_login`, `LoginRequired`, `first_run_report`, `needs_maic_login` strings ✓
- `installer.nsi` hook verified via Tauri 2 source (`tauri-bundler-2.9.4/src/bundle/windows/nsis/installer.nsi` line 641)

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.1_x64-setup.exe` (filename v1.0.1 after version bump)
- **MD5:** TBD (after rebuild with v1.0.1 version)
- **Size:** TBD
- **Format:** PE32 GUI NSIS
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.1_x64-setup.exe` (after rebuild)

### Test plan (Lesson 432 chat roundtrip — release gate)
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T`
2. `rm -rf "%APPDATA%\MiracleClaw"` (clean state)
3. Run v1.0.1 installer from Desktop (upgrade-over-install path)
4. Verify NO Abort/Retry/Ignore dialog appears (Lesson 430 fix)
5. Verify `resources/node` 0-byte stub absent (Lesson 428 fix still holds)
6. Launch `miracle-claw.exe` → expect login modal to appear (Lesson 444 fix)
7. Enter `mc-test-pro@milagro.cloud` test credentials → submit
8. Verify JWT baked into `openclaw.json` as literal string under `models.providers.maic.apiKey`
9. Modal closes, chat UI loads at `http://localhost:28789/`
10. Send a chat message → expect response from MAIC (Lesson 432 — actual release gate)

[v1.0.1-rc2]: https://github.com/adealauto/miracle-claw/compare/v1.0.1-rc1...47640b1

## [v1.0.2-rc1] — 2026-08-18 21:05 MDT (commit `bbb1ff0`)

### Fixed
- **Lesson 447 (login endpoint wrong) — first-run login modal hit `/v1/auth/login` (Milagro dashboard) instead of `/v1/users/login` (consumer).** Symptom (David 21:14 MDT, real `championnm@yahoo.com` install): login form submit returned `HTTP 422 — {"detail":[{"type":"missing","loc":["body","name"],"msg":"Field required","input":{"email":"...","password":"..."}}]}`. Root cause: MAIC has TWO login endpoints with different schemas:
  - `POST /v1/auth/login` → `api__routes__dashboard__LoginIn` (required: `name` + `password`) — used by the Milagro dashboard, NOT consumer apps
  - `POST /v1/users/login` → `api__routes__users__LoginIn` (required: `email` + `password`, optional totp/recovery_code) — used by consumer apps like Miracle Claw
  The Lesson 444 implementation called the dashboard endpoint, which rejected `{email, password}` with 422 "Field required: name". Fix: change the path in `maic_login` to `/v1/users/login`. Response shape (`{token, user:{email,tier,...}}`) is identical to what the parser already handled, so no other changes needed. Six unit tests still pass.

### Verified
- `cargo check --bin miracle-claw` — clean, no warnings
- `cargo test --lib` — 6/6 pass (`is_empty_api_key_literal_and_ref`, `env_var_key_writes_literal_string`, `existing_entry_with_secret_ref_is_preserved`, `no_env_var_returns_login_required_and_writes_no_provider`, `idempotency_no_duplication`, `tool_execution_param_pinned_to_client`)
- Live endpoint probe via curl:
  - `POST /v1/users/login` with `{email, password}` returns 200 `{token, user:{email,tier:"free",...}}` ✓
  - `POST /v1/auth/login` with `{email, password}` returns 422 `{detail:[{type:missing, loc:[body,name], ...}]}` (regression check confirming the original bug)
  - `POST /v1/users/signup` with 12+ char alphanumeric password returns 200 `{token, user:{...}}` (free tier)

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.2_x64-setup.exe`
- **MD5:** TBD (after rebuild)
- **Size:** TBD
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.2_x64-setup.exe` (after rebuild)

### Test plan (Lesson 432 chat roundtrip — release gate)
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T` (clean state)
2. Uninstall any existing MC via Settings → Apps (or `rm -rf "%LOCALAPPDATA%\Programs\MiracleClaw"`)
3. Run v1.0.2 installer from Desktop (no upgrade-over-install)
4. Verify NO Abort/Retry/Ignore dialog appears (Lesson 430 fix)
5. Launch `miracle-claw.exe` → expect login modal to appear (Lesson 444 fix)
6. Enter `championnm@yahoo.com` (David's real account) + password → submit
7. Verify NO 422 error (Lesson 447 fix)
8. Verify JWT baked into `openclaw.json` as literal string under `models.providers.maic.apiKey`
9. Modal closes, chat UI loads at `http://localhost:28789/`
10. Send a chat message → expect response from MAIC (Lesson 432 — actual release gate)

[v1.0.2-rc1]: https://github.com/adealauto/miracle-claw/compare/v1.0.1-rc2...bbb1ff0

## [v1.0.3-rc1] — 2026-08-18 21:50 MDT (commit `bdbe2cf`)

### Fixed
- **Lesson 449 (localhost refused to connect after login) — `setup()` spawned the
  launcher sidecar BEFORE first-run login completed, with `MAIC_API_KEY` still
  unset in the launcher's process env. The openclaw gateway crashed at startup
  with `SecretRefResolutionError: Environment variable "MAIC_API_KEY" is missing
  or empty` and the webview hit `ERR_CONNECTION_REFUSED` on
  `http://localhost:28789/`.** Three bugs in concert:
  1. **Bug A (bootstrap early-return):** `ensure_maic_provider_config` treated
     a SecretRef (`apiKey: {source:"env", id:"MAIC_API_KEY"}`) as a complete
     entry and early-returned `MaicKeySource::Existing`. The SecretRef never
     counts as complete at the bootstrap layer — only a literal non-empty
     apiKey does. With `is_unresolvable_api_key(value)` added, the bootstrap
     correctly falls through when a SecretRef's target env var is unset, so
     `maic_login` can replace it with the literal JWT.
  2. **Bug B (launcher spawn timing):** `setup()` spawned the launcher as soon
     as the bootstrap returned, regardless of whether we had a usable key. With
     no key in env the gateway fails its self-check and `launcher.exe` exits
     within a second. The webview then has nothing to connect to. Fix:
     `setup()` now defers the spawn until `maic_provider_configured = true`.
  3. **Bug C (frontend navigation race):** the JS frontend called
     `maic_login` then immediately `window.location.href =
     "http://localhost:28789/"`. The Tauri→backend login completed, but the
     gateway wasn't running yet, so the navigation hit ERR_CONNECTION_REFUSED
     in the webview's address bar. Fix: new `start_gateway_after_login`
     Tauri command that the frontend `await`s before navigation; it spawns
     the launcher with the now-set env var and blocks until TCP connect
     succeeds on `127.0.0.1:28789`.

### Added
- New Tauri command `start_gateway_after_login`. Spawns the launcher sidecar
  with `MAIC_API_KEY` now set in the parent process env, waits for the
  gateway to bind `127.0.0.1:28789`, returns once reachable. Defensively
  refuses to run if `MAIC_API_KEY` is still missing (caller must call
  `maic_login` first).
- New helper `replace_secret_ref_with_literal()` that runs after `maic_login`
  succeeds. Reads `MAIC_API_KEY` from env, replaces any legacy SecretRef in
  `models.providers.maic.apiKey` with the literal JWT. Idempotent — returns
  `true` if the disk now contains the correct literal (written or already
  there), `false` only when the env var is missing or the openclaw.json
  read failed. This is the explicit "fix the legacy config" step that
  complements Bug A's "don't claim the entry is complete when it isn't"
  logic.
- New helper `is_unresolvable_api_key(value)` (used by the bootstrap). A
  SecretRef is "unresolvable" when its target env var is missing or empty
  in process env — i.e. the openclaw gateway would crash trying to resolve
  it. Distinct from `is_empty_api_key` which only checks syntactic emptiness.
- Refactored launcher spawn into `spawn_launcher_and_wait(app_handle, port,
  timeout)`. Used by both `setup()` (returning users with a key already in
  place) and `start_gateway_after_login` (post-login spawn). Replaces the
  duplicated spawn logic that was inline in `setup`.

### Verified
- `cargo check --bin miracle-claw --lib` — clean
- `cargo test --lib` — **11/11 pass** (added 5 new tests):
  - `is_unresolvable_api_key_checks_env_var` — SecretRef env vars
    (set/unset/empty/whitespace) classified correctly
  - `existing_secret_ref_with_no_env_var_falls_through_to_login_required` —
    Bug A regression: SecretRef + no env = `LoginRequired` (not `Existing`)
  - `existing_entry_with_secret_ref_is_preserved_when_env_set` — user-
    customized configs with working SecretRef + env set are preserved
  - `login_replaces_legacy_secret_ref_with_literal` — Bug C regression:
    `maic_login` post-conditions guarantee the literal JWT is on disk
  - `replace_secret_ref_is_idempotent_when_already_literal_and_correct` —
    no churn when the disk already has the right literal
  - `replace_secret_ref_overwrites_wrong_literal` — stale tokens get
    refreshed
  - All 6 prior tests still pass (idempotency, env-var-key, etc.)
- Also fixed a latent test infrastructure bug: `ENV_LOCK` was using
  `lock().unwrap()` which poisoned the mutex when a test panicked, taking
  out all subsequent tests. Now uses `lock_or_recover()` that falls through
  to `into_inner()` on poisoning.

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.3_x64-setup.exe`
- **MD5:** `0f91d5483ac36e6b909693594bc2685d`
- **Size:** 56,733,265 bytes (~54 MB)
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.3_x64-setup.exe`

### Test plan (Lesson 432 chat roundtrip — release gate)
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T` (clean state)
2. Uninstall any existing MC via Settings → Apps (or `rm -rf "%LOCALAPPDATA%\Programs\MiracleClaw"`)
3. Run v1.0.3 installer from Desktop (upgrade-over-install is OK since the
   upstream Lesson 430 NSIS pre-install taskkill handles the running process)
4. Verify NO Abort/Retry/Ignore dialog (Lesson 430 fix)
5. Launch `miracle-claw.exe` → expect login modal to appear (Lesson 444 fix)
6. Enter `championnm@yahoo.com` + password → submit
7. Verify NO 422 error (Lesson 447 fix)
8. Verify NO `ERR_CONNECTION_REFUSED` (Lesson 449 fix) — webview shows
   "Starting gateway…" briefly, then redirects to `http://localhost:28789/`
9. Verify JWT baked into `openclaw.json` as literal string under
   `models.providers.maic.apiKey` (Lesson 449 fix, Bug C)
10. Modal closes, chat UI loads at `http://localhost:28789/` (Lesson 449
    fix, Bug B+C combined)
11. Send a chat message → expect response from MAIC (Lesson 432 — actual
    release gate)

[v1.0.3-rc1]: https://github.com/adealauto/miracle-claw/compare/v1.0.2-rc1...bdbe2cf

## [v1.0.4-rc1] — 2026-08-18 22:55 MDT (commit `63bc70c`)

### Fixed
- **Lesson 450 (chat returns "The selected model was not found by the
  provider" after login) — MC's `ensure_maic_provider_config` wrote the
  bare origin `"https://maicserver.com"` into openclaw.json
  `models.providers.maic.baseUrl`. openclaw's OpenAI SDK appends
  `/chat/completions` to that URL verbatim (no auto `/v1` prefix), so
  the gateway POSTs to `https://maicserver.com/chat/completions` which
  MAIC returns 404 for (MAIC exposes its OpenAI-compatible route at
  `/v1/chat/completions`, not `/chat/completions`). openclaw's
  `isModelNotFoundErrorMessage` regex chain then matched the 404 body
  text and surfaced the misleading "selected model was not found by
  the provider" error — even though `milagro-dev` IS in MAIC's
  LiteLLM allow-list.** Three concrete fixes:
  1. **`normalize_maic_base_url(value: &str) -> String`** — stamps the
     `/v1` suffix when writing the baseUrl for the first time
     (Lesson 444 path) and any time a `MAIC_API_URL` override lacks
     it. Idempotent: calling twice is a no-op. Tested via
     `normalize_maic_base_url_appends_v1_when_missing`.
  2. **`upgrade_legacy_maic_base_url(existing: &str) -> Option<String>`** —
     rewrites baseUrl entries in v1.0.0..v1.0.3-era openclaw.json
     files in-place. Conservative: only rewrites URLs that are clearly
     the "bare origin" form (no path or `/` only). Anything with a
     user-added path (proxy mount, alternate route) is preserved
     verbatim. Tested via
     `upgrade_legacy_maic_base_url_only_rewrites_bare_origin`.
  3. **`strip_trailing_v1(value: &str) -> String`** — used by
     `maic_login` to normalize a user-supplied `MAIC_API_URL` before
     appending `/v1/users/login`. Handles the "user already included
     /v1" case so we never POST to `/v1/v1/users/login`. Tested via
     `strip_trailing_v1_handles_both_forms`.

  **Display semantics**: `MaicProviderBootstrap.endpoint` (the URL the
  login UI displays as "Logged in to https://maicserver.com") still
  reports the bare origin — `/v1` is purely an implementation detail
  of openclaw's OpenAI SDK URL composition. The `/v1`-normalized
  form lives in the entry's `baseUrl` for openclaw's gateway to
  consume.

### Verified
- `cargo check --bin miracle-claw --lib` — clean
- `cargo test --lib` — **14/14 pass** (3 new Lesson 450 tests
  + 11 regression tests; updated 3 existing tests to assert the
  normalized `/v1` baseUrl forms):
  - `normalize_maic_base_url_appends_v1_when_missing`
  - `upgrade_legacy_maic_base_url_only_rewrites_bare_origin`
  - `strip_trailing_v1_handles_both_forms`
- `curl https://maicserver.com/chat/completions` — 404 (route not
  exposed, root cause confirmed)
- `curl https://maicserver.com/v1/chat/completions` — 401 (route
  exists, would pass with valid JWT)

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.4_x64-setup.exe`
- **MD5:** `c43baddc4dcc04e27e805c3c8e417a2d`
- **Size:** 56,719,552 bytes (~54 MB)
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.4_x64-setup.exe`

### Test plan (Lesson 432 chat roundtrip — release gate)
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T`
2. Uninstall v1.0.3 via Settings → Apps (or `rm -rf "%LOCALAPPDATA%\Programs\MiracleClaw"`)
3. Delete `C:\Users\Adeal\AppData\Roaming\MiracleClaw\openclaw.json`
   (force fresh bootstrap so Lesson 450 migration path runs cleanly)
4. Run v1.0.4 installer
5. Launch `miracle-claw.exe` → login modal (Lesson 444 fix)
6. Enter `championnm@yahoo.com` + password → submit
7. Verify openclaw.json now has `baseUrl: "https://maicserver.com/v1"`
   (Lesson 450 fix) — was `"https://maicserver.com"` in v1.0.3
8. Modal closes, chat UI loads
9. **Send a chat message → expect response (not "model not found")**
   (Lesson 450 — actual release gate)
10. Verify the selected model in the response is `milagro-dev` (the
    default in `src/main.js`)

[v1.0.4-rc1]: https://github.com/adealauto/miracle-claw/compare/v1.0.3-rc1...63bc70c

## [v1.0.5-rc1] — 2026-08-18 23:55 MDT (commit `429ab9e`)

### Fixed
- **Lesson 451 (v1.0.4 regression — early-return path skipped the baseUrl
  /v1 migration).** v1.0.4 added the `upgrade_legacy_maic_base_url`
  migration but placed it in the *write path*, after the existing-entry
  early-return. On a v1.0.4 launch where the user already had a complete
  openclaw.json entry from v1.0.0..v1.0.3 (literal apiKey + non-empty
  baseUrl), the early-return fired before the migration could rewrite
  the stale bare MAIC origin. Result: David's v1.0.4 install still
  showed `baseUrl: "https://maicserver.com"` and chat still failed with
  "The selected model was not found by the provider". The fix is a
  one-line restructuring: the migration now runs *inside* the
  early-return block too, persisting the rewritten file before
  returning the migrated URL. User-customized paths (proxy mounts,
  alternate routes) are still preserved by the conservative
  `upgrade_legacy_maic_base_url` policy.

### Verified
- `cargo test --lib` — **15/15 pass** (1 new Lesson 451 regression test
  + 14 from v1.0.4; new test:
  `lesson_451_early_return_path_migrates_existing_bare_origin`).
- `cargo check --bin miracle-claw --lib` — clean.

### Installer
- **File:** `dist-installers/windows/MiracleClaw_1.0.5_x64-setup.exe`
- **MD5:** `10b06dc7e61ca21a70ec5b5a81602a09`
- **Size:** 56,712,789 bytes (~54 MB)
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.5_x64-setup.exe`

### Binary verification
- Extracted via `7z x`: bundled `miracle-claw.exe` contains the
  eprintln string `[miracle-claw] Lesson 451: migrated baseUrl`
  — confirms the fix is in the shipped binary (Rust release strips
  function symbols, so the eprintln string is the canonical
  end-to-end marker).

### Test plan
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T`
2. Uninstall v1.0.4 via Settings → Apps
3. **Do NOT delete `openclaw.json`** — this is the regression case. We
   want the v1.0.0..v1.0.3-era bare-origin entry to be migrated in-place
   by the Lesson 451 fix.
4. Run v1.0.5 installer
5. Launch → login modal
6. Enter `championnm@yahoo.com` + password
7. After login, **verify openclaw.json now has
   `baseUrl: "https://maicserver.com/v1"`** (was
   `"https://maicserver.com"` in v1.0.4 and earlier — the migration
   should have rewritten it).
8. Modal closes, chat UI loads
9. **Send a chat message → expect response** (no more "model not found").

[v1.0.5-rc1]: https://github.com/adealauto/miracle-claw/compare/v1.0.4-rc1...593d4d1

## [v1.0.6-rc1] — 2026-08-19 08:04 MDT (commit `39a7917`, installer `48cff6ab9313c17a5442666f95e0e3cf`)

### Installer (SHIPPED)
- **File:** `dist-installers/windows/MiracleClaw_1.0.6_x64-setup.exe`
- **MD5:** `48cff6ab9313c17a5442666f95e0e3cf`
- **Size:** 56,742,962 bytes (~54 MB)
- **Path on David's desktop:** `C:\Users\Adeal\Desktop\MiracleClaw_1.0.6_x64-setup.exe`
- **Built via:** `bash scripts/build-windows-docker.sh` (Docker cold cache; NSIS `makensis` step took ~9 minutes for the 13MB binary + OpenClaw bundle compression)

### Binary verification
- Extracted via `7z x`; bundled `miracle-claw.exe` contains the eprintln strings:
  - `[miracle-claw] auto_relogin: stashed cached creds for endpoint=`
  - `[miracle-claw] auto_relogin: cleared cached creds`
  - `cached-creds-key`, `cached-creds` (keychain entry usernames)
  - `src/auto_relogin.rs` (module path)
  - These confirm the Lesson 458 fix is in the shipped binary.



### Added — "Stay signed in" opt-in
- **Lesson 458: silent-relogin on 401 via cached credentials.**
  The login form now has a checkbox: **"Stay signed in (encrypts your
  password in this machine's secure store)"**. Default is **unchecked**
  (explicit opt-in).
  - When **checked**, MC encrypts the user's email + password with a
    per-install AES-256-GCM key and stashes both the encrypted blob and
    the encryption key in the OS keychain (Windows Credential Manager /
    macOS Keychain / Linux Secret Service). Future 401s from the OpenClaw
    chat can silently mint a new JWT instead of forcing a manual re-login.
  - When **unchecked**, any prior stash is wiped on this login. Idempotent
    — safe to un-check and re-log-in on a borrowed machine.
- **New Tauri command `silent_relogin`** — called by the OpenClaw child
  window when its MAIC chat request returns 401. Loads cached creds,
  mints a new JWT, returns it. `Ok(None)` means "no cached creds" (user
  didn't check the box); OpenClaw surfaces the original 401 as a clean
  login prompt instead of treating it as an error.
- **New Tauri command `maic_logout`** — for the v1.1.0 dashboard. Kills
  the launcher sidecar, wipes the keychain stash, restores the
  `models.providers.maic.apiKey` SecretRef in `openclaw.json`, unsets
  `MAIC_API_KEY` in the process env, closes the OpenClaw child window.

### Added — `auto_relogin` module
- **New file: `src-tauri/src/auto_relogin.rs`** (~430 LOC + 200 LOC tests).
- **Pure crypto helpers** (`generate_key`, `generate_nonce`,
  `encrypt_blob`, `decrypt_blob`) take a key as input and don't touch
  the OS keychain. Unit-testable without any platform backend.
- **Keychain integration** (`stash_cached_credentials`,
  `load_cached_credentials`, `clear_cached_credentials`) is exercised
  end-to-end on David's machine before each release — the `keyring`
  crate v3.x dropped the `mock` feature so there's no in-process
  keychain backend anymore.
- **`CachedCreds` struct is `Zeroize`-on-drop** so decrypted secrets
  don't linger in process memory after the login round-trip.
- **Endpoint binding via AAD**: each credential blob is bound to the
  MAIC endpoint it was issued for. A creds stash for `staging.maicserver.com`
  cannot be used to log into `maicserver.com` and vice versa — protects
  against accidental cross-environment reuse.

### Changed — `maic_login` signature
- Was: `fn maic_login(email: String, password: String) -> Result<...>`
- Now: `fn maic_login(email: String, password: String, remember: bool) -> Result<...>`
- Existing callers (frontend `invoke('maic_login', { email, password })`)
  would have broken silently — the new field is required. The frontend
  was updated in this commit to pass `remember` from the new checkbox.

### Dependencies added
- `keyring = "3"` (Windows Credential Manager / macOS Keychain / Linux Secret Service)
- `aes-gcm = "0.10"` (AES-256-GCM authenticated encryption)
- `rand = "0.8"` (cryptographically-secure random key + nonce)
- `base64 = "0.22"` (envelope encoding)
- `zeroize = "1"` (Drop-time wipe of decrypted secrets)

### Verified
- `cargo test --lib` — **23/23 pass** (8 new auto_relogin tests + 15 from v1.0.5).
  New tests:
  - `encrypt_decrypt_round_trip`
  - `nonce_uniqueness_same_plaintext_different_ciphertexts`
  - `wrong_key_fails_decrypt`
  - `tampered_ciphertext_fails_decrypt`
  - `truncated_blob_rejected`
  - `wrong_aad_fails_decrypt`
  - `ascii_password_round_trips` (empty / unicode / very-long / `\0`-containing)
  - `envelope_round_trips_through_json`
- `cargo check --bin miracle-claw --lib` — clean (1 pre-existing unused-field warning).

### Anti-patterns learned (Lesson 458 corollary)
- **"MEMORY.md says this exists" ≠ "this exists in this repo"**. Lesson 194 in
  MEMORY.md described an `auto_relogin.rs` module from a downstream product
  line (miracle-claw 1.7.2). The current repo (miracle-claw 1.0.5 master)
  does NOT have it. Reaching for that module on the false assumption it
  existed would have wasted hours. Always verify by `grep -r` or `ls` before
  building on a MEMORY.md claim, especially across product version lines.
- **The Lesson CONTENT is still useful** — Lesson 194's design (keychain,
  AES-GCM, silent relogin on 401) is exactly what v1.0.6 needed. The mistake
  was treating "described in MEMORY.md" as "exists in this tree".

### What's NOT shipped in v1.0.6
- **OpenClaw-side wire-up**: the `silent_relogin` Tauri command is registered
  and ready to be called, but the OpenClaw child window doesn't yet invoke
  it on 401. That's an OpenClaw-side change (not MC), scheduled for the
  OpenClaw vendor's next patch. Until then, users will see 401 + manual
  re-login even if they checked "Stay signed in". This is called out in
  the v1.0.6 README as a known limitation.
- **Dashboard "Sign out" button**: `maic_logout` is wired and ready, but the
  v1.1.0 dashboard UI isn't shipped yet. v1.0.6 + manual logout (delete
  `~/.miracle_claw/`) is the current escape hatch.

### Test plan
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T`
2. Uninstall v1.0.5 via Settings → Apps
3. Run v1.0.6 installer
4. Launch → login modal appears with the new "Stay signed in" checkbox
5. Enter `championnm@yahoo.com` + password, **leave checkbox UNCHECKED**
6. Verify login works, chat works
7. Log out via Windows Credential Manager (clear `com.adealauto.miracle-claw`)
   OR sign out via the upcoming v1.1.0 dashboard
8. Re-login, this time **CHECK the checkbox**
9. Verify login works, chat works
10. **Keychain check**: open Windows Credential Manager → Web Credentials
    → look for `com.adealauto.miracle-claw` → should see two entries
    (`cached-creds-key` + `cached-creds`)
11. Restart MC → chat should still work (JWT was re-baked on login)
12. **Known-limitation check**: wait for JWT to expire OR rotate MAIC_API_KEY
    → expect 401 in chat (OpenClaw vendor hasn't shipped silent_relogin yet).
    This is expected; document for David.

### Next
- v1.0.7 — tool parity (11 tools, tier-gated) + token-nudges + tier-change
  forced-logout modal. Spec finalized today (memory/2026-08-19.md).
- v1.1.0 — dashboard pivot (post-login = tiles, OpenClaw child window,
  tier badge, token usage display). Spec finalized today.

## [v1.0.7-rc1] — 2026-08-19 (rc for testing) — SHA `185e29eff0dda45de4443dc410e382e5` (57,071,827 bytes)

### Lesson 460 — `AuthMeResponse` schema must match MAIC's actual response shape (2026-08-19 09:38 MDT)

**Symptom (David 2026-08-19)**: Clicking the dashboard's "Refresh tier" button or any path that triggered `mc_refresh_tier` returned:
> "Could not refresh tier: parse /v1/auth/me: Failed to read JSON: missing field `user` at line 1 column 92"

**Root cause**: `auth/tier.rs` deserialized MAIC's `/v1/auth/me` into `{user: {tier, plan_code, email}}`, but MAIC actually returns `{id, name, is_master, tier, plan_code}` — flat, no `user` wrapper. Latent since v1.0.6; only surfaced in v1.0.7 because v1.0.7's dashboard calls `mc_refresh_tier` for the first time (previous versions had no dashboard and never exercised this path).

**Fix**:
- `src-tauri/src/auth/tier.rs`: replaced `AuthMeResponse { user: AuthMeUser }` with flat `AuthMeResponse { tier, plan_code, name, email }`.
- Reads `parsed.tier` instead of `parsed.user.tier`, stashes `parsed.name` into the `email` field (frontend just renders a label).
- Added test `auth_me_response_deserializes_maic_actual_schema` that pins both the real shape and asserts the old `{"user": {...}}` shape does NOT deserialize — so future schema changes in either direction will fail loudly.

**Why the bug existed**: When I first wrote `tier.rs` in v1.0.7 development, I never called `/v1/auth/me` against real MAIC to inspect the response. I wrote a `wrapper.user.field` schema based on how I had been writing MAIC `/v1/users/login` responses (which DO wrap in `user`). Without an integration test that hits the actual endpoint, the bug stayed invisible until the first dashboard fetch.

**Anti-patterns**:
- Writing serialization code without an integration test that hits the real endpoint with a real JWT.
- Letting the test suite ("60/60 tests pass") give false confidence — every test was a unit test of normal-shape data, never a deserialize-the-real-MAIC-shape test.
- Trusting the dashboard "looks right" as an end-to-end test — David was the first real user of the dashboard-with-tier flow.

**Symptom → cause → fix table** (Lesson 460 additions):
| Symptom | Likely cause | Fix |
|---|---|---|
| `Could not refresh tier: parse /v1/auth/me: Failed to read JSON: missing field X at line 1 column Y` | `X` field doesn't exist in MAIC's actual response shape | Match MC's schema to MAIC's actual JSON (verified via `curl https://maicserver.com/v1/auth/me -H "Authorization: Bearer <jwt>"`) |
| Dashboard tile parses fine but tier shows as "free" no matter what MAIC tier is | Schema fields are present but wrong path (e.g. `parsed.user.tier` instead of `parsed.tier`) | Pin a test using MAIC's actual response payload; assert both ways |
| Login works but dashboard can't fetch tier | `AuthMeResponse` deserialization fails on every fetch | Same — add an integration test against real MAIC |

**Status**: fixed in v1.0.7-rc1 hotfix. New MD5 `185e29eff0dda45de4443dc410e382e5` (57,071,827 bytes). Dashboard now resolves tier correctly.

### Lesson 459 — Installer filename is `tauri.conf.json:version`, not `Cargo.toml`
**Gotcha caught 2026-08-19 09:00 MDT**: I bumped `Cargo.toml` (1.0.6→1.0.7), `package.json`, and `BUNDLE_VERSION` to 1.0.7, but **forgot to bump `src-tauri/tauri.conf.json:version`**. The build's NSIS bundler uses `tauri.conf.json:version` for the installer filename (`MiracleClaw_<version>_x64-setup.exe`), so the resulting installer was named `MiracleClaw_1.0.6_x64-setup.exe` — even though the binary content was v1.0.7 (it shipped `miracle-claw-tools.exe` and all the new tier-gated logic). Caught it before anyone installed, but it's a trap.

**Fix**: bump FOUR places, not three:
1. `src-tauri/Cargo.toml` — `version` field
2. `package.json` — `version` field
3. `src-tauri/resources/BUNDLE_VERSION` — single-line text file shown in About panel
4. `src-tauri/tauri.conf.json` — `version` field (drives installer filename + .exe metadata)

**How to catch this in the future**: write a pre-build check that greps for `version` mismatches across all 4 files. If any drift, fail the build. Or: use one canonical source (e.g., a `VERSION` file) and have `cargo:rerun-if-changed` plus a `build.rs` inject it into the others. The latter is cleanest but more work.

**Anti-pattern**: trusting "the version is bumped" without verifying which version lives where. Each version field serves a different consumer:
- `Cargo.toml` → cargo resolver (binary versions in registry)
- `package.json` → npm resolver (frontend deps)
- `BUNDLE_VERSION` → displayed in MC's About panel
- `tauri.conf.json` → installer filename + Windows .exe version metadata

**Symptom → cause → fix table**:
| Symptom | Likely cause | Fix |
|---|---|---|
| Installer is named `1.0.6` but binary contains `1.0.7` features | Only `Cargo.toml` was bumped | Bump `tauri.conf.json:version` too |
| About panel shows `1.0.7` but installer is `1.0.6` | `BUNDLE_VERSION` bumped but `tauri.conf.json` not | Same — bump `tauri.conf.json` |
| Windows reports the .exe as `1.0.6` in Properties | Same root cause | Same — bump `tauri.conf.json` |

### What's new
- **Tool parity (paid tier)**: MC now ships 7 local tools that the
  model can invoke via the MAIC plugin — `read_file`, `write_file`,
  `edit_file`, `list_dir`, `bash_run`, `apply_patch`, `remember_fact`.
  Free users still get MAIC's 4 server tools (weather, web_search,
  get_current_time, calculate) for free. Paid users get all 11.
- **Tier-aware tool gating**: tier is read from `MC_USER_TIER` env var
  (set by `maic_login` from the response). The MAIC plugin only registers
  local tools when the user's tier is `pro / pro_plus / team / enterprise`.
  Free users see no local tools in the chat — they don't get to invoke
  them, and the model doesn't even know they exist.
- **Tier badge + token usage in dashboard**: post-login is now the
  dashboard (was: redirect to OpenClaw chat). The dashboard shows the
  current tier as a colored pill (free=gray, pro=blue, pro_plus=purple,
  team=amber, enterprise=red) and token usage text below.
- **Nudge modal**: when MAIC says the user is at 50% / 100% / 500 / 1000
  / cap, MC shows a non-blocking modal with a copy that's stub-laden
  by default (server-side copy is the A/B test target — Lesson locked:
  copy comes from MAIC). Modal dismisses on click, doesn't reappear
  until `mc_refresh_tier` is called.
- **Tier-change detection**: when the tier drops from paid to free (e.g.
  subscription canceled), MC shows a one-shot modal saying "Some tools
  are no longer available" with a "Got it" dismiss. The dashboard
  re-renders without the locked tools.

### New Tauri commands
- `mc_get_tier` → returns `TierInfo { tier, tier_changed, last_updated }`. Cached 5 min.
- `mc_get_nudge` → returns `NudgeDecision { kind, text, used, limit }`. Cached 30s.
- `mc_list_tools` → returns the list of tool names for the current tier.
- `mc_refresh_tier` → bypasses cache, refetches from `/v1/auth/me`.
- `mc_apply_tier_change(tier_str)` → publishes the new tier to env +
  invalidates caches. The frontend is responsible for re-spawning the
  OpenClaw window so the MAIC plugin re-reads the env.

### New `miracle-claw-tools` helper binary
- **New file: `src-tauri/src/tools_main.rs`** (~80 LOC) — thin
  wire-protocol shim (argv parsing, stdin read, exit codes).
- **New file: `src-tauri/src/tools/exec.rs`** (~600 LOC + tests) — the
  7 tool executors + path allowlist enforcement.
- **Path allowlist** (Lesson 169, applied regardless of tier): only
  Documents/, Desktop/, Downloads/, and MC's workspace dir. WSL paths
  `/mnt/c/...` get converted to Windows-native before the check.
- **Bash sandboxing**: `cmd /C <command>` execution with cwd pinned to
  an allowed path. 30s default timeout (max 60s — not enforced strictly
  in v1.0.7, see "Known limitations" below).
- **Compiled as a separate `[[bin]]`** — no Tauri runtime dependency,
  ~7 MB standalone. The MAIC plugin spawns it; the result is the stdout
  string the model sees as the tool result.

### MAIC plugin v0.2.0
- **File: `depot/maic-plugin/index.js`** — now also registers the 7
  local tools when `MC_USER_TIER` is paid. Inherits the v0.1.0
  `tool_execution: "client"` injection (Lesson 293).
- **Schemas kept in sync with Rust**: the JS `TOOL_SCHEMAS` object
  mirrors `src-tauri/src/tools/schemas.rs`. Drift is caught by the Rust
  test `each_tool_serializes_to_openai_format` which checks the
  wire-format JSON.

### Changed — `maic_login` now publishes tier
- Calls `Tier::from_str(login_response.tier)` and writes to `MC_USER_TIER`
  env var. Invalidates both tier and quota caches so the next dashboard
  read returns fresh data.
- **No new parameter** — the tier is in the existing login response.

### Dashboard frontend rewrite
- **File: `src/main.js`** — post-login = dashboard (was: redirect to
  OpenClaw chat). The dashboard has a tier badge, a usage bar, and a
  single OpenClaw tile. Clicking the tile opens
  `http://localhost:28789/` in a child window via `window.open()`.
- **Nudge modal + tier-change modal** appended to the DOM with
  click-to-dismiss. No animations in v1.0.7 (CSS transitions are
  minimal — fade-in only).
- **Sign Out button** in the dashboard footer — calls `maic_logout`
  (added in v1.0.6) and returns to the login form.

### Dependencies added
- `once_cell = "1"` (lazy statics for tier + quota caches)

### Verified
- `cargo test --lib` — **49/49 pass** (2 new: `tools_for_tier_free_returns_empty`,
  `tools_for_tier_paid_returns_all_seven`).
- `cargo test --bin miracle-claw-tools` — **11/11 pass** (4 new exec tests
  + 7 schemas tests).
- `cargo build --bin miracle-claw-tools` — clean (1 unrelated pre-existing
  warning in `lib.rs`).
- **End-to-end smoke test** (Linux dev box):
  - `miracle-claw-tools read_file /etc/hostname` → rejected (outside allowlist) ✓
  - `miracle-claw-tools write_file` to `~/Documents/mc-tools-test/test.txt` → wrote 13 bytes ✓
  - `miracle-claw-tools read_file` (via stdin) → returned "hello v1.0.7" ✓
  - `miracle-claw-tools edit_file` → replaced 5 chars with 7 ✓
  - `miracle-claw-tools list_dir` → returned `[file] test.txt (15 bytes)` ✓
  - `miracle-claw-tools bash_run` → ran `echo hello from bash` ✓
  - `miracle-claw-tools apply_patch` → applied hunk, file modified ✓
  - `miracle-claw-tools remember_fact` → "stored locally (not yet synced)" ✓

### Known limitations
- **`/v1/usage/quota` endpoint not yet live on MAIC**: until David ships
  it, the nudge modal won't fire (it'll show "—"). All other features
  work. The endpoint spec is documented in
  `/home/adeal/.openclaw/workspace/memory/2026-08-19.md` (David's
  section).
- **`tier_changed: true` flag not yet in `/v1/auth/login` response**:
  David needs to add this flag to the login response so we can detect
  downgrades. Without it, the tier-change modal won't fire on the
  downgrade case. We do still detect it via the next `mc_refresh_tier`
  call after the user clicks the tier badge.
- **`bash_run` timeout not strictly enforced**: it's passed to the
  process descriptor but `Command::output` doesn't honor it. Future
  v1.0.8 enhancement: switch to `tokio::process` with a real timeout.
- **`remember_fact` is a stub**: returns "stored locally (not yet
  synced to MAIC)". Full implementation lands in v1.0.8 once MAIC's
  `/v1/user/facts` endpoint is locked.
- **Single-child-window dashboard**: not yet a multi-window UI. The
  v1.1.0 dashboard UX (multiple tiles, navigation, deep-linking) is
  scoped separately.

### Test plan (one-shot, by David)
1. `taskkill /F /IM miracle-claw.exe /T; taskkill /F /IM node.exe /T`
2. Uninstall v1.0.6 via Settings → Apps
3. Run v1.0.7 installer
4. Launch → login modal appears
5. Log in with `championnm@yahoo.com` + password
6. **Dashboard check**: post-login should land on the dashboard (NOT
   redirect to OpenClaw). Tier badge should show "Pro" (or whatever
   tier the user has). Usage bar should show "0 / X tokens".
7. Click the OpenClaw tile → should open in a new window at
   `http://localhost:28789/`
8. **Free user check**: log out, log in as a free account → dashboard
   shows "Free" badge. Attempt to use file tools in chat → model
   should say "I don't have access to file tools" (no local tools
   registered).
9. **Tool execution check (paid user)**: in chat, ask the model to
   read a file under `~/Documents/` → should work, returns the file
   contents. Ask to write to `~/Desktop/` → should work, file appears.
10. **Tier-change check**: manually downgrade a test account on MAIC
    → log out, log back in → dashboard should show "Free" badge. If
    the `tier_changed` flag is shipped, the modal fires once.
11. **Logout check**: click "Sign out" in dashboard footer → returns
    to login form. Keychain entries are wiped (Windows Credential
    Manager → look for `com.adealauto.miracle-claw` after logout).
12. **Re-login check**: log back in → dashboard appears, no errors.

### Next
- v1.0.8 — `remember_fact` real impl + MAIC syncs, `bash_run` strict
  timeout via tokio. Smaller, focused patch.
- v1.1.0 — dashboard UX iteration (multiple tiles, navigation, deep
  links, child-window management). Spec at `notes/V1.1.0-DASHBOARD-PLAN.md`.
- MAIC backend (parallel, David's work): `/v1/usage/quota` endpoint
  + `tier_changed: true` flag in `/v1/auth/login` response.

## [v1.0.8] — 2026-08-19 11:00 MDT (commit `e8cf632`, installer `ea86a487d45b43590e4706a5e06229a5`, 57,081,588 bytes)

### Lesson 461 — OpenClaw tile spawns a sibling webview window (not a popup)

**Symptom (David 10:35 MDT, after v1.0.7-rc1 hotfix)**: "When I click the Openclaw Box, I get nothing it doesn't open another page or window".

**Root cause**: The dashboard was calling `window.open('http://localhost:28789/', ...)` from the Tauri 2 main webview. **Tauri's webview silently returns null** for popup requests — no exception, no error. The v1.0.7-rc1 hotfix's `window.location.href` fallback would have navigated the dashboard AWAY from itself, leaving the user stuck staring at chat with no way back to the dashboard. Same outcome from the user's perspective: dashboard unusable.

**Fix**: Replaced the `window.open` JS path with a Tauri command that uses `tauri::WebviewWindowBuilder` to spawn a dedicated sibling webview (label `openclaw-chat`) pointing at the chat gateway. The dashboard and chat are now siblings — both child webviews of the Tauri app process — not parent-and-popup.

**Files changed**:
- `src-tauri/src/lib.rs` — new `openclaw_open_window` command using `WebviewWindowBuilder::new(...).title(...).inner_size(...).build()`. Idempotent: if window with that label already exists, focuses it instead of spawning a duplicate. Registered in `invoke_handler![]` next to `start_gateway_after_login`.
- `src-tauri/capabilities/openclaw.json` (NEW) — whitelists the `openclaw-chat` window label for `core:default`, `core:window:default`, `core:webview:default`, `core:event:default`. Without this entry, Tauri 2's runtime rejects the new window with "window label 'openclaw-chat' not allowed by capabilities".
- `src/main.js::openOpenClaw()` — replaced `window.open()` and `window.location.href` fallback with `invoke('openclaw_open_window')`. Loading state on the tile ("Starting…") + console diagnostics preserved.
- `src-tauri/src/{Cargo.toml,tauri.conf.json}`, `package.json`, `src-tauri/resources/BUNDLE_VERSION` — version bump 1.0.7 → 1.0.8 (Lesson 459).

### Sub-lesson 461b — Dashboard must handle "logged out" state explicitly

**Symptom (David, same session)**: "There is another box with nothing below that unknown, i'm guessing that should be the login options instead of having to click unknow".

**Root cause**: When `mc_get_tier` failed (no JWT, expired JWT, network blip), `renderDashboard()` rendered with `tier=null` → label "Unknown" → usage-bar showing "—". The only escape was clicking the hidden-clickable tier badge. Bad UX.

**Fix**: In `renderDashboard()`, after the parallel `mc_get_tier` / `mc_get_nudge` fetches, check if the tier fetch rejected with `"not logged in"` and redirect to `renderLogin()` instead of rendering a half-broken dashboard. One early-return, ~5 lines.

### Installer verification
- **MD5**: `ea86a487d45b43590e4706a5e06229a5` (was `185e29eff0dda45de4443dc410e382e5` for v1.0.7-rc1 hotfix)
- **Size**: 57,081,588 bytes (~55 MB)
- **Path on David's desktop**: `C:\Users\Adeal\Desktop\MiracleClaw_1.0.8_x64-setup.exe`
- **Bundle contents**: `miracle-claw.exe` (13.4 MB, includes new command), `resources/miracle-claw-tools.exe` (455 KB), `miracle-claw-launcher.exe` (325 KB).
- **Archived previous**: `MiracleClaw_1.0.7_185e29e_x64-setup.exe` (v1.0.7-rc1 hotfix).
- **Also archived**: `MiracleClaw_1.0.6_b280a93_x64-setup.exe` — the bogus v1.0.6 file from the Lesson 459 incident (misnamed installer from when tauri.conf.json wasn't bumped). The legitimate v1.0.6 ship (MD5 `48cff6ab9313c17a5442666f95e0e3cf`) lives only on David's desktop from the original install.

### Test plan for David
1. Uninstall v1.0.7 hotfix (or just install v1.0.8 over it — installer should be safe-upgrade)
2. Launch v1.0.8 → login (if not auto-signed-in)
3. Click the OpenClaw tile → a SEPARATE window opens with the chat UI
4. Close the chat window → click OpenClaw again → window reopens
5. Click OpenClaw while chat is already open → chat window comes to front (focused), no duplicate spawns
6. Report back any issues

### What's NOT in v1.0.8
- v1.1.0 dashboard pivot (`notes/V1.1.0-DASHBOARD-PLAN.md`) — deferred until v1.0.8 is verified by David.
- YoClaw skills-folder port — v1.1.x candidate.
- MAIC-side `/v1/usage/quota` and `tier_changed` flag — David's separate workstream.

---

## [v1.0.9-rc1] — 2026-08-19 12:35 MDT (rc for testing)

### Lesson 462 — Orphan gateway process holds port 28789 across restarts → blank window

**Symptom (David 12:30 MDT, after v1.0.8 install)**: Click OpenClaw tile → new window opens but is **completely blank** (white page, no error message). Sidecar log shows:

```
[gateway] ready in 12.5s (http://127.0.0.1:28789, ws://127.0.0.1:28789)
[gateway] starting channels and sidecars...ok
Gateway failed to start: gateway already running (pid 27332); lock timeout
Port 28789 is already in use. - pid 27332
```

**Root cause**: The launcher sidecar pattern is `miracle-claw-launcher.exe → node openclaw.mjs → launcher exits with code 0`. On Windows, when the launcher wrapper exits, the openclaw.mjs child process keeps running. The new launcher MC tries to spawn on the next session sees port 28789 bound by the **orphan**, bails with `port already in use`, and the launcher terminates cleanly. MC's webview window points at the ORPHAN's HTTP server (which knows nothing about the current session) and renders blank.

**Two-part fix**:

**Part 1 — `kill_orphan_holding_port(port)` in `spawn_launcher_and_wait`**: Before spawning the launcher, check if port 28789 is in use via `netstat -ano | findstr :28789`. If yes, find the PID holding it (LISTENING state only — don't kill clients), kill via `taskkill /F /T /PID <pid>`, wait 500ms for the OS to release the port, then proceed with normal spawn. Logs each killed PID with the `[miracle-claw] Lesson 462:` prefix so future debugging can trace orphans back.

**Part 2 — `check_http_ready(url, timeout)` in `openclaw_open_window`**: After the TCP-only pre-flight check, do a real HTTP GET on `http://127.0.0.1:28789/` with a 1-second timeout. If it doesn't return a 2xx, return `Err("OpenClaw gateway port is open but not serving the chat UI. Please log out and back in to reset it.")` instead of letting the user see a blank window.

These are **layered defenses**, not alternatives:
- Part 1 prevents most cases (active restarts never encounter the orphan).
- Part 2 catches the edge cases Part 1 misses (a fresh-but-broken gateway, an OS race during port release, a future bug in the launcher pattern).

**Files changed**:
- `src-tauri/src/lib.rs`:
  - New `kill_orphan_holding_port(port)` (~80 LOC, Windows-specific via `cfg(target_os = "windows")`, with a no-op fallback for cross-platform code paths).
  - New `check_http_ready(url, timeout)` (~80 LOC, hand-rolled minimal HTTP/1.1 client — no reqwest dep, keeps the binary small).
  - `spawn_launcher_and_wait` calls `kill_orphan_holding_port` before spawning the sidecar.
  - `openclaw_open_window` calls `check_http_ready` after the existing TCP pre-flight.

### Bug #1 — New Account button on login screen (David 11:20 MDT)

**Symptom**: Login screen had only a tiny "Create one" link in the footer. New users installing MC had to hunt for it.

**Fix**: New `<button type="button" id="register-btn">` directly below the Sign in button, full-width, labeled "New here? Create a MAIC account". Click handler calls new `open_register_url` Tauri command which opens `https://milagrocloud.com/register` in the OS default browser via `cmd /c start "" <url>`. Defense-in-depth: the Rust command re-checks the URL is `https://` and the host is in our allow-list (`milagrocloud.com`, `www.milagrocloud.com`).

### Bug #2 — Dashboard polish (David 11:20 MDT)

**Symptom**: OpenClaw tile looked like a placeholder; description was wordy; visual hierarchy weak.

**Fix**:
- New `.tile.tile-primary` CSS class — gradient background, 2px link-colored border, larger icon, slightly bigger padding. The OpenClaw tile is the only tile in v1.1.0 and the primary CTA; visual emphasis matters.
- `.tile:hover` now also adds a subtle shadow (`box-shadow: 0 4px 12px rgba(0, 0, 0, 0.08)`).
- Tightened the description (removed the "Free tier gets weather, web search, time and calculator" sentence — that detail belongs in the upgrade nudge, not the CTA copy).
- Footer links ("Refresh tier", "Sign out") now have hover color transition.

### Files changed
- `src-tauri/src/lib.rs` — `kill_orphan_holding_port`, `check_http_ready`, `open_register_url`, integrated into existing commands + registered in `invoke_handler!`.
- `src/main.js` — `tile tile-primary` class on OpenClaw tile, tightened description, register button wiring (already done in prior session).
- `src/styles.css` — `.tile.tile-primary`, hover shadow, footer link transitions.
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json`, `src-tauri/resources/BUNDLE_VERSION` — version bump 1.0.8 → 1.0.9 (Lesson 459).

### What's NOT in v1.0.9-rc1
- v1.1.0 dashboard pivot (`notes/V1.1.0-DASHBOARD-PLAN.md`) — still deferred.
- Pre-build version-check guard in `build-windows-docker.sh` (Lesson 459 follow-up) — v1.0.x polish.
- YoClaw skills-folder port — v1.1.x.

---

## [v1.0.9-rc2] — 2026-08-19 13:35 MDT (diagnostic rc)

### Lesson 462 follow-up — Why did rc1's orphan-killer not prevent the rc1 test failure?

**Symptom (David 13:30 MDT, after rc1 install + test)**: David installed v1.0.9-rc1, clicked OpenClaw tile, and got the **same error** as before:

```
Could not open OpenClaw: gateway did not bind port 28789 within 15s.
Please restart the app...
```

Sidecar log showed the FIRST `setup()`-spawned launcher **successfully bound port 28789** (`[gateway] http server listening` ~4.6s after spawn) and reached `[gateway] ready` ~5s after spawn — but MC reported "NOT ready" anyway. Then a SECOND launcher was spawned (presumably by `start_gateway_after_login` after the dashboard tile was re-clicked), the orphan-killer found the first launcher still running as pid 13916, killed it, and the second launcher is currently bound to `127.0.0.1:28789`.

The contradiction: **the gateway bound within 15s, but `wait_for_gateway_ready` returned Err anyway.** We have no way to debug from the sidecar log alone — Tauri 2 GUI apps on Windows **discard stderr** (no console attached), so all `eprintln!` output between `setup()` and `start_gateway_after_login` is invisible to the user.

### Fix — file-based diagnostic logging + per-iteration TCP timeout

Two changes to `wait_for_gateway_ready` and the surrounding call sites:

**1. New `log_to_file(msg)` helper** — writes `msg` (prefixed with a Unix epoch timestamp) to:
- Windows: `%APPDATA%\MiracleClaw\miracle-claw.log`
- Linux/macOS: `$HOME/.miracle-claw/miracle-claw.log`

Append-only, best-effort (failures swallowed — this is a diagnostic, not a critical log). **This is the post-mortem channel** when stderr is gone.

**2. `wait_for_gateway_ready` now uses `TcpStream::connect_timeout`** instead of bare `TcpStream::connect`. Per-iteration ceiling: 500ms. **This is the actual fix for the symptom:**

Bare `TcpStream::connect` against an unbound port on Windows **blocks for the OS-default TCP retry timeout (~21 seconds)** — Windows sends up to 3 SYN retransmissions before giving up, each with exponential backoff. When MC's wait loop calls `connect()` while the gateway is still booting (port not yet bound), the call **blocks for ~21 seconds, consuming our entire 15-second outer deadline in a single iteration** — so the loop never gets to test subsequent seconds when the bind finally lands.

`connect_timeout(addr, 500ms)` per-iteration forces a clean 500ms ceiling. The wait loop now actually polls: connect (≤500ms) → fail → sleep 100ms → connect → ... → connect succeeds at second ~5.

**3. `wait_for_gateway_ready` logs every iteration's outcome to `miracle-claw.log`** with iter count + elapsed time, so even if the bug recurs we'll see exactly which iter succeeded/failed and when.

**4. `kill_orphan_holding_port` got verify-after-kill** (was already in progress during rc1 work, finished in rc2): after the `taskkill` + 500ms sleep, **re-run netstat** and return Err if the port is still bound. Defends against silent taskkill failures (Windows ACL, antivirus, etc.) where taskkill says "success" but the OS hasn't actually released the socket.

**5. All `kill_orphan_holding_port` call sites log to `miracle-claw.log`** — scan results, kill results, verify results.

**6. `setup()` and `start_gateway_after_login` log their spawn decisions** to `miracle-claw.log` — provider_configured=true/false, key_present=true/false, spawn result.

### What David needs to do for v1.0.9-rc2

1. Install the rc2 installer.
2. Reproduce the same flow (fresh launch → click OpenClaw tile).
3. If error appears, **send the contents of `%APPDATA%\MiracleClaw\miracle-claw.log`** to me. That file will show every TCP connect attempt, every spawn result, every orphan-kill decision, every verify check.

### Files changed
- `src-tauri/src/lib.rs`:
  - New `log_to_file(msg)` helper (~50 LOC, `cfg(target_os = "windows")` + `cfg(not(target_os = "windows"))` branches).
  - `wait_for_gateway_ready` rewritten: per-iteration `connect_timeout(500ms)` instead of bare `connect`, iter-counted logging.
  - `kill_orphan_holding_port` got verify-after-kill and per-step `log_to_file` calls.
  - `setup()` and `start_gateway_after_login` got `log_to_file` calls at every spawn decision.
  - `spawn_launcher_and_wait` got additional `log_to_file` calls at sidecar spawn, CommandEvent::Error, and CommandEvent::Terminated.
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json` — version bump 1.0.9 → 1.0.9-rc2.

### Known follow-ups (deferred)
- Once we have the rc2 log we can decide if the per-iteration `connect_timeout` already fixed it (likely) or if there's a deeper bug.
- The HTTP readiness check (`check_http_ready`) is still in `openclaw_open_window` as the second-layer defense.
- v1.1.0 dashboard pivot — still deferred.

## [v1.0.9-rc3] — 2026-08-19 15:54 MDT (idempotency fix)

### What rc3 fixes

David's 15:10 MDT question cut to the heart of the issue: "Why is getting
Openclaw to run any different than in the early versions where it came up
after the log in?" The answer:

- **v1.0.5-v1.0.8**: launcher was spawned ONCE in `setup()` at app start.
  Login → tile click just opened the webview to the already-running gateway.
  No respawn cycle, no orphan risk.

- **v1.0.9-rc1/rc2**: the launcher spawn was moved OUT of `setup()` and
  INTO `start_gateway_after_login` (Lesson 449 — required because
  openclaw needs MAIC_API_KEY in env, which only exists after login).
  BUT the frontend still calls `start_gateway_after_login` on EVERY tile
  click (src/main.js:162 had a "cheap no-op if already running" comment
  that was wrong — the function unconditionally killed any previous handle
  and respawned).

- The rc2 fix (orphan-killer + connect_timeout) worked AROUND this by
  cleaning up orphans after each kill+respawn cycle. The rc3 fix makes
  the cycle itself skip when the gateway is already serving.

### The fix (1 function, ~25 lines)

`start_gateway_after_login` now does an idempotency probe at the top:

```rust
// rc3 idempotency probe (Lesson 466)
match std::net::TcpStream::connect_timeout(
    &format!("127.0.0.1:{}", OPENCLAW_PORT).parse()?,
    std::time::Duration::from_millis(500),
) {
    Ok(_) => {
        log_to_file("start_gateway_after_login: idempotent no-op — gateway already serving");
        return Ok(());
    }
    Err(e) => log_to_file(&format!("...port probe failed ({}); proceeding to spawn", e)),
}
```

If the port is bound → return Ok immediately, don't touch `launcher_child`,
don't kill anything, don't spawn anything.

If the port is unbound → fall through to the existing
kill-previous-handle-then-respawn path (covers first-run and login-retry
cases where we genuinely need a fresh spawn).

### Behavior after rc3

- **Login first time** → spawn launcher → gateway binds → dashboard
  renders → first tile click is no-op ✅
- **Subsequent tile clicks** → pure no-ops (no kill, no spawn, no orphan
  risk) ✅
- **Hot-reload config change during chat** → openclaw respawns itself →
  port briefly unbound → next tile click falls through to respawn → may
  briefly orphan during reload, but rare edge case ✅

### What David needs to do for v1.0.9-rc3

1. Install `MiracleClaw_1.0.9-rc3_x64-setup.exe` over rc2
2. Login
3. Click the OpenClaw tile — chat window opens
4. **Click the tile 5 more times in a row** — window should just refocus,
   no respawn cycles, no blank windows
5. Optional: share `%APPDATA%\MiracleClaw\miracle-claw.log` to confirm
   the `idempotent no-op — gateway already serving` line fires on each
   subsequent click

### Files changed

- `src-tauri/src/lib.rs` — idempotency probe added to
  `start_gateway_after_login` (line ~2376)
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json` — version bump
  1.0.9-rc2 → 1.0.9-rc3

### Lesson 466 (to be promoted after verification)

**Spawn-once functions must be idempotent on the resource state, not
just the handle state.** A "kill previous handle, spawn fresh" pattern
only works if the underlying resource (bound port, child PID, file lock)
is also killed. With wrapper-sidecar patterns where the wrapper exits
clean but the child persists, an idempotency check on the resource
(port probe, PID lookup, HTTP check) is mandatory before any respawn
cycle.

**Symptom signature**: clicking the same UI control N times causes the
launcher / daemon / subprocess to restart N times, leaving N-1 orphans
holding shared resources.

**Fix**: probe the resource state (port bound? PID alive? HTTP responding?)
before any kill+respawn cycle. If the resource is healthy, return Ok
without touching the handle.

### Known follow-ups (deferred)

- v1.1.0 dashboard pivot — still deferred.
- Once rc3 verified → bump 1.0.9-rc3 → 1.0.9 (final) and promote Lesson
  466 to MEMORY.md.

## [v1.0.9-rc3] — 2026-08-19 15:54 MDT (idempotency fix)

### What rc3 fixes

David's 15:10 MDT question cut to the heart of the issue: "Why is getting
Openclaw to run any different than in the early versions where it came up
after the log in?" The answer:

- **v1.0.5-v1.0.8**: launcher was spawned ONCE in `setup()` at app start.
  Login → tile click just opened the webview to the already-running gateway.
  No respawn cycle, no orphan risk.

- **v1.0.9-rc1/rc2**: the launcher spawn was moved OUT of `setup()` and
  INTO `start_gateway_after_login` (Lesson 449 — required because
  openclaw needs MAIC_API_KEY in env, which only exists after login).
  BUT the frontend still calls `start_gateway_after_login` on EVERY tile
  click (src/main.js:162 had a "cheap no-op if already running" comment
  that was wrong — the function unconditionally killed any previous handle
  and respawned).

- The rc2 fix (orphan-killer + connect_timeout) worked AROUND this by
  cleaning up orphans after each kill+respawn cycle. The rc3 fix makes
  the cycle itself skip when the gateway is already serving.

### The fix (1 function, ~25 lines)

`start_gateway_after_login` now does an idempotency probe at the top:

```rust
// rc3 idempotency probe (Lesson 466)
match std::net::TcpStream::connect_timeout(
    &format!("127.0.0.1:{}", OPENCLAW_PORT).parse()?,
    std::time::Duration::from_millis(500),
) {
    Ok(_) => {
        log_to_file("start_gateway_after_login: idempotent no-op — gateway already serving");
        return Ok(());
    }
    Err(e) => log_to_file(&format!("...port probe failed ({}); proceeding to spawn", e)),
}
```

If the port is bound → return Ok immediately, don't touch `launcher_child`,
don't kill anything, don't spawn anything.

If the port is unbound → fall through to the existing
kill-previous-handle-then-respawn path (covers first-run and login-retry
cases where we genuinely need a fresh spawn).

### Behavior after rc3

- **Login first time** → spawn launcher → gateway binds → dashboard
  renders → first tile click is no-op ✅
- **Subsequent tile clicks** → pure no-ops (no kill, no spawn, no orphan
  risk) ✅
- **Hot-reload config change during chat** → openclaw respawns itself →
  port briefly unbound → next tile click falls through to respawn → may
  briefly orphan during reload, but rare edge case ✅

### What David needs to do for v1.0.9-rc3

1. Install `MiracleClaw_1.0.9-rc3_x64-setup.exe` over rc2
2. Login
3. Click the OpenClaw tile — chat window opens
4. **Click the tile 5 more times in a row** — window should just refocus,
   no respawn cycles, no blank windows
5. Optional: share `%APPDATA%\MiracleClaw\miracle-claw.log` to confirm
   the `idempotent no-op — gateway already serving` line fires on each
   subsequent click

### Files changed

- `src-tauri/src/lib.rs` — idempotency probe added to
  `start_gateway_after_login` (line ~2376)
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json` — version bump
  1.0.9-rc2 → 1.0.9-rc3

### Lesson 466 (to be promoted after verification)

**Spawn-once functions must be idempotent on the resource state, not
just the handle state.** A "kill previous handle, spawn fresh" pattern
only works if the underlying resource (bound port, child PID, file lock)
is also killed. With wrapper-sidecar patterns where the wrapper exits
clean but the child persists, an idempotency check on the resource
(port probe, PID lookup, HTTP check) is mandatory before any respawn
cycle.

**Symptom signature**: clicking the same UI control N times causes the
launcher / daemon / subprocess to restart N times, leaving N-1 orphans
holding shared resources.

**Fix**: probe the resource state (port bound? PID alive? HTTP responding?)
before any kill+respawn cycle. If the resource is healthy, return Ok
without touching the handle.

### Known follow-ups (deferred)

- v1.1.0 dashboard pivot — still deferred.
- Once rc3 verified → bump 1.0.9-rc3 → 1.0.9 (final) and promote Lesson
  466 to MEMORY.md.

## [v1.0.9-rc4] — 2026-08-19 16:25 MDT (HTTP-level readiness probe)

### What rc4 fixes

David's 16:19 MDT sidecar log exposed that the rc3 probe was too coarse.
Sequence from his install:

```
16:17:21  http server listening (4.9s)
16:17:22  gateway ready
16:17:28  maic_login success → JWT injected → triggers config change
16:17:29  hot reload applied → openclaw respawns itself briefly
16:17:??  start_gateway_after_login called (tile click)
          ├─ rc3 TCP probe: SUCCEEDS (old node still bound during reload)
          └─ returns Ok(()) — no kill, no respawn ✅
16:17:??  openclaw_open_window called next
          ├─ TCP probe: SUCCEEDS
          └─ HTTP probe: FAILS (10060) — server mid-reload, not serving
16:17:??  error returned: "OpenClaw gateway port is open but not serving the chat UI"
```

rc3 prevented the respawn-on-every-click cycle (Lesson 466 fix held), but
the TCP-only probe returned Ok on stale port-holders. The HTTP probe in
`openclaw_open_window` correctly caught the stale state and refused to
open the window, but the user experience was a dead-end error instead of
chat.

### The fix (1 function, 12 line delta vs rc3)

Replace `TcpStream::connect_timeout(500ms)` in `start_gateway_after_login`
with `check_http_ready("http://127.0.0.1:28789/", Duration::from_millis(500))`:

```rust
// rc4 idempotency probe (Lesson 466 + Lesson 467)
match check_http_ready(
    "http://127.0.0.1:28789/",
    std::time::Duration::from_millis(500),
) {
    Ok(status) => {
        log_to_file(&format!(
            "start_gateway_after_login: idempotent no-op — gateway already serving (HTTP {})",
            status
        ));
        return Ok(());
    }
    Err(e) => {
        log_to_file(&format!(
            "start_gateway_after_login: HTTP probe failed ({}); proceeding to clean+respawn",
            e
        ));
    }
}
```

If `GET /` returns 2xx → return Ok (gateway genuinely ready, no-op).
If port unbound OR HTTP fails → fall through to existing kill+respawn
path (orphan-killer catches stale pid, fresh spawn starts).

### Symmetry win

`start_gateway_after_login` and `openclaw_open_window` now check the same
readiness signal (HTTP 2xx from `check_http_ready`). Both fall back to
the same kill+respawn path on failure. The two click-tile functions are
no longer making different assumptions about what "ready" means.

### Behavior matrix (rc3 → rc4)

| State at click time | rc3 result | rc4 result |
|---|---|---|
| Gateway serving normally | no-op ✅ | no-op ✅ |
| Tile click during hot reload | no-op → HTTP probe fails in `openclaw_open_window` → error | kill+respawn → clean start |
| Orphan from prior session | no-op → orphan keeps port → HTTP fails → error | kill+respawn → orphan-killer catches stale → clean |
| First click after login | no-op (TCP succeeds within 4.9s) | no-op (HTTP also succeeds within ~4.9s) |

### Lesson 467 (new — promoted)

**TCP-port-bound is not gateway-ready.** A bare `TcpStream::connect_timeout`
returns success on any process holding the port — including dying nodes
mid-reload, orphans from earlier sessions, even non-openclaw processes
that happened to grab the port. Real readiness = HTTP 2xx from `GET /`.

**Anti-overengineering rule**: when probing "is the service ready?", check
the **highest-level signal the caller will use**. For an HTTP gateway,
that's `GET /` returning 2xx — not a TCP connect, not a PID lookup, not
a process list scan. The TCP probe is a necessary precondition but not
sufficient on its own.

**Symptom signature**: TCP connect succeeds but the actual operation
(HTTP GET, gRPC call, DB query) times out. The service is mid-shutdown,
mid-reload, or never started serving despite binding the port.

**Fix**: probe at the application layer (HTTP/1.1 GET for HTTP services,
HELLO/EHLO for SMTP, PING for Redis, etc.) before declaring readiness.

### Files changed

- `src-tauri/src/lib.rs` — `start_gateway_after_login` probe swapped
  from `TcpStream::connect_timeout` to `check_http_ready` (~12 line
  delta from rc3)
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json` — version
  bump 1.0.9-rc3 → 1.0.9-rc4

### What David needs to do for v1.0.9-rc4

1. Install `MiracleClaw_1.0.9-rc4_x64-setup.exe` over rc3
2. Login → dashboard renders → click OpenClaw tile (chat opens, one
   spawn)
3. Click the tile 4 more times — should refocus, log "idempotent no-op"
   with HTTP 200 each time
4. To trigger the cleanup path: while the chat window is open, change a
   config setting that forces openclaw hot-reload (or kill the node
   process externally and click the tile) — should respawn cleanly,
   log "HTTP probe failed; proceeding to clean+respawn"
5. Optional: share `%APPDATA%\MiracleClaw\miracle-claw.log` for the full
   trace

### Known follow-ups (deferred)

- v1.1.0 dashboard pivot — still deferred.
- Once rc4 verified → bump 1.0.9-rc4 → 1.0.9 (final) and promote Lesson
  466 + Lesson 467 to MEMORY.md.

## [v1.0.9-rc5] — 2026-08-19 18:35 MDT (HTTP probe in wait_for_gateway_ready + 30s budget)

### Symptom David saw

Fresh `MiracleClaw_1.0.9-rc4_x64-setup.exe` install, cold boot, click
OpenClaw tile → 15-20 second "sits there loading" → eventually either
chat opens OR frontend surfaces "Could not open OpenClaw: gateway did
not bind port 28789 within 15s".

rc4 was supposed to fix the cold-boot failure with the HTTP-level probe
in `start_gateway_after_login` and `openclaw_open_window`. But the
**third call site** — `wait_for_gateway_ready` itself, used by `setup()`
at boot — was still TCP-only. That's the call site that produced the
user-visible "gateway did not bind port 28789 within 15s" error.

### Diagnostic data (mc-rc4-fresh-fb.log)

```
[1787173445] wait_for_gateway_ready: start port=28789 timeout=15s
[1787173459] wait_for_gateway_ready: TCP connect failed iter=12 elapsed=509ms err=connection timed out
[1787173460] wait_for_gateway_ready: TIMEOUT after 12 iters; gateway did not bind port 28789 within 15s
```

All 9 attempts in David's log show: **iter=1 to iter=11 time out at
~510ms**, then **iter=12 either succeeds with elapsed=415µs OR
times out**. The race is that:

- The gateway consistently binds at ~T+14s from probe start.
- iter=12 starts at T+13.5s and gets 500ms to complete its connect.
- When the bind lands during iter=12's connect attempt → SUCCESS
  (415µs, instant).
- When the bind lands slightly later (T+13.6s or after) → iter=12
  times out → loop exits → Err.

The 15s budget gave iter=12 only a 1.5s window to catch the bind.
**Marginal by ~100ms.** Every run is a coin flip.

### The fix

Two changes in `src-tauri/src/lib.rs`:

1. **`wait_for_gateway_ready` now uses `check_http_ready` for final
   confirmation** after each successful TCP connect. The TCP probe
   stays for port-bound detection (faster, cheap), but the function
   no longer returns Ok on TCP connect alone — it confirms HTTP 2xx
   from `GET /`. Closes the Lesson 469 race: TCP succeeds the moment
   bind() lands, but the HTTP server may not be accepting yet.

2. **Probe timeout extended from 15s → 30s** in both `setup()` and
   `start_gateway_after_login`. With the HTTP probe as the readiness
   signal, 30s gives us a 16-second safety margin over the observed
   14s cold-boot bind time.

3. **Build script regex fixed** (Lesson 459 4th strike): the regex
   `[0-9]+\.[0-9]+\.[0-9]+` in `scripts/build-windows-docker.sh`
   stripped pre-release suffixes (`-rc4`, `-rc5`, `-hotfix`), so the
   script looked for `MiracleClaw_1.0.9_x64-setup.exe` when tauri
   actually built `MiracleClaw_1.0.9-rc5_x64-setup.exe`. Silently
   failed to copy to Desktop. Updated regex to
   `[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?`.

### Lesson 469 (new — promoted)

**For Tauri 2 GUI apps wrapping an HTTP-served gateway, the readiness
probe must be HTTP-level, not TCP-only.**

TCP `connect()` succeeds the moment `bind()` lands, but the HTTP server
may not be accepting/responding yet — worker pool still spinning up,
plugin pre-warm still in progress, auth middleware not yet wired.
openclaw emits `[gateway] http server listening` BEFORE `[gateway]
ready`, and there's a 0.5-1.5s gap where TCP probes return Ok but the
server can't actually serve a 200.

**Symptom signature**: TCP connect succeeds but the actual operation
(HTTP GET) times out. The service is bound-but-not-ready.

**Fix**: probe at the application layer (`GET /` returning 2xx for
HTTP services). TCP probe is a necessary precondition but not
sufficient. Reuse `check_http_ready` with a 500ms per-iteration
ceiling; loop until 2xx or deadline.

### Lesson 459 4th strike (new — promoted)

Build scripts that extract semver from `tauri.conf.json` MUST preserve
optional pre-release suffixes. The regex
`grep -oE '[0-9]+\.[0-9]+\.[0-9]+'` is wrong for any version like
`1.0.9-rc4` — it returns `1.0.9` and the script then looks for a
filename that doesn't exist. Use
`[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?` instead.

### Files changed

- `src-tauri/src/lib.rs` — `wait_for_gateway_ready` now does
  HTTP-level final confirmation (~50 line delta from rc4)
- `src-tauri/src/lib.rs` — `setup()` and `start_gateway_after_login`
  timeouts bumped 15s → 30s (2 line delta)
- `scripts/build-windows-docker.sh` — regex preserves pre-release
  suffix (1 line delta)
- `src-tauri/{Cargo.toml,tauri.conf.json}`, `package.json` — version
  bump 1.0.9-rc4 → 1.0.9-rc5

### What David needs to do for v1.0.9-rc5

1. Install `MiracleClaw_1.0.9-rc5_x64-setup.exe` over rc4
2. Cold boot: open MC → login → click OpenClaw tile
3. Expected: chat opens in <5 seconds after tile click (was 15-20s in rc4)
4. Verify `setup()` no longer prints "gateway NOT ready: gateway did
   not bind port 28789 within 15s" — should print "gateway READY"
   within ~5-15s.
5. Share `%APPDATA%\MiracleClaw\miracle-claw.log` for the trace.
   Expected new log lines:
   - `wait_for_gateway_ready: HTTP ready iter=N http_status=200`
   - `setup(): spawn_launcher_and_wait Ok — gateway READY port=28789`

### Known follow-ups (deferred)

- v1.1.0 dashboard pivot
- Promote Lesson 469 to MEMORY.md
- Promote Lesson 459 4th strike to MEMORY.md (this is a recurring
  bug — fix the script properly or add a pre-build version-sync check)
- Once rc5 verified → bump 1.0.9-rc5 → 1.0.9 (final)

## v1.0.9-rc7 — 2026-08-19 (Lesson 471: blank window round 2)

**NOT YET SHIPPED — build in progress.**

rc6 (Lesson 470) failed: cache-buster URL was logged, but neither
`on_page_load` hook nor `build FAILED` log line ever appeared.
Diagnosis: `incognito(true)` likely panicked silently in WebviewWindowBuilder
on Tauri 2.11 + WebView2 on David's machine, OR `PageLoadEvent` doesn't fire.

**Three changes**:

1. **`nuke_webview2_cache_dir()`** — delete `%LOCALAPPDATA%\MiracleClaw\EBWebView`
   before build. Most reliable cache-bust: nothing to cache, no poisoned references.
2. **Drop `incognito(true)` and `on_page_load(...)`** — both were suspect.
3. **Polling probe** — std::thread, 10s, 500ms interval, logs URL+title
   every state transition. This is the post-mortem signal that rc6 lacked.

**Polling probe troubleshooting guide**:
- URL never moves past `about:blank` → asset-load problem (CSP/network)
- URL changes but title stays empty → custom-element mount problem
- Polling never fires → build() panicked


### RC7 RESULT (David installed 2026-08-19 21:15 MDT)

**Blank window persists, but root cause now confirmed.**

Last 3 lines of `miracle-claw.log`:
```
[1787195567] openclaw_open_window: cache_buster v=1787195567330, chat_url=http://127.0.0.1:28789/?v=1787195567330
[1787195567] nuke_webview2_cache_dir: no-op (not found) C:\Users\Adeal\AppData\Local\MiracleClaw\EBWebView
[1787195567] nuke_webview2_cache_dir: no-op (not found) C:\Users\Adeal\AppData\Roaming\MiracleClaw\EBWebView
```

No "window created", no "build FAILED", no `poll:` lines. **The polling
thread never fired — `builder.build()` panicked silently before the
thread spawned.**

**Smoking gun in Tauri 2.11.5 source** (`window/mod.rs:339`):
```rust
let webview = window.webviews().first().unwrap().clone();
```
The `.unwrap()` panics if `build_internal` returns a window with zero
webviews — happens on David's machine due to a WebView2 init edge case
(corrupted user-data-dir, missing redistributable, AV interference,
child-process spawn race). **Lesson 464 strikes again**: Tauri 2 GUI
apps on Windows discard stderr, panic is invisible.

**Cache theory ruled out**: `nuke_webview2_cache_dir` was a no-op
(neither `EBWebView` dir existed on this fresh install). Lesson 470's
cache-busting hypothesis was wrong from the start.

---

## v1.0.9-rc8 — 2026-08-20 00:36 MDT (Lesson 472: catch the panic)

### The fix

Three changes in `src-tauri/src/lib.rs`:

1. **Install custom panic hook** in `setup()`:
   ```rust
   std::panic::set_hook(Box::new(|info| {
       let msg = ...; // format panic payload
       let location = info.location().map(...);
       log_to_file(&format!("PANIC at {location}: {msg}"));
   }));
   ```
   Writes panic message + source location to log file. Lesson 464-recovery:
   even though stderr is invisible, our `log_to_file()` IS captured.

2. **Wrap `builder.build()` in `std::panic::catch_unwind`**:
   ```rust
   let built_window = match std::panic::catch_unwind(
       std::panic::AssertUnwindSafe(|| builder.build())
   ) {
       Ok(Ok(w)) => w,
       Ok(Err(e)) => return Err(...),
       Err(panic_payload) => return Err(format!("build panicked: {msg}")),
   };
   ```
   Catches the silent panic in Tauri's `with_webview` and surfaces it
   as a clean error to the frontend instead of disappearing.

3. **Minimal builder chain** — dropped `.title()`, `.resizable()`,
   `.center()`, `.min_inner_size()` to rule out builder-method
   interaction with the panic path.

### Lesson 472 (new — root cause confirmed)

**Tauri 2.11.5's `WebviewWindowBuilder::build()` can panic silently
inside `.with_webview()` at `window.webviews().first().unwrap()` when
`build_internal` returns a window whose webview list is empty.** This
is a `Result::unwrap()` anti-pattern in Tauri's own source code.

**Defense pattern**:
- Install `std::panic::set_hook(Box::new(|info| log_to_file(...)))`
  in `setup()` — captures panics to log because stderr is invisible.
- Wrap every Tauri webview creation in `std::panic::catch_unwind`
  with `AssertUnwindSafe` to surface the silent panic as a clean error.
- **Never** rely on stderr or console output in Tauri 2 GUI apps —
  always log to file (Lesson 464).

### Lesson 473 (new — diagnostic strategy)

Three-mode polling-probe diagnostic for blank Tauri webview windows:
1. **No poll lines at all** → `builder.build()` panicked (rc7 mode)
2. **URL stuck at `about:blank`** → asset-load problem (CSP/network)
3. **URL changes but title empty** → custom-element mount failed
4. **URL + title populate but blank UI** → JS bundle error in page

### Lesson 474 (new — diagnostic signal)

`nuke_webview2_cache_dir: no-op (not found)` for both `LOCALAPPDATA`
and `APPDATA` paths is a **diagnostic signal**: cache was NEVER the
problem. Lesson 470 (cache-poisoning hypothesis) was wrong from the
start — should have been retired on first blank-window after
cache-buster deployed, not after.

## v1.0.9-rc12 — 2026-08-20 09:58 MDT (Lesson 482 + 483: CSP + race fix)

### The fix

Two changes:

1. **CSP fix** (Lesson 482, was originally rc11): Tauri CSP
   allowlisted `http://localhost:28789 ws://localhost:28789` but the
   webview loaded `http://127.0.0.1:28789/?v=<ts>`. WebView2 treats
   `localhost` and `127.0.0.1` as different hosts → CSP blocks all
   connect-src requests → empty UI. Added `http://127.0.0.1:28789`
   and `ws://127.0.0.1:28789` to every relevant directive in
   `tauri.conf.json` security.csp alongside the existing localhost
   entries.

2. **Race fix** (Lesson 483): MAIC login writes the JWT to openclaw.json,
   triggering an async chokidar hot-reload in the openclaw gateway
   (~600ms–5s). User clicking OpenClaw immediately after login races
   against this reload. Two-layer fix:
   - In `maic_login`, after the JWT write: `wait_for_gateway_ready(15s)
     + 1.5s stable window` (6 consecutive Ok probes) before returning Ok.
   - Bumped `openclaw_open_window`'s HTTP probe retry budget from
     3×1.5s + 2×200ms (4.9s) to 6×2s + 5×500ms (13s) as defense-in-depth.

### Result on David's system

rc12 installed cleanly. CSP fix shipped (verified by extracting the
installer and grepping the CSP string). All fixes verified present in
the binary. **But the blank chat window persisted.** The crash was
deeper than CSP or the hot-reload race — see rc13.

## v1.0.9-rc13 — 2026-08-20 (Lesson 491: same-window navigation)

### The fix

**Stop creating a second webview window. Navigate the existing main
webview to the chat URL.**

Pre-rc13 `openclaw_open_window` called
`WebviewWindowBuilder::new("openclaw-chat", WebviewUrl::External(...)).build()`
to spawn a sibling webview. On David's system this hangs deterministically
inside Tauri's webview2 init path (Lesson 487 — Tauri 2.11.5 + multiple
WebView2 runtimes + WebView2 user-data-dir that wasn't actually being
cleaned because of Lesson 488's path bug). The Tauri main process gets
killed by Windows Application Hang detection after ~7 minutes with no
log line, no panic message, no error dialog.

rc13 architecture:
1. **`openclaw_open_window`** (`src-tauri/src/lib.rs`) — gets the
   existing `main` window (created at app boot by tauri.conf.json),
   calls `w.navigate(chat_url)` to load the chat UI in-place. The
   `WebviewWindowBuilder` path is removed entirely. All the upstream
   layers (TCP probe, HTTP probe with 13s retry budget, /assets/ probe)
   are preserved — they catch gateway-down and gateway-mid-reload
   cases the same way they did before.
2. **`openclaw_back_to_dashboard`** (`src-tauri/src/lib.rs`) — new
   Tauri command registered in `invoke_handler`. Called by the
   chat page's "← Dashboard" overlay to navigate back to
   `tauri://localhost/index.html`.
3. **`src/openclaw-host-bridge.js`** (new file) — initialization script
   registered on the main window in `tauri.conf.json`
   (`app.windows[0].initializationScripts`). Runs at document creation
   on every page loaded in the main window. Detects
   `window.location.host === '127.0.0.1:28789'` (chat URL) and renders
   a floating "← Dashboard" pill in the top-left corner. The pill calls
   `invoke('openclaw_back_to_dashboard')` when clicked.
4. **`tauri.conf.json`** — version bumped to `1.0.9-rc13`. Main window
   config gains `initializationScripts: ["../src/openclaw-host-bridge.js"]`.

### Why this works even though Lesson 487's hang persists

The hang is in `WebviewWindowBuilder::build()` — the path that creates
a NEW webview. The existing main webview is already running successfully
(rc11/rc12 dashboard renders fine in it), so navigating it is an
in-place op that doesn't touch Tauri's webview2 init path. Whatever
combination of Tauri bugs + WebView2 quirks + multi-runtime directory
state caused the hang, none of it applies to navigating an already-
running webview.

### Lesson 491 (new — same-window chat)

When a Tauri 2 desktop app's `WebviewWindowBuilder::build()` hangs in
your environment and you can't easily diagnose which underlying
component is the culprit, navigate an existing webview instead of
creating a new one. The webview is a Chromium tab — Chromium tabs can
navigate between URLs without re-spawning the renderer process. As
long as your UX is OK with a single window transforming between
views, this sidesteps the multi-webview init path entirely.

### Lesson 488 (new — confirmed in rc12 log)

`nuke_webview2_cache_dir()` hardcodes `MiracleClaw\EBWebView` but
Tauri's WebView2 user-data-dir uses the bundle identifier, so the
actual path is `com.adealauto.miracle-claw\EBWebView`. The nuke is
a no-op every time on David's system. Lesson 470's
"cache-poisoning is the root cause" hypothesis was wrong, but the
nuke-path bug is independently a real bug worth fixing in a future
release.

### Lesson 489 (new — rc13 candidate cleanup)

David's system has three parallel WebView2 runtimes installed
(`151.0.4129.78`, `.86`, `.93`). Registry says current is `.93` but
the old ones are still on disk. Wry/Tauri may pick a stale version
when spawning `msedgewebview2.exe`. Manual cleanup:
- Run `setup.exe --force-uninstall` on each version dir under
  `C:\Program Files (x86)\Microsoft\EdgeWebView\Application\`
  (keep only `.93`)
- Reboot
- This is a one-shot fix and may also help future installs even
  after rc13 navigates around the multi-webview init path.

### Lesson 487 (new — confirmed via log trace)

`WebviewWindowBuilder::build()` hangs deterministically at the
`with_webview` callback in Tauri 2.11.5 when creating a second webview
in an environment with multiple WebView2 runtimes and an uncleaned
WebView2 user-data-dir. Last log line is always
`openclaw-chat: about to call builder.build() (Lesson 472 catch_unwind
armed)`. No panic message. Process dies after ~7 minutes (Windows
Application Hang detection). Confirmed via:
- `tasklist` — `miracle-claw.exe` PID is GONE after the hang
- `Get-CimInstance Win32_Process` — parent PID 11944 doesn't exist
- Windows Application log — only yesterday's Application Hang event
  for miracle-claw; today's silent
Application Hang detection). Confirmed via:
- `tasklist` — `miracle-claw.exe` PID is GONE after the hang
- `Get-CimInstance Win32_Process` — parent PID 11944 doesn't exist
- Windows Application log — only yesterday's Application Hang event
  for miracle-claw; today's silent

## v1.0.9-rc14 — 2026-08-20 (Lesson 491 bugfix: pill click invoke path)

rc13's same-window navigation worked, but the floating "← Dashboard"
overlay (added by `src-tauri/src/openclaw-host-bridge.js`) clicked
silently did nothing. David: "the dashboard hover window doesn't go
back to the dashboard."

Root cause: the rc13 bridge tried to invoke via
`window.__TAURI__.invoke`, but Tauri 2.x with `withGlobalTauri: true`
exposes invoke at `window.__TAURI__.core.invoke` — that's the path
`src/main.js` already uses. The bridge was using a different (wrong)
path, so the click handler called `undefined()` and nothing reached
Rust.

No log line was written because the invoke call never reached the
Rust command boundary — it died in JS as a TypeError on
`undefined.is not a function`.

Fix: bridge now prefers `tauri.core.invoke` (canonical Tauri 2.x
path), falls back to legacy `tauri.invoke` if `core` is missing.
Same pattern the dashboard uses, so they share a contract.

Verified path by inspecting `src/main.js` line 19:
```js
const { invoke } = window.__TAURI__.core;
```

Files touched:
- `src-tauri/src/openclaw-host-bridge.js` — invoke path corrected
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc14`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc14`
- `package.json` — version bumped to `1.0.9-rc14`

## v1.0.9-rc15 — 2026-08-20 (Lesson 495: multi-layered invoke + visible diagnostics)

rc14's `tauri.core.invoke` path fix (Lesson 493) didn't help: David's
pill click still doesn't reach Rust — log file shows no
`openclaw_back_to_dashboard:` entry after click, and chat UI's WS /
all API calls succeed (so IPC is fine in general). The bridge click
is dying silently in JS.

We confirmed via `strings` dump of the installed binary that:
- `window.__TAURI__.core.invoke` IS the resolved path
- `tauri.core.invoke.bind(tauri.core)` IS embedded in the bridge
- The Rust `openclaw_back_to_dashboard` command IS registered

So either:
A. `window.__TAURI__` is `undefined` on the cross-origin
   http://127.0.0.1:28789 page (high-level global API didn't
   inject). Most likely cause.
B. `__TAURI_INTERNALS__` IS present (lower-level, injected by
   `add_script_to_execute_on_document_created`) and works, but
   `__TAURI__` IIFE didn't run.
C. Click event isn't reaching the bridge (verified z-index —
   bridge pill is at 2147483647, chat UI max is 100, so no
   overlay conflict).

rc15: don't guess — make the failure visible.

1. **Multi-layered invoke fallback** (Lesson 495 fix):
   Try in order: `__TAURI__.core.invoke` →
   `__TAURI_INTERNALS__.invoke` → legacy `__TAURI__.invoke`.
   Whichever resolves first is used. If it rejects, try the next.

2. **Visible on-screen toast** (the killer diagnostic):
   WebView2 production builds don't expose DevTools, so David
   has zero visibility into bridge failures. rc15 adds a
   floating toast (top-left, z-index 2147483647) that displays
   on click:
   - Which invoke path was tried
   - Whether it succeeded or failed
   - Error message on failure
   - Auto-fades after 6s (12s for errors)

   David can now READ what's happening on click without any
   DevTools.

3. **Lazy resolution at click time** instead of at script load:
   The previous versions resolved invoke once when the bridge
   loaded. If `__TAURI__` was undefined at load but appears later
   (or after navigation), the bridge would have a stale `null`
   invoke. rc15 resolves fresh on every click.

Files touched:
- `src-tauri/src/openclaw-host-bridge.js` — multi-layered
  fallback + toast helper + lazy resolve
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc15`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc15`
- `package.json` — version bumped to `1.0.9-rc15`

## v1.0.9-rc15 — 2026-08-20 (Lesson 501: openclaw-dist patcher + chat-UI back-button fallback)

While rc15 was building (the Lesson 495 diagnostic), David asked whether
we could patch the chat UI HTML directly to add a back button. The answer
is yes, but openclaw's dist gets re-extracted from the cached npm tarball
on every `bundle-runtime.sh --force` run, so any manual edit to
`src-tauri/resources/dist/...` gets wiped. We need a build-time patcher.

Files added:
- `scripts/patch-openclaw-dist.sh` — sentinel-aware patcher. Walks
  `depot/openclaw-patches/`, applies each patch to its corresponding
  path under `src-tauri/resources/`. Supports `.html` (APPEND with
  marker), `.js`/`.css`/`.mjs`/`.cjs` (WRITE), `.json` (WRITE). Idempotent
  via content comparison + marker detection. Logs to
  `src-tauri/resources/.mc-applied-patches.log`.
- `depot/openclaw-patches/dist/control-ui/index.html` — appends a
  `<script type="module" src="./mc-back-button.js"></script>` to the
  chat UI's `index.html` (with `<!-- MC-PATCH: mc-back-button -->` marker).
- `depot/openclaw-patches/dist/control-ui/mc-back-button.js` — fallback
  chat-UI back button. Detects `window.__openclawHostBridge.hosted`,
  renders a "← Dashboard" button that navigates via
  `window.location.href = 'tauri://localhost/index.html'` (no IPC).
  Belt-and-suspenders to the bridge pill: works even if Tauri invoke
  silently fails on the cross-origin page.

Hookup needed in `build-windows-docker.sh` (NOT YET DONE): add
`bash scripts/patch-openclaw-dist.sh` after `bundle-runtime.sh` and
before the docker run. Until hookup, the chat UI button won't ship.

## v1.0.9-rc16 — 2026-08-20 (Lesson 506: Tauri ACL `remote` URL fix)

David installed rc15 and the on-screen toast told us exactly what was
wrong: `[mc-brdg] All invoke paths failed - first error: command
openclaw_back_to_dashboard not allowed by ACL`. That is **not** the
WebView2 init issue we'd been theorizing about — the invoke call IS
reaching the Rust IPC handler; the Rust ACL is rejecting it.

**Root cause** (verified in `tauri-2.11.5/src/webview/mod.rs:1819-1853`):

```rust
if (plugin_command.is_some() || has_app_acl_manifest || !is_local)
    && request.cmd != crate::ipc::channel::FETCH_CHANNEL_DATA_COMMAND
    && invoke.acl.is_none()
{
    invoke.resolver.reject(format!("Command {} not allowed by ACL", request.cmd));
    return;
}
```

The main window is loaded with `tauri://localhost/index.html`. We navigate
the main window to `http://127.0.0.1:28789/` via `openclaw_open_window`.
After the navigation, `request.url = http://127.0.0.1:28789/`, which is
NOT considered "local" by `is_local_url()` (Tauri's URL classification
only treats `tauri://`, `frontendDist` relatives, and user-registered
custom protocols as local). So `is_local = false`, and the ACL gate
fires.

`resolve_access()` returns `None` because none of our capabilities grant
access to a `Remote { url: http://127.0.0.1:28789/ }` origin. The two
existing capabilities (`main.json`, `openclaw.json`) only have
`local: true` (default) and no `remote.urls`, so they don't match the
remote origin.

**Fix**: add a new capability `bridge.json` scoped to the `main` window
with `local: false` and `remote.urls: ["http://127.0.0.1:28789/*",
"http://localhost:28789/*"]`, granting only `core:default`. The
`shell:allow-execute` permission stays locked to the `main.json`
capability (local-only), so a compromised chat UI page cannot spawn the
launcher sidecar.

This was the simplest possible fix — no code changes, no rebuild of the
bridge script, no patcher required. Pure ACL config.

**Files added**:
- `src-tauri/capabilities/bridge.json` — new capability for bridge context

**Files modified**:
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc16`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc16`
- `package.json` — version bumped to `1.0.9-rc16`

**Lesson 506 (NEW)**: When debugging "command not allowed by ACL" errors
in Tauri 2.x:

1. Determine the request URL origin. If it's a remote origin
   (`http://`, `https://`, custom port), it will be classified as
   `Origin::Remote`, NOT `Origin::Local`, regardless of the webview
   window label.
2. Check the `tauri::ipc::authority::resolve_access` logic in
   `webview/mod.rs:1819-1853`. The check fires if
   `is_local == false && invoke.acl.is_none()`.
3. Add a capability with `remote.urls` matching the origin pattern.
4. **ALWAYS** keep dangerous permissions (`shell:allow-execute`,
   filesystem write) in a local-only capability. The `remote` field
   applies to ALL permissions in the capability. Split into multiple
   capabilities for defense in depth.

## v1.0.9-rc17 — 2026-08-20 (Lesson 511: chat-UI back button as primary fix)

David reported rc16 still failed with the same ACL error. The bridge
pill couldn't recover. Per Lesson 502 plan, this release makes the
chat-UI back-button the PRIMARY navigation path back to the dashboard,
with the bridge pill as diagnostic-only.

### Changes
- `depot/openclaw-patches/dist/control-ui/index.html.insert` — renamed
  from `index.html`. Patcher now uses INSERT mode (insert before
  `</body>`) instead of APPEND (after `</html>`). Lesson 511.
- `depot/openclaw-patches/dist/control-ui/mc-back-button.js` — removed
  `__openclawHostBridge.hosted` gate. Now uses
  `window.location.host === '127.0.0.1:28789'` directly. Lesson 511.
  Renders the back button regardless of whether Tauri's bridge
  initialization_script ran on the cross-origin page.
- `scripts/patch-openclaw-dist.sh` — added `*.html.insert` mode that
  splices the patch in before `</body>` (correct HTML placement).
  Also fixed rel-path bug where `.insert` suffix was treated as part
  of the filename.

### Lesson 511 (NEW): Belt-and-suspenders patterns must NOT share dependencies

The original Lesson 502 design gated the chat-UI button on
`window.__openclawHostBridge.hosted === true`. That depends on the
bridge initialization_script running on the cross-origin page. That's
the SAME dependency that causes the bridge pill to fail. Belt-and-
suspenders with the same dependency is no belt-and-suspenders.

New design: detect the host context via `window.location.host`. That
signal is always available on the chat page, regardless of Tauri
state. The two buttons now have INDEPENDENT detection mechanisms:

- Bridge pill: `window.__TAURI__.core.invoke` (Tauri IPC)
- Chat-UI button: `window.location.href` (DOM navigation)

Either can fail independently. If Tauri IPC silently dies (rc13-rc16
scenario), the chat-UI button still works.

### Lesson 511 sub-lesson: HTML patch placement matters

Append-mode patches that add `<script>` tags land AFTER `</html>` —
invalid HTML. Browsers usually execute them anyway (lenient parsing)
but stricter parsers may refuse. New `*.html.insert` mode splices the
patch before `</body>` for correct placement.


## v1.0.9-rc18 — 2026-08-20 (Lesson 512: MAIC base URL shape mismatch — tier fix)

User reported "Free" tier shown in dashboard, but MAIC's
`/v1/auth/me` actually returns `tier: "enterprise"`. Root cause:
MC was hitting `https://maicserver.com/v1/v1/auth/me` (double-`/v1`)
which 404s on MAIC.

### Why

- `openclaw.json`'s `models.providers.maic.baseUrl` is the
  **OpenAI-completions** endpoint, which conventionally includes
  `/v1` (e.g. `https://maicserver.com/v1`).
- `mc_get_tier` calls `fetch_tier_fresh(jwt, maic_base)` which
  naively did `format!("{}/v1/auth/me", maic_base)` → produces
  `https://maicserver.com/v1/v1/auth/me` → 404 → backend returns
  Err → frontend falls back to `data-tier="free"`.

### Fix

Added `normalize_api_base()` helper in `src-tauri/src/auth/tier.rs`
that strips trailing `/v1` (and `/`) from the base URL before the
function appends `/v1/auth/me`. Applied the same fix to
`fetch_quota_fresh` in `nudge.rs` so quota usage is also correct.

### Files
- `src-tauri/src/auth/tier.rs` — added `normalize_api_base()`,
  used in `fetch_tier_fresh()`
- `src-tauri/src/auth/nudge.rs` — same fix in `fetch_quota_fresh()`
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc18`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc18`
- `package.json` — version bumped to `1.0.9-rc18`

### Lesson 512 (NEW): When a config field holds an OpenAI-style
base URL with `/v1`, and the same field is also used to build URLs
for non-OpenAI endpoints (auth, quota, etc.), strip the `/v1` before
appending the non-OpenAI path. Otherwise you get a double-`/v1` URL
that 404s on servers that don't path-alias `/v1/v1/...` to `/v1/...`.

**Symptom → cause → fix**:

| Symptom | Likely cause | Fix |
|---|---|---|
| Tier badge shows "Free" or "Unknown" | `mc_get_tier` returned Err from MAIC 404 | Check the URL the backend actually hit — if it has double-`/v1`, fix the URL builder |
| Quota usage bar shows "—" | Same: `mc_get_nudge` quota fetch 404'd | Same fix |
| Auth-related endpoints silently fail | Same double-`/v1` | Same fix |

**Verification**:

- Pre-fix URL: `https://maicserver.com/v1/v1/auth/me` → 404
- Post-fix URL: `https://maicserver.com/v1/auth/me` → 200,
  returns `{"tier":"enterprise","plan_code":"team",...}`



## v1.0.9-rc19 — 2026-08-20 (Lesson 513: batched fix — bridge pill ACL + tools injection + full model surface)

This rc bundles THREE independent fixes into one build per Lesson 513
(12 min rebuild cost = one rc per natural batch point). Each fix on its
own would have been its own rc; batched because they all needed the
same rebuild and David asked for all three in this session.

### Fix 1 — Bridge pill ACL ("Not allowed by ACL" resolved)

**Symptom**: Clicking the small back-to-dashboard pill in the top-left
of the chat UI logged `[mc-bridge] openclaw_back_to_dashboard → Not
allowed by ACL` and the click silently failed.

**Root cause**: Tauri 2 only auto-allows built-in plugin commands.
Custom `#[tauri::command]` functions declared in `generate_handler!`
need an **app-level permission manifest** in `src-tauri/permissions/`
to be invocable from a non-local origin. We had no such manifest, so
`has_app_acl_manifest` was `false` for app commands and the runtime
silently rejected the call from the chat UI.

**Fix**: Created `src-tauri/permissions/` with TOML manifests for all
13 custom Tauri commands (allow-* + deny-* variants per command, plus
a `default.toml` that bundles them all for first-time activation).

- `src-tauri/permissions/autogenerated/commands/app_commands.toml`
- `src-tauri/permissions/default.toml`
- `src-tauri/capabilities/bridge.json` — references `allow-back-to-dashboard`
- `src-tauri/capabilities/main.json` — references all 13 commands
- `src-tauri/capabilities/bridge.json` — description updated

**Verification**: Verified the build pipeline picks up the manifest
files by inspecting `target/.../acl-manifests.json`:
- Top-level plugin keys now include `__app-acl__` (was missing before).
- 13 `allow-<command>` entries present, one per custom command.
- `cargo check --bin miracle-claw-launcher` exit 0.

**Note**: The chat-UI back-button (Lesson 511) is still the
**guaranteed** navigation path — pure DOM nav, no IPC dependency.
The bridge pill is now ALSO working as a redundant convenience.

### Fix 2 — Local tools schema injection (Lesson 513)

Per Lesson 513 and David's instruction "if it's too hard to separate
from the free to the paid accounts then just add the tools to all,
and we'll lower the tokens for the free": we write all 7 local tool
schemas (`write_file`, `read_file`, `list_dir`, `bash_run`,
`remember_fact`, etc.) into `params.tools` in `openclaw.json`
unconditionally. MAIC server-side enforces Free tier blocking;
server-side token reduction for Free handles the cost.

**Files** (all in place from earlier this session — rc19 just bundles):
- `src-tauri/src/tools/schemas.rs` — `local_tool_to_openai()` +
  `local_tools_to_openai_array()` helpers
- `src-tauri/src/lib.rs` — write all 7 schemas into `params.tools`
  after the `tool_execution: "client"` setter
- `depot/maic-plugin/index.js` — no code change; plugin already
  spreads `params` via `...providerParams` so the new `tools` key
  flows through to the outbound chat-completions request body

### Fix 3 — Full Milagro model surface seeded into the dropdown

**Symptom**: Model dropdown only showed `milagro-dev` (the default).
David wanted to switch between local 7B/14B models and cloud cascades.

**Fix**: `src-tauri/src/lib.rs` (`ensure_maic_provider_config`) now
seeds 17 entries into `openclaw.json`'s `models` array on first
run AND merges any missing entries on upgrade:

| Model id | Description |
|---|---|
| `milagro-dev` | MAIC default (miracle-claw) — 14B local generalist |
| `milagro-dev-coder` | MAIC coder — 14B local code-tuned |
| `milagro-m1` | MAIC m1 — base |
| `milagro-m1-t1` | MAIC m1-t1 — 7B LoRA-distilled (fast) |
| `milagro-m1-t2` | MAIC m1-t2 — 7B LoRA-distilled (mid) |
| `milagro-m1-t3` | MAIC m1-t3 — 7B LoRA-distilled (top of t-series) |
| `milagro-chat` | MAIC chat — small general baseline |
| `milagro-coder` | MAIC coder — small-mid code baseline |
| `milagro-stock` | MAIC stock — stock-specific small |
| `milagro-oc-minimax` | Cloud cascade — MiniMax M3 (MiniMax-M3) |
| `milagro-oc-glm` | Cloud cascade — OpenChat GLM |
| `milagro-oc-qwen` | Cloud cascade — OpenChat Qwen |
| `milagro-oc-deepseek` | Cloud cascade — OpenChat DeepSeek |
| `milagro-oc-kimi` | Cloud cascade — OpenChat Kimi |
| `chat-glm` | Cloud — GLM (direct) |
| `chat-deepseek` | Cloud — DeepSeek (direct) |
| `chat-qwen` | Cloud — Qwen (direct) |

User upgrades (e.g. rc18 → rc19) get missing entries merged in,
existing entries preserved (David's custom renames survive).

### Lesson 513 (NEW): Batch Tauri fixes per build, not per rc

**Why**: Each Tauri rebuild costs ~12 minutes (Docker cross-compile +
NSIS bundling). When 2+ independent fixes are queued in the same
session, ship them in ONE build instead of one-per-fix. rc19 is the
canonical example — bridge pill + tools + models = 1 build instead of 3.

**When to apply**: any time you have ≥2 independent fixes queued AND
they don't depend on each other for verification, batch them. Don't
batch if a fix would invalidate another fix's test plan.

### Lesson 514 (NEW): App-level Tauri commands need a permission manifest

**Symptom**: Custom `#[tauri::command]` function registered in
`generate_handler!` works fine from local origin (`tauri://localhost`)
but silently fails from remote origin (`http://127.0.0.1:28789`)
with "Not allowed by ACL" — even when the capability's `remote.urls`
correctly matches the request origin.

**Root cause**: Tauri 2's runtime enforces ACL on:
- All plugin commands (auto-resolved against built-in plugin perms)
- ALL custom commands **once `has_app_acl_manifest` is true**

The first time you add a `permissions/` directory, `has_app_acl_manifest`
flips from `false` to `true` for the whole app. Until that flip, app
commands are unrestricted (which is fine for local but wrong for
remote). After the flip, every custom command needs an explicit
`allow-*` permission registered.

**Fix**: Create `src-tauri/permissions/` with TOML files declaring
each custom command. Reference the `allow-*` identifiers in your
`capabilities/<file>.json` `permissions` arrays.

**Symptom → cause → fix**:

| Symptom | Likely cause | Fix |
|---|---|---|
| Custom command works from dashboard, fails from remote URL with "Not allowed by ACL" | No `permissions/` manifest, OR `remote` capability doesn't list the `allow-<command>` identifier | Create `src-tauri/permissions/autogenerated/commands/<name>.toml` with `[[permission]] identifier = "allow-<name>" commands.allow = ["<fn_name>"]`, reference in capability |
| Capability build error: `invalid plugin or permission identifier '<fn>:<perm>'` | Tried to use `<fn>:<perm>` form for an app-level command | Use bare `allow-<fn>` identifier instead — colons only separate plugin name from permission name |
| ACL enforcement suddenly started AFTER adding an unrelated fix | The unrelated fix added `permissions/`, which flipped `has_app_acl_manifest` to true | Update existing capabilities to include all `allow-*` identifiers for commands they invoke |

### Files
- `src-tauri/capabilities/bridge.json` — references `allow-back-to-dashboard`
- `src-tauri/capabilities/main.json` — references all 13 custom commands
- `src-tauri/permissions/default.toml` — default permission set
- `src-tauri/permissions/autogenerated/commands/app_commands.toml` — 13 commands
- `src-tauri/src/lib.rs` — model seeding (17 entries + upgrade merge)
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc19`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc19`
- `package.json` — version bumped to `1.0.9-rc19`

### Verification plan
1. Install rc19 → log in → chat sends/receives (rc18 regression check)
2. Click bridge pill → should navigate back to dashboard WITHOUT
   ACL error. Check `miracle-claw.log` for absence of
   `[mc-bridge] openclaw_back_to_dashboard → Not allowed by ACL`.
3. Open model dropdown → should show all 17 model entries.
4. Try a chat with `read_file`/`bash_run`/`write_file` prompt →
   model should emit `tool_calls` (check the request body to MAIC
   includes `tools: [...]` array with all 7 schemas).
5. Tier badge should still show "Enterprise" (rc18 fix unchanged).

## v1.0.9-rc20 — 2026-08-20 (Lesson 517: tier-conditional default model + Lesson 516 sandbox awareness)

This rc bundles TWO independent fixes into one build per Lesson 513
(12 min rebuild cost = one rc per natural batch point).

### Fix 1 — Lesson 517: tier-conditional default model + fallbacks

**Symptom**: After login, MC's chat defaulted to `milagro-dev` regardless
of the user's MAIC tier. Paid accounts (Pro/ProPlus/Team/Enterprise)
were missing out on the cloud cascade chain (Kimi → MiniMax-M3 → GLM
→ local) that MAIC provides.

**Decision** (David 2026-08-20 16:59 MDT): "paid accounts automatically
set up with Kimi, fallback Minimax-m3 fallback glm etc, etc." Free
users stay on local 14B (no cloud quota to burn).

**Fix** (`src-tauri/src/auth/tier.rs` + `src-tauri/src/lib.rs`):

| Tier | Primary | Fallbacks |
|---|---|---|
| Free | `milagro-dev` (local 14B) | *(none)* |
| Pro / ProPlus / Team / Enterprise | `milagro-oc-kimi` (cloud) | `milagro-oc-minimax` → `milagro-oc-glm` → `milagro-dev` |

- `tier_default_model_id(tier)` and `tier_default_fallbacks(tier)` —
  pure helpers, pinned by 4 unit tests.
- `ensure_agents_default_model_for_tier(tier)` — non-destructive writer
  that walks `agents.defaults.model.{primary, fallbacks}` in
  `openclaw.json` and stamps the tier defaults ONLY when no user choice
  is present (empty/missing primary triggers; non-empty primary is left
  alone so logins don't clobber manual picks).
- Wired into `maic_login`, `silent_relogin`, `mc_refresh_tier`, and
  `mc_apply_tier_change` — every place we learn a tier now writes the
  routing config atomically (temp file + rename).
- `mc_set_tier_defaults(force?: bool)` — frontend-callable migration
  helper. `force=true` blanks an existing manual primary before
  writing the tier default (used for Free→Pro upgrades on existing
  installs that have already picked `milagro-dev`).
- Atomic write: temp file + rename so a crash mid-write doesn't leave
  the user with a half-written `openclaw.json`.

**Tests added** (6 new, all passing): `lesson_517_free_writes_local_default_when_empty`,
`lesson_517_pro_writes_kimi_with_fallbacks`, `lesson_517_does_not_overwrite_user_choice`,
`lesson_517_writes_when_existing_primary_is_empty_string`,
`lesson_517_pro_plus_team_enterprise_share_routing`,
`lesson_517_free_clears_stale_paid_fallbacks_on_downgrade`. Plus 4
helpers in `tier.rs` (`free_default_is_local_14b`, `paid_default_is_kimi`,
`paid_fallbacks_are_ordered_minimax_then_glm_then_local`,
`fallback_chain_distinct_from_primary`). Full suite: 60 passed, 0 failed.

### Fix 2 — Lesson 516: MAIC plugin sandbox-awareness system prompt

**Symptom**: When MC's OpenClaw agent (m1-t1, 7B ternary) tried to use
`bash_run` or `write_file`, it hallucinated paths under
`~/.openclaw/workspace/` (the OpenClaw **native** workspace convention)
instead of MC's actual sandbox: `%LOCALAPPDATA%\miracle-claw\workspace\`
on Windows or `~/.local/share/miracle-claw/workspace/` on Linux. The
model produced plausible-looking paths that resolved to non-existent
directories, and the user saw `ENOENT` errors.

**Root cause**: The MAIC plugin added tool schemas (Lesson 507) but
didn't tell the model **where** MC's sandbox is. The model fell back
to "what I might know about openclaw's defaults" → wrong answer.

**Fix** (`depot/maic-plugin/index.js` v0.2.0): add a
`before_prompt_build` hook that injects a system-context block
describing the sandbox:

- Concrete allowed paths (Documents/Desktop/Downloads + workspace)
  with the user's actual paths resolved at hook time.
- Disallowed paths (AppData, ProgramData, other drives, WSL native,
  relative paths).
- Path style reminders (Windows backslash, `~/mnt/c/...` auto-convert,
  `~` not expanded by bash).
- Quirks (bash_run default CWD is the workspace, not the user's cwd).

**Implementation notes** (lessons baked in):
- Top-level `import os from "node:os"` (NOT lazy `require` — the
  lazy `try { require() } catch {}` silently swallowed the ESM
  context error and returned empty paths; the system prompt ended
  up 1289 chars instead of 1444 and was missing all desktop paths).
- Uses `os.homedir()` + `os.platform()` instead of the
  `PluginHookAgentContext.workspaceDir` field, because that field
  is only populated in the per-turn context (not the system-context
  hook), and we want the same prompt regardless of OS / install state.
- Returns `{ prependSystemContext: <block> }` — provider-cached, not
  per-turn, so token cost is amortized across the whole conversation.

**Tests added** (10 new in `depot/maic-plugin/test_plugin.js`,
all passing): desktop paths, downloads path, Windows-style
backslashes, non-Windows paths, sandbox awareness, hook installation,
prepend vs append, no-regression on extraParamsForTransport, idempotent
`onBeforePromptBuild` re-binding, ESM context (`require` failure
fallback path).

### Files
- `src-tauri/src/auth/tier.rs` — `tier_default_model_id` +
  `tier_default_fallbacks` + 4 unit tests
- `src-tauri/src/lib.rs` — `ensure_agents_default_model_for_tier` +
  `mc_set_tier_defaults` + 6 unit tests + wiring into 4 callers
- `depot/maic-plugin/index.js` — v0.2.0 (Lesson 516 prefix + system
  prompt builder)
- `depot/maic-plugin/openclaw.plugin.json` — version bumped to 0.2.0
- `depot/maic-plugin/package.json` — version bumped to 0.2.0
- `depot/maic-plugin/test_plugin.js` — 21 tests (11 smoke + 10 Lesson 516)
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc20`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc20`
- `package.json` — version bumped to `1.0.9-rc20`

### Verification plan
1. Install rc20 → log in with pro@adealauto.com → model dropdown
   should show Kimi as primary, fallbacks listed below.
2. Log in with free@adealauto.com → primary = `milagro-dev`,
   no fallbacks.
3. Send a message like "list files in C:\Users\Adeal\Documents" — the
   agent should use the right path (no `~/.openclaw/workspace`
   hallucination).
4. Try `bash_run` with no `cwd` arg — the agent should warn about the
   default CWD being the workspace.
5. Verify the system prompt includes the sandbox-awareness block by
   tailing `miracle-claw.log` for `prependSystemContext` length = 1444.
6. `cargo test --lib` → 60 passed, 0 failed (regression check).

## v1.0.9-rc21 — 2026-08-20 (Lesson 520 + 521: fix gateway provider-prefix fallback on rc18→rc20 upgrades)

### Symptom (David, 2026-08-20 18:23 MDT)

Chat panel shows:
```
⚠️ Agent failed before reply: All models failed (4):
  openai/milagro-oc-kimi: Unknown model: openai/milagro-oc-kimi (model_not_found)
  openai/milagro-oc-minimax: Unknown model: openai/milagro-oc-minimax (model_not_found)
  openai/milagro-oc-glm: Unknown model: openai/milagro-oc-glm (model_not_found)
  openai/milagro-dev: Unknown model: openai/milagro-dev (model_not_found)
```

Gateway log shows the smoking gun:
```
[model-selection] Model "milagro-oc-kimi" specified without provider.
  Falling back to "openai/milagro-oc-kimi".
  Please use "openai/milagro-oc-kimi" in your config.
```

### Root cause (Lesson 520)

Two coupled bugs:

1. **`models.providers.maic.models[]` was incomplete on rc18 → rc19/20
   upgrades.** `ensure_maic_provider_config()` has two write paths:
   - "new entry" (no apiKey/baseUrl yet) — runs the full merge logic
     including the known_ids append at line 745.
   - "existing entry" (literal apiKey + baseUrl present) — early-returns
     at line 577 BEFORE reaching the merge.

   Users who installed rc18 (which only seeded `milagro-dev`) and then
   upgraded to rc19/rc20 hit the existing-entry path on every launch —
   so the catalog never grew beyond the original 1 entry.

2. **`agents.defaults.model.primary` was emitted as a bare id** (e.g.
   `"milagro-oc-kimi"`) instead of `provider/id` form. The openclaw
   gateway's `resolveBareModelDefaultProvider` calls
   `inferUniqueProviderFromCatalog`, which scans
   `models.providers[*].models[]`. If the lookup fails (because of bug
   #1 above), it falls through to `defaultProvider = "openai"` and
   rewrites the request as `openai/<id>` — which MAIC upstream rejects.

### Fix (Lesson 520 + 521, batched into rc21 per Lesson 513)

- **Lesson 520**: extracted the known_ids merge into a helper
  `merge_known_model_ids_into_provider(cfg, provider_id)`. Called from
  BOTH write paths. On a complete existing entry, the merge runs, then
  we persist the file with the new 16 entries appended (idempotent —
  never duplicates, never overwrites user renames).
- **Lesson 521**: defensive — `ensure_agents_default_model_for_tier()`
  now emits `maic/<id>` instead of bare `<id>`. Belt-and-suspenders so
  even if the catalog merge regresses, the explicit provider prefix
  forces correct dispatch.

### Tests added (60 → 62 passing)

- `lesson_520_known_ids_merge_into_existing_entry` — proves an rc18-
  style state (only `milagro-dev` in catalog) gets all 17 ids merged
  in on the next `ensure_maic_provider_config()` run.
- `lesson_520_known_ids_merge_is_idempotent` — re-running the
  bootstrap doesn't grow the file or duplicate entries.
- 5 existing Lesson 517 tests updated for the `maic/` prefix on
  primary + fallbacks.

### Files changed

- `src-tauri/src/lib.rs`:
  - New helper `merge_known_model_ids_into_provider()` (~30 lines)
  - Existing-entry early-return path now calls the merge + persists
  - `ensure_agents_default_model_for_tier()` emits `maic/<id>` (Lesson 521)
  - 5 Lesson 517 tests updated, 2 Lesson 520 tests added
- `src-tauri/Cargo.toml` — version bumped to `1.0.9-rc21`
- `src-tauri/tauri.conf.json` — version bumped to `1.0.9-rc21`
- `package.json` — version bumped to `1.0.9-rc21`

### Verification plan

1. Install rc21 → openclaw.json should automatically grow to 17 model
   entries on first launch (existing-entry path).
2. Log in with pro@adealauto.com → `agents.defaults.model.primary`
   should be `"maic/milagro-oc-kimi"`, fallbacks prefixed with `maic/`.
3. Send a chat message → should roundtrip to MAIC + Kimi without
   `Unknown model` errors.
4. `cargo test --lib` → 62 passed, 0 failed.

---

## v1.0.9-rc22 — 2026-08-20 (Lesson 523: tier-gated tool injection)

### Symptom

Bot (paid user) reported only 5 tools: the MAIC server-side set
(`get_weather`, `web_search`, `get_current_time`, `calculate`,
`describe_image`). No local tools (`read_file`, `write_file`, `edit_file`,
`list_dir`, `bash_run`, `apply_patch`, `remember_fact`) advertised.
Same symptom possible for Free users (we wrote all 7 schemas into
`params.tools` for everyone, hoping MAIC's server-side enforcement would
block Free tool_calls, but that's a fragile hand-off and the wrong fix).

### Root cause

`ensure_maic_provider_config()` wrote `params.tools` unconditionally —
all 7 LocalTool schemas for every tier, including Free. Two problems:

1. **Wrong shape for Free**: Free users got `tools: [7 entries]` on disk,
   which leaked tool availability info even if MAIC rejected the calls.
2. **No fresh re-stamp on tier change**: when a user upgraded
   Free → Pro mid-session, `params.tools` stayed as it was (or empty,
   depending on order of operations) until a fresh login re-stamped it.
   `mc_refresh_tier` and `mc_apply_tier_change` didn't touch the
   provider entry.

### Fix (Lesson 523, batched per Lesson 513)

1. **New `ensure_maic_provider_config_for_tier(tier)` function**:
   filters the 7-tool static slice through `tools_for_tier(tier)` →
   empty array for Free, all 7 for paid tiers. Existing
   `ensure_maic_provider_config()` (no tier) becomes a thin wrapper that
   defaults to `Tier::Free` (safe per Lesson 176 — "anything outside the
   canonical tier set is treated as free").

2. **`maic_login`** (where tier is known post-/v1/auth/me) now calls
   `ensure_maic_provider_config_for_tier(parsed_tier)` instead of the
   no-tier variant.

3. **`silent_relogin`** (auto-relogin on 401) does the same — critical
   for users whose first login was on Free and whose MAIC plan was
   upgraded later.

4. **`mc_refresh_tier`** and **`mc_apply_tier_change`** now also call
   `ensure_maic_provider_config_for_tier(tier)` after they re-route the
   default model. Clicking the tier badge or applying a tier change
   re-stamps `params.tools` immediately.

5. **`setup()` early-init and login-required bootstraps** keep the
   no-tier variant → defaults to Free → empty tools array. The user
   only gets tool access once they log in.

### Idempotency

The `if !params.contains_key("tools")` guard is preserved. Downgrades
don't strip user-edited tools; user edits aren't re-stamped on
subsequent logins. Verified by `tools_array_not_overwritten_on_subsequent_calls`.

### Tests added (62 → 66 passing)

- `no_tier_default_writes_empty_tools_array_for_free` — default
  bootstrap writes 0 tools
- `free_tier_explicit_writes_empty_tools_array` — Free tier writes 0
  tools
- `paid_tiers_write_all_seven_tools` — Pro/ProPlus/Team/Enterprise all
  write 7 tools, wire format verified
- `tools_array_not_overwritten_on_subsequent_calls` — Lesson 449
  idempotency pattern applied to `params.tools`

### Files changed

- `src-tauri/src/lib.rs` — new `ensure_maic_provider_config_for_tier`
  function, `params.tools` block reads tier via parameter, login
  flow + tier-refresh/apply call sites pass tier
- `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json` —
  version bumps 1.0.9-rc21 → 1.0.9-rc22
- (no plugin change — `depot/maic-plugin/index.js` already spreads
  `params.tools` via `...providerParams`)

### Verification plan

1. Install rc22 over rc21 → no openclaw.json shape change on upgrade
   (existing `params.tools` value preserved if present).
2. Log in with `pro@adealauto.com` → `params.tools` should now contain
   exactly 7 entries (function objects for read_file, write_file,
   edit_file, list_dir, bash_run, apply_patch, remember_fact).
3. Log in with a Free account → `params.tools` should be `[]` (empty
   array).
4. Open MC's bundled OpenClaw window → bot in chat should now see the
   full 11-tool set (4 MAIC server + 7 MC local) and call local tools
   for file/bash tasks.
5. `cargo test --lib` → 66 passed, 0 failed.

---

## v1.0.9-rc23 — 2026-08-20 (Lesson 524: existing-entry path was skipping tool injection)

### Symptom (David, 2026-08-20 22:50 MDT)

Installed rc22 over rc21, logged in as `pro@adealauto.com`, asked the bot
to read a file. Got:
```
Error: tool 'read' is not available.
Available tools: get_weather, web_search, get_current_time, calculate, describe_image.
```
Bot only saw 5 tools — the 4 MAIC server tools + 1 dashboard image tool.
No `read_file` / `write_file` / `bash_run` / etc.

### Root cause

`ensure_maic_provider_config_for_tier()` (Lesson 523) wrote
`params.tool_execution` and `params.tools` in the **new-entry write
path** only. The **existing-entry early-return path** (line ~660) — hit
by every login after first install, when the maic entry already has
`apiKey + baseUrl` — returned BEFORE the Lesson 523 block. So
`params.tools` was never written for users on rc18+ upgrades.

Verified live: `%APPDATA%\MiracleClaw\openclaw.json` (mtime 22:41
MDT, after the 22:41 MDT login) had `params.tool_execution: "client"`
but **no `params.tools` key at all**. Only Lesson 513 (tool_execution)
had landed there. Lesson 523 (tier-gated tools array) had been silently
skipped on every login since rc18.

### Fix (Lesson 524)

Extracted `write_tier_gated_tool_execution_and_tools(&mut cfg, tier)`
helper inside `ensure_maic_provider_config_for_tier()`. Helper logic:
- If `models.providers.maic` entry exists, ensure `params.tool_execution = "client"` (idempotent) and `params.tools = [...]` (Lesson 523 tier-gated, idempotent).
- Called from BOTH the existing-entry early-return path AND continues to work in the new-entry write path.

The write path still has its inline `entry_obj`-based logic (because
that's where the entry is being constructed before being merged into
`cfg`), but the early-return path now invokes the helper to mutate
`cfg` directly before the persistence write.

### Tests added (66 → 67 passing)

- `existing_entry_path_also_stamps_tools_array` — pre-populate disk
  with a complete rc18-era maic entry (apiKey + baseUrl + 1 model),
  call `ensure_maic_provider_config_for_tier(Pro)`, verify the
  helper stamped `params.tools` with 7 entries AND preserved the
  original `apiKey` (Lesson 449 idempotency on apiKey preserved).

### Files changed

- `src-tauri/src/lib.rs` — new `write_tier_gated_tool_execution_and_tools`
  helper, called from the existing-entry early-return path
- `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json` —
  version bumps 1.0.9-rc22 → 1.0.9-rc23

### Verification plan

1. Install rc23 over rc22 → on next login, `params.tools` should be
   written (7 entries for paid, `[]` for Free)
2. Verify with:
   ```bash
   jq '.models.providers.maic.params | {tool_execution, tools_count: (.tools | length // 0)}' \
     "$APPDATA/MiracleClaw/openclaw.json"
   # Expected: tool_execution="client", tools_count=7 (paid) or 0 (free)
   ```
3. Open bundled OpenClaw window → bot should now see all 11 tools
   (4 server + 7 local) and successfully call `read_file`/`bash_run`
4. `cargo test --lib` → 67 passed, 0 failed ✅ (verified)

---

## v1.0.9-rc24 — 2026-08-21 (Lesson 525: empty `params.tools: []` was treated as "already populated")

### Symptom (David, 2026-08-21 13:44 MDT)

After installing rc23 over rc22 and logging in as Enterprise tier, the
bot STILL had no local tools. Checking the on-disk config:

```json
{
  "apiKey": "eyJhbGc...",
  "baseUrl": "https://maicserver.com/v1",
  "models": [...17 models...],
  "params": {
    "tool_execution": "client",
    "tools": []              ← BUG: empty array, not 7 entries
  }
}
```

The `tool_execution: "client"` was stamped correctly (Lesson 524), but
the `tools` array was empty even though David is Enterprise (paid tier).

### Root cause

Lesson 524 added `write_tier_gated_tool_execution_and_tools()` to the
existing-entry early-return path. The helper's idempotency guard was:

```rust
if !params.contains_key("tools") {  // stamp tools only if KEY missing
    let tool_names = tools_for_tier(tier);
    ...
}
```

But users who went through the rc17 → rc22 window (when Lesson 523
wasn't yet wired into the existing-entry path) had `params.tools: []`
written by a different code path. The empty array **is** a valid
JSON value, so `contains_key("tools")` returns `true` — and the
helper skipped stamping, leaving the empty array in place.

### Fix

Treat empty array the same as missing key in BOTH the helper (existing-
entry path) AND the inline write path. Non-empty user-customized
schemas (someone manually edited `params.tools` to add/remove tools)
are still preserved.

```rust
let needs_tools_stamp = match params.get("tools") {
    None => true,
    Some(Value::Array(a)) => a.is_empty(),  // ← NEW: re-stamp on empty
    Some(_) => false,                        // preserve non-empty arrays
};
if needs_tools_stamp {
    let tool_names = tools_for_tier(tier);
    ...
}
```

### Files changed

- `src-tauri/src/lib.rs` — both `write_tier_gated_tool_execution_and_tools`
  helper (existing-entry path) AND inline write path (around line 1029)
  now use the `needs_tools_stamp` check instead of `!contains_key`.
- `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json` —
  version bumps 1.0.9-rc23 → 1.0.9-rc24
- `src-tauri/resources/BUNDLE_VERSION` — bumped post-build

### Verification plan

1. Install rc24 over rc23 → on next login, `params.tools` should be
   re-stamped to 7 entries (paid) or remain `[]` (Free).
2. Verify with PowerShell:
   ```powershell
   $cfg = Get-Content "$env:APPDATA\MiracleClaw\openclaw.json" -Raw | ConvertFrom-Json
   Write-Host "tools count: $($cfg.models.providers.maic.params.tools.Count)"
   # Expected for paid tier: 7
   ```
3. Open bundled OpenClaw window → bot should now see all 11 tools
   (4 server + 7 local) and successfully call `read_file`/`bash_run`
4. Re-test the exact symptom case from this lesson: free-tier user
   keeps `[]`, paid-tier user gets 7 tools — no customer can end up
   stuck with `[]` after the fix lands.

### Anti-pattern to remember

Idempotency checks on user-editable JSON arrays should distinguish
"missing key", "empty array", and "non-empty array". A simple
`contains_key` guard treats empty array as "user wants this" — but
for our `params.tools` field, an empty array is never the user's
intent (Free-tier users get `[]` from the bootstrap itself, not by
editing; everyone else wants their tier-appropriate tools).

---

## v1.0.9-rc24 — Lesson 526 supplement (2026-08-21 13:55 MDT)

### Decision (David, 2026-08-21 13:54 MDT)

> "Break that test and remove the do not add tools. We need to get
> this fixed, even if the Free accounts have tools access, we can
> simply drop how many tokens they can use. Bottom line we need all
> of these tools to work."

**All tiers get the 7 local tools.** Free users still pay for it
via the per-tier TPM ceiling (50K), which is the real cost-control
mechanism. Tool gating was a UX bug disguised as a cost-control
mechanism — it blocked critical onboarding flows (file inspection,
project bootstrapping) for users who couldn't see the pricing page
yet.

### Changes

- `src-tauri/src/auth/tier.rs::has_local_tools()` — always returns
  `true`. Removed the `!matches!(self, Tier::Free)` guard.
- `src-tauri/src/lib.rs::tools_for_tier(_tier)` — returns
  `ALL_LOCAL_TOOL_NAMES.to_vec()` unconditionally. Tier parameter
  kept for signature stability (forward-compat if we later add
  tier-specific tools).
- `src-tauri/src/lib.rs::tools_for_tier_free_returns_empty` — **DELETED**.
  Replaced by `tools_for_tier_returns_all_seven_for_every_tier`
  which asserts 7 tools for Free, Pro, ProPlus, Team, Enterprise.
- `src-tauri/src/lib.rs::tools_array_not_overwritten_on_subsequent_calls`
  — repurposed as `tools_array_preserves_user_customizations`. The
  old test asserted Free kept `[]`; that was the gating behavior we
  just removed. New test asserts user-edited non-empty arrays are
  preserved across logins.
- `src-tauri/src/auth/tier.rs::has_local_tools_only_for_paid` —
  renamed to `has_local_tools_for_all_tiers`, all tiers now return
  true.

### Why not keep the gating?

Three reasons:
1. **Onboarding dead-end**: Free users hit "I don't have file tools"
   *before* they see the pricing page. They can't bootstrap a project
   to even evaluate whether MC is worth paying for.
2. **Wrong control plane**: TPM already throttles. A Free user with
   all 7 tools but a 50K TPM ceiling can't actually do harm — they
   run out of tokens before they touch anything dangerous.
3. **Tier discovery is in-app**: We don't have an in-app upgrade flow
   yet (Lesson 525 → /upgrade page still TODO). Until users can
   self-upgrade, gating tools blocks the entire bottom-of-funnel
   experiment.

If/when we want to add a "premium-only" tool (e.g. `deploy_k8s`),
that's a single name in `tools_for_tier()` — gating moves from
"tier has tools" (yes/no) to "tool is in tier" (per-tool check).

### Verification

1. Install rc24 over rc23 → on next login (any tier), `params.tools`
   should have 7 entries.
2. Free user specifically: log in to a fresh Free account → tools
   array should still have 7 entries (was 0 before Lesson 526).
3. `cargo test --lib tools` → all green.
4. `cargo test --lib has_local_tools` → all green.
5. `cargo test --lib` → 68+ passed, 0 failed.

### Anti-pattern to remember

"Don't gate capabilities for cost control when the capability is
the entire value prop." Users who can't read files, write files,
or run shell commands aren't customers with limited tools — they're
non-customers who bounce. TPM/RPM gates the usage; capability
gating just hides the product behind a paywall that can't be seen.

---

## v1.0.9-rc24 — Lesson 527 supplement (2026-08-21 13:57 MDT)

### Decision (David, 2026-08-21 13:56 MDT)

> "And actually 1 step further. We only give the Free accounts access
> to M1 T1 and M1 T2 that will pretty much just limit them to Chat
> anyway."

Lesson 526 was the tool gating. Lesson 527 is the **model gating**.
Free tier sees only the two smallest distilled m1 models
(`milagro-m1-t1`, `milagro-m1-t2`) — 7B ternary, chat-only fast tier.
Paid tiers see the full 17-model catalog.

### Why this is the right control plane

TPM caps the **volume** of token spend. Model gating caps the
**per-message** cost. A Free user with access to `milagro-dev` (14B)
could burn through their entire 50K TPM ceiling in 3 messages by
asking the 14B model for verbose answers. With `m1-t1` only, each
message is cheap, so the user can have 50+ messages before TPM runs
out — which is what "Free tier" should feel like.

The user CAN still chat (which is what David wants Free users to do).
They just can't accidentally pick `milagro-dev` and have a $0.05
chat session.

### Changes

- `src-tauri/src/lib.rs::merge_known_model_ids_into_provider` — now
  takes a `tier: Tier` parameter. Free → `FREE_MODEL_IDS` (m1-t1,
  m1-t2). Paid → `ALL_MODEL_IDS` (17). Additionally **removes**
  models from the on-disk list that the current tier doesn't allow
  (handles upgrade and downgrade paths).
- `src-tauri/src/lib.rs::ensure_maic_provider_config_for_tier` —
  write path now tier-gates both the `seeds` array (first install)
  AND the `known_ids` array (upgrade merge).
- New tests added (4): `free_tier_sees_only_two_models`,
  `paid_tiers_see_all_seventeen_models`,
  `downgrade_from_pro_to_free_removes_paid_models`,
  `upgrade_from_free_to_pro_adds_paid_models`.
- `lesson_520_known_ids_merge_is_idempotent` updated — Free
  default is now 2 models, not 17.
- `env_var_key_writes_literal_string` updated — default model
  for Free tier is now `milagro-m1-t1`, not `milagro-dev`.

### Verification

1. Install rc24 over rc23 → on next login:
   - Free users: model picker shows ONLY m1-t1 + m1-t2
   - Pro users: model picker shows all 17 models (unchanged)
2. Fresh install (Free): openclaw.json has 2 entries in
   `models.providers.maic.models`.
3. Pro user downgrades to Free via `/v1/billing/portal-session`:
   next MC login removes 15 paid models from their config.
4. `cargo test --lib` → 71 passed, 0 failed.

### Why not just rely on MAIC server-side filtering?

We could ask MAIC to 403 a Free user requesting `milagro-dev`.
But that:
- Hides the upgrade trigger (user sees a 403, doesn't realize
  there's an /upgrade page)
- Costs us a round-trip on every chat for the model's name
- Doesn't help UX — user picks from a dropdown, not from a
  raw API call

Client-side filtering is the right seam: user only sees models
they can use, and the dropdown itself becomes the upgrade CTA
("Want more models? Upgrade →").

### Anti-pattern to remember

"Cost control belongs in the model picker, not in the API." If
a user can pick a model they can't afford, you have a UX bug.
The picker is where pricing meets product. Filter there.
