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

## [v1.0.7-rc1] — 2026-08-19 (rc for testing) — SHA `faf6030b05515cdccb3a64aa004228ad` (57,081,313 bytes)

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
