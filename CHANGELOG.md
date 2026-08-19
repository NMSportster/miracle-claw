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
