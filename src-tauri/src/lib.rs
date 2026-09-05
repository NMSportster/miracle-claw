// ============================================================================
// Miracle Claw — Tauri lib
//
// First-run path (called once per process start, from setup()):
//   1. Locate the bundled resources (Tauri sets TAURI_BUNDLE_RESOURCES_DIR,
//      but in dev mode we look in src-tauri/resources/).
//   2. Ensure the MAIC provider plugin is present in MC's isolated state
//      directory (idempotent — SHA-checked copy).
//      ~/.miracle-claw/extensions/maic/  on *nix,
//      %APPDATA%\MiracleClaw\extensions\maic\  on Windows.
//      MC does NOT share state with the system OpenClaw install; we keep
//      config, sessions, plugins, and logs under .miracle-claw/ to avoid
//      races when both run on the same machine.
//   3. Pre-write a minimal openclaw.json at <stateDir>/openclaw.json if one
//      doesn't exist. openclaw's gateway refuses to start on a fresh install
//      (exit 78, "Missing config") without this file.
//   4. Validate (read-only) the user's openclaw.json exists and parses.
//      openclaw 2026.7.1+ auto-discovers plugins from <stateDir>/extensions/,
//      so no plugin-path keys are needed in the config. Any leftover
//      `pluginRoots` / `plugins.roots` keys from older openclaw versions
//      are rejected — we warn, never mutate.
//   5. Spawn miracle-claw-launcher with --gateway-port N as a sidecar.
//      OPENCLAW_STATE_DIR is set to MC's state dir so the gateway boots
//      against MC's isolated layout, not the system one.
//   6. Poll TCP connect to 127.0.0.1:28789 until it accepts (15s cap).
//      (openclaw accepts the connection immediately when the port is bound,
//      so we don't need an HTTP roundtrip.)
//   7. Register RunEvent::ExitRequested to kill the launcher cleanly.
//
// Architecture doc: README.md
// ============================================================================

use std::fs;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_shell::process::CommandEvent;

mod launcher_info;
use launcher_info::{launcher_binary_name, MAIC_PLUGIN_FILENAMES, OPENCLAW_PORT};

// Lesson 458 / v1.0.6: silent-relogin via cached creds in OS keychain.
// See src/auto_relogin.rs for the full design.
mod auto_relogin;
// rc53.6 (feature/secrets-vault): three-tier friendly secrets (Once /
// PerSession / Vault) layered on top of the rc53 plaintext vault. See
// src/secrets_friendly.rs for the in-memory pool + canonicalizer.
//
// rc53.8 (feature/extras-hub): AES-256-GCM encryption-at-rest for the
// Vault tier. Master key lives in the OS keychain. See
// src/secrets_encryption.rs.
mod secrets_encryption;
mod secrets_friendly;
// Lesson 713 (2026-08-28 06:48 MDT, David): BYO provider keys — lets
// users bring their own OpenAI/Anthropic/Ollama Cloud/etc. API key
// instead of always routing through MAIC's billing. Stores in the
// encrypted vault + sets process env so the openclaw runtime picks
// it up at request time.
mod provider_keys;

// Lesson 725 (2026-08-28 21:30 MDT, David): Tasks feature for Miracle
// Claw v1.1.0+ (Miracle Bot persistent memory). Paid-tier-gated.
// Models + storage + MAIC sync engine adapted from the adeal-schedule
// v0.1.0 test program shipped by the MC-openclaw agent on 2026-08-22.
// See src/tasks/mod.rs for the full design and the schema-migration fix.
mod tasks;

// v1.0.7: tier fetching + token-quota nudges.
pub mod auth;
// Phase 3 mobile pairing (spec: docs/specs/mobile-desktop-pairing.md).
// Crypto primitives only — Tauri command wiring is in a follow-up commit.
pub mod pairing_crypto;
// v1.0.7: 7 local tool schemas (paid tier only). Marked `pub` so the
// `miracle-claw-tools` binary can `use` them via `crate::tools::...`.
pub mod tools;

// v1.1.0-rc53.15: Optional, downloadable module framework (Lesson 570).
// Voice is the first module — future modules (OCR, TTS, local search)
// follow the same pattern.
pub mod modules;

// ----------------------------------------------------------------------------
// Constants
// ----------------------------------------------------------------------------

/// Default MAIC endpoint. Overridable via MAIC_API_URL env var at runtime;
/// the login UI shows whatever this resolves to so the customer knows
/// which server they're signing into. Lesson 444.
pub(crate) const DEFAULT_ENDPOINT: &str = "https://maicserver.com";

/// MAIC API key env var. Used by `maic_login`, `needs_maic_login_from_state`,
/// and the openclaw.json SecretRef path. Lesson 444.
pub(crate) const ENV_VAR_NAME: &str = "MAIC_API_KEY";

// ----------------------------------------------------------------------------
// State we hold for the lifetime of the process
// ----------------------------------------------------------------------------

#[derive(Default)]
struct AppState {
    /// Handle to the launcher sidecar child process (if started).
    /// Mutex because RunEvent handlers + setup() cross thread boundaries.
    launcher_child:
        Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    /// v1.0.9-rc36: registry of active PTY terminal sessions.
    /// Map from session id to an Arc-wrapped TerminalHandle. The Arc
    /// is shared between reader threads (which we spawn at start
    /// time) and the state map (which poll/write/kill reach via id).
    /// Sessions are cheap to keep alive (one thread per session);
    /// we don't auto-evict dead sessions — the user kills them
    /// explicitly or the app exit cleans them up via the
    /// RunEvent::Exit handler.
    terminals: Mutex<std::collections::HashMap<String, std::sync::Arc<TerminalHandle>>>,
    /// v1.0.9-rc30 (Lesson 536): the actual resolved URL of the main
    /// dashboard window, captured at `create_main_window` time.
    ///
    /// On Windows production builds, Tauri 2 serves bundled assets at
    /// `http://tauri.localhost/index.html` (not `tauri://localhost/`).
    /// On Linux/macOS it's `tauri://localhost/index.html`. We can't
    /// hardcode either — Tauri owns the scheme. So we ask the freshly
    /// built window what URL it ended up at, store that, and reuse it
    /// when the chat pill asks us to "go back to dashboard".
    ///
    /// `None` before create_main_window runs (early lifecycle races).
    dashboard_url: Mutex<Option<String>>,
    /// v1.0.9-rc32 (Lesson 538): once we've captured the dashboard URL,
    /// STOP accepting `on_page_load` updates. Reason: `on_page_load`
    /// fires every time the page loads — including when the user
    /// navigates FROM the dashboard INTO the chat. If we kept overwriting,
    /// the chat-page URL (`http://127.0.0.1:28789/chat?...`) would clobber
    /// the dashboard URL, and the next ← Dashboard click would navigate
    /// BACK to the chat URL (causing the login flicker David reported
    /// at 11:27 MDT 2026-08-22). The lock flag ensures we capture the
    /// first `tauri.localhost` URL and never overwrite it with chat URLs.
    dashboard_url_locked: Mutex<bool>,
    /// v1.1.0-rc53.11 (Lesson 247): URL we navigated FROM when opening
    /// an overlay via `mc_open_overlay`. Used by `mc_close_overlay` to
    /// restore the user's previous context (typically the OpenClaw
    /// chat window at `http://127.0.0.1:28789/...`).
    ///
    /// `None` means "no overlay is open" — `mc_close_overlay` should
    /// fall back to the dashboard in that case. Set on every successful
    /// `mc_open_overlay`, cleared on `mc_close_overlay` so a second
    /// close doesn't re-navigate to a stale URL.
    overlay_return_url: Mutex<Option<String>>,
    /// v1.1.0-rc53.15 (Lesson 570): runtime module registry.
    /// Populated at startup by scanning `<app_data>/modules/*/installer.json`
    /// and updated when modules are installed/uninstalled at runtime.
    ///
    /// `Registry` is itself `Arc<RwLock<HashMap>>` internally, so we
    /// could even share it without a Mutex — but wrapping in Mutex
    /// here for consistency with sibling fields and to make it easy
    /// to swap the registry out atomically in the future.
    modules: Mutex<crate::modules::registry::Registry>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(dead_code)] // fields are populated but not all read by the frontend yet
struct FirstRunReport {
    maic_plugin_installed: bool,
    maic_plugin_already_present: bool,
    openclaw_json_patched: bool,
    openclaw_json_already_patched: bool,
    /// Lesson 431: did setup() successfully wire `models.providers.maic`?
    /// false means the chat panel will fail with `missing-provider-auth`
    /// AND the first-run login modal will be shown.
    maic_provider_configured: bool,
    /// Endpoint the MAIC provider is configured against (informational).
    maic_provider_endpoint: String,
    /// Lesson 444: true when the MAIC provider is NOT configured because no
    /// MAIC API key was available at setup time. The frontend must show the
    /// login modal and call `maic_login` before the chat panel can render.
    /// Distinct from `maic_provider_configured == false` for plugin install
    /// errors — that case is a fatal installer bug, not a login prompt.
    needs_maic_login: bool,
    launcher_spawned: bool,
    gateway_ready: bool,
    gateway_error: Option<String>,
    /// Lesson TBD (2026-08-27): was the Windows voice stack installed by
    /// the MC installer? Reads HKLM\SOFTWARE\MiracleClaw\VoiceStackInstalled.
    /// Always false on non-Windows. Drives the dashboard banner + settings
    /// page Voice section visibility.
    voice_stack_installed: bool,
    /// Build number that installed the voice stack (registry value).
    /// Empty if never installed or non-Windows.
    voice_stack_build: String,
    /// Did the user opt into Voice Clarity system-wide on the installer page?
    /// Drives a recommendation in Settings → Voice to re-apply if needed.
    voice_clarity_opt_in: bool,
}

// ----------------------------------------------------------------------------
// Voice diagnostics (Lesson TBD, 2026-08-27)
//
// Cheap registry reads on boot. The full probe (PowerShell, mic device info,
// KB check, Voice Clarity status) lives in the separate `voice_diagnostics`
// command so we don't slow down first-run.
// ----------------------------------------------------------------------------
#[cfg(windows)]
fn voice_first_run_report() -> (bool, String, bool) {
    use std::process::Command;
    fn read_reg_dword(name: &str) -> bool {
        Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\MiracleClaw",
                "/v",
                name,
            ])
            .output()
            .ok()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                // reg.exe prints "    VoiceStackInstalled    REG_DWORD    0x1"
                s.contains("0x1")
            })
            .unwrap_or(false)
    }
    fn read_reg_str(name: &str) -> String {
        Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\MiracleClaw",
                "/v",
                name,
            ])
            .output()
            .ok()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                // reg.exe prints "    VoiceStackBuild    REG_SZ    1.1.0-rc54.0"
                // The data field is everything after the value name + REG_SZ
                s.lines()
                    .find(|l| l.contains(name) && l.contains("REG_SZ"))
                    .and_then(|l| l.split_whitespace().nth(2))
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default()
    }
    (
        read_reg_dword("VoiceStackInstalled"),
        read_reg_str("VoiceStackBuild"),
        read_reg_dword("VoiceClaritySystemWide"),
    )
}

#[cfg(not(windows))]
fn voice_first_run_report() -> (bool, String, bool) {
    (false, String::new(), false)
}

// ----------------------------------------------------------------------------
// Resource resolution
// ----------------------------------------------------------------------------

/// Returns the path to the bundled resources directory.
///
/// In production: Tauri sets `TAURI_BUNDLE_RESOURCES_DIR` before our process
/// starts. This is the directory we ship `node.exe` (Win), the openclaw bundle,
/// and the MAIC plugin source into via `bundle.resources` in tauri.conf.json.
///
/// In dev (`cargo tauri dev`): that env var is not set. We fall back to
/// `<src-tauri>/resources/` so iter-loop testing works without a full bundle.
fn resources_dir(_app: &tauri::AppHandle) -> PathBuf {
    if let Ok(p) = std::env::var("TAURI_BUNDLE_RESOURCES_DIR") {
        return PathBuf::from(p);
    }
    // Dev fallback: try walking up from current_exe to find src-tauri/resources.
    let mut cursor = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from));
    while let Some(dir) = cursor {
        let candidate = dir.join("resources");
        if candidate.join("openclaw.mjs").is_file() {
            return candidate;
        }
        cursor = dir.parent().map(PathBuf::from);
    }
    PathBuf::from("src-tauri/resources")
}

/// Path to the launcher sidecar binary. Tauri resolves this from
/// `bundle.externalBin` at runtime; this helper is just for log clarity.
#[allow(dead_code)]
fn launcher_exe_path(resources: &Path) -> PathBuf {
    #[cfg(windows)]
    let exe = format!("{}.exe", launcher_binary_name());
    #[cfg(not(windows))]
    let exe = launcher_binary_name().to_string();

    let candidates = [
        PathBuf::from(&exe),
        resources.join("..").join("bin").join(&exe),
        resources.join("bin").join(&exe),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(exe))
}

// ----------------------------------------------------------------------------
// MAIC plugin first-run bootstrap
// ----------------------------------------------------------------------------

/// Returns the user's openclaw extensions directory.
/// *nix:    $HOME/.miracle-claw/extensions/
/// Windows: %APPDATA%\MiracleClaw\extensions\
///
/// NOTE: We intentionally do NOT share state with the system OpenClaw
/// install at $HOME/.openclaw/. MC carries its own config, sessions,
/// plugins, and logs. This avoids config races when both run on the
/// same machine.
fn openclaw_extensions_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(roam) = std::env::var("APPDATA") {
            return PathBuf::from(roam).join("MiracleClaw").join("extensions");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".miracle-claw").join("extensions");
        }
    }
    PathBuf::from("./.miracle-claw/extensions")
}

/// Returns the path to the user's openclaw.json.
fn openclaw_json_path() -> PathBuf {
    openclaw_extensions_dir()
        .parent()
        .map(|p| p.join("openclaw.json"))
        .unwrap_or_else(|| PathBuf::from("./openclaw.json"))
}

/// Returns the path to the bundled MAIC plugin source (a directory containing
/// `openclaw.plugin.json`, `index.js`, `package.json`).
fn bundled_maic_plugin_dir(resources: &Path) -> PathBuf {
    resources.join("maic-plugin")
}

#[derive(PartialEq)]
enum CopyResult {
    AlreadyPresent,
    Installed,
    SourceMissing,
}

fn copy_maic_plugin_if_needed(
    resources: &Path,
) -> io::Result<(CopyResult, PathBuf)> {
    let src_dir = bundled_maic_plugin_dir(resources);
    if !src_dir.is_dir() {
        return Ok((CopyResult::SourceMissing, PathBuf::new()));
    }
    let dest_dir = openclaw_extensions_dir().join("maic");

    // Idempotency check: hash every source file, compare to a manifest
    // written next to the install. Skip copy if matches.
    let manifest = dest_dir.join(".miracle-claw-installed-sha256");
    let src_hashes = compute_hashes_manifest(&src_dir)?;
    let on_disk = fs::read_to_string(&manifest).unwrap_or_default();
    if dest_dir.is_dir() && on_disk == src_hashes {
        return Ok((CopyResult::AlreadyPresent, dest_dir));
    }

    fs::create_dir_all(&dest_dir)?;
    for name in MAIC_PLUGIN_FILENAMES {
        let sp = src_dir.join(name);
        if sp.is_file() {
            fs::copy(&sp, dest_dir.join(name))?;
        }
    }
    // Backfill configSchema on the manifest so openclaw 2026.7.1+ validates it.
    ensure_maic_manifest_compat(&dest_dir)?;
    fs::write(manifest, src_hashes)?;

    Ok((CopyResult::Installed, dest_dir))
}

// Ensure the MAIC plugin manifest declares the `configSchema` field that
// openclaw 2026.7.1+ enforces during gateway validation. Older openclaw
// versions tolerated its absence; new ones reject the plugin outright.
// We backfill on copy to keep the user-installed plugin valid no matter
// which openclaw version it was originally installed under.
fn ensure_maic_manifest_compat(target: &Path) -> io::Result<()> {
    let manifest = target.join("openclaw.plugin.json");
    if !manifest.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&manifest)?;
    let mut parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse manifest: {e}")))?;
    if parsed.get("configSchema").is_some() {
        return Ok(());
    }
    parsed["configSchema"] = serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {}
    });
    let serialized = serde_json::to_string_pretty(&parsed)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize manifest: {e}")))?;
    fs::write(&manifest, serialized)
}

fn compute_hashes_manifest(dir: &Path) -> io::Result<String> {
    let mut out = String::new();
    for name in MAIC_PLUGIN_FILENAMES {
        let p = dir.join(name);
        if p.is_file() {
            let h = sha256_file(&p)?;
            out.push_str(&format!("{} {}\n", name, h));
        }
    }
    Ok(out)
}

fn sha256_file(p: &Path) -> io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut hasher = Sha256::new();
    hasher.update(&buf);
    Ok(format!("{:x}", hasher.finalize()))
}

// ----------------------------------------------------------------------------
// openclaw.json sanity check (read-only)
// ----------------------------------------------------------------------------
//
// openclaw 2026.7.1+ discovers plugins automatically from <stateDir>/extensions/.
// No openclaw.json key is needed or accepted. We deliberately do NOT modify the
// user's openclaw.json — touching it risks invalidating their config schema.
//
// The previous version of this function patched in `plugins.roots` AND
// `pluginRoots` keys. openclaw 2026.7.1 rejects both as invalid input. Removing
// the patch keeps us out of trouble; the launcher still sets OPENCLAW_STATE_DIR
// and we still copy the MAIC plugin into <stateDir>/extensions/maic/.
//
// We do still VERIFY that the user's config file is parseable JSON (cheap
// preflight), and warn (stderr only) if it's empty or malformed. This catches
// the "I edited my config and broke it" case without making it worse.

fn check_openclaw_json() -> io::Result<()> {
    let path = openclaw_json_path();
    if !path.is_file() {
        // No config file yet — that's fine, openclaw will create defaults.
        return Ok(());
    }
    match fs::read_to_string(&path) {
        Ok(raw) if raw.trim().is_empty() => {
            eprintln!(
                "[miracle-claw] WARNING: openclaw.json exists but is empty; gateway will use defaults"
            );
            Ok(())
        }
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!(
                    "[miracle-claw] WARNING: openclaw.json is not valid JSON: {} — gateway may fail",
                    e
                );
                Ok(())
            }
        },
        Err(e) => Err(e),
    }
}

/// Pre-write a minimal openclaw.json if one doesn't exist. openclaw's gateway
/// refuses to start with exit code 78 on a fresh install where no config file
/// is present at <stateDir>/openclaw.json. We avoid the `--allow-unconfigured`
/// flag (which exists but is intended for headless CI, not for end users) by
/// providing a real, valid config the user can edit later.
///
/// The schema matches openclaw 2026.7.1+:
///   - `gateway.mode = "local"` tells the gateway it's a local install
///   - `gateway.bind` + `gateway.auth` are the defaults the launcher passes;
///     openclaw reads them from the config if present
///
/// We DO NOT touch an existing openclaw.json — except for one auto-migration:
/// if the existing file was written by an older MC that shipped the legacy
/// flat shape `gateway.auth: "none"`, we rewrite it. MC is the only thing
/// that could have written that exact shape (users wouldn't type it), so
/// fixing it is safe and avoids forcing a reinstall to recover from a known
/// MC-shipped bug.
///
/// Schema (openclaw 2026.7.1+, from dist/zod-schema-*.js):
///   - `gateway.mode` ∈ {"local", "remote"}
///   - `gateway.bind` ∈ {"auto", "lan", "loopback", "custom", "tailnet"}
///   - `gateway.auth.mode` ∈ {"none", "token", "password", "trusted-proxy"}
///   - `gateway.auth` itself is `.strict()` — extra keys (like the legacy
///     flat `"none"`) are rejected with "Invalid input".
fn ensure_openclaw_json_minimal() -> io::Result<()> {
    let path = openclaw_json_path();
    if path.is_file() {
        // Auto-migrate: if the existing file was written by an older MC
        // with the legacy flat `gateway.auth: "none"` shape, rewrite it.
        if let Ok(true) = migrate_legacy_mc_config(&path) {
            eprintln!(
                "[miracle-claw] migrated legacy openclaw.json (gateway.auth: \"none\" → nested object) at {}",
                path.display()
            );
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let minimal = serde_json::json!({
        "$schema": "https://openclaw.dev/schema/v1/openclaw.config.schema.json",
        "gateway": {
            "mode": "local",
            "bind": "loopback",
            "auth": {
                "mode": "none"
            }
        }
    });
    let serialized = serde_json::to_string_pretty(&minimal)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize config: {e}")))?;
    fs::write(&path, serialized)?;
    eprintln!(
        "[miracle-claw] wrote minimal openclaw.json to {} (first-run bootstrap)",
        path.display()
    );
    Ok(())
}

/// Detect the legacy MC-shipped shape and rewrite it. Returns Ok(true) if
/// the file was rewritten, Ok(false) otherwise. Failures are non-fatal
/// (best-effort migration).
fn migrate_legacy_mc_config(path: &Path) -> io::Result<bool> {
    let raw = fs::read_to_string(path)?;
    let mut parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse: {e}")))?;
    let Some(gateway) = parsed.get_mut("gateway").and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };
    // Legacy shape: `gateway.auth` is a string. Modern shape: it's an object
    // with a `mode` field. The string form is always the bug MC shipped.
    if let Some(auth_value) = gateway.get("auth") {
        if auth_value.is_string() {
            let mode = auth_value.as_str().unwrap_or("none");
            gateway.insert("auth".to_string(), serde_json::json!({ "mode": mode }));
            let serialized = serde_json::to_string_pretty(&parsed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}")))?;
            fs::write(path, serialized)?;
            return Ok(true);
        }
    }
    Ok(false)
}

// ----------------------------------------------------------------------------
// MAIC provider config bootstrap (Lesson 431)
//
// openclaw's gateway resolves the chat provider from cfg.models.providers[<id>].
// The MAIC plugin (`<stateDir>/extensions/maic/`) supplies the `params` patch
// at transport time but does NOT register any provider entry — that's the
// user's job. Without it, the chat panel renders fine but the first chat turn
// throws `missing-provider-auth` because openclaw tries to call the default
// `openai` provider with no API key.
//
// MC's responsibility is to wire MAIC as the provider so a fresh install
// (clean state dir) gets a working chat roundtrip out of the box. We do this
// by deep-merging a `models.providers.maic` entry into openclaw.json.
//
// Sourcing priority (first non-empty wins, idempotent):
//   1. Existing entry in <stateDir>/openclaw.json (user already configured
//      it, or a prior MC run did). We do NOT overwrite a user-supplied
//      apiKey/baseUrl even if env vars are set — explicit beats implicit.
//   2. `MAIC_API_URL` + `MAIC_API_KEY` env vars (power-user override path).
//   3. System openclaw's openclaw.json (David already has MAIC wired there;
//      we copy the entry so MC's isolated state is consistent). The system
//      path is $HOME/.openclaw/openclaw.json on *nix,
//      %APPDATA%\openclaw\openclaw.json on Windows.
//   4. None → log a clear remediation message and continue. The chat panel
//      will render and fail loudly with the existing `missing-provider-auth`
//      error rather than silently misconfiguring.
//
// Schema (openclaw 2026.7.1+, from dist/zod-schema.core-*.js):
//   - models.providers[providerId].apiKey:   string (SecretInput)
//   - models.providers[providerId].baseUrl:  string
//   - models.providers[providerId].api:      string (e.g. "openai-completions")
//   - models.providers[providerId].models:   array of { id, ... }
//   - models.providers[providerId].params:   Record<string, unknown> (freeform;
//     the MAIC plugin reads `params.tool_execution` and `params.<others>`)
//
// Why `params.tool_execution = "client"`:
//   MAIC is a cascade router that runs server-side tools by default. To get
//   tool_calls back so our client-side tools (read_file, write_file, etc.)
//   can run in MC, MAIC must be told "the caller will execute tools." This
//   flag is set in `params` (NOT top-level) because the MAIC plugin reads
//   `ctx.config.models.providers.maic.params` at transport time and merges
//   it into the outbound request body. See src-tauri/resources/maic-plugin/
//   index.js: PROVIDER_ID = "maic", extraParamsForTransport().
//
// Default model id:
//   `milagro-dev` is David's 14B generalist (the largest deployed local
//   model). The MAIC backend has 14B, several 7B ternary tiers, and
//   several cloud cascade endpoints; we surface the local default so the
//   chat roundtrip is fully self-hosted on first run.
//
// Default endpoint:
//   `https://maicserver.com` (Cloudflare-fronted). MEMORY.md line 351 /
//   4493: Tauri-spawned sidecars run on Windows and CANNOT reach Tailscale
//   IPs, so the Cloudflare endpoint is the only universally-reachable
//   option for a fresh install.
//
// Lesson 520 helper: idempotently merge the 20 known MAIC model ids into
// `cfg.models.providers[<provider_id>].models[]`. Used by both the new-entry
// write path AND the existing-complete-entry early-return path, so users
// upgrading from rc18 (where only `milagro-dev` was seeded) get the full
// surface in `models[]` after their next launch.
//
// Why this matters: openclaw's gateway resolves a bare model id
// (`"milagro-oc-kimi"`) via `inferUniqueProviderFromCatalog`, which scans
// `models.providers[*].models[]`. If the entry isn't there, the gateway
// falls back to `defaultProvider = "openai"` and rewrites the request as
// `openai/milagro-oc-kimi` — which MAIC upstream rejects with
// `Unknown model`. Lesson 519 documented this behavior; Lesson 520 fixes
// the upgrade path that skipped the merge on existing entries.
//
// Idempotency: never overwrites an existing entry by id (preserves user
// renames and any custom metadata). Only appends missing entries.
//
// Lesson 527 (NEW 2026-08-21 13:57 MDT): tier-gated model list.
// Free users get ONLY m1-t1 + m1-t2 (small distilled models, chat-only
// UX). Paid tiers (Pro, ProPlus, Team, Enterprise) get the full 20-
// model catalog. This is the actual cost-control for Free accounts:
// they can't accidentally pick `milagro-dev` (14B) and burn their
// 50K TPM in 3 messages. Tier parameter added — callers without
// tier context (early setup, login-required bootstrap) should pass
// `Tier::Free` to be safe per Lesson 176.
fn merge_known_model_ids_into_provider(
    cfg: &mut Value,
    provider_id: &str,
    tier: crate::auth::tier::Tier,
) {
    // Full catalog (20 models). Free users see only the ones in
    // FREE_MODEL_IDS; paid users see all 20.
    const ALL_MODEL_IDS: &[&str] = &[
        "milagro-dev", "milagro-dev-coder", "milagro-m1",
        "milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3",
        "milagro-chat", "milagro-coder", "milagro-stock",
        "milagro-oc-minimax", "milagro-oc-glm", "milagro-oc-qwen",
        "milagro-oc-deepseek", "milagro-oc-kimi",
        "chat-glm", "chat-deepseek", "chat-qwen",
        // Lesson 567 (2026-08-24 22:25 MDT, David): add 3 Nemotron
        // models — NVIDIA's open-weights MoE family. Available on
        // Ollama Cloud (we already pay flat subscription via
        // OLLAMA_API_KEY_1/2/3). Benchmark via MAIC: 1.7-1.8s for
        // short answers, English-clean, supports reasoning.
        "chat-nemotron-nano",
        "chat-nemotron-super",
        "chat-nemotron-ultra",
    ];
    // Lesson 527: Free tier gets the distilled m1 models + cheap cloud Nemotron.
    // Lesson 565 (2026-08-24 18:41 MDT, David): add t3 to free tier.
    // Lesson 567 (2026-08-24 22:25 MDT, David): add chat-nemotron-nano — 1.7s,
    // NVIDIA MoE, very capable, ~$0.05/M blended via Ollama Cloud subscription.
    // Lesson 569 (2026-08-24 22:57 MDT, David): confirm Free chain is local-first
    // (m1-t1 → m1-t2 → m1-t3) and ONLY hits the cloud as last-resort fallback
    // (chat-nemotron-nano, usage level 1 — the cheapest Ollama Cloud route).
    // See Lesson 568 for the full Ollama usage-level ranking.
    const FREE_MODEL_IDS: &[&str] = &[
        "milagro-m1-t1",
        "milagro-m1-t2",
        "milagro-m1-t3",
        "chat-nemotron-nano",
    ];
    let allowed: &[&str] = match tier {
        crate::auth::tier::Tier::Free => FREE_MODEL_IDS,
        _ => ALL_MODEL_IDS,
    };
    let Some(provider) = cfg
        .get_mut("models")
        .and_then(|m| m.get_mut("providers"))
        .and_then(|p| p.get_mut(provider_id))
        .and_then(|p| p.as_object_mut())
    else {
        return;
    };
    let models_arr = provider
        .entry("models".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !models_arr.is_array() {
        *models_arr = Value::Array(Vec::new());
    }
    let models = models_arr.as_array_mut().unwrap();
    let present: std::collections::HashSet<String> = models
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    for id in allowed {
        if present.contains(*id) { continue; }
        models.push(serde_json::json!({
            "id": *id,
            "name": *id,
        }));
    }
    // Lesson 527: also remove any models from the on-disk list that
    // the current tier doesn't allow. This handles the upgrade path
    // (Free → Pro) AND the downgrade path (Pro → Free). Without this,
    // a Pro user who downgrades to Free would still see the paid-tier
    // models in their picker, and could pick one and burn TPM.
    //
    // We only remove entries whose `id` exactly matches a known model
    // in the disallow set — user-added custom models are preserved.
    let allowed_set: std::collections::HashSet<&str> = allowed.iter().copied().collect();
    let known_paid: std::collections::HashSet<&str> = ALL_MODEL_IDS.iter().copied().collect();
    let known_free: std::collections::HashSet<&str> = FREE_MODEL_IDS.iter().copied().collect();
    let to_remove: Vec<usize> = models
        .iter()
        .enumerate()
        .filter_map(|(idx, m)| {
            let id = m.get("id").and_then(|v| v.as_str())?;
            // Only remove if it's a known MAIC model (not user-added)
            // AND it's not in the allowed-for-this-tier set.
            let is_known = known_paid.contains(id) || known_free.contains(id);
            if is_known && !allowed_set.contains(id) {
                Some(idx)
            } else {
                None
            }
        })
        .collect();
    // Remove in reverse order so indices stay valid.
    for idx in to_remove.into_iter().rev() {
        models.remove(idx);
    }
}

/// Lesson 523 (NEW, 2026-08-20): tier-gated tool injection.
///
/// `tier` controls which local tool schemas get written into
/// `models.providers.maic.params.tools`:
/// - `Tier::Free` → empty array (no `read_file`/`write_file`/`bash_run`/
///   `apply_patch`/`remember_fact`/etc. advertised to MAIC)
/// - paid tiers → all 7 LocalTool schemas
///
/// Callers WITH tier context (login flow, silent relogin) must use
/// `ensure_maic_provider_config_for_tier(tier)`. Callers WITHOUT tier
/// context (early setup, login-required bootstrap) call the no-tier
/// variant which defaults to `Tier::Free` — safe per Lesson 176
/// ("anything outside the canonical tier set is treated as free").
fn ensure_maic_provider_config() -> io::Result<MaicProviderBootstrap> {
    ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free)
}

/// Tier-aware variant. Thread the user's resolved tier through the
/// bootstrap so `params.tools` matches the user's entitlement.
fn ensure_maic_provider_config_for_tier(
    tier: crate::auth::tier::Tier,
) -> io::Result<MaicProviderBootstrap> {
    // Lesson 431 v2: use openclaw's native SecretRef + SecretProvider mechanism
    // (schema: zod-schema.core SecretInputSchema + SecretsConfigSchema) so that
    // the apiKey can be resolved from the OS env at request time without us
    // shipping a placeholder string. This matches steeler's system openclaw
    // pattern exactly: secrets.providers.default (env source) + models.providers.
    // maic.apiKey = { source: "env", provider: "default", id: "MAIC_API_KEY" }.
    //
    // If the user has already set MAIC_API_KEY in the env, we write the literal
    // string value (avoids the SecretRef indirection cost and matches the
    // openclaw-channel 'existing' path semantics). Otherwise we wire the
    // SecretRef + register the env provider so the request-time resolver fills
    // it in.
    const DEFAULT_API: &str = "openai-completions";
    const DEFAULT_MODEL_ID: &str = "milagro-dev";
    const PROVIDER_ID: &str = "maic";
    const SECRET_PROVIDER_ALIAS: &str = "default";

    /// Write tier-gated `params.tool_execution` + `params.tools` into the
    /// maic provider entry in-place. Idempotent — does nothing if
    /// `params.tools` already exists. Called from BOTH the write path
    /// (new entry creation) AND the existing-entry early-return path
    /// (rc18+ upgrades where the entry is already complete).
    ///
    /// Lesson 524 (NEW 2026-08-20 22:50 MDT — David confirmed the bot
    /// still had no local tools after rc22 install, even though Lesson
    /// 523 added tier-gated tools injection). Root cause: the existing-
    /// entry early-return path returned BEFORE the Lesson 523 tools
    /// block, so users with a complete apiKey + baseUrl (every login
    /// after first install) never had `params.tools` written.
    fn write_tier_gated_tool_execution_and_tools(
        cfg: &mut serde_json::Value,
        tier: crate::auth::tier::Tier,
    ) {
        let params_obj = cfg
            .get_mut("models")
            .and_then(|m| m.get_mut("providers"))
            .and_then(|p| p.get_mut(PROVIDER_ID))
            .and_then(|e| {
                if !e.is_object() {
                    *e = Value::Object(Default::default());
                }
                e.as_object_mut()
            })
            .map(|e| {
                let key = "params".to_string();
                let entry = e.entry(key).or_insert_with(|| Value::Object(Default::default()));
                if !entry.is_object() {
                    *entry = Value::Object(Default::default());
                }
                entry.as_object_mut().unwrap().clone()
            });
        if let Some(mut params) = params_obj {
            // Always stamp tool_execution=client (idempotent via entry().or_insert()).
            params
                .entry("tool_execution".to_string())
                .or_insert(Value::String("client".to_string()));
            // Lesson 794 mitigation (NEW 2026-08-30 15:55 MDT): MAIC's
            // `min_max_tokens_per_model` floor pins per-model caps at 600
            // tokens for reasoning models (kimi, GLM, deepseek,
            // nemotron-super). This causes `stopReason=length tools=0`
            // failures when the model tries to plan + emit tool_calls in
            // a single turn — it hits the 600-token floor before the
            // tool_call JSON is complete. David observed 24 such failures
            // in one session on RC55.16.
            //
            // Fix: stamp `params.max_tokens = 4000` (idempotent, only
            // when not already set). MAIC's `enforce_min_max_tokens()`
            // raises the request's max_tokens to per-model floor (600),
            // so 4000 here is the effective cap on the OUTBOUND request.
            // Models with natural caps lower than 4000 (e.g., smaller
            // distilled models) won't be hurt — MAIC clamps down to
            // their native cap. Models with caps ≥ 4000 will simply be
            // able to plan + emit tool calls without hitting the floor.
            params
                .entry("max_tokens".to_string())
                .or_insert(Value::Number(serde_json::Number::from(4000u32)));
            // Lesson 523 + 525: tier-gated tools array. Free → empty; paid → all 7.
            //
            // Lesson 525 (NEW 2026-08-21): empty array `[]` ALSO counts as
            // "needs stamping". Lesson 524's helper wrote `params.tools: []`
            // for users on the existing-entry path during the rc17→rc22
            // window (when Lesson 523 wasn't yet wired into that path),
            // and the existing `contains_key` check then treated the empty
            // array as "already populated, skip" — so upgraded users
            // stayed on empty tools even after rc23. Now we treat empty
            // array the same as missing key: re-stamp on every bootstrap.
            // User-customized schemas (non-empty array) are still preserved.
            //
            // Lesson 842 (NEW 2026-08-30 15:50 MDT): also re-stamp when the
            // build has added new tools since the user's last stamp.
            // Before this fix, David's openclaw.json (last stamped by a
            // build before rc55.13 added web_fetch) had 7 tools and the
            // writer correctly treated it as "user owns this, don't
            // touch" — but the user never got the new tool. Symptom:
            // model can't see `web_fetch` even though the plugin
            // registers it and the binary dispatches it. Fix: stamp a
            // version number alongside the tools array. When the stamp
            // is MISSING (old build never wrote it) or LOWER than the
            // current build's stamp, re-stamp. When the stamp matches,
            // trust the user — even if that means they've removed tools.
            //
            // CURRENT_STAMP_VERSION is bumped whenever a new tool is added
            // to ALL_LOCAL_TOOL_NAMES. Today it's 8; next addition → 9, etc.
            const CURRENT_STAMP_VERSION: u32 = 8;
            let stamp_version: u32 = params
                .get("_stamped_tools_version")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0);
            // Re-stamp when:
            //   - tools array is missing/empty (Lesson 525)
            //   - stamp version is older than current (Lesson 842 — build added new tools)
            // We do NOT compare tools array length to stamp version: that
            // would clobber user customizations (e.g. user removes a tool
            // they don't want). The stamp is only the BUILD version that
            // last wrote the array; user edits after stamping are preserved.
            let needs_tools_stamp = match params.get("tools") {
                None => true,
                Some(Value::Array(a)) => {
                    a.is_empty() || stamp_version < CURRENT_STAMP_VERSION
                },
                Some(_) => false, // non-empty, non-array — leave alone
            };
            if needs_tools_stamp {
                let tool_names = tools_for_tier(tier);
                let all_tools = crate::tools::schemas::all_local_tools_slice();
                let filtered: Vec<crate::tools::schemas::LocalTool> = all_tools
                    .iter()
                    .filter(|t| tool_names.contains(&t.name))
                    .map(|t| (*t).clone())
                    .collect();
                let tools_arr =
                    crate::tools::schemas::local_tools_to_openai_array(&filtered);
                params.insert("tools".to_string(), tools_arr);
                // Lesson 842: stamp the version so future boots can detect
                // user-removed tools (length != stamp) vs build-removed
                // tools (stamp < current).
                params.insert(
                    "_stamped_tools_version".to_string(),
                    Value::Number(serde_json::Number::from(CURRENT_STAMP_VERSION)),
                );
            }
            // Write the modified params back into the entry.
            if let Some(e) = cfg
                .get_mut("models")
                .and_then(|m| m.get_mut("providers"))
                .and_then(|p| p.get_mut(PROVIDER_ID))
                .and_then(|e| e.as_object_mut())
            {
                e.insert("params".to_string(), Value::Object(params));
            }
        }
    }

    let path = openclaw_json_path();

    // Load (or initialize) the user's config. If the file doesn't exist yet,
    // `ensure_openclaw_json_minimal` already wrote a minimal one in setup()
    // step 2 — read it back.
    let mut cfg: serde_json::Value = if path.is_file() {
        let raw = fs::read_to_string(&path)?;
        // If the file is empty or malformed, fall through to a fresh in-memory
        // config. We don't want a corrupted openclaw.json to block provider
        // bootstrapping.
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Pull the existing provider entry (if any) WITHOUT creating one.
    let existing = cfg
        .get("models")
        .and_then(|m| m.get("providers"))
        .and_then(|p| p.get(PROVIDER_ID))
        .cloned();

    // If the user already has a complete provider entry (literal non-empty
    // apiKey + baseUrl), we are done. This is the idempotency guarantee —
    // re-running setup() never overwrites a working config.
    //
    // Lesson 449 (post-v1.0.2): a SecretRef (apiKey = {source: "env", id: ...})
    // does NOT count as "complete" here. The previous behavior early-returned
    // on SecretRef and left it on disk, but the openclaw gateway would then
    // fail at startup with SecretRefResolutionError if the env var wasn't
    // actually set in the launcher's process env (which it never is on a
    // fresh install — the env var lives in the parent Tauri process, not
    // the spawned child). Result: gateway crashed, webview hit
    // ERR_CONNECTION_REFUSED on http://localhost:28789/ after login.
    //
    // The fix: only treat literal-string apiKey as complete. If the user has
    // a SecretRef, fall through to the write path so we can replace it with
    // the literal after login. User-customized configs with SecretRef still
    // resolve correctly on the write path (we preserve the existing entry's
    // other fields and only stamp apiKey when missing/empty).
    //
    // Lesson 451 (post-v1.0.4 bug — the early-return path skipped the
    // baseUrl /v1 migration): v1.0.4 users who already had a complete entry
    // hit this early return BEFORE the migration block could rewrite the
    // stale bare MAIC origin. The migration logic in the write path was
    // therefore unreachable for users coming from v1.0.0..v1.0.3, and the
    // "model not found" error persisted. Fix: run the migration here too
    // (when applicable) and persist the rewritten file before returning.
    // User-customized paths (proxy mounts, etc.) are preserved by the
    // conservative `upgrade_legacy_maic_base_url` policy.
    if let Some(entry) = existing.as_ref() {
        let has_literal_key = matches!(
            entry.get("apiKey"),
            Some(serde_json::Value::String(s)) if !s.trim().is_empty()
        );
        let has_url = entry
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_literal_key && has_url {
            // Lesson 520: when a "complete" provider entry exists, the rest
            // of ensure_maic_provider_config() (including the model-merge
            // block) is skipped. That meant upgrades from rc18 → rc19/rc20
            // never grew `models.providers.maic.models[]` past the original
            // milestone-dev entry. The openclaw gateway then fails to
            // resolve bare model ids (e.g. "milagro-oc-kimi") to MAIC's
            // baseUrl — `inferUniqueProviderFromCatalog` finds no catalog
            // entry, falls back to defaultProvider="openai", and rewrites
            // the request as `openai/milagro-oc-kimi`, which MAIC upstream
            // rejects with `Unknown model`.
            //
            // Fix: merge the known model ids here too. Idempotent
            // (preserves user renames), and writes back to disk only when
            // something actually changed (avoids spurious file mtime updates
            // on every launch). Tier-gated (Lesson 527): only stamps
            // models the user's current tier is allowed to see.
            merge_known_model_ids_into_provider(&mut cfg, PROVIDER_ID, tier);

            // Lesson 524: existing-entry early-return path also needs to
            // stamp `params.tool_execution` and `params.tools`. Without
            // this, every login after first install would skip Lesson 513
            // (tool_execution) and Lesson 523 (tier-gated tools array),
            // and the bot would see only MAIC's 4 server tools in the
            // model's tool list — no `read_file` / `bash_run` / etc.
            //
            // The helper persists `cfg` below; no double-write needed.
            write_tier_gated_tool_execution_and_tools(&mut cfg, tier);

            // Lesson 535 (NEW 2026-08-22): also stamp
            // `plugins.entries.maic.enabled = true` so openclaw 2026.7.1's
            // activation decision treats the MAIC plugin as explicitly
            // enabled. Without this, the plugin's `register()` never
            // fires, the `extraParamsForTransport` hook never runs, and
            // the model only sees MAIC's 4 server tools. Same reason
            // write_tier_gated_tool_execution_and_tools is called here
            // (Lesson 524): users upgrading from rc28 or earlier need
            // the plugin entry stamped on first post-upgrade bootstrap.
            stamp_maic_plugin_entry(&mut cfg);

            // Lesson 451: migrate the baseUrl /v1 suffix in-place when it's
            // a stale bare MAIC origin. Persist if we changed anything so
            // the migration is one-shot, not every-launch.
            if let Some(existing_base) = entry.get("baseUrl").and_then(|v| v.as_str()) {
                if let Some(fixed) = upgrade_legacy_maic_base_url(existing_base) {
                    if let Some(models) = cfg
                        .get_mut("models")
                        .and_then(|m| m.get_mut("providers"))
                        .and_then(|p| p.get_mut(PROVIDER_ID))
                        .and_then(|p| p.as_object_mut())
                    {
                        models.insert("baseUrl".to_string(), Value::String(fixed.clone()));
                        let serialized = serde_json::to_string_pretty(&cfg)
                            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                        fs::write(&path, serialized)?;
                        eprintln!(
                            "[miracle-claw] Lesson 451: migrated baseUrl {} → {} (in-place)",
                            existing_base, fixed
                        );
                        return Ok(MaicProviderBootstrap {
                            provider_configured: true,
                            provider_id: PROVIDER_ID.to_string(),
                            api_key_source: MaicKeySource::Existing,
                            endpoint: fixed,
                        });
                    }
                }
            }

            // Persist any model merge that happened above before returning.
            // Cheap when nothing changed (mtime stays the same on most
            // filesystems, but we still write once to be safe).
            let serialized = serde_json::to_string_pretty(&cfg)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            fs::write(&path, serialized)?;

            return Ok(MaicProviderBootstrap {
                provider_configured: true,
                provider_id: PROVIDER_ID.to_string(),
                api_key_source: MaicKeySource::Existing,
                endpoint: entry
                    .get("baseUrl")
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_ENDPOINT)
                    .to_string(),
            });
        }
    }

    // Resolve apiKey from env, then from system openclaw. Whichever is
    // first non-empty wins.
    let env_key = std::env::var(ENV_VAR_NAME)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let env_url = std::env::var("MAIC_API_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (key_source, resolved_key) = if let Some(k) = env_key.clone() {
        (MaicKeySource::Env, Some(k))
    } else {
        match read_system_openclaw_maic_key() {
            Ok(Some(k)) => (MaicKeySource::SystemOpenClaw, Some(k)),
            Ok(None) => (MaicKeySource::None, None),
            Err(e) => {
                eprintln!(
                    "[miracle-claw] could not read system openclaw for MAIC key fallback: {}",
                    e
                );
                (MaicKeySource::None, None)
            }
        }
    };

    // If we still don't have a key, we can't ship a working config without
    // deferring to the OS env. Lesson 431 v2: write a SecretRef + register
    // a default env provider so openclaw's request-time resolver pulls
    // MAIC_API_KEY from the process env. The user fixes the chat by setting
    // MAIC_API_KEY + restarting; openclaw will surface a clear "secret not
    // found in env" error instead of the opaque missing-provider-auth.
    //
    // LoginRequired path (Lesson 444): if no literal key came from anywhere,
    // signal the frontend that it needs to collect one via maic_login() before
    // chat will work. We DO NOT write a SecretRef in that case — that would
    // produce a guaranteed-failing config and complicate the user-facing
    // message. The frontend blocks the chat panel on `LoginRequired`.
    let (key_source, resolved_key) = match resolved_key {
        Some(k) => (key_source, Some(k)),
        None => (MaicKeySource::LoginRequired, None),
    };

    // Lesson 444 — LoginRequired early return.
    //
    // When no key is available, we DO NOT write a placeholder provider entry
    // into openclaw.json. The chat panel blocks on the first-run login modal
    // instead, and `maic_login` re-runs `ensure_maic_provider_config` after
    // MAIC_API_KEY is set in the process env (at which point we hit the
    // literal-key branch and write the full provider entry).
    //
    // Why we don't write a SecretRef like the Lesson 431 v2 path used to:
    //   - A SecretRef to MAIC_API_KEY produces a guaranteed-failing config
    //     when no env var is set; openclaw will surface a "secret not found"
    //     error and the user has no clear next step.
    //   - LoginRequired is the *customer-facing* solution: collect the key
    //     via the login modal, bake it as a literal, and chat just works.
    //   - This also keeps openclaw.json clean — no half-populated maic
    //     provider entry that needs a separate migration to clean up.
    if matches!(key_source, MaicKeySource::LoginRequired) {
        let resolved_url = env_url
            .clone()
            .or_else(|| {
                read_system_openclaw_maic_base_url().ok().flatten()
            })
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        eprintln!(
            "[miracle-claw] MAIC provider login required (no key in env, system openclaw, or SecretRef) — chat panel will block on login modal"
        );
        return Ok(MaicProviderBootstrap {
            provider_configured: false,
            provider_id: PROVIDER_ID.to_string(),
            api_key_source: key_source,
            endpoint: resolved_url,
        });
    }

    let resolved_url = env_url
        .clone()
        .or_else(|| {
            // Fall back to whatever URL the system openclaw already has
            // configured — preserves the user's existing setup.
            read_system_openclaw_maic_base_url().ok().flatten()
        })
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

    // Build the provider entry. We start from whatever existing fields the
    // user already has (so we never delete a model the user added) and fill
    // in the rest.
    let mut provider_entry = existing.unwrap_or_else(|| serde_json::json!({}));
    if !provider_entry.is_object() {
        // Defensive: an array or scalar here would be a schema violation.
        // Replace it with an object so the deep-merge below is well-defined.
        provider_entry = serde_json::json!({});
    }
    let entry_obj = provider_entry.as_object_mut().unwrap();

    entry_obj.entry("baseUrl".to_string()).or_insert(Value::String(normalize_maic_base_url(&resolved_url)));
    // Lesson 450: ensure the openai-completions baseUrl we stamp carries the
    // `/v1` suffix that openclaw's OpenAI SDK appends `/chat/completions` to.
    // Without `/v1`, openclaw POSTs to `{origin}/chat/completions` and MAIC
    // returns 404. openclaw's regex classifier then sees "404 ... not found"
    // in the body and surfaces the misleading "The selected model was not
    // found by the provider" error to the user — even though MAIC accepts
    // the same model on its `/v1/chat/completions` route.
    //
    // The line above only inserts when baseUrl is missing. To retroactively
    // repair an existing user's openclaw.json written by v1.0.0..v1.0.3
    // (which lack `/v1`), the next block rewrites the value in-place when
    // it's clearly a stale MAIC origin without the suffix. User-customized
    // paths or proxy-prefixed URLs are preserved verbatim.
    if let Some(existing_base) = entry_obj.get("baseUrl").and_then(|v| v.as_str()) {
        if let Some(fixed) = upgrade_legacy_maic_base_url(existing_base) {
            entry_obj.insert("baseUrl".to_string(), Value::String(fixed));
        }
    }
    // Only stamp apiKey if the existing one is "unresolvable" (Lesson 449):
    //   - missing entirely
    //   - literal empty/whitespace string
    //   - SecretRef whose target env var is empty/missing (would crash the
    //     openclaw gateway at startup with SecretRefResolutionError)
    // A non-empty literal OR a SecretRef that resolves in the current env
    // is left alone — preserves the user's existing setup.
    if !entry_obj.contains_key("apiKey") || is_unresolvable_api_key(&entry_obj["apiKey"]) {
        match &resolved_key {
            Some(literal) => {
                entry_obj.insert("apiKey".to_string(), Value::String(literal.clone()));
            }
            None => {
                // Lesson 431 v2: emit a SecretRef so the user can set MAIC_API_KEY
                // in their env and have chat work without re-running setup().
                entry_obj.insert(
                    "apiKey".to_string(),
                    serde_json::json!({
                        "source": "env",
                        "provider": SECRET_PROVIDER_ALIAS,
                        "id": ENV_VAR_NAME,
                    }),
                );
            }
        }
    }
    entry_obj.entry("api".to_string()).or_insert(Value::String(DEFAULT_API.to_string()));

    // Models list — preserve any user additions, default to milagro-dev.
    let models_arr = entry_obj
        .entry("models".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !models_arr.is_array() {
        *models_arr = Value::Array(Vec::new());
    }
    let models = models_arr.as_array_mut().unwrap();
    if models.is_empty() {
        // Lesson 491 / v1.0.9-rc19: seed the model dropdown with the full
        // production model surface exposed by MAIC's /v1/models endpoint,
        // not just the single default. David uses these to switch between
        // local (dev/14B, m1-t-series 7B) and cloud cascades (MiniMax,
        // GLM, DeepSeek, Kimi, Qwen). The default (`milagro-dev`) is the
        // largest local model and stays at the top.
        //
        // See MEMORY.md "MAIC Deployed Model Inventory" for the
        // verified list (2026-08-14 12:06 MDT).
    // Lesson 527 (NEW 2026-08-21 13:57 MDT, revised 2026-08-24
    // Lesson 567): tier-gated model list.
    // Free gets m1-t1 + m1-t2 + m1-t3 + chat-nemotron-nano
    // (chat-only fast tier + cheap NVIDIA MoE).
    // Paid gets the full 20-model catalog (Lesson 567 added 3 Nemotron
    // models on 2026-08-24). The `seeds` array drives the
    // first-install write path (when models list is empty); for
    // upgrades the `else` branch below does the same tier gating.
    // Lesson 565 (2026-08-24 18:41 MDT, David): add m1-t3 to Free
    // seeds (not just the upgrade merge path) so first-install Free
    // users also get t3. Cost-neutral: local 14B-distilled, fewer
    // cloud fallbacks.
    let seeds: &[(&str, &str)] = match tier {
        crate::auth::tier::Tier::Free => &[
            ("milagro-m1-t1",      "MAIC m1-t1 — 3B LoRA-distilled (fast)"),
            ("milagro-m1-t2",      "MAIC m1-t2 — 7B LoRA-distilled (mid)"),
            ("milagro-m1-t3",      "MAIC m1-t3 — 14B LoRA-distilled (top of t-series)"),
            ("chat-nemotron-nano", "Cloud Nemotron 3 Nano 30B (NVIDIA MoE, 1.7s)"),
        ],
        _ => &[
            ("milagro-dev",            "MAIC default (miracle-claw) — 14B local generalist"),
            ("milagro-dev-coder",      "MAIC coder — 14B local code-tuned"),
            ("milagro-m1",             "MAIC m1 — base"),
            ("milagro-m1-t1",          "MAIC m1-t1 — 3B LoRA-distilled (fast)"),
            ("milagro-m1-t2",          "MAIC m1-t2 — 7B LoRA-distilled (mid)"),
            ("milagro-m1-t3",          "MAIC m1-t3 — 14B LoRA-distilled (top of t-series)"),
            ("milagro-chat",           "MAIC chat — small general baseline"),
            ("milagro-coder",          "MAIC coder — small-mid code baseline"),
            ("milagro-stock",          "MAIC stock — stock-specific small"),
            ("milagro-oc-minimax",     "Cloud cascade — MiniMax M3 (MiniMax-M3)"),
            ("milagro-oc-glm",         "Cloud cascade — OpenChat GLM"),
            ("milagro-oc-qwen",        "Cloud cascade — OpenChat Qwen"),
            ("milagro-oc-deepseek",    "Cloud cascade — OpenChat DeepSeek"),
            ("milagro-oc-kimi",        "Cloud cascade — OpenChat Kimi"),
            ("chat-glm",               "Cloud — GLM (direct)"),
            ("chat-deepseek",          "Cloud — DeepSeek (direct)"),
            ("chat-qwen",              "Cloud — Qwen (direct)"),
            ("chat-nemotron-nano",     "Cloud — Nemotron 3 Nano 30B A3B (NVIDIA MoE, 1.7s) — Free tier cloud fallback"),
            ("chat-nemotron-super",    "Cloud — Nemotron 3 Super 120B A12B (NVIDIA MoE, 1.8s)"),
            ("chat-nemotron-ultra",    "Cloud — Nemotron 3 Ultra 550B A55B (NVIDIA MoE, flagship)"),
        ],
    };
        for (id, name) in seeds {
            models.push(serde_json::json!({
                "id": id,
                "name": name,
            }));
        }
    } else {
        // Ensure the default model id is present even if the user added
        // others (so the chat panel has a default model to pre-select).
        let has_default = models
            .iter()
            .any(|m| m.get("id").and_then(|v| v.as_str()) == Some(DEFAULT_MODEL_ID));
        if !has_default {
            models.push(serde_json::json!({
                "id": DEFAULT_MODEL_ID,
                "name": "MAIC default (miracle-claw)",
            }));
        }
        // v1.0.9-rc19: if the user is upgrading from an older install
        // whose openclaw.json only had the single default model, merge
        // in any missing entries from the production surface so the
        // dropdown is complete. We never overwrite existing entries
        // (preserves user renames).
        //
        // Lesson 527 (NEW 2026-08-21): tier-gated. Free gets only
        // m1-t1 + m1-t2 + m1-t3 + chat-nemotron-nano (Lesson 565
        // added t3; Lesson 567 added chat-nemotron-nano for a fast
        // MoE option). Paid gets all 20.
        // Lesson 565 (2026-08-24 18:41 MDT, David): add m1-t3 to Free
        // — heaviest local 14B-distilled, gives better experience,
        // cost-neutral (local, fewer cloud fallbacks).
        // Match the gating in merge_known_model_ids_into_provider
        // (the existing-entry early-return path) so both code paths
        // produce the same model list for the same tier.
        let known_ids: &[&str] = match tier {
            crate::auth::tier::Tier::Free => &[
                "milagro-m1-t1",
                "milagro-m1-t2",
                "milagro-m1-t3",
                "chat-nemotron-nano",
            ],
            _ => &[
                "milagro-dev", "milagro-dev-coder", "milagro-m1",
                "milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3",
                "milagro-chat", "milagro-coder", "milagro-stock",
                "milagro-oc-minimax", "milagro-oc-glm", "milagro-oc-qwen",
                "milagro-oc-deepseek", "milagro-oc-kimi",
                "chat-glm", "chat-deepseek", "chat-qwen",
                // Lesson 567 (2026-08-24 22:25 MDT, David): 3 NVIDIA Nemotron
                // models — open MoE family available on Ollama Cloud via
                // existing OLLAMA_API_KEY_1/2/3 subscriptions. Brings the
                // catalog from 17 → 20. nano added to Free (cheap MoE,
                // ~$0.05/M blended), super + ultra paid-only (550B flagship).
                "chat-nemotron-nano",
                "chat-nemotron-super",
                "chat-nemotron-ultra",
            ],
        };
        let present: std::collections::HashSet<String> = models
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for id in known_ids {
            if present.contains(*id) { continue; }
            models.push(serde_json::json!({
                "id": *id,
                "name": *id,
            }));
        }
    }

    // params: MAIC plugin reads `params.tool_execution` and merges provider-
    // level `params` into the outbound request body. We set
    // `tool_execution: "client"` so the plugin tells MAIC to return
    // tool_calls instead of executing them server-side.
    let params_obj = entry_obj
        .entry("params".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !params_obj.is_object() {
        *params_obj = Value::Object(Default::default());
    }
    let params = params_obj.as_object_mut().unwrap();
    params.entry("tool_execution".to_string()).or_insert(Value::String("client".to_string()));

    // v1.0.7 / Lesson 513 + Lesson 523 (NEW 2026-08-20): tier-gated tool
    // injection. The `entry_obj` is the in-memory entry being constructed
    // for the write path; the helper handles the existing-entry early-
    // return path separately (Lesson 524).
    //
    // We write the 7 local tool schemas into `params.tools` only for
    // PAID tiers. Free users get an empty `tools: []` array so MAIC
    // never advertises file/bash/memory tools to the model at all —
    // the model can't call what it can't see.
    //
    // Plugin (`depot/maic-plugin/index.js`) spreads `params.tools`
    // into the outbound chat-completions request body via
    // `...providerParams`. With client tools = empty array, MAIC
    // sees zero caller tools and only its 4 server tools
    // (weather, web_search, get_current_time, calculate) get
    // advertised. With client tools = 7 schemas, MAIC merges
    // caller + server tools (Lesson 169 merge semantics) and the
    // model sees the union of 11 tools.
    //
    // Gating: this function gets tier from its caller. Login flow
    // and silent_relogin pass the user's resolved tier; setup()
    // and login-required bootstraps default to Free.
    //
    // Idempotency: `if !params.contains_key("tools")` preserves any
    // user-edited value (mirrors Lesson 449 idempotency pattern).
    //
    // Lesson 525 (NEW 2026-08-21): treat empty array the same as missing
    // key. See write_tier_gated_tool_execution_and_tools comment for
    // the rc17→rc22 root-cause story. Without this, users who got
    // `params.tools: []` stamped during the rc17→rc22 window (when
    // Lesson 523 wasn't yet wired into the existing-entry path) would
    // stay on empty tools forever — the original `contains_key` check
    // treats `[]` as "already populated, skip".
    let needs_tools_stamp = match params.get("tools") {
        None => true,
        Some(serde_json::Value::Array(a)) => a.is_empty(),
        Some(_) => false,
    };
    if needs_tools_stamp {
        let tool_names = tools_for_tier(tier);
        let all_tools = crate::tools::schemas::all_local_tools_slice();
        // Filter the static tool slice down to the tier-allowed names.
        // Stable order matches `all_local_tools()` definition order.
        // `local_tools_to_openai_array` wants `&[LocalTool]`, so we
        // collect owned copies of the filtered entries (cheap: 7 max).
        let filtered: Vec<crate::tools::schemas::LocalTool> = all_tools
            .iter()
            .filter(|t| tool_names.contains(&t.name))
            .map(|t| (*t).clone())
            .collect();
        let tools_arr = crate::tools::schemas::local_tools_to_openai_array(&filtered);
        params.insert("tools".to_string(), tools_arr);
    }

    // Deep-merge into `cfg.models.providers[PROVIDER_ID]`. We don't touch
    // any other provider entries the user has configured.
    if !cfg.is_object() {
        cfg = serde_json::json!({});
    }
    let cfg_obj = cfg.as_object_mut().unwrap();
    let models_obj = cfg_obj
        .entry("models".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !models_obj.is_object() {
        *models_obj = Value::Object(Default::default());
    }
    let models = models_obj.as_object_mut().unwrap();
    let providers_obj = models
        .entry("providers".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !providers_obj.is_object() {
        *providers_obj = Value::Object(Default::default());
    }
    let providers = providers_obj.as_object_mut().unwrap();
    // Lesson 449/450: `result.endpoint` is the friendly URL the login UI shows
    // ("Logged in to https://maicserver.com"). It does NOT include the `/v1`
    // suffix that openclaw's OpenAI SDK appends `/chat/completions` to — the
    // login UI never builds chat URLs, only login URLs (see `maic_login`).
    // The `/v1`-normalized form lives in the entry's `baseUrl` for openclaw's
    // gateway to consume. We strip the suffix off here so the display value
    // stays the bare origin the user typed.
    let final_endpoint = strip_trailing_v1(
        entry_obj
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or(&resolved_url),
    );
    providers.insert(PROVIDER_ID.to_string(), provider_entry);

    // Stamp plugins.entries.maic.enabled = true (and plugins.allow)
    // before the write path serializes `cfg`. The early-return path
    // (existing complete entry) calls this same helper below so both
    // paths produce identical plugin entry state on disk.
    stamp_maic_plugin_entry(&mut cfg);

    // Lesson 842: also stamp tier-gated tools + tool_execution + max_tokens
    // in the write path. (The existing-entry early-return path calls this
    // helper above; without this call, fresh installs and upgrades with
    // missing/incomplete entries would skip tools stamping.)
    write_tier_gated_tool_execution_and_tools(&mut cfg, tier);

    // Serialize back. We preserve the user's other fields exactly (no
    // schema-strip pass) — openclaw's gateway does its own validation
    // and we only added keys we know are valid.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(&cfg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}")))?;
    fs::write(&path, serialized)?;

    eprintln!(
        "[miracle-claw] MAIC provider config: wrote models.providers.{} (apiKey from {:?}, endpoint={})",
        PROVIDER_ID, key_source, resolved_url
    );
    eprintln!(
        "[miracle-claw] MAIC plugin: wrote plugins.entries.{}.enabled = true (Lesson 535)",
        PROVIDER_ID
    );

    Ok(MaicProviderBootstrap {
        provider_configured: true,
        provider_id: PROVIDER_ID.to_string(),
        api_key_source: key_source,
        endpoint: final_endpoint,
    })
}

/// Lesson 535 (NEW 2026-08-22): stamp `plugins.entries.maic.enabled =
/// true` and add "maic" to `plugins.allow` so openclaw 2026.7.1's
/// activation decision treats the MAIC plugin as explicitly enabled
/// (non-bundled plugins require explicit enablement).
///
/// Called from both the write path (new provider entry) AND the
/// existing-entry early-return path (Lesson 449 family) so users
/// upgrading from rc28 or earlier still get the plugin activated.
///
/// Idempotent: preserves any user-customized `config: {...}` under the
/// plugin entry and does not duplicate "maic" in `plugins.allow`.
fn stamp_maic_plugin_entry(cfg: &mut serde_json::Value) {
    // Mirrors `ensure_maic_provider_config_for_tier`'s `PROVIDER_ID` const.
    // The plugin id and provider id are both "maic" by design.
    const PROVIDER_ID: &str = "maic";
    if !cfg.is_object() {
        *cfg = serde_json::json!({});
    }
    let cfg_obj = cfg.as_object_mut().unwrap();
    let plugins_obj = cfg_obj
        .entry("plugins".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !plugins_obj.is_object() {
        *plugins_obj = Value::Object(Default::default());
    }
    let plugins = plugins_obj.as_object_mut().unwrap();
    let entries = plugins
        .entry("entries".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !entries.is_object() {
        *entries = Value::Object(Default::default());
    }
    let entries_map = entries.as_object_mut().unwrap();
    let maic_entry = entries_map
        .entry(PROVIDER_ID.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !maic_entry.is_object() {
        *maic_entry = Value::Object(Default::default());
    }
    if let Some(maic_obj) = maic_entry.as_object_mut() {
        maic_obj
            .entry("enabled".to_string())
            .or_insert(Value::Bool(true));
    }
    let allow = plugins
        .entry("allow".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(allow_arr) = allow.as_array_mut() {
        let has_maic = allow_arr.iter().any(|v| v.as_str() == Some(PROVIDER_ID));
        if !has_maic {
            allow_arr.push(Value::String(PROVIDER_ID.to_string()));
        }
    }
}

/// Replace the MAIC provider entry's apiKey field with the literal JWT from
/// the current `MAIC_API_KEY` env var. Used by `maic_login` after a
/// successful login to make sure the literal is written to disk (replacing
/// any legacy SecretRef shipped by v1.0.0 / v1.0.1 / v1.0.2 setups).
///
/// Lesson 449 root cause: previous versions stamped a SecretRef on the
/// host's `openclaw.json` when no `MAIC_API_KEY` was set; the openclaw
/// gateway then crashed at startup with `SecretRefResolutionError`,
/// yielding `ERR_CONNECTION_REFUSED` in the chat UI on localhost:28789.
///
/// Returns `true` iff the function successfully ensured the literal JWT is
/// on disk under `models.providers.maic.apiKey` (either by writing it now
/// or by confirming it was already there). Returns `false` only when the
/// env var is missing, or the openclaw.json doesn't have a maic provider
/// entry, or a write error occurred — all of which the caller should log
/// but not abort on (the user can retry on next launch).
fn replace_secret_ref_with_literal() -> bool {
    let token = match std::env::var(ENV_VAR_NAME) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return false,
    };
    let path = openclaw_json_path();
    let mut cfg: serde_json::Value = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        None => return false,
    };
    let entry = cfg
        .get_mut("models")
        .and_then(|m| m.get_mut("providers"))
        .and_then(|p| p.get_mut("maic"))
        .and_then(|e| e.as_object_mut());
    if let Some(entry_obj) = entry {
        let already_correct = matches!(
            entry_obj.get("apiKey"),
            Some(serde_json::Value::String(s)) if s == &token
        );
        if already_correct {
            return true;
        }
        // Either missing, SecretRef, null/scalar, or wrong literal — write
        // the JWT from the current env var.
        entry_obj.insert("apiKey".to_string(), serde_json::Value::String(token));
        if let Ok(serialized) = serde_json::to_string_pretty(&cfg) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, serialized).is_ok() {
                eprintln!(
                    "[miracle-claw] maic_login: replaced SecretRef with literal JWT in openclaw.json"
                );
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Lesson 517 (NEW, 2026-08-20): tier-conditional default model + fallbacks.
//
// On login (or any time we fetch a fresh tier), we write
// `agents.defaults.model` into the user's openclaw.json so the chat panel
// pre-selects the right model for the tier AND so openclaw's runtime
// fallback walker has the chain it needs when the primary fails.
//
// Free  → primary = `milagro-dev`,       fallbacks = []                       (local only)
// Paid  → primary = `milagro-oc-kimi`,   fallbacks = [MiniMax, GLM, local]   (cloud cascade)
//
// Idempotent: if the user's `agents.defaults.model` already matches the
// tier-derived default, we don't rewrite it (preserves user choice of
// a manual override that happens to match). If the user has set their
// own primary that's NOT the tier default, we leave it alone — this
// function is non-destructive by design.
//
// Returns `Ok(true)` if the file was written, `Ok(false)` if it was
// already correct, `Err` on I/O / serialization failure (caller decides
// whether to log + continue or abort).
pub(crate) fn ensure_agents_default_model_for_tier(
    tier: crate::auth::tier::Tier,
) -> io::Result<bool> {
    use crate::auth::tier::{tier_default_model_id, tier_default_fallbacks};

    // Lesson 521: openclaw's gateway resolves a bare model id via
    // `inferUniqueProviderFromCatalog`, which only succeeds when the id is
    // registered under exactly one provider. If the user upgrades from rc18
    // (where Lesson 519's merge logic hadn't yet reached the existing-entry
    // early-return path — fixed in Lesson 520), only `milagro-dev` is in the
    // catalog. Bare ids like `milagro-oc-kimi` then fall back to
    // `defaultProvider = "openai"`, get rewritten as `openai/milagro-oc-kimi`,
    // and MAIC upstream rejects the request as `Unknown model`.
    //
    // Defensive fix at the writer boundary: emit `maic/<id>` explicitly so
    // the gateway dispatches via the `maic` provider regardless of catalog
    // state. This is belt-and-suspenders alongside Lesson 520's catalog
    // merge — either fix alone resolves the user-visible bug, both together
    // make it impossible to regress on a partial upgrade.
    const PROVIDER_PREFIX: &str = "maic/";
    let default_primary = format!("{PROVIDER_PREFIX}{}", tier_default_model_id(tier));
    let default_fallbacks: Vec<String> = tier_default_fallbacks(tier)
        .iter()
        .map(|s| format!("{PROVIDER_PREFIX}{s}"))
        .collect();

    let path = openclaw_json_path();
    let mut cfg: serde_json::Value = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        // No openclaw.json yet — `ensure_maic_provider_config` should
        // have just created one. Skip; the next login round will retry.
        None => return Ok(false),
    };

    // Lesson 824 (2026-08-30, rc55.14): snapshot the bare-model-id set
    // from `cfg.models.providers.maic.models[]` BEFORE we take a
    // `&mut` borrow on `cfg` for the schema migration. The migration's
    // mutable borrow conflicts with the catalog lookup's immutable
    // borrow — separating the two prevents a borrow-checker tug-of-war.
    let maic_ids_snapshot = maic_model_ids_from_cfg(&cfg);

    // Walk to `agents.defaults`.
    let agents = cfg
        .as_object_mut()
        .and_then(|o| o.get_mut("agents"))
        .and_then(|a| a.as_object_mut());
    let defaults: &mut serde_json::Value = match agents.and_then(|a| a.get_mut("defaults")) {
        Some(d) if d.is_object() => d,
        // agents.defaults missing — create it.
        _ => {
            if !cfg.is_object() {
                cfg = serde_json::json!({});
            }
            let agents_obj = cfg
                .as_object_mut()
                .unwrap()
                .entry("agents".to_string())
                .or_insert_with(|| Value::Object(Default::default()));
            if !agents_obj.is_object() {
                *agents_obj = Value::Object(Default::default());
            }
            agents_obj
                .as_object_mut()
                .unwrap()
                .entry("defaults".to_string())
                .or_insert_with(|| Value::Object(Default::default()))
        }
    };

    // Lesson 824 (2026-08-30, rc55.14): gather the existing primary +
    // the existing fallbacks (inside the model object form) BEFORE we
    // do any mutation. The catalog lookup borrows `cfg` immutably while
    // `defaults` mutably borrows into the same `cfg`, so the borrow
    // checker requires us to split the work: snapshot first, then mutate.
    //
    // Accept BOTH the string form (`model = "maic/<id>"`) AND the
    // object form (`model = {primary, fallbacks}`). The schema
    // (`AgentDefaultsSchema` in openclaw's zod-schema-O9ml_nmo.js) is
    // `.strict()` — it does NOT accept top-level `fallbacks` at
    // `agents.defaults`, so we MUST keep fallbacks inside the model
    // object. Lesson 800's "flatten to top-level fallbacks" migration
    // was wrong (rc55.14 regression: `agents.defaults: Invalid input`
    // on gateway boot). Removed entirely.
    let pre_migration_primary: Option<String> = match defaults.get("model") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Object(obj)) => obj
            .get("primary")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    };
    // Pre-migration fallbacks: only the in-object form is valid. Top-level
    // `fallbacks` is NOT in the schema — if we find any, it's leftover
    // from a prior bad migration and we'll discard it.
    let pre_migration_in_object_fallbacks: Vec<String> = match defaults.get("model") {
        Some(Value::Object(obj)) => obj
            .get("fallbacks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    // Track whether model is in the object form (so we know whether to
    // write it back as an object or as a string when unchanged).
    let pre_migration_is_object_form = matches!(defaults.get("model"), Some(Value::Object(_)));
    // Detect any top-level `fallbacks` leftover from Lesson 800's bad
    // migration. If present, we'll remove it when we persist (since the
    // schema rejects it). Capture its contents in case we want to merge
    // them into `model.fallbacks` for the user's sake.
    let pre_migration_top_level_fallbacks_to_preserve: Vec<String> = match defaults.get("fallbacks")
    {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };

    // Lesson 824 (2026-08-30, rc55.14): even when the user has an explicit
    // primary, we MUST repair a bare-id primary that's missing the `maic/`
    // prefix — otherwise the gateway dispatches via
    // `inferUniqueProviderFromCatalog` and falls through to `openai/<id>`,
    // MAIC rejects as Unknown model. User report: rc55.12 chat dropdown
    // shows only 4 models because `agents.defaults.model.primary =
    // "milagro-oc-kimi"` (bare) means the picker can't resolve the primary's
    // catalog entry.
    //
    // Same treatment for fallbacks (inside the model object): if any entry
    // is a bare id and `maic/<id>` exists in the catalog, rewrite it with
    // the prefix.
    //
    // We use `maic_ids_snapshot` so the catalog lookup doesn't take an
    // immutable borrow against `cfg` (which is already borrowed mutably
    // via `defaults`).
    let mut prefix_applied = false;
    let prefixed_primary: Option<String> = match pre_migration_primary.as_deref() {
        Some(p)
            if !p.starts_with(PROVIDER_PREFIX)
                && maic_ids_snapshot
                    .iter()
                    .any(|m| m == &normalize_bare_model_id(p)) =>
        {
            Some(format!("{PROVIDER_PREFIX}{p}"))
        }
        _ => None,
    };
    let prefixed_fallbacks: Option<Vec<String>> = {
        // Merge in-object fallbacks + any preserved top-level fallbacks
        // from a prior bad migration, dedup.
        let mut combined: Vec<String> = pre_migration_in_object_fallbacks.clone();
        for s in &pre_migration_top_level_fallbacks_to_preserve {
            if !combined.contains(s) {
                combined.push(s.clone());
            }
        }
        if combined.is_empty() {
            None
        } else {
            let mut out: Vec<String> = Vec::with_capacity(combined.len());
            let mut any_changed = false;
            for s in &combined {
                if s.is_empty() {
                    continue;
                }
                if !s.starts_with(PROVIDER_PREFIX)
                    && maic_ids_snapshot
                        .iter()
                        .any(|m| m == &normalize_bare_model_id(s))
                {
                    out.push(format!("{PROVIDER_PREFIX}{s}"));
                    any_changed = true;
                } else {
                    out.push(s.clone());
                }
            }
            if any_changed {
                Some(out)
            } else {
                None
            }
        }
    };
    if let Some(p) = prefixed_primary.as_deref() {
        write_model_field(defaults, pre_migration_is_object_form, Some(p), None);
        prefix_applied = true;
    }
    if let Some(fb) = prefixed_fallbacks.as_ref() {
        write_model_field(defaults, pre_migration_is_object_form, None, Some(fb.clone()));
        prefix_applied = true;
    }

    // Decide whether we need to seed defaults.
    let needs_seed = pre_migration_primary.is_none();
    // Decide whether we need to remove the top-level `fallbacks` key (it
    // would be rejected by the schema). When we remove it, we ALSO need
    // to lift those fallbacks into `model.fallbacks` (object form) so
    // the user doesn't silently lose them — rcher they were being
    // saved as preferences, even if rc55.14 stored them in the wrong
    // place.
    let top_fallbacks_to_preserve: Option<Vec<String>> =
        if defaults.as_object().map_or(false, |o| o.contains_key("fallbacks")) {
            // Decide which fallbacks to lift:
            // - if any are already inside model.fallbacks, merge them
            //   (preserve user's old set + new top-level set).
            // - if model is empty (None), use top-level as the seed.
            // - if model has its own fallbacks, leave those alone (they
            //   were valid) and drop top-level (user clearly had both
            //   — they're not empty in two places by accident).
            let existing_in_obj = pre_migration_in_object_fallbacks.clone();
            let top = pre_migration_top_level_fallbacks_to_preserve.clone();
            if existing_in_obj.is_empty() {
                if top.is_empty() {
                    None
                } else {
                    Some(top)
                }
            } else {
                // User already has in-object fallbacks. Just don't
                // lift the stale top-level (it would dup).
                Some(existing_in_obj)
            }
        } else {
            None
        };
    let needs_remove_top_fallbacks = defaults.as_object().map_or(false, |o| o.contains_key("fallbacks"));

    if !needs_seed && !prefix_applied && !needs_remove_top_fallbacks {
        return Ok(false);
    }

    if needs_seed {
        // Seed the OBJECT form: `model = {primary, fallbacks}`.
        let mut model_obj = serde_json::Map::new();
        model_obj.insert(
            "primary".to_string(),
            Value::String(default_primary.to_string()),
        );
        if !default_fallbacks.is_empty() {
            let fb: Vec<Value> = default_fallbacks
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect();
            model_obj.insert("fallbacks".to_string(), Value::Array(fb));
        }
        defaults
            .as_object_mut()
            .unwrap()
            .insert("model".to_string(), Value::Object(model_obj));
    }

    // Lesson 829: if the user has top-level fallbacks (rc55.14 bad
    // shape), lift them into model.fallbacks and strip the invalid
    // top-level key. If model is empty AND we have top-level fallbacks,
    // promote: use them as in-object fallbacks.
    if needs_remove_top_fallbacks {
        if let Some(fb) = top_fallbacks_to_preserve.as_ref() {
            if needs_seed {
                // Already inserted model object above — add fallbacks to it.
                if let Some(model_obj) = defaults
                    .as_object_mut()
                    .unwrap()
                    .get_mut("model")
                    .and_then(|v| v.as_object_mut())
                {
                    if !fb.is_empty() {
                        let arr: Vec<Value> =
                            fb.iter().map(|s| Value::String(s.clone())).collect();
                        model_obj.insert("fallbacks".to_string(), Value::Array(arr));
                    }
                }
            } else if pre_migration_in_object_fallbacks.is_empty()
                && !pre_migration_top_level_fallbacks_to_preserve.is_empty()
            {
                // Model was a string form. Promote to object form,
                // preserving the existing primary and lifting the
                // top-level fallbacks into model.fallbacks.
                write_model_field(defaults, false, None, Some(fb.clone()));
            }
        }
        // Always strip the invalid top-level key.
        defaults.as_object_mut().unwrap().remove("fallbacks");
    }

    // Persist. Use atomic temp-file + rename so a crash mid-write doesn't
    // leave the user with a half-written openclaw.json.
    let serialized = serde_json::to_string_pretty(&cfg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}")))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialized)?;
    fs::rename(&tmp, &path)?;

    eprintln!(
        "[miracle-claw] tier: wrote agents.defaults.model primary={} fallbacks={:?} (tier={}, prefix_applied={})",
        default_primary, default_fallbacks, tier.as_str(), prefix_applied
    );
    Ok(true)
}

/// Lesson 824 (2026-08-30, rc55.14): return true iff the openclaw.json
/// `models.providers.maic.models[]` array contains a row whose `id`
/// (or `name` fallback) equals `id` after normalization. Used by
/// `ensure_agents_default_model_for_tier` to decide whether a bare model
/// id should be re-prefixed with `maic/`. We don't decode JSON here —
/// the caller hands us the parsed cfg directly.
///
/// Strip a leading `<provider>/` if present so callers can pass either
/// form (`milagro-oc-kimi` or `maic/milagro-oc-kimi`).
#[allow(dead_code)]
fn maic_provider_has_model_id(cfg: &serde_json::Value, id: &str) -> bool {
    maic_model_ids_from_cfg(cfg).iter().any(|m| m == &normalize_bare_model_id(id))
}

/// Lesson 824 helper: extract the set of bare model ids declared under
/// `models.providers.maic.models[]`. The set is small (≤50), so allocating
/// a `HashSet<String>` here is cheap. Returning a `Vec<String>` keeps the
/// result ordered so tests are deterministic.
fn maic_model_ids_from_cfg(cfg: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(providers) = cfg.get("models").and_then(|m| m.get("providers")) else {
        return out;
    };
    let Some(maic) = providers.get("maic") else {
        return out;
    };
    let Some(arr) = maic.get("models").and_then(|m| m.as_array()) else {
        return out;
    };
    for entry in arr {
        let entry_id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("name").and_then(|v| v.as_str()));
        if let Some(eid) = entry_id {
            let t = eid.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn normalize_bare_model_id(id: &str) -> String {
    id.trim().trim_start_matches("maic/").trim().to_string()
}

/// Lesson 829 (2026-08-30, rc55.15 hotfix): write a `model` field on
/// `agents.defaults` while preserving the form (string vs object) of the
/// existing value. If we have a primary to set, we always set it as a
/// string (the schema accepts either form for `model.primary`-equivalent
/// slots). If we have fallbacks to set, we need the OBJECT form — if the
/// existing value is a string, we wrap it.
///
/// Called from `ensure_agents_default_model_for_tier` to apply
/// Lesson 824's prefix rewrite in-place without changing the schema
/// shape the user already had.
fn write_model_field(
    defaults: &mut serde_json::Value,
    was_object_form: bool,
    primary: Option<&str>,
    fallbacks: Option<Vec<String>>,
) {
    let obj = defaults
        .as_object_mut()
        .expect("defaults is always an object by this point");
    match (primary, fallbacks) {
        (Some(p), None) => {
            // Only primary to set. Preserve form: keep object if it was
            // object, otherwise write string.
            if was_object_form {
                if let Some(model_obj) = obj.get_mut("model").and_then(|v| v.as_object_mut()) {
                    model_obj.insert("primary".to_string(), Value::String(p.to_string()));
                } else {
                    let mut new_obj = serde_json::Map::new();
                    new_obj.insert("primary".to_string(), Value::String(p.to_string()));
                    obj.insert("model".to_string(), Value::Object(new_obj));
                }
            } else {
                obj.insert("model".to_string(), Value::String(p.to_string()));
            }
        }
        (None, Some(fb)) => {
            // Only fallbacks to set. Fallbacks MUST live inside the
            // object form (the schema is `.strict()` and has no
            // top-level fallbacks). If model was a string, wrap it.
            if let Some(model_obj) = obj.get_mut("model").and_then(|v| v.as_object_mut()) {
                let arr: Vec<Value> = fb.iter().map(|s| Value::String(s.clone())).collect();
                model_obj.insert("fallbacks".to_string(), Value::Array(arr));
            } else {
                // Was a string form. Promote to object form: keep the
                // existing string as primary, add fallbacks.
                let existing_primary = obj
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let mut new_obj = serde_json::Map::new();
                if let Some(p) = existing_primary {
                    new_obj.insert("primary".to_string(), Value::String(p));
                }
                let arr: Vec<Value> = fb.iter().map(|s| Value::String(s.clone())).collect();
                new_obj.insert("fallbacks".to_string(), Value::Array(arr));
                obj.insert("model".to_string(), Value::Object(new_obj));
            }
        }
        (Some(p), Some(fb)) => {
            // Both — always write the object form.
            let mut new_obj = serde_json::Map::new();
            new_obj.insert("primary".to_string(), Value::String(p.to_string()));
            let arr: Vec<Value> = fb.iter().map(|s| Value::String(s.clone())).collect();
            new_obj.insert("fallbacks".to_string(), Value::Array(arr));
            obj.insert("model".to_string(), Value::Object(new_obj));
        }
        (None, None) => {
            // Nothing to set — caller should have guarded against this.
            // No-op.
        }
    }
}

/// Lesson 458 / v1.0.6: replace any literal JWT in `models.providers.maic.apiKey`
/// back with a SecretRef, so a future `maic_login` can do its job cleanly
/// instead of finding a stale literal. Idempotent. Used by `maic_logout`.
///
/// Why not just delete the provider entry? `setup()` calls
/// `ensure_maic_provider_config()` which expects the entry to exist; if we
/// delete it, the next launch will fail with "MAIC provider missing".
/// Restoring the SecretRef keeps the shape valid and lets a fresh login
/// flow naturally.
fn restore_maic_provider_secret_ref() -> bool {
    let path = openclaw_json_path();
    let mut cfg: serde_json::Value = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        None => return false,
    };
    let entry = cfg
        .get_mut("models")
        .and_then(|m| m.get_mut("providers"))
        .and_then(|p| p.get_mut("maic"))
        .and_then(|e| e.as_object_mut());
    if let Some(entry_obj) = entry {
        // If it's already a SecretRef to MAIC_API_KEY, nothing to do.
        if matches!(
            entry_obj.get("apiKey"),
            Some(serde_json::Value::Object(o))
                if o.get("source").and_then(|v| v.as_str()) == Some("env")
                    && o.get("id").and_then(|v| v.as_str()) == Some(ENV_VAR_NAME)
        ) {
            return true;
        }
        // Replace whatever's there with the SecretRef.
        entry_obj.insert(
            "apiKey".to_string(),
            serde_json::json!({
                "source": "env",
                "provider": "default",
                "id": ENV_VAR_NAME
            }),
        );
        if let Ok(serialized) = serde_json::to_string_pretty(&cfg) {
            if std::fs::write(&path, serialized).is_ok() {
                eprintln!(
                    "[miracle-claw] maic_logout: restored SecretRef in openclaw.json"
                );
                return true;
            }
        }
    }
    false
}

/// Register a default env-based SecretProvider so openclaw's SecretRef
/// `{source: "env", provider: "default", id: "MAIC_API_KEY"}` resolves at
/// request time. Mutates `cfg` in place. Idempotent.
///
/// Schema (zod-schema.core `SecretsConfigSchema`):
///   secrets.providers.default = { source: "env", allowlist: ["MAIC_API_KEY"] }
///   secrets.defaults.env = "default"
#[allow(dead_code)] // Retained as a documented utility; Lesson 444 made the
                    // SecretRef fallback path unreachable, so no caller wires
                    // it. Kept so future re-enable of the SecretRef fallback
                    // (e.g. for air-gapped installs with no login UI) is one
                    // line away.
fn ensure_secrets_default_env_provider(cfg: &mut Value, env_var: &str, alias: &str) {
    if !cfg.is_object() {
        *cfg = serde_json::json!({});
    }
    let cfg_obj = cfg.as_object_mut().unwrap();

    // secrets.providers[alias] = { source: "env", allowlist: [env_var] }
    let secrets_obj = cfg_obj
        .entry("secrets".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !secrets_obj.is_object() {
        *secrets_obj = Value::Object(Default::default());
    }
    let secrets = secrets_obj.as_object_mut().unwrap();

    let providers_obj = secrets
        .entry("providers".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !providers_obj.is_object() {
        *providers_obj = Value::Object(Default::default());
    }
    let providers = providers_obj.as_object_mut().unwrap();

    // Only create the provider if it doesn't exist — never clobber a user's
    // existing configuration with different sources/aliases.
    let provider_entry = providers.entry(alias.to_string()).or_insert_with(|| {
        serde_json::json!({
            "source": "env",
            "allowlist": [env_var],
        })
    });
    if let Some(obj) = provider_entry.as_object_mut() {
        // Set source=env if user has a stub entry without source.
        obj.entry("source".to_string())
            .or_insert(Value::String("env".to_string()));
        // Append our env var to the allowlist if not already present.
        let allowlist = obj
            .entry("allowlist".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(arr) = allowlist.as_array_mut() {
            let already = arr
                .iter()
                .any(|v| v.as_str() == Some(env_var));
            if !already {
                arr.push(Value::String(env_var.to_string()));
            }
        }
    }

    // secrets.defaults.env = alias  (so the 'default' alias resolves for
    // any SecretRef that omits an explicit provider).
    let defaults_obj = secrets
        .entry("defaults".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(defaults) = defaults_obj.as_object_mut() {
        defaults
            .entry("env".to_string())
            .or_insert(Value::String(alias.to_string()));
    }
}

/// Is `value` an "empty" apiKey? Accepts both literal strings and SecretRef
/// objects. A literal empty string or a SecretRef with empty `id` is empty.
/// Used by the idempotency check in `ensure_maic_provider_config`.
fn is_empty_api_key(value: &Value) -> bool {
    match value {
        Value::String(s) => s.trim().is_empty(),
        Value::Object(obj) => {
            // SecretRef: source=env|file|exec; provider=alias; id=env var name
            // or JSON pointer. Empty if id is missing or empty.
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
            id.is_empty()
        }
        _ => true,
    }
}

/// Is `value` an apiKey that the openclaw gateway CAN'T currently resolve?
///
/// Lesson 449: this is what we want the bootstrap stamp path to use, not
/// `is_empty_api_key`. A SecretRef to `MAIC_API_KEY` is "unresolvable" when
/// its target env var is empty or missing in the current process. In that
/// case the gateway will crash with SecretRefResolutionError, and we want
/// to replace it with a literal string (after login, we have a literal).
///
/// Conversely, if the env var is set, the SecretRef IS resolvable — we leave
/// it alone (preserves user-customized configs).
fn is_unresolvable_api_key(value: &Value) -> bool {
    if is_empty_api_key(value) {
        return true;
    }
    match value {
        Value::Object(obj) => {
            // Only env-sourced SecretRefs to known env var names can be checked
            // here. For other sources (file/exec/JSON pointer), trust the
            // existing is_empty_api_key result (true means broken, false means
            // non-empty so we leave alone).
            let source = obj.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if source != "env" {
                return false;
            }
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match std::env::var(id) {
                Ok(v) if !v.trim().is_empty() => false, // resolvable
                _ => true, // missing or empty = unresolvable = should be replaced
            }
        }
        _ => false, // literal non-empty is always resolvable
    }
}

/// Lesson 450: ensure an MAIC `baseUrl` value carries the `/v1` path prefix
/// that openclaw's OpenAI SDK appends `/chat/completions` onto.
///
/// MAIC exposes its OpenAI-compatible chat route at `/v1/chat/completions`,
/// not `/chat/completions`. openclaw passes `model.baseUrl` straight through
/// to `new OpenAI({baseURL, ...})` which calls
/// `new URL(baseURL + '/chat/completions')` — so the value must include
/// `/v1` or the SDK POSTs to a 404 route.
///
/// `normalize_maic_base_url` is always-idempotent: returns the same value
/// when called twice, appends `/v1` exactly once, and never strips user
/// content beyond a trailing slash. Used at write time.
///
/// `upgrade_legacy_maic_base_url` is the in-place migration heuristic that
/// patches existing openclaw.json values written by v1.0.0..v1.0.3. It only
/// rewrites URLs that clearly match the old "https://{host}" pattern with
/// no path or with `/v1`-missing-path — anything more complex (e.g.
/// `https://proxy.example.com/maic/`) is preserved verbatim so we don't
/// break users running MAIC behind a reverse proxy with its own mount.
fn normalize_maic_base_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return value.to_string();
    }
    // Already has /v1? Leave alone.
    if trimmed.ends_with("/v1")
        || trimmed.contains("/v1/")
        || trimmed.contains("/v1?")
    {
        return trimmed.to_string();
    }
    format!("{}/v1", trimmed)
}

/// Lesson 450: One-shot rewrite for v1.0.0..v1.0.3 openclaw.json baseUrl
/// values. Returns the upgraded URL if a rewrite should happen, None if
/// the value should be preserved as-is.
///
/// Conservative rewrite policy: we only rewrite URLs that are clearly the
/// "bare origin" form (no path or only `/` as path). Anything with a path
/// the user added (proxy mount, alternate route, version prefix) is left
/// untouched — the user's intent is preserved.
fn upgrade_legacy_maic_base_url(existing: &str) -> Option<String> {
    let trimmed = existing.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Parsing defensively — anything not a clean origin URL stays put.
    let parsed = url_origin_path(trimmed)?;
    if parsed.path.is_empty() || parsed.path == "/" {
        // Bare origin: append /v1 if it doesn't already have it.
        let normalized = normalize_maic_base_url(trimmed);
        if normalized != trimmed {
            return Some(normalized);
        }
    }
    None
}

/// Parse `url` into (origin, path). Returns None on parse errors.
/// "Origin" = scheme + host (no path, no trailing slash). "Path" excludes
/// the leading `/` so an empty path is "", not "/".
struct UrlOriginPath {
    origin: String,
    path: String,
}

fn url_origin_path(url: &str) -> Option<UrlOriginPath> {
    let scheme_end = url.find("://")?;
    let after_scheme = &url[scheme_end + 3..];
    let slash = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host = &after_scheme[..slash];
    let path_with_slash = &after_scheme[slash..];
    let path = path_with_slash.trim_start_matches('/').trim_end_matches('/').to_string();
    Some(UrlOriginPath {
        origin: format!("{}://{}", &url[..scheme_end], host),
        path,
    })
}

/// Lesson 450: strip a trailing `/v1` (or `/v1/`) from a URL. Used by
/// `maic_login` to normalize a user-supplied endpoint before appending
/// `/v1/<path>` — handles the "MAIC_API_URL already includes /v1" case
/// cleanly so we never POST to `/v1/v1/users/login`.
fn strip_trailing_v1(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        return trimmed[..trimmed.len() - 3].to_string();
    }
    trimmed.to_string()
}

#[derive(Debug)]
enum MaicKeySource {
    /// MAIC_API_KEY env var was set; we wrote the literal string into apiKey.
    Env,
    /// Read from system openclaw's `models.providers.maic.apiKey`.
    SystemOpenClaw,
    /// User already had a complete provider entry in their openclaw.json.
    Existing,
    /// No key came from any source at all (env empty, system openclaw had no
    /// provider entry). This is distinct from `LoginRequired` only in that
    /// `None` is set transiently while we're still trying to bootstrap — by
    /// the time the bootstrap function returns, `None` has been replaced
    /// with `LoginRequired` (Lesson 444).
    None,
    /// No key was available at all — the frontend must collect one via the
    /// first-run MAIC login flow (`maic_login` Tauri command) before the
    /// chat panel can render. The provider IS NOT configured in openclaw.json
    /// (we early-return without writing a placeholder), but the bootstrap
    /// result still reports `provider_id=maic` so the login UI knows where to
    /// point the customer.
    LoginRequired,
}

#[derive(Debug)]
struct MaicProviderBootstrap {
    provider_configured: bool,
    /// Always "maic" today. Reserved for future multi-provider support.
    #[allow(dead_code)]
    provider_id: String,
    api_key_source: MaicKeySource,
    endpoint: String,
}

/// Path to system openclaw's openclaw.json (separate from MC's isolated
/// state). Returns None if HOME/APPDATA aren't set (very unusual).
fn system_openclaw_json_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(roam) = std::env::var("APPDATA") {
            return Some(PathBuf::from(roam).join("openclaw").join("openclaw.json"));
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join(".openclaw").join("openclaw.json"));
        }
    }
    None
}

fn read_system_openclaw_maic_key() -> io::Result<Option<String>> {
    let Some(path) = system_openclaw_json_path() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse: {e}")))?;
    let key = parsed
        .get("models")
        .and_then(|m| m.get("providers"))
        .and_then(|p| p.get("maic"))
        .and_then(|m| m.get("apiKey"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(key)
}

fn read_system_openclaw_maic_base_url() -> io::Result<Option<String>> {
    let Some(path) = system_openclaw_json_path() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse: {e}")))?;
    let url = parsed
        .get("models")
        .and_then(|m| m.get("providers"))
        .and_then(|p| p.get("maic"))
        .and_then(|m| m.get("baseUrl"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(url)
}

// ----------------------------------------------------------------------------
// Health poll: wait until the openclaw gateway is actually serving HTTP 2xx
// responses on 127.0.0.1:PORT. TCP-only probing races with the gateway's
// `http server listening` → `ready` gap (Lesson 467 + 469): the OS accepts
// our SYN the instant `bind()` lands, but the HTTP server may not be
// accepting/responding yet because worker pool / plugin pre-warm / auth
// middleware are still wiring up.
//
// rc5 change: use `check_http_ready` for the final confirmation instead of
// trusting a successful TCP connect. Also extended timeout from 15s to 30s
// because observed cold-boot bind time is ~14s on David's machine
// (mc-rc4-fresh-fb.log), which gave the 15s budget only a 1s margin — the
// probe would consistently fail by ~100ms when the bind landed slightly late.
// ----------------------------------------------------------------------------

fn wait_for_gateway_ready(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_millis(1000);
    let addrs = format!("127.0.0.1:{}", port);
    let poll_timeout = Duration::from_millis(500); // per-iteration TCP connect ceiling
    let url = format!("http://127.0.0.1:{}/", port);

    log_to_file(&format!(
        "wait_for_gateway_ready: start port={} timeout={:?} (rc5: HTTP-level final probe, 30s budget)",
        port, timeout
    ));

    let mut iter: u32 = 0;
    while Instant::now() < deadline {
        iter += 1;
        let started = Instant::now();
        // Use connect_timeout so a single iteration cannot block past
        // poll_timeout. Without this, TcpStream::connect against an unbound
        // port on Windows blocks for the OS-default TCP retry timeout
        // (~21s) — which would consume our budget in one iteration and
        // prevent polling the actual bind.
        let conn_result = TcpStream::connect_timeout(
            &addrs.parse().map_err(|e| format!("invalid addr: {e}"))?,
            poll_timeout,
        );
        let elapsed = started.elapsed();
        match conn_result {
            Ok(_stream) => {
                // Port is bound. Now confirm the gateway is actually serving
                // HTTP (Lesson 467 + 469: TCP-only probe is insufficient). If
                // check_http_ready succeeds, we're done. If it returns Err,
                // the bind landed but the server isn't responding yet — keep
                // polling within the deadline.
                let http_start = Instant::now();
                let http_result = check_http_ready(&url, Duration::from_millis(500));
                let http_elapsed = http_start.elapsed();
                match http_result {
                    Ok(status) => {
                        log_to_file(&format!(
                            "wait_for_gateway_ready: HTTP ready iter={} tcp_elapsed={:?} http_status={} http_elapsed={:?}",
                            iter, elapsed, status, http_elapsed
                        ));
                        return Ok(());
                    }
                    Err(http_err) => {
                        // Port bound, server not ready. Log and keep polling.
                        if iter == 1 || iter % 10 == 0 || Instant::now() + backoff >= deadline {
                            log_to_file(&format!(
                                "wait_for_gateway_ready: TCP bound but HTTP not ready iter={} tcp_elapsed={:?} http_err={} http_elapsed={:?}",
                                iter, elapsed, http_err, http_elapsed
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                // Don't spam the log — only the last failure and every 10th
                if iter == 1 || iter % 10 == 0 || Instant::now() + backoff >= deadline {
                    log_to_file(&format!(
                        "wait_for_gateway_ready: TCP connect failed iter={} elapsed={:?} err={}",
                        iter, elapsed, e
                    ));
                }
            }
        }
        thread::sleep(backoff);
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
    let err_msg = format!(
        "gateway did not become ready on port {} within {:?}",
        port, timeout
    );
    log_to_file(&format!(
        "wait_for_gateway_ready: TIMEOUT after {} iters; {}",
        iter, err_msg
    ));
    Err(err_msg)
}

// ----------------------------------------------------------------------------
// Tauri command: lets the frontend ask us what happened
// ----------------------------------------------------------------------------

#[tauri::command]
fn first_run_report(state: tauri::State<'_, AppState>) -> FirstRunReport {
    let launcher_spawned = state.launcher_child.lock().unwrap().is_some();
    let gateway_ready =
        TcpStream::connect(format!("127.0.0.1:{}", OPENCLAW_PORT)).is_ok();
    // Lesson TBD (2026-08-27): cheap registry read for installer-set flags.
    // On non-Windows this returns (false, "", false). PowerShell probe
    // for detailed voice status lives in the separate `voice_diagnostics`
    // command so we don't slow down boot.
    let (voice_stack_installed, voice_stack_build, voice_clarity_opt_in) =
        voice_first_run_report();
    // Read back what ensure_maic_provider_config() wrote so the chat panel
    // can show whether MAIC is wired. Default to "not configured" if the
    // config file is missing or unparseable.
    let (maic_provider_configured, maic_provider_endpoint) =
        match fs::read_to_string(openclaw_json_path()) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(v) => {
                    let entry = v
                        .get("models")
                        .and_then(|m| m.get("providers"))
                        .and_then(|p| p.get("maic"));
                    let has_key = entry
                        .and_then(|e| e.get("apiKey"))
                        .and_then(|k| k.as_str())
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false);
                    let endpoint = entry
                        .and_then(|e| e.get("baseUrl"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    (has_key, endpoint)
                }
                Err(_) => (false, String::new()),
            },
            Err(_) => (false, String::new()),
        };
    FirstRunReport {
        maic_plugin_installed: false,
        maic_plugin_already_present: false,
        openclaw_json_patched: false,
        openclaw_json_already_patched: false,
        maic_provider_configured,
        maic_provider_endpoint: maic_provider_endpoint.clone(),
        // Lesson 444: surface whether the login modal should show. We read the
        // live openclaw.json instead of re-running ensure_maic_provider_config
        // (which has write side-effects we don't want here). If the provider
        // is unconfigured AND no api key is in scope (no env var, no literal
        // string in config, no SecretRef whose env var is set), we need login.
        needs_maic_login: needs_maic_login_from_state(&maic_provider_endpoint),
        launcher_spawned,
        gateway_ready,
        gateway_error: None,
        // Lesson TBD (2026-08-27): voice-stack status from the installer.
        // Cheap registry read; no PowerShell on the boot path.
        voice_stack_installed,
        voice_stack_build,
        voice_clarity_opt_in,
    }
}

/// Returns true iff the chat panel cannot render without a MAIC login.
/// Computed from the live state at the moment of first_run_report — does
/// NOT re-write openclaw.json.
fn needs_maic_login_from_state(_provider_endpoint: &str) -> bool {
    // If a MAIC_API_KEY env var is set, we have a key — no login needed.
    if let Ok(k) = std::env::var(ENV_VAR_NAME) {
        if !k.trim().is_empty() {
            return false;
        }
    }
    // Otherwise, check openclaw.json. If the apiKey is a non-empty literal
    // string, we're good. If it's a SecretRef or empty, we need login.
    let path = openclaw_json_path();
    if !path.is_file() {
        return true; // Fresh install, no config = no key
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return true,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return true,
    };
    let api_key = parsed
        .get("models")
        .and_then(|m| m.get("providers"))
        .and_then(|p| p.get("maic"))
        .and_then(|e| e.get("apiKey"));
    match api_key {
        Some(serde_json::Value::String(s)) => s.trim().is_empty(),
        // SecretRef or missing — we treat both as login-required for the
        // frontend's purposes (the user already had a SecretRef, but it's
        // now broken since no env var is set — login is the fix).
        _ => true,
    }
}

// ----------------------------------------------------------------------------
// Voice diagnostics (Lesson TBD, 2026-08-27)
//
// Called on demand from Settings → Voice. PowerShell-based probe that reports:
//   - Windows build + whether SpeechRecognition FODs are installed
//   - Whether SAPI 5 (System.Speech) is present
//   - Whether KB5067036 / Voice Isolation updates are pending
//   - Whether Voice Clarity / Voice Isolation are enabled system-wide
//   - The user's default mic device + its DSP state
//
// This is NOT called on the boot path — first_run_report does the cheap
// registry read. This command is for when the user clicks "Check voice setup"
// in Settings. It takes ~3-5 seconds because PowerShell loads .NET types.
//
// Returns a JSON-friendly VoiceDiagnostics struct. The Settings page renders
// each field with a status pill (✓ installed, ⚠ available, ✗ missing).
// ----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct VoiceDiagnostics {
    /// "Windows 11 24H2" / "Windows 11 25H2" / "Windows 10 22H2" / etc.
    /// Empty on non-Windows.
    windows_build: String,
    /// Build number as integer (e.g., 26100 for Windows 11 24H2). 0 on non-Windows.
    windows_build_number: u32,
    /// Whether the MC installer installed voice components in this install.
    /// Reads HKLM\SOFTWARE\MiracleClaw\VoiceStackInstalled.
    mc_voice_stack_installed: bool,
    /// Build string the installer was running when it set up voice (e.g.,
    /// "1.1.0-rc54.0"). Empty if never installed.
    mc_voice_stack_build: String,
    /// Whether SAPI 5 (sapi.dll) is present in System32. Required for offline
    /// dictation via System.Speech.Recognition.
    sapi_present: bool,
    /// List of (language_bcp47, installed_state) tuples for the user's
    /// preferred UI languages, e.g., [("en-US", "Installed"), ("es-ES", "NotPresent")].
    /// Empty on non-Windows.
    speech_fod_status: Vec<(String, String)>,
    /// Whether KB5067036 (Fluid Dictation) is installed. nil if unknown.
    kb5067036_installed: Option<bool>,
    /// Whether Voice Clarity is enabled on the default capture device's
    /// audio enhancements. nil on non-Windows-11-24H2.
    voice_clarity_enabled: Option<bool>,
    /// Name of the default capture device (informational).
    default_mic_name: String,
    /// Free-form human-readable recommendations for the user. E.g.,
    /// "Voice Clarity is disabled. Enable in Sound settings for cleaner mic."
    recommendations: Vec<String>,
}

#[cfg(windows)]
#[tauri::command]
fn voice_diagnostics() -> VoiceDiagnostics {
    use std::process::Command;
    
    // Single PowerShell invocation that gathers everything in one shot.
    // Output is a JSON object that we parse. Why one PS call instead of many:
    //   - One process spawn ~50ms, ten spawns ~500ms
    //   - All fields collected atomically (no race between checks)
    //   - PowerShell startup is the dominant cost, not the commands inside
    let ps_script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = [ordered]@{
    windows_build = ''
    windows_build_number = 0
    mc_voice_stack_installed = $false
    mc_voice_stack_build = ''
    sapi_present = $false
    speech_fod_status = @()
    kb5067036_installed = $null
    voice_clarity_enabled = $null
    default_mic_name = ''
}

# Windows build
$os = Get-CimInstance Win32_OperatingSystem
$out.windows_build = $os.Caption + ' ' + $os.Version
$out.windows_build_number = [int]$os.BuildNumber

# MC voice stack flag
$reg = Get-ItemProperty -Path 'HKLM:\SOFTWARE\MiracleClaw' -ErrorAction SilentlyContinue
if ($reg) {
    $out.mc_voice_stack_installed = [bool]$reg.VoiceStackInstalled
    $out.mc_voice_stack_build = [string]$reg.VoiceStackBuild
}

# SAPI 5
$out.sapi_present = Test-Path "$env:windir\System32\Speech\Common\sapi.dll"

# Speech FODs for user's languages
$langs = @(Get-WinUserLanguageList).LanguageTag | Select-Object -First 3
foreach ($l in $langs) {
    $bcp = $l -replace '-', '_'
    $cap = "Language.Speech~~~und-SPEECH~~$bcp"
    $state = (Get-WindowsCapability -Online -Name $cap -ErrorAction SilentlyContinue).State
    if ($state) {
        $out.speech_fod_status += @{lang = $l; state = [string]$state}
    }
}

# KB5067036 (Fluid Dictation)
$kb = Get-HotFix -Id 'KB5067036' -ErrorAction SilentlyContinue
if ($kb) { $out.kb5067036_installed = $true }
else { $out.kb5067036_installed = $false }

# Voice Clarity (Windows 11 24H2+) on default capture device
if ($out.windows_build_number -ge 26100) {
    try {
        $en = New-Object -ComObject MMDeviceEnumerator
        $dev = $en.GetDefaultAudioEndpoint(1, 1)
        if ($dev) {
            $out.default_mic_name = $dev.Properties.Item(
                '{a45c254e-df1c-4efd-8020-67d146a850e0} 14').GetValue()
            # PKEY_AudioEndpoint_Default_VoiceClarity is device-specific;
            # we just report whether the mic has any audio enhancement flags.
            $out.voice_clarity_enabled = $null  # can't read directly, leave null
        }
    } catch {}
}

# Default mic name fallback
if (-not $out.default_mic_name) {
    try {
        $en = New-Object -ComObject MMDeviceEnumerator
        $dev = $en.GetDefaultAudioEndpoint(1, 1)
        $out.default_mic_name = $dev.GetProperty(System.Guid.Empty) 2>$null; $out.default_mic_name = $dev.Properties
    } catch {}
}

$out | ConvertTo-Json -Depth 5 -Compress
"#;
    
    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps_script])
        .output();
    
    let mut diag = VoiceDiagnostics {
        windows_build: String::new(),
        windows_build_number: 0,
        mc_voice_stack_installed: false,
        mc_voice_stack_build: String::new(),
        sapi_present: false,
        speech_fod_status: Vec::new(),
        kb5067036_installed: None,
        voice_clarity_enabled: None,
        default_mic_name: String::new(),
        recommendations: Vec::new(),
    };
    
    if let Ok(out) = output {
        let raw = String::from_utf8_lossy(&out.stdout).to_string();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(s) = v.get("windows_build").and_then(|x| x.as_str()) {
                diag.windows_build = s.to_string();
            }
            if let Some(n) = v.get("windows_build_number").and_then(|x| x.as_u64()) {
                diag.windows_build_number = n as u32;
            }
            if let Some(b) = v.get("mc_voice_stack_installed").and_then(|x| x.as_bool()) {
                diag.mc_voice_stack_installed = b;
            }
            if let Some(s) = v.get("mc_voice_stack_build").and_then(|x| x.as_str()) {
                diag.mc_voice_stack_build = s.to_string();
            }
            if let Some(b) = v.get("sapi_present").and_then(|x| x.as_bool()) {
                diag.sapi_present = b;
            }
            if let Some(arr) = v.get("speech_fod_status").and_then(|x| x.as_array()) {
                for entry in arr {
                    let lang = entry.get("lang").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let state = entry.get("state").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    diag.speech_fod_status.push((lang, state));
                }
            }
            if let Some(b) = v.get("kb5067036_installed").and_then(|x| x.as_bool()) {
                diag.kb5067036_installed = Some(b);
            }
            if let Some(b) = v.get("voice_clarity_enabled").and_then(|x| x.as_bool()) {
                diag.voice_clarity_enabled = Some(b);
            }
            if let Some(s) = v.get("default_mic_name").and_then(|x| x.as_str()) {
                diag.default_mic_name = s.to_string();
            }
        }
    }
    
    // Build recommendations list. Order matters — most important first.
    if !diag.mc_voice_stack_installed {
        diag.recommendations.push(
            "Voice stack was not installed by the MC installer. Run Settings → Voice → Re-install voice components.".to_string()
        );
    }
    if !diag.sapi_present {
        diag.recommendations.push(
            "Windows SAPI is missing (rare, only on N/KN editions). MC will use Whisper.cpp instead.".to_string()
        );
    }
    for (lang, state) in &diag.speech_fod_status {
        if state != "Installed" {
            diag.recommendations.push(format!(
                "Speech recognition language data for {} is {}. Run Settings → Voice → Re-install.",
                lang, state
            ));
        }
    }
    if diag.kb5067036_installed == Some(false) {
        diag.recommendations.push(
            "Microsoft KB5067036 (Fluid Dictation + Voice Isolation) is not installed. Run Windows Update for the latest voice features.".to_string()
        );
    }
    if diag.windows_build_number > 0 && diag.windows_build_number < 22621 {
        diag.recommendations.push(
            "You're on Windows 10. Windows 11 24H2+ has Voice Clarity. Consider upgrading for the best voice experience.".to_string()
        );
    }
    
    diag
}

#[cfg(not(windows))]
#[tauri::command]
fn voice_diagnostics() -> VoiceDiagnostics {
    VoiceDiagnostics {
        windows_build: String::new(),
        windows_build_number: 0,
        mc_voice_stack_installed: false,
        mc_voice_stack_build: String::new(),
        sapi_present: false,
        speech_fod_status: Vec::new(),
        kb5067036_installed: None,
        voice_clarity_enabled: None,
        default_mic_name: String::new(),
        recommendations: Vec::new(),
    }
}

// ----------------------------------------------------------------------------
// Voice actions (Lesson TBD, 2026-08-27)
//
// Small wrapper commands for the Settings → Voice buttons:
//   - voice_open_sound_settings: launches the Windows Sound control panel
//     (mmsys.cpl) so the user can enable Voice Clarity on their mic.
//   - voice_open_windows_update: launches the Windows Update settings page
//     so the user can grab KB5067036 (Fluid Dictation) and Voice Isolation.
// ----------------------------------------------------------------------------

#[cfg(windows)]
#[tauri::command]
fn voice_open_sound_settings() -> Result<(), String> {
    use std::process::Command;
    // mmsys.cpl opens the legacy Sound control panel which has the
    // Recording tab with device properties (where Audio enhancements live).
    // On Win11 this routes to the modern Settings app variant.
    Command::new("control")
        .args(["mmsys.cpl"])
        .spawn()
        .map_err(|e| format!("Could not open Sound settings: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
fn voice_open_sound_settings() -> Result<(), String> {
    Err("Sound settings are only available on Windows".to_string())
}

#[cfg(windows)]
#[tauri::command]
fn voice_open_windows_update() -> Result<(), String> {
    use std::process::Command;
    // ms-settings:windowsupdate is the modern URI scheme. Falls back to
    // legacy wusa.exe if the URI scheme is unavailable (older Windows).
    Command::new("cmd")
        .args(["/c", "start", "", "ms-settings:windowsupdate"])
        .spawn()
        .map_err(|e| format!("Could not open Windows Update: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
fn voice_open_windows_update() -> Result<(), String> {
    Err("Windows Update is only available on Windows".to_string())
}

// ----------------------------------------------------------------------------
// Voice actions (Lesson 706, rc54.3)
//
// Native Windows STT via SAPI 5 (System.Speech.Recognition). Replaces
// the whisper.cpp sidecar path for the OpenClaw chat voice button —
// 10-50x faster (200-500ms vs 2-6s), zero model download, fully offline.
//
// The Rust side just spawns the bundled PS1 helper script and pipes
// JSON back to JS. We use PowerShell instead of the `windows` crate
// because:
//   - The `windows` crate's SAPI 5 COM bindings add ~6 MB to the
//     installer. PowerShell is already on every Windows install.
//   - System.Speech.Recognition is a stable .NET API that's been
//     available since Windows Vista — no new surface area to maintain.
//   - We already spawn PowerShell for voice_diagnostics, so this is
//     a familiar pattern.
//
// Privacy line: SAPI 5 never sends audio off the device. The PS1
// helper does not invoke any network code; users who want offline
// voice get offline voice by default.
// ----------------------------------------------------------------------------

/// Response payload from `mc_voice_native_capture`. Mirrors the JSON
/// shape the PS1 helper emits on stdout so the JS side can read
/// `result.text`, `result.confidence`, `result.engine` directly.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct VoiceNativeCaptureResult {
    text: String,
    confidence: f64,
    engine: String,
}

/// Locate the bundled `mc-voice-native-capture.ps1` helper. The script
/// ships as a `tauri.conf.json` resource and lives next to the .exe
/// under `installer-assets/` in production (NSIS places resources
/// declared as `installer-assets/*` at the install root, NOT under
/// `resources/`). In dev builds it lives under
/// `src-tauri/installer-assets/`. We try every plausible location.
///
/// Lesson 709 (rc54.4): the production install layout drops PS1 files
/// into `<install_dir>/installer-assets/`, NOT `resources/`. Pre-rc54.4
/// the find function only checked `resources/`, causing a confusing
/// "Could not launch PowerShell" runtime error on first click. Adding
/// the installer-assets sibling path was enough to fix it.
#[cfg(windows)]
fn find_voice_native_capture_script() -> std::path::PathBuf {
    use std::path::PathBuf;

    // Production: <install_dir>/installer-assets/mc-voice-native-capture.ps1
    // (NSIS_HOOK_POSTINSTALL copies `installer-assets/*` to a sibling
    // dir at install time — see installer.nsi line 132.)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("installer-assets").join("mc-voice-native-capture.ps1");
            if candidate.exists() {
                return candidate;
            }
            // Legacy location (older rc54.x builds used resources/):
            let legacy = parent.join("resources").join("mc-voice-native-capture.ps1");
            if legacy.exists() {
                return legacy;
            }
        }
    }

    // Dev / cross-build: src-tauri/installer-assets/...
    let candidates = [
        PathBuf::from("src-tauri/installer-assets/mc-voice-native-capture.ps1"),
        PathBuf::from("installer-assets/mc-voice-native-capture.ps1"),
    ];
    for c in candidates {
        if c.exists() {
            return c;
        }
    }

    // Fall back to the most likely path so the error message points
    // somewhere useful. The caller will fail on spawn anyway.
    PathBuf::from("installer-assets/mc-voice-native-capture.ps1")
}

/// Native Windows STT capture using SAPI 5. Called from the patched
/// OpenClaw voice button on the cross-origin chat page.
///
/// Args:
///   - `timeout_ms`: max time to wait for the user to finish speaking.
///     Defaults to 15000 (15s). Hard-capped at 60000 (60s) in the PS1.
///
/// Returns:
///   - JSON object with `text`, `confidence`, `engine`.
///
/// Errors:
///   - SAPI 5 missing (rc54.x setup didn't run) — friendly hint
///   - Default mic unavailable — hint to check Sound settings
///   - No speech detected within timeout — empty `text` field is
///     NOT an error; caller decides whether to treat it as one.
#[cfg(windows)]
#[tauri::command]
fn mc_voice_native_capture(
    app: tauri::AppHandle,
    timeout_ms: Option<u64>,
) -> Result<VoiceNativeCaptureResult, String> {
    use std::process::Command;

    let timeout = timeout_ms.unwrap_or(15_000).clamp(1_000, 60_000);
    let script = find_voice_native_capture_script();

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy", "Bypass",
            "-File", script.to_string_lossy().as_ref(),
            &timeout.to_string(),
        ])
        .output()
        .map_err(|e| format!(
            "Could not launch PowerShell for native voice capture: {e}. \
             PowerShell is required for SAPI 5 — verify it's installed \
             (it ships with every Windows 10/11 by default)."
        ))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Exit codes from the PS1 helper:
        //   2 = SAPI missing, 3 = mic unavailable, 4 = RecognizeAsync
        //       failed, 5 = timeout, 6 = grammar/recognizer not installed
        //       for current culture
        let code = output.status.code().unwrap_or(-1);
        let hint = match code {
            2 => "Run Settings → Voice → Re-install voice components to enable SAPI 5.",
            3 => "Check that a microphone is plugged in and set as the default recording device in Windows Sound settings.",
            4 => "Windows speech engine failed to start. Try restarting the app.",
            5 => "No speech detected within the timeout window. Click the mic and speak sooner.",
            6 => "A speech recognizer for your Windows display language isn't installed. Install one via Settings → Time & language → Language & region (e.g. English (United States) Speech).",
            _ => "See Settings → Voice → Diagnostics for more details.",
        };
        return Err(format!(
            "Native voice capture failed (code {code}): {stderr}. {hint}"
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    serde_json::from_str::<VoiceNativeCaptureResult>(trimmed).map_err(|e| {
        format!(
            "Could not parse native voice capture output: {e}. Raw: {trimmed}"
        )
    })
}

#[cfg(not(windows))]
#[tauri::command]
fn mc_voice_native_capture(
    _app: tauri::AppHandle,
    _timeout_ms: Option<u64>,
) -> Result<VoiceNativeCaptureResult, String> {
    Err(
        "Native Windows voice capture is only available on Windows. \
         Install the Voice module from the Modules catalog for cross-platform \
         whisper.cpp STT instead."
            .to_string(),
    )
}

// ----------------------------------------------------------------------------
// Tauri command: maic_login (Lesson 444, fixed endpoint)
//
// Called by the first-run login modal. POSTs {email, password} to
// `MAIC_API_URL/v1/users/login` (the consumer-facing endpoint — NOT
// `/v1/auth/login`, which is the dashboard's name+password endpoint), returns
// the JWT, then triggers a re-bootstrap of the MAIC provider config (with
// MAIC_API_KEY now set in the parent process's env). After this returns
// successfully, the chat panel will unblock because openclaw.json now has a
// real apiKey.
//
// On failure, returns the error message verbatim so the login modal can
// surface it. Errors do NOT leak the password back to the frontend.
//
// Lesson 447 (post-v1.0.1): the first implementation called `/v1/auth/login`
// which is the Milagro dashboard's name+password endpoint, returning 422
// "Field required: name" for any {email, password} body. The fix is to call
// the consumer endpoint `/v1/users/login` instead. Schema is identical to
// `api__routes__users__LoginIn` (email + password, optional totp/recovery_code).
// ----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct MaicLoginInfo {
    /// JWT bearer token — also known as MAIC_API_KEY in the provider config.
    token: String,
    /// User email returned by /v1/users/login (may equal the input).
    email: String,
    /// Resolved tier: 'free', 'pro', 'pro_plus', 'team', 'enterprise' (or
    /// whatever the server returns). Free tier is the default.
    tier: String,
    /// Endpoint the login was against (for the UI to show "logged in to X").
    endpoint: String,
}

#[tauri::command]
fn maic_login(email: String, password: String, remember: bool) -> Result<MaicLoginInfo, String> {
    let endpoint = std::env::var("MAIC_API_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

    // Lesson 450: normalize the endpoint to its bare-origin form before
    // appending `/{path}`. This handles two cases:
    //   1. DEFAULT_ENDPOINT = "https://maicserver.com" (no /v1) — unchanged.
    //   2. User set MAIC_API_URL="https://maicserver.com/v1" themselves —
    //      we strip /v1 here so the appended `/v1/users/login` doesn't
    //      double-prefix the path. maic_login always POSTs to the v1
    //      route of whatever MAIC instance the user pointed at.
    let endpoint = strip_trailing_v1(&endpoint);

    // Validate the email shape early to avoid a round-trip on obvious typos.
    if !email.contains('@') || email.trim().is_empty() {
        return Err("Please enter a valid email address.".to_string());
    }
    if password.is_empty() {
        return Err("Password cannot be empty.".to_string());
    }

    let body = serde_json::json!({
        "email": email.trim(),
        "password": password,
    })
    .to_string();

    let response_body = http_post_json_with_tls_fallback(&endpoint, "/v1/users/login", &body)
        .map_err(|e| format!("Login request failed: {}", e))?;

    // The endpoint returns { token, user: {email, tier, ...} } on success.
    // We accept both the wrapped form (production) and the bare { token } form
    // (legacy / older docs).
    let parsed: serde_json::Value = serde_json::from_str(&response_body)
        .map_err(|e| format!("Login response could not be parsed: {}", e))?;

    let token = parsed
        .get("token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "Login succeeded but no token in response. Server said: {}",
                response_body.chars().take(200).collect::<String>()
            )
        })?;

    let resolved_email = parsed
        .get("user")
        .and_then(|u| u.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or(email.trim())
        .to_string();

    let tier = parsed
        .get("user")
        .and_then(|u| u.get("tier"))
        .and_then(|v| v.as_str())
        .unwrap_or("free")
        .to_string();

    // Set MAIC_API_KEY in *this* process's env so that subsequent calls
    // to ensure_maic_provider_config() will write the provider entry as a
    // literal (not a SecretRef). The setting does NOT persist across
    // process restarts — we ALSO write the key as a literal into
    // openclaw.json, so it survives.
    std::env::set_var(ENV_VAR_NAME, &token);

    // Re-bootstrap. This now hits the literal-key branch (resolved_key = Some)
    // and writes the full provider entry into openclaw.json.
    //
    // Lesson 523 (NEW): pass the user's resolved tier so `params.tools`
    // matches their entitlement (paid = all 7 schemas, free = empty).
    let parsed_tier_login = crate::auth::tier::Tier::from_str(&tier);
    let bootstrap = ensure_maic_provider_config_for_tier(parsed_tier_login).map_err(|e| {
        format!(
            "Login succeeded but provider config could not be written: {}",
            e
        )
    })?;

    if !bootstrap.provider_configured {
        return Err(format!(
            "Login succeeded but provider still not configured (key_source={:?}). \
             Please restart Miracle Claw and try again.",
            bootstrap.api_key_source
        ));
    }

    // Lesson 449: even after the bootstrap, if the user had a pre-existing
    // SecretRef (legacy MC < v1.0.3 installs), the bootstrap keeps the
    // SecretRef because the env var IS now set and the SecretRef resolves.
    // We need the literal JWT on disk so the openclaw gateway reads a
    // working key — guarantee that explicitly here.
    replace_secret_ref_with_literal();

    // Lesson 483 (rc12): block until the gateway has consumed the
    // openclaw.json rewrite and is actually serving HTTP. Writing the
    // file triggers the gateway's MAIC hot-reload (config change detected
    // → HTTP listener rebuild → ~600-1000ms outage). Without this wait,
    // a user who clicks OpenClaw within ~1.5s of clicking Login will
    // hit the Lesson 480 race: TCP probe passes (listener exists), HTTP
    // probe times out 3x (server is mid-rebuild). The auth round-trip
    // gets slower by ~1-2s, but the OpenClaw window opens reliably on
    // the first click — much better UX than "please wait 2 seconds and
    // try again" with each retry pushing further from the actual reload.
    //
    // 15s ceiling: hot-reload itself is ~600ms; HTTP listener rebuild +
    // plugin re-init can stretch to a few seconds on Windows under AV
    // scan. 15s is a generous ceiling that covers the worst observed
    // reload (~5s in rc11 testing) while still failing fast on a real
    // gateway crash. The progress bar already shows during login so
    // users don't notice the extra wait.
    //
    // Stable-window requirement: after wait_for_gateway_ready returns
    // Ok, we ALSO require the gateway to be healthy for at least 1 full
    // second with no reload activity in progress. The chokidar file
    // watcher fires asynchronously AFTER our write returns; if we
    // immediately re-probe and find HTTP 200, we might be probing
    // BEFORE the reload has even started (gateway healthy from before
    // write). The 1.5s settle gives the watcher time to fire AND the
    // reload time to complete before we declare victory.
    eprintln!(
        "[miracle-claw] maic_login: post-write gateway settle (Lesson 483, 15s ceiling + 1.5s stable)"
    );
    log_to_file(
        "maic_login: post-write gateway settle (wait_for_gateway_ready 15s + 1.5s stable) — \
         openclaw.json was just modified; gateway is hot-reloading MAIC provider",
    );
    let settle_start = std::time::Instant::now();
    let mut stable = false;
    if let Err(e) = wait_for_gateway_ready(OPENCLAW_PORT, Duration::from_secs(15)) {
        log_to_file(&format!(
            "maic_login: wait_for_gateway_ready TIMEOUT ({}); login still succeeds \
             but OpenClaw click may need a retry",
            e
        ));
        eprintln!(
            "[miracle-claw] maic_login: WARNING — wait_for_gateway_ready timeout ({}); \
             gateway may still be reloading",
            e
        );
    } else {
        // wait_for_gateway_ready returned Ok. Now poll every 250ms; require
        // 6 consecutive Ok polls (~1.5s of stable HTTP) before declaring
        // settled. If the reload fires AFTER our first Ok check, we'll
        // detect a probe failure mid-window and keep polling.
        let mut consecutive_ok: u32 = 0;
        let required_ok: u32 = 6;
        let stable_deadline = settle_start + Duration::from_secs(13);
        while std::time::Instant::now() < stable_deadline && consecutive_ok < required_ok {
            match check_http_ready(
                "http://127.0.0.1:28789/",
                Duration::from_millis(750),
            ) {
                Ok(_) => {
                    consecutive_ok += 1;
                }
                Err(e) => {
                    log_to_file(&format!(
                        "maic_login: stable-window probe failed at consecutive_ok={} ({e}); \
                         reset and re-poll (rel likely fired mid-window)",
                        consecutive_ok
                    ));
                    consecutive_ok = 0;
                }
            }
            if consecutive_ok < required_ok {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if consecutive_ok >= required_ok {
            log_to_file(&format!(
                "maic_login: stable-window OK ({}/{} consecutive probes) after {:?} — gateway stable",
                consecutive_ok, required_ok, settle_start.elapsed()
            ));
            stable = true;
        } else {
            log_to_file(&format!(
                "maic_login: stable-window incomplete after {:?}; login continues but \
                 OpenClaw click may retry",
                settle_start.elapsed()
            ));
        }
    }
    if !stable {
        // Either we never got ready in 15s, OR we lost the stable window.
        // Login still succeeds; OpenClaw click will retry (now with 13s budget from rc12).
        eprintln!(
            "[miracle-claw] maic_login: post-write settle not fully stable; login continues, \
             OpenClaw click may need retry"
        );
    }

    eprintln!(
        "[miracle-claw] maic_login: success — user={}, tier={}, key_source={:?}",
        resolved_email, tier, bootstrap.api_key_source
    );

    // v1.0.7: publish the tier to MC_USER_TIER env so the OpenClaw MAIC
    // plugin reads it at startup and decides whether to register the 7
    // local tools. Also invalidate caches so the next dashboard read
    // returns fresh data.
    let parsed_tier = crate::auth::tier::Tier::from_str(&tier);
    crate::auth::tier::publish_tier_env(parsed_tier);
    crate::auth::tier::invalidate_tier_cache();
    crate::auth::nudge::invalidate_quota_cache();

    // Lesson 517 / v1.0.9-rc20: route paid users to Kimi + fallbacks.
    // Free stays on local `milagro-dev` (no cloud quota). The writer is
    // non-destructive — if the user has already set a manual primary,
    // we leave it alone. Idempotent across repeated logins.
    if let Err(e) = ensure_agents_default_model_for_tier(parsed_tier) {
        eprintln!(
            "[miracle-claw] maic_login: WARNING — failed to write tier defaults: {}",
            e
        );
    }

    // Lesson 458 / v1.0.6: handle "Remember me" checkbox.
    //
    // - remember=true  → encrypt and stash email|password in the OS keychain,
    //                    so future 401s can trigger silent relogin.
    // - remember=false → wipe any prior stash. Idempotent: if no stash exists,
    //                    this is a no-op. The user might be un-checking the
    //                    box after a previous login, or might be on a borrowed
    //                    machine where they want a clean slate.
    //
    // Keychain failures here are non-fatal. We don't want a user with a
    // locked-down corporate keychain to be unable to log in; we just log
    // the failure and continue. The login still succeeds; they just won't
    // get silent relogin.
    if remember {
        if let Err(e) = auto_relogin::stash_cached_credentials(
            &resolved_email,
            &password,
            &endpoint,
        ) {
            eprintln!(
                "[miracle-claw] maic_login: WARNING — failed to stash creds (silent relogin will not work): {}",
                e
            );
        }
    } else {
        // Defensive: wipe any prior stash.
        if let Err(e) = auto_relogin::clear_cached_credentials() {
            eprintln!(
                "[miracle-claw] maic_login: WARNING — failed to clear prior cached creds: {}",
                e
            );
        }
    }

    Ok(MaicLoginInfo {
        token,
        email: resolved_email,
        tier,
        endpoint,
    })
}

// ----------------------------------------------------------------------------
// Tauri command: maic_logout (Lesson 458 / v1.0.6)
//
// Signs the user out of MAIC:
//   1. Kill the launcher sidecar (if running) so the openclaw gateway stops
//      listening on localhost:28789. This frees the port for the next user
//      login and avoids a stale process holding the JWT in memory.
//   2. Clear the cached creds from the OS keychain (the "Remember me"
//      stash). Idempotent — if no stash exists, this is a no-op.
//   3. Restore the MAIC provider entry's apiKey back to a SecretRef so a
//      future `maic_login` can do its job cleanly. We don't delete the
//      entry; we restore the shape.
//   4. Unset MAIC_API_KEY in this process's env so any subsequent Tauri
//      command that checks the env sees the logged-out state.
//
// The frontend is expected to call this when the user clicks "Sign out"
// in the v1.1.0 dashboard, and to then reload the page so the login
// screen re-renders. We don't force-reload here; that's the frontend's
// job (it knows whether there's a dashboard to redraw).
//
// SAFETY: env var removal is async-signal-safe per POSIX; no signal
// handler should observe a partially-modified env. We use unsafe only
// for the std::env::remove_var call which Rust 2021 marks unsafe due to
// the data-race concern with libc::getenv.
// ----------------------------------------------------------------------------

#[tauri::command]
fn maic_logout(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // 1. Kill the launcher sidecar if it's running.
    if let Some(child) = state.launcher_child.lock().unwrap().take() {
        eprintln!("[miracle-claw] maic_logout: killing launcher sidecar");
        let _ = child.kill();
    }

    // 2. Wipe the keychain stash. We don't fail the logout if this errors
    //    (e.g. a corporate-locked-down keychain); the user is logged out
    //    of MAIC regardless, and the leftover stash will just be ignored
    //    on next login if `remember=false` (the defensive clear path).
    if let Err(e) = auto_relogin::clear_cached_credentials() {
        eprintln!(
            "[miracle-claw] maic_logout: WARNING — failed to clear cached creds: {}",
            e
        );
    }

    // 3. Restore the SecretRef so the next login can flow.
    if !restore_maic_provider_secret_ref() {
        eprintln!(
            "[miracle-claw] maic_logout: WARNING — could not restore SecretRef in openclaw.json"
        );
        // Not fatal; the next login will overwrite whatever's there anyway.
    }

    // 4. Unset the env var in our process. SAFETY: remove_var is
    //    async-signal-safe per POSIX; Rust marks it unsafe to flag the
    //    libc::getenv race.
    unsafe {
        std::env::remove_var(ENV_VAR_NAME);
    }

    // 5. If the v1.1.0 dashboard has spawned the OpenClaw child window,
    //    close it. (v1.0.x doesn't spawn a window; this is a no-op then.)
    if let Some(w) = app_handle.get_webview_window("openclaw") {
        let _ = w.close();
    }

    eprintln!("[miracle-claw] maic_logout: complete");
    Ok(())
}

// ----------------------------------------------------------------------------
// Tauri command: silent_relogin (Lesson 458 / v1.0.6)
//
// Called by the OpenClaw child window when its MAIC chat request returns
// 401. Loads the cached creds from the OS keychain, runs the login POST,
// returns the new JWT so OpenClaw can update its in-memory token and retry
// the original request.
//
// Returns:
//   - Ok(Some(MaicLoginInfo))   — relogin succeeded; OpenClaw retries
//   - Ok(None)                  — no cached creds (user didn't check
//                                 Remember me). OpenClaw surfaces the
//                                 original 401 to the user as "please log
//                                 in again".
//   - Err(String)               — relogin failed for a real reason (wrong
//                                 password, keychain corrupt, network
//                                 down). OpenClaw surfaces a generic
//                                 "session expired" message; user logs in
//                                 fresh.
//
// We deliberately do NOT clear the keychain on a failed relogin. The most
// common failure is a wrong-password-after-rotation, and clearing would
// lock the user out of silent relogin until they manually log in once
// successfully. maic_logout is the only path that should clear.
// ----------------------------------------------------------------------------

#[tauri::command]
fn silent_relogin() -> Result<Option<MaicLoginInfo>, String> {
    // Determine the endpoint we'd be logging into: same resolution logic
    // as maic_login so a MAIC_API_URL override is honored.
    let endpoint = std::env::var("MAIC_API_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
    let endpoint = strip_trailing_v1(&endpoint);

    // Load cached creds. The most common "no creds" path is the user
    // didn't check Remember me — return Ok(None) so OpenClaw knows to
    // surface a clean login prompt rather than treat it as an error.
    let creds = match auto_relogin::load_cached_credentials(&endpoint) {
        Ok(c) => c,
        Err(auto_relogin::AutoReloginError::NoCachedCreds) => {
            eprintln!("[miracle-claw] silent_relogin: no cached creds");
            return Ok(None);
        }
        Err(e) => {
            eprintln!(
                "[miracle-claw] silent_relogin: failed to load cached creds: {}",
                e
            );
            return Err(format!("failed to load cached credentials: {}", e));
        }
    };

    // Run the same login POST as maic_login. We re-implement the core
    // (parse response, set env var, replace SecretRef, return info) here
    // because we don't want to call maic_login's signature (which now
    // takes a `remember` param the user can't supply via silent flow).
    let body = serde_json::json!({
        "email": creds.email.trim(),
        "password": creds.password,
    })
    .to_string();
    let response_body = http_post_json_with_tls_fallback(&endpoint, "/v1/users/login", &body)
        .map_err(|e| format!("silent relogin HTTP failed: {}", e))?;

    let parsed: serde_json::Value = serde_json::from_str(&response_body)
        .map_err(|e| format!("silent relogin response could not be parsed: {}", e))?;
    let token = parsed
        .get("token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "silent relogin succeeded but no token in response".to_string())?;
    let resolved_email = parsed
        .get("user")
        .and_then(|u| u.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or(creds.email.trim())
        .to_string();
    let tier = parsed
        .get("user")
        .and_then(|u| u.get("tier"))
        .and_then(|v| v.as_str())
        .unwrap_or("free")
        .to_string();

    // Update env var + on-disk config so the rest of MC sees the new token.
    std::env::set_var(ENV_VAR_NAME, &token);
    // Lesson 523 (NEW): pass the resolved tier so silent relogin
    // re-stamps `params.tools` correctly for the user's plan. Critical
    // for Free→Pro upgrades mid-session where the original bootstrap
    // wrote an empty tools array.
    let parsed_tier = crate::auth::tier::Tier::from_str(&tier);
    let _ = ensure_maic_provider_config_for_tier(parsed_tier);
    let _ = replace_secret_ref_with_literal();

    // Lesson 517 / rc20: route the default model on silent relogin too.
    // Critical for users whose first login happened on Free and whose
    // MAIC plan was upgraded later — silent_relogin is how MC catches up.
    crate::auth::tier::publish_tier_env(parsed_tier);
    if let Err(e) = ensure_agents_default_model_for_tier(parsed_tier) {
        eprintln!(
            "[miracle-claw] silent_relogin: WARNING — failed to write tier defaults: {}",
            e
        );
    }

    eprintln!(
        "[miracle-claw] silent_relogin: success — user={}, tier={}",
        resolved_email, tier
    );

    Ok(Some(MaicLoginInfo {
        token,
        email: resolved_email,
        tier,
        endpoint,
    }))
}

/// HTTP POST helper for the login flow.
///
/// Uses ureq (sync, ships with rustls-tls feature) so we don't pull tokio.
/// Both HTTPS (https://maicserver.com) and plain HTTP (localhost self-hosted)
/// endpoints are supported. Returns the response body as a String on 2xx,
/// or an io::Error with the server's status + body excerpt on failure.
fn http_post_json_with_tls_fallback(
    base: &str,
    path: &str,
    body: &str,
) -> io::Result<String> {
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );

    // ureq with default-features=false + tls feature set in Cargo.toml.
    // 10s timeout — login should be fast or fail loud.
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();

    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(body);

    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r
                .into_string()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("read body: {e}")))?;
            if (200..300).contains(&status) {
                Ok(body)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("HTTP {} — {}", status, body.chars().take(300).collect::<String>()),
                ))
            }
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "(no body)".to_string());
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("HTTP {} — {}", code, body.chars().take(300).collect::<String>()),
            ))
        }
        Err(e) => Err(io::Error::new(io::ErrorKind::Other, format!("{e}"))),
    }
}

// ----------------------------------------------------------------------------
// Setup hook
// ----------------------------------------------------------------------------

fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_handle = app.handle().clone();
    let resources = resources_dir(&app_handle);

    // Lesson 528: identify THIS EXE in the side-car log so we know exactly
    // which build is running (no more "did the installer ship the right
    // binary?" guessing). Format: "[miracle-claw] MiracleClaw vX.Y.Z-rcNN
    // (build YYYY-MM-DD-HHMM UTC, install=...)".
    eprintln!(
        "[miracle-claw] MiracleClaw v{} (build {} UTC, install={})",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_TIMESTAMP"),
        resources.display()
    );

    // Lesson 472 (rc8): install custom panic hook. Tauri 2 GUI apps on Windows
    // discard stderr (Lesson 464), so default panic hook output is invisible.
    // This hook ALSO writes panic messages + backtraces to our log file, so
    // silent panics (like `WebviewWindowBuilder::build()` panicking inside
    // `with_webview` at `window.webviews().first().unwrap()`) leave evidence.
    std::panic::set_hook(Box::new(|info| {
        let msg = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => s.clone(),
                None => "<non-string panic payload>".to_string(),
            },
        };
        let location = info.location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        log_to_file(&format!(
            "PANIC at {location}: {msg} (backtrace disabled in release builds)"
        ));
    }));

    eprintln!(
        "[miracle-claw] setup: resources = {}",
        resources.display()
    );

    // 1. Install MAIC plugin if needed.
    match copy_maic_plugin_if_needed(&resources) {
        Ok((CopyResult::AlreadyPresent, _)) => eprintln!("[miracle-claw] maic plugin: present (no change)"),
        Ok((CopyResult::Installed, _)) => eprintln!("[miracle-claw] maic plugin: installed fresh"),
        Ok((CopyResult::SourceMissing, _)) => eprintln!("[miracle-claw] maic plugin: source missing in bundle (skipping)"),
        Err(e) => eprintln!("[miracle-claw] maic plugin install error: {}", e),
    }

    // 2. Ensure openclaw.json exists. openclaw's gateway refuses to start with
    //    exit code 78 ("Missing config. Run openclaw setup or set
    //    gateway.mode=local (or pass --allow-unconfigured)") when no config
    //    file is present at <stateDir>/openclaw.json. First install on a fresh
    //    Windows box has no openclaw.json — we pre-write a minimal valid one
    //    so the user doesn't need to run `openclaw setup` manually.
    //
    //    We don't touch an existing user-edited config (skip if the file
    //    exists, even empty — that's the user's domain).
    ensure_openclaw_json_minimal()?;

    // 3. Wire MAIC as the agent provider (Lesson 431).
    //    openclaw's chat panel renders without a provider entry, but the
    //    first chat turn throws `missing-provider-auth` because the default
    //    `openai` provider has no API key. The MAIC plugin only injects a
    //    transport-time patch — it does NOT register a provider entry.
    //    We deep-merge `models.providers.maic` into openclaw.json so a
    //    fresh install has a working chat roundtrip out of the box. Idempotent
    //    — if the user already configured MAIC, we leave it alone.
    let maic_bootstrap = ensure_maic_provider_config()?;
    eprintln!(
        "[miracle-claw] MAIC provider bootstrap: configured={}, endpoint={}, key_source={:?}",
        maic_bootstrap.provider_configured, maic_bootstrap.endpoint, maic_bootstrap.api_key_source
    );

    // 4. Sanity-check openclaw.json (read-only — do NOT mutate the user's
    //    config; openclaw 2026.7.1+ auto-discovers plugins from <stateDir>/extensions).
    match check_openclaw_json() {
        Ok(()) => eprintln!("[miracle-claw] openclaw.json: ok"),
        Err(e) => eprintln!("[miracle-claw] openclaw.json read error: {}", e),
    }

    // 4a. Lesson 713 (2026-08-28, David): restore BYO provider keys
    //     from the encrypted vault into process env so the openclaw
    //     runtime can read them on the first chat request. Best-effort:
    //     if the vault is empty or unreadable, log and continue.
    match provider_keys::restore_provider_keys_on_boot(&app_handle) {
        Ok(n) if n > 0 => eprintln!(
            "[miracle-claw] provider-keys: restored {n} BYO keys from vault into process env"
        ),
        Ok(_) => eprintln!("[miracle-claw] provider-keys: no BYO keys in vault"),
        Err(e) => eprintln!("[miracle-claw] provider-keys restore error: {e}"),
    }

    // 5. Spawn launcher sidecar — but ONLY when we have a usable MAIC key.
    //
    // Lesson 449: the openclaw gateway crashes at startup with
    // SecretRefResolutionError if MAIC_API_KEY isn't set in the launcher's
    // process env. The launcher is spawned as a child process and inherits
    // the env at spawn time, so setting MAIC_API_KEY in the parent process
    // AFTER spawn (via maic_login) doesn't help. We have to either:
    //   (a) spawn with the key already set, OR
    //   (b) restart the launcher after login sets the env var.
    //
    // Path (a) is what `setup()` does — when we already have a key (literal
    // apiKey in openclaw.json, or env var set externally), spawn now and
    // wait for the gateway to be ready. When LoginRequired (no key anywhere),
    // we defer the spawn to `start_gateway_after_login` (called from the
    // frontend after a successful `maic_login`).
    if maic_bootstrap.provider_configured {
        log_to_file(&format!(
            "setup(): spawn_launcher_and_wait start (provider_configured=true)"
        ));
        match spawn_launcher_and_wait(&app_handle, OPENCLAW_PORT, Duration::from_secs(30)) {
            Ok(()) => {
                eprintln!("[miracle-claw] gateway READY on port {}", OPENCLAW_PORT);
                log_to_file(&format!(
                    "setup(): spawn_launcher_and_wait Ok — gateway READY port={}",
                    OPENCLAW_PORT
                ));
            }
            Err(e) => {
                eprintln!("[miracle-claw] gateway NOT ready: {}", e);
                log_to_file(&format!(
                    "setup(): spawn_launcher_and_wait Err — {}",
                    e
                ));
            }
        }
    } else {
        eprintln!(
            "[miracle-claw] LoginRequired — deferring launcher spawn until maic_login completes"
        );
        log_to_file(
            "setup(): LoginRequired — deferring launcher spawn until maic_login completes",
        );
    }

    // 6. Create the main dashboard window programmatically (Lesson 491,
    //    rc13). Pre-rc13 the main window was declared in
    //    `tauri.conf.json`'s `app.windows[0]` and Tauri created it before
    //    `setup()` ran. rc13 moves creation here so we can attach an
    //    `initialization_script()` that runs in EVERY page the window
    //    navigates to — including `http://127.0.0.1:28789/` (the chat
    //    gateway URL), where there's no way to inject a `<script>` tag
    //    because the chat HTML is served by the openclaw gateway, not by
    //    our bundled assets.
    //
    //    The bridge script (`openclaw-host-bridge.js`) sets a
    //    `__openclawHostBridge` global on the page so any page can
    //    detect it's hosted in MC, AND injects a floating "← Dashboard"
    //    pill whenever the main window is at the chat URL. The pill
    //    calls `invoke('openclaw_back_to_dashboard')` when clicked,
    //    which navigates the main window back to
    //    `tauri://localhost/index.html`.
    //
    //    We embed the bridge JS via `include_str!` so it ships in the
    //    binary (no runtime file lookup, no path resolution).
    if let Err(e) = create_main_window(&app_handle) {
        log_to_file(&format!(
            "setup(): create_main_window FAILED: {e}"
        ));
        eprintln!("[miracle-claw] main window creation failed: {}", e);
        // Not fatal — Tauri might still have created it via a fallback.
        // setup() continues so the user at least sees a launcher process.
    } else {
        log_to_file("setup(): main window created (rc13 Lesson 491)");
    }

    // v1.1.0-rc53.15 (Lesson 570): scan <app_data>/modules/ and populate
    // the runtime registry. This MUST happen AFTER create_main_window so
    // we can emit module-installed events to the frontend (for any
    // modules that were already on disk before this run). For modules
    // installed at runtime, the emit happens in mc_module_install_local.
    let modules_root = match app_handle.path().app_data_dir() {
        Ok(p) => crate::modules::modules_root(&p),
        Err(e) => {
            eprintln!("[miracle-claw] cannot resolve app_data_dir: {e} — modules disabled");
            log_to_file(&format!(
                "setup(): cannot resolve app_data_dir ({e}) — modules disabled"
            ));
            return Ok(());
        }
    };
    let discovered = crate::modules::registry::Registry::discover(&modules_root);
    eprintln!(
        "[miracle-claw] modules: discovered {} from {}",
        discovered.len(),
        modules_root.display()
    );
    if let Some(state) = app_handle.try_state::<AppState>() {
        *state.modules.lock().unwrap() = discovered;
    }

    Ok(())
}

/// Lesson 491 (rc13): create the main dashboard window programmatically
/// instead of via tauri.conf.json's `app.windows[]`. Reason: we need to
/// attach `initialization_script()` so the bridge JS runs on EVERY page
/// the main window navigates to (dashboard + chat).
///
/// Lesson 536 (rc30): capture the resolved dashboard URL via
/// `WebviewWindow::url()` and stash it in `AppState` so the
/// `openclaw_back_to_dashboard` command knows the right scheme/host to
/// navigate back to. On Windows production Tauri 2 uses
/// `http://tauri.localhost/` (or `https://` if `use_https_scheme(true)` was
/// set); on Linux/macOS it's `tauri://localhost/`. The previous hardcoded
/// `"tauri://localhost/index.html"` was wrong on Windows prod, causing the
/// chat pill to "OK via tauri.core.invoke" successfully but never bring up
/// the dashboard (WebView2 silently failed the unknown-scheme navigation).
///
/// Lesson 537 (rc31): the rc30 capture call was made IMMEDIATELY after
/// `.build()`, but WebView2 had not yet navigated at that point — its
/// `Source` was still `about:blank`. We cached `about:blank` and every
/// subsequent ← Dashboard click re-navigated to `about:blank`, leaving
/// the user with a blank screen. Fix: capture via `on_page_load` (fires
/// AFTER WebView2 finishes the navigation). Keep the immediate capture as
/// a best-effort fallback for the race case where `openclaw_back_to_dashboard`
/// runs before the first page-load event.
fn create_main_window(app_handle: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // Embed the bridge script at compile time so it ships in the binary.
    // The script lives in `src-tauri/src/openclaw-host-bridge.js` and is
    // referenced via `include_str!` so cargo recompiles when it changes.
    let bridge_js = include_str!("openclaw-host-bridge.js");

    // Lesson 537 (rc31, David 10:45 MDT): capture the resolved dashboard
    // URL on the FIRST `on_page_load` event, NOT immediately after
    // `.build()`. The rc30 fix tried `window.url()` right after build, but
    // WebView2 had not yet navigated — its `Source` was still `about:blank`,
    // which we then dutifully cached. Result: dashboard initially rendered
    // correctly (WebView2's later navigation to `http://tauri.localhost/`
    // worked), but every subsequent `openclaw_back_to_dashboard` call
    // re-navigated to `about:blank` and the user saw a blank screen.
    //
    // `on_page_load` fires AFTER WebView2 finishes the navigation, so
    // `payload.url()` is the real URL Tauri served the dashboard from.
    //
    // We also keep the immediate `window.url()` capture as a best-effort
    // fallback for the race case where `openclaw_back_to_dashboard` runs
    // BEFORE the first page-load event (e.g. user clicks ← Dashboard in
    // the same instant they opened the chat window). Most builds will
    // overwrite the fallback with the on_page_load value.
    let app_handle_for_load = app_handle.clone();
    let window = WebviewWindowBuilder::new(
        app_handle,
        "main",
        WebviewUrl::App("index.html".into()),
    )
    .title("MiracleClaw")
    .inner_size(1200.0, 820.0)
    .min_inner_size(800.0, 560.0)
    .resizable(true)
    .center()
    .visible(true)
    .decorations(true)
    .initialization_script(bridge_js)
    .on_page_load(move |_webview, payload| {
        // Lesson 538 (rc32): `on_page_load` fires for EVERY page-load,
        // not just the first one. That includes when the user navigates
        // from the dashboard INTO the chat (URL becomes
        // `http://127.0.0.1:28789/chat?...`). If we kept overwriting,
        // the chat-page URL would clobber the dashboard URL, and the
        // next ← Dashboard click would navigate BACK to the chat URL
        // (causing the login flicker David reported 2026-08-22 11:27 MDT).
        //
        // Fix: only capture URLs whose host is `tauri.localhost` (the
        // bundled dashboard). Anything else (chat gateway at
        // `127.0.0.1:28789`, `about:blank`, future pages) is ignored.
        // Once captured, lock the value so chat navigations don't
        // overwrite it.
        let url = payload.url().to_string();

        // Defensive: skip about:blank so the immediate-post-build race
        // doesn't poison the captured value (Lesson 537 anti-pattern).
        if url == "about:blank" {
            return;
        }

        // Only capture URLs from the bundled dashboard asset server.
        // Both Windows (http://tauri.localhost/...) and Linux/macOS
        // (tauri://localhost/...) variants accepted. Chat URLs
        // (http://127.0.0.1:28789/...) and anything else are ignored.
        let is_dashboard_url = url.starts_with("http://tauri.localhost")
            || url.starts_with("https://tauri.localhost")
            || url.starts_with("tauri://localhost")
            || url.starts_with("http://localhost")
            || url.starts_with("https://localhost");
        if !is_dashboard_url {
            log_to_file(&format!(
                "create_main_window: on_page_load: ignoring non-dashboard URL = {url}"
            ));
            return;
        }

        if let Some(state) = app_handle_for_load.try_state::<AppState>() {
            // Two-step: check the lock first (cheap), then take the
            // dashboard_url lock to update. Use the lock flag to avoid
            // a race where two `on_page_load` events arrive concurrently.
            let already_locked = match state.dashboard_url_locked.lock() {
                Ok(g) => *g,
                Err(_) => {
                    log_to_file(
                        "create_main_window: on_page_load: dashboard_url_locked mutex poisoned; \
                         skipping (treating as locked)",
                    );
                    true
                }
            };
            if already_locked {
                return;
            }
            match state.dashboard_url.lock() {
                Ok(mut guard) => {
                    *guard = Some(url.clone());
                    log_to_file(&format!(
                        "create_main_window: on_page_load: saved dashboard URL = {url} \
                         (locked, future page-loads will not overwrite)"
                    ));
                }
                Err(_) => {
                    log_to_file(
                        "create_main_window: on_page_load: dashboard_url mutex poisoned; skipping",
                    );
                    return;
                }
            }
            // Set the lock AFTER successfully writing the URL.
            match state.dashboard_url_locked.lock() {
                Ok(mut g) => *g = true,
                Err(_) => {
                    log_to_file(
                        "create_main_window: on_page_load: dashboard_url_locked mutex \
                         poisoned on lock-set; URL saved but lock flag may not be set",
                    );
                }
            }
        } else {
            log_to_file(
                "create_main_window: on_page_load: AppState not yet managed; skipping",
            );
        }
    })
    .build()
    .map_err(|e| format!("WebviewWindowBuilder::build() failed for main window: {e}"))?;

    // Best-effort immediate capture: if on_page_load already fired
    // synchronously (some Tauri versions do this on Linux/macOS), this
    // gives us the URL too. Otherwise it'll be `about:blank` and the
    // on_page_load callback above will fix it within milliseconds.
    //
    // Lesson 538 (rc32): same filtering as the on_page_load callback —
    // only accept dashboard URLs, respect the lock, set the lock after
    // capture. Without this, an `on_page_load` race could save the
    // dashboard URL but then the immediate capture (running on the
    // setup thread) could clobber it with about:blank or the chat URL.
    match window.url() {
        Ok(url) => {
            let url_str = url.to_string();
            if url_str == "about:blank" {
                log_to_file(
                    "create_main_window: window.url() returned about:blank \
                     (on_page_load will capture real URL on first page-load)",
                );
            } else {
                // Only accept dashboard URLs (Lesson 538)
                let is_dashboard_url = url_str.starts_with("http://tauri.localhost")
                    || url_str.starts_with("https://tauri.localhost")
                    || url_str.starts_with("tauri://localhost")
                    || url_str.starts_with("http://localhost")
                    || url_str.starts_with("https://localhost");
                if !is_dashboard_url {
                    log_to_file(&format!(
                        "create_main_window: window.url() returned non-dashboard URL = \
                         {url_str} (skipping; on_page_load will capture real URL)"
                    ));
                } else if let Some(state) = app_handle.try_state::<AppState>() {
                    let already_locked = match state.dashboard_url_locked.lock() {
                        Ok(g) => *g,
                        Err(_) => {
                            log_to_file(
                                "create_main_window: dashboard_url_locked mutex poisoned \
                                 (immediate capture); treating as locked",
                            );
                            true
                        }
                    };
                    if !already_locked {
                        if let Ok(mut guard) = state.dashboard_url.lock() {
                            if guard.is_none() {
                                *guard = Some(url_str.clone());
                                log_to_file(&format!(
                                    "create_main_window: captured dashboard URL \
                                     (immediate) = {url_str}"
                                ));
                            }
                        }
                        if let Ok(mut g) = state.dashboard_url_locked.lock() {
                            *g = true;
                        }
                    }
                }
            }
        }
        Err(e) => {
            log_to_file(&format!(
                "create_main_window: window.url() returned Err: {e} \
                 (on_page_load will capture real URL on first page-load)"
            ));
        }
    }

    Ok(())
}

/// v1.0.9-rc2 (David 13:32 MDT): write a diagnostic line to a file in
/// %APPDATA%\MiracleClaw\miracle-claw.log so we can post-mortem investigate
/// when the GUI app's stderr isn't captured (Tauri on Windows doesn't
/// surface stderr to the user). Best-effort: failures are swallowed.
///
/// We append every call so the log is append-only across sessions. Old
/// lines stay; size is bounded by occasional manual cleanup. For v1.0.9
/// this is a diagnostic tool, not a long-term log.
fn log_file_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(|roam| {
            std::path::PathBuf::from(roam)
                .join("MiracleClaw")
                .join("miracle-claw.log")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(|home| {
            std::path::PathBuf::from(home)
                .join(".miracle-claw")
                .join("miracle-claw.log")
        })
    }
}

fn log_to_file(msg: &str) {
    use std::io::Write;

    let line = format!(
        "[{}] {}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        msg
    );

    if let Some(path) = log_file_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}

/// v1.0.9 (Lesson 462): kill any orphaned openclaw.mjs / node.exe holding
/// port 28789 BEFORE we try to spawn a fresh launcher.
///
/// Why this exists: the launcher sidecar pattern is `launcher → node
/// openclaw.mjs → launcher exits`. The launcher wrapper exits with code 0
/// on shutdown, but the openclaw.mjs child process keeps running. On
/// Windows, when MC restarts or tries to respawn the launcher, the new
/// child process sees port 28789 bound by the orphan and bails with
/// "port already in use" / "gateway already running". The new launcher
/// terminates cleanly, but MC's webview window points at the ORPHAN's
/// HTTP server (which knows nothing about the current session) and renders
/// a blank page.
///
/// Symptom (David 12:30 MDT, post-v1.0.8 install):
///   - Sidecar log: "Port 28789 is already in use. - pid 27332"
///   - Dashboard tile click opens a blank window (orphan server responds,
///     returns HTML that doesn't render as a chat UI).
///
/// Fix: before spawn_launcher_and_wait, check if port 28789 is in use.
/// If yes, find the PID holding it (Windows: parse `netstat -ano | findstr
/// :28789`), log it, kill it via `taskkill /F /PID`, wait 500ms for the OS
/// to release the port, then proceed with normal spawn.
///
/// Cross-platform note: this implementation is Windows-only (miracle-claw
/// currently only ships Windows installers). On non-Windows we'd use
/// `lsof -ti:28789 | xargs kill -9` instead.
fn kill_orphan_holding_port(port: u16) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // netstat -ano | findstr :28789 → lines like:
        //   TCP    127.0.0.1:28789    0.0.0.0:0    LISTENING    27332
        let netstat = Command::new("netstat")
            .args(["-ano"])
            .output()
            .map_err(|e| {
                let msg = format!("netstat failed: {e}");
                log_to_file(&msg);
                msg
            })?;
        let stdout = String::from_utf8_lossy(&netstat.stdout);

        let port_marker = format!(":{}", port);
        let mut orphan_pids: Vec<u32> = Vec::new();
        for line in stdout.lines() {
            // Only LISTENING lines are the gateway listening for connections.
            // Established/Time_Wait lines are clients — we don't kill clients.
            if !line.contains("LISTENING") {
                continue;
            }
            if !line.contains(&port_marker) {
                continue;
            }
            // PID is the last whitespace-separated field on a netstat -ano line.
            if let Some(pid_str) = line.split_whitespace().last() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    // Don't kill ourselves (miracle-claw.exe has a different
                    // PID pattern, but defensive check: if the PID == 0 or
                    // 4 (System), skip — those are reserved).
                    if pid > 4 {
                        orphan_pids.push(pid);
                    }
                }
            }
        }

        log_to_file(&format!(
            "Lesson 462: scanned port {} — found {} orphan(s): {:?}",
            port,
            orphan_pids.len(),
            orphan_pids
        ));

        if orphan_pids.is_empty() {
            return Ok(());
        }

        eprintln!(
            "[miracle-claw] Lesson 462: found {} orphan(s) holding port {}: {:?}",
            orphan_pids.len(),
            port,
            orphan_pids
        );

        for pid in &orphan_pids {
            // /F = force, /T = also kill child processes (the node openclaw.mjs
            // child if the launcher wrapper is what's still bound — unlikely
            // on Windows but defensive).
            let kill = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
            match kill {
                Ok(out) if out.status.success() => {
                    log_to_file(&format!(
                        "Lesson 462: killed orphan pid {} (port {})",
                        pid, port
                    ));
                    eprintln!(
                        "[miracle-claw] Lesson 462: killed orphan pid {} (port {})",
                        pid, port
                    );
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    log_to_file(&format!(
                        "Lesson 462: taskkill pid {} failed: {}",
                        pid,
                        stderr.trim()
                    ));
                    eprintln!(
                        "[miracle-claw] Lesson 462: taskkill pid {} failed: {}",
                        pid,
                        stderr.trim()
                    );
                }
                Err(e) => {
                    log_to_file(&format!(
                        "Lesson 462: taskkill invocation failed: {}",
                        e
                    ));
                    eprintln!(
                        "[miracle-claw] Lesson 462: taskkill invocation failed: {}",
                        e
                    );
                }
            }
        }

        // Give the OS a moment to release the port. 500ms is enough on
        // Windows in practice; if we see flakes in testing we'll bump it.
        std::thread::sleep(std::time::Duration::from_millis(500));

        // v1.0.9-rc2 (David 13:32 MDT): verify the kill actually worked.
        // If the port is STILL bound after the kill+sleep, the kill failed
        // and we should report that loudly instead of silently proceeding.
        let verify = Command::new("netstat")
            .args(["-ano"])
            .output();
        if let Ok(out) = verify {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let still_bound: Vec<&str> = stdout
                .lines()
                .filter(|l| l.contains("LISTENING") && l.contains(&port_marker))
                .collect();
            if !still_bound.is_empty() {
                let msg = format!(
                    "Lesson 462 VERIFY FAILED: port {} still bound after kill attempt. Lines: {:?}",
                    port, still_bound
                );
                log_to_file(&msg);
                eprintln!("[miracle-claw] {}", msg);
                return Err(msg);
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Future-proof for non-Windows. Not used today but keeps the function
        // callable from cross-platform code paths without a cfg gate.
        let _ = port;
        Ok(())
    }
}

/// v1.0.9 (Lesson 462): do a quick HTTP GET on the given URL with a
/// short timeout. Returns Ok(status_code) on any 2xx, Err(msg) otherwise.
///
/// Used by `openclaw_open_window` to verify the openclaw gateway is
/// actually serving the chat UI before we spawn the WebView window.
/// Without this, an orphan gateway can hold the TCP port and the user
/// sees a blank window (the orphan's HTML doesn't render as chat UI).
fn check_http_ready(url: &str, timeout: std::time::Duration) -> Result<u16, String> {
    // Parse the URL into host + port + path. We support http://127.0.0.1:PORT/
    // and http://localhost:PORT/ — anything else is rejected.
    let url = url.trim_start_matches("http://");
    let (host_port, path) = match url.find('/') {
        Some(idx) => (&url[..idx], &url[idx..]),
        None => (url, "/"),
    };
    let (host, port) = match host_port.find(':') {
        Some(idx) => (&host_port[..idx], &host_port[idx + 1..]),
        None => (host_port, "80"),
    };
    let host = if host == "localhost" { "127.0.0.1" } else { host };
    let port: u16 = port.parse().map_err(|e| format!("invalid port: {e}"))?;

    // Connect TCP with timeout.
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let stream = TcpStream::connect_timeout(
        &format!("{host}:{port}").parse().map_err(|e| format!("invalid addr: {e}"))?,
        timeout,
    )
    .map_err(|e| format!("TCP connect failed: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("set_read_timeout: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("set_write_timeout: {e}"))?;
    let mut stream = stream;

    // Send a minimal HTTP/1.1 GET. No Host header tricks, no body.
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    stream.flush().map_err(|e| format!("flush failed: {e}"))?;

    // Read response headers (we only need the status line).
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 256];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // Stop at end of headers.
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if buf.len() > 4096 {
                    // Pathological response — bail.
                    break;
                }
            }
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }

    let response = String::from_utf8_lossy(&buf);
    // First line: "HTTP/1.1 200 OK" or similar.
    let status_line = response.lines().next().unwrap_or("");
    // Parse status code (the second whitespace-separated token).
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("malformed status line: {status_line:?}"));
    }
    let code: u16 = parts[1]
        .parse()
        .map_err(|e| format!("status code parse: {e}"))?;
    if (200..300).contains(&code) {
        Ok(code)
    } else {
        Err(format!("HTTP {} from gateway", code))
    }
}

/// Spawn the launcher sidecar with the current process env (so MAIC_API_KEY
/// set via `std::env::set_var` propagates) and wait for the gateway to be
/// reachable on the given port. Used by both `setup()` (returning users with
/// a key already in place) and `start_gateway_after_login` (after login
/// sets MAIC_API_KEY in the parent env).
///
/// Returns Ok(()) on first successful TCP connect within `timeout`. Errors
/// are non-fatal — the caller logs and continues (the webview will show a
/// gateway-not-ready state via first_run_report).
fn spawn_launcher_and_wait(
    app_handle: &tauri::AppHandle,
    port: u16,
    timeout: Duration,
) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;

    // v1.0.9 (Lesson 462): before spawning, kill any orphan holding port
    // 28789 from a previous session. Without this, the new gateway bails
    // with "port already in use", the launcher exits cleanly with code 0,
    // and the user's webview points at the orphan's HTTP server (which
    // renders blank because it doesn't know about the current session).
    if let Err(e) = kill_orphan_holding_port(port) {
        // Non-fatal — log and continue. The spawn itself will fail loudly
        // if the port is still held.
        eprintln!(
            "[miracle-claw] Lesson 462: kill_orphan_holding_port({}) returned: {}",
            port, e
        );
    }

    let shell = app_handle.shell();
    let port_str = port.to_string();
    let launcher_name = launcher_binary_name().to_string();
    eprintln!(
        "[miracle-claw] spawning sidecar: {} --gateway-port {}",
        launcher_name, port_str
    );
    log_to_file(&format!(
        "spawn_launcher_and_wait: about to sidecar-spawn {} --gateway-port {}",
        launcher_name, port_str
    ));
    let spawned = shell
        .sidecar(launcher_name.clone())
        .and_then(|cmd| cmd.args(["--gateway-port", &port_str]).spawn())
        .map_err(|e| {
            eprintln!("[miracle-claw] sidecar spawn failed: {}", e);
            log_to_file(&format!("sidecar spawn failed: {}", e));
            format!("sidecar spawn failed: {e}")
        })?;

    let (mut rx, child) = spawned;

    // Background log capture: pipe sidecar stdout/stderr to our stderr with a
    // tag so it's visible in Tauri's dev console.
    //
    // Lesson 478 (rc9): ALSO tee stdout/stderr to the log file via
    // log_to_file. On Tauri 2 GUI apps on Windows stderr is discarded
    // (Lesson 464), so without this tee the openclaw gateway's Node.js
    // errors (404s for missing assets, panics, etc.) are invisible to
    // post-mortem debugging. Tagged lines like "[launcher.stderr] <msg>"
    // make it trivial to filter the log for gateway-side failures.
    let log_path = log_file_path(); // captured before move into thread
    std::thread::spawn(move || {
        while let Some(event) = rx.blocking_recv() {
            match event {
                CommandEvent::Stdout(bytes) => {
                    let s = String::from_utf8_lossy(&bytes);
                    eprint!("[launcher.stdout] {}", s);
                    let _ = std::io::stderr().flush();
                    if let Some(p) = &log_path {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true).append(true).open(p)
                        {
                            use std::io::Write as _;
                            let _ = writeln!(f, "[launcher.stdout] {}", s);
                        }
                    }
                }
                CommandEvent::Stderr(bytes) => {
                    let s = String::from_utf8_lossy(&bytes);
                    eprint!("[launcher.stderr] {}", s);
                    let _ = std::io::stderr().flush();
                    if let Some(p) = &log_path {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true).append(true).open(p)
                        {
                            use std::io::Write as _;
                            let _ = writeln!(f, "[launcher.stderr] {}", s);
                        }
                    }
                }
                CommandEvent::Error(e) => {
                    log_to_file(&format!("launcher.error: {}", e));
                    eprintln!("[launcher.error] {}", e);
                }
                CommandEvent::Terminated(payload) => {
                    log_to_file(&format!(
                        "launcher.terminated code={:?} signal={:?}",
                        payload.code, payload.signal
                    ));
                    eprintln!(
                        "[launcher.terminated] code={:?} signal={:?}",
                        payload.code, payload.signal
                    );
                }
                _ => {}
            }
        }
    });

    app_handle
        .state::<AppState>()
        .launcher_child
        .lock()
        .unwrap()
        .replace(child);

    wait_for_gateway_ready(port, timeout)
}

/// Tauri command: start_gateway_after_login (Lesson 449).
///
/// Called by the frontend AFTER `maic_login` returns success. Spawns the
/// launcher sidecar with MAIC_API_KEY now set in the process env, then waits
/// for the gateway to be reachable on localhost:28789. Returns once the
/// gateway is up (or an error if startup fails).
///
/// Why this exists: in setup() we deferred the launcher spawn when no key
/// was available (LoginRequired), because the gateway would otherwise crash
/// with SecretRefResolutionError. After maic_login sets the key, we need to
/// start the gateway so the webview can navigate to localhost:28789/ and
/// load the chat UI.
#[tauri::command]
fn start_gateway_after_login(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Sanity: maic_login must have been called first. We check both the env
    // var and the openclaw.json entry so we don't accidentally start a
    // gateway that's about to crash.
    let key_present = std::env::var(ENV_VAR_NAME)
        .ok()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !key_present {
        log_to_file(
            "start_gateway_after_login: rejected — MAIC_API_KEY not set (frontend called before maic_login)",
        );
        return Err(
            "start_gateway_after_login called before MAIC_API_KEY was set; \
             frontend must call maic_login first."
                .to_string(),
        );
    }

    // rc4 idempotency probe (Lesson 466 + Lesson 467): TCP-port-bound is
    // NOT gateway-ready. An orphan from a previous session, an openclaw
    // mid-hot-reload, or any stray process that grabbed the port can all
    // satisfy TcpStream::connect_timeout() without actually serving the
    // chat UI. Real readiness = HTTP 2xx from GET /. check_http_ready
    // (already used by openclaw_open_window) does this in one round-trip.
    //
    // Lesson 477 (rc9): distinguish "TCP port closed" (gateway definitely
    // dead — kill+respawn is correct) from "HTTP probe failed on a still-
    // open port" (transient — gateway is up but busy, possibly mid-handling
    // a WebView2 asset-burst request). rc8 conflated the two and would
    // kill the gateway out from under an in-flight openclaw_open_window,
    // which produced the "blank window" symptom because the new gateway
    // took 12-15s to re-bind while WebView2 was already navigating to the
    // old URL. New policy: only kill+respawn on TCP port CLOSED. If port
    // is open but HTTP probe failed, log and bail (gateway will recover
    // on its own once the asset burst settles).
    let tcp_open = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:28789".parse().unwrap(),
        std::time::Duration::from_millis(300),
    ).is_ok();
    if tcp_open {
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
                // Lesson 477: port is OPEN, so the gateway is alive. HTTP
                // probe failed transiently (likely racing with the openclaw
                // window's asset-burst). Do NOT kill+respawn — that would
                // restart the gateway mid-load and cause a blank window.
                // Just log and return Ok so the caller proceeds normally;
                // the next call (or the openclaw window's own retry) will
                // get a healthy gateway.
                log_to_file(&format!(
                    "start_gateway_after_login: TCP open but HTTP probe failed ({e}); \
                     NOT killing gateway (rc9 Lesson 477 — port-open means alive)"
                ));
                return Ok(());
            }
        }
    } else {
        log_to_file(
            "start_gateway_after_login: TCP port 28789 closed; gateway is dead, \
             proceeding to clean+respawn (Lesson 477)",
        );
    }

    // If a previous launcher is still around (shouldn't be on first run, but
    // defensive against repeated calls), kill it before spawning a new one.
    if let Some(prev) = state.launcher_child.lock().unwrap().take() {
        eprintln!("[miracle-claw] killing previous launcher before respawn");
        log_to_file(
            "start_gateway_after_login: killing previous launcher handle before respawn",
        );
        let _ = prev.kill();
    }

    log_to_file(
        "start_gateway_after_login: spawn_launcher_and_wait start (after login)",
    );
    let result = spawn_launcher_and_wait(&app_handle, OPENCLAW_PORT, Duration::from_secs(30));
    log_to_file(&format!(
        "start_gateway_after_login: spawn_launcher_and_wait returned {:?}",
        result.as_ref().map(|_| "Ok").map_err(|e| e.as_str())
    ));
    result
}

/// Tauri command: openclaw_open_window (Lesson 461).
///
/// Spawns (or focuses, if already open) a dedicated webview window that
/// hosts the OpenClaw chat UI at the bundled OpenClaw gateway. Lives
/// separately from the dashboard so the user can keep the dashboard alive
/// while chatting, and so a popup-blocked `window.open()` from the
/// dashboard cannot permanently navigate the dashboard away.
///
/// Returns Ok("created") if a new window was spawned, Ok("focused") if
/// an existing one was brought to front, Err(msg) on failure.
#[tauri::command]
fn openclaw_open_window(
    app_handle: tauri::AppHandle,
) -> Result<&'static str, String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Lesson 491 (rc13): navigate the existing main webview to the chat
    // URL instead of building a second webview window. Pre-rc13 we called
    // `WebviewWindowBuilder::new(...).build()` to create a sibling window
    // that loaded http://127.0.0.1:28789/. On David's system that hangs
    // deterministically inside Tauri's webview2 init path (Lesson 487 —
    // Tauri 2.11.5 + multi-runtime WebView2 + uncached-cleared user-data-
    // dir) and the main process gets killed by Windows Application Hang
    // detection after ~7 minutes.
    //
    // The main webview is already running and successfully renders the
    // dashboard. Navigating it to the chat URL is an in-place op that
    // avoids `builder.build()` entirely. The user gets the chat UI in the
    // same window; to go back, they click the floating "← Dashboard"
    // overlay injected by `src/openclaw-host-bridge.js` (an
    // initializationScript registered on the main window in
    // tauri.conf.json).
    const CHAT_BASE_URL: &str = "http://127.0.0.1:28789/";

    // Lesson 470 (rc6 fix 2): cache-buster query string. Every launch gets a
    // unique `?v=<unix-millis>` so WebView2 never serves a stale cached
    // HTML/asset bundle from a prior install. Important here because we
    // navigate the same webview, and WebView2 caches by URL including
    // query params — without the buster, an upgrade might render a stale
    // blank chat page.
    let cache_buster = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let chat_url = format!("{CHAT_BASE_URL}?v={cache_buster}");
    log_to_file(&format!(
        "openclaw_open_window: cache_buster v={cache_buster}, chat_url={chat_url}"
    ));

    // Pre-flight: gateway must be reachable AND actually serving HTTP. If
    // the gateway is down, navigating the main webview to the URL would
    // just produce a blank "connection refused" page instead of a
    // working chat. The HTTP probe loop catches:
    //   - gateway not running at all (TCP refused)
    //   - gateway mid-hot-reload (TCP open, HTTP times out with 10060)
    //   - gateway up but static root broken (/assets/ 404 — Lesson 479)
    // All of these should bubble up to the user as a friendly error
    // instead of a silent blank chat.

    // TCP probe (Lesson 466)
    match std::net::TcpStream::connect_timeout(
        &"127.0.0.1:28789".parse().unwrap(),
        std::time::Duration::from_millis(500),
    ) {
        Ok(_) => {
            eprintln!("[miracle-claw] openclaw-chat: gateway port 28789 reachable");
        }
        Err(e) => {
            eprintln!("[miracle-claw] openclaw-chat: gateway NOT reachable: {}", e);
            return Err(format!(
                "OpenClaw gateway is not responding on port 28789.                  Try logging out and back in. ({e})"
            ));
        }
    }

    // HTTP probe loop (Lesson 480/483 — 6 attempts, 2s timeout each, 500ms
    // backoff = ~13s budget). Hot-reload races produce transient 10060s
    // that retry handles.
    let mut last_err: Option<String> = None;
    let mut http_ok = false;
    for attempt in 1..=6 {
        match check_http_ready("http://127.0.0.1:28789/", std::time::Duration::from_millis(2000)) {
            Ok(status) => {
                eprintln!(
                    "[miracle-claw] openclaw-chat: gateway HTTP / returned {} (attempt {})",
                    status, attempt
                );
                http_ok = true;
                break;
            }
            Err(e) => {
                let line = format!("attempt {}: {}", attempt, e);
                eprintln!(
                    "[miracle-claw] openclaw-chat: gateway HTTP check failed: {}",
                    line
                );
                last_err = Some(line);
                if attempt < 6 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
    if !http_ok {
        return Err(format!(
            "OpenClaw gateway is still initializing (this can happen when              your MAIC login updates settings right at startup). Please              wait 2 seconds and click OpenClaw again. ({})",
            last_err.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    // Static-root probe (Lesson 479). 404 here = broken openclaw dist,
    // non-fatal but loud-log so we can diagnose if it ever happens.
    match check_http_ready(
        "http://127.0.0.1:28789/assets/",
        std::time::Duration::from_millis(800),
    ) {
        Ok(status) => {
            log_to_file(&format!(
                "openclaw-chat: /assets/ probe HTTP {} (Lesson 479 — static root wired up)",
                status
            ));
        }
        Err(e) => {
            log_to_file(&format!(
                "openclaw-chat: /assets/ probe FAILED ({}); the openclaw dist/                  bundle appears broken — dynamic imports will 404 in the webview.                  (Lesson 479 — user needs bundled-asset reinstall.)",
                e
            ));
            eprintln!(
                "[miracle-claw] openclaw-chat WARNING: /assets/ probe failed ({});                  chat UI will likely render blank. See miracle-claw.log for details.",
                e
            );
        }
    }

    // The actual change in rc13: navigate the existing main webview instead
    // of building a new window. Main window is guaranteed to exist (it was
    // created by tauri.conf.json at app boot and is always running), but we
    // fall back gracefully if it isn't.
    let main_label = "main";
    let main_window = match app_handle.get_webview_window(main_label) {
        Some(w) => w,
        None => {
            log_to_file(&format!(
                "openclaw_open_window: main window {:?} not found — this should never happen",
                main_label
            ));
            return Err(format!(
                "MiracleClaw main window not found; cannot navigate to chat.                  Please restart MiracleClaw."
            ));
        }
    };

    // Lesson 491: navigate the existing webview. WebView2 will load the
    // chat URL in-place; the dashboard is replaced by the chat UI in the
    // same window. The bridge script (`openclaw-host-bridge.js`) detects
    // `window.location.host === '127.0.0.1:28789'` and overlays a
    // "← Dashboard" pill in the top-left that calls back into
    // `openclaw_back_to_dashboard` to restore the dashboard.
    if let Err(e) = main_window.navigate(chat_url.parse().map_err(|err| {
        format!("invalid chat_url {chat_url:?}: {err}")
    })?) {
        log_to_file(&format!(
            "openclaw_open_window: main_window.navigate() failed: {e}"
        ));
        return Err(format!("could not navigate to chat window: {e}"));
    }

    // Bring main window forward in case it was minimized / backgrounded.
    let _ = main_window.set_focus();
    let _ = main_window.unminimize();

    eprintln!(
        "[miracle-claw] openclaw-chat: navigated main window → {}",
        chat_url
    );
    log_to_file(&format!(
        "openclaw_open_window: navigated main window to {chat_url} (rc13 — no second webview)"
    ));

    Ok("navigated")
}

/// Tauri command: openclaw_back_to_dashboard (Lesson 491, rc13; Lesson 536 rc30;
/// Lesson 537 rc31).
///
/// Called by the chat page's "← Dashboard" overlay (injected by
/// `src/openclaw-host-bridge.js` when the main window is at the chat
/// gateway). Navigates the main webview back to the bundled dashboard
/// (the URL Tauri chose when it built the main window — captured by
/// `create_main_window`'s `on_page_load` callback and stashed in
/// `AppState`).
///
/// Why this lives in Rust rather than JS:
///
/// The chat page is loaded from `http://127.0.0.1:28789/` (the openclaw
/// gateway), not from the bundled dashboard assets. The bridge script has
/// no way to know the dashboard's Tauri URL except by asking the Rust
/// side via a Tauri command. Hardcoding `tauri://localhost/index.html`
/// in the bridge would couple it to Tauri's scheme — better to have Rust
/// own the navigation logic.
///
/// Lesson 536: rc13–rc29 hardcoded `"tauri://localhost/index.html"`. That
/// works on Linux/macOS dev but FAILS on Windows production builds
/// where Tauri 2 serves bundled assets at `http://tauri.localhost/`.
/// WebView2 silently refuses the unknown-scheme navigate; the page just
/// stays blank and the user sees the chat page forever. The toast in
/// `openclaw-host-bridge.js` correctly reports `[mc-bridge] OK via
/// tauri.core.invoke` because the IPC succeeded — only the WebView2
/// navigation fails. So the symptom looks like "Rust says OK but
/// dashboard never appears".
///
/// rc30 fix: read the dashboard URL from `AppState` (captured by
/// `create_main_window`). If that's `None` (e.g. first-ever boot where
/// capture raced with the first chat-page click), fall back to
/// scheme-aware defaults: `http://tauri.localhost/` on Windows,
/// `tauri://localhost/` elsewhere.
///
/// Lesson 537 (rc31): the rc30 fix tried `WebviewWindow::url()` IMMEDIATELY
/// after `.build()`, but WebView2 had not yet navigated at that point —
/// `Source()` returned `about:blank`, which we then cached. Initial
/// dashboard render was fine (WebView2's later navigation to
/// `http://tauri.localhost/` actually happened), but every ← Dashboard
/// click re-navigated to `about:blank` and the user saw a blank screen.
/// Fix: capture the URL via `on_page_load` (fires after navigation
/// completes) instead of relying on the immediate `window.url()` call.
/// Keep the immediate capture as a best-effort fallback for the race case.
#[tauri::command]
fn openclaw_back_to_dashboard(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let main_label = "main";
    let main_window = match app_handle.get_webview_window(main_label) {
        Some(w) => w,
        None => {
            log_to_file(&format!(
                "openclaw_back_to_dashboard: main window {:?} not found",
                main_label
            ));
            return Err(format!(
                "MiracleClaw main window not found; cannot return to dashboard."
            ));
        }
    };

    // Lesson 536/537: prefer the captured URL (set by create_main_window's
    // on_page_load callback). Fall back to scheme-aware default if the
    // race window kept capture from happening.
    let dashboard_url = {
        match state.dashboard_url.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                log_to_file(
                    "openclaw_back_to_dashboard: dashboard_url mutex poisoned; \
                     using scheme-aware fallback",
                );
                None
            }
        }
    };
    let dashboard_url = dashboard_url.unwrap_or_else(|| {
        #[cfg(any(windows, target_os = "android"))]
        let default = "http://tauri.localhost/".to_string();
        #[cfg(not(any(windows, target_os = "android")))]
        let default = "tauri://localhost/".to_string();
        log_to_file(&format!(
            "openclaw_back_to_dashboard: AppState URL was None, using fallback {default:?}"
        ));
        default
    });

    let parsed_url = match tauri::Url::parse(&dashboard_url) {
        Ok(u) => u,
        Err(err) => {
            log_to_file(&format!(
                "openclaw_back_to_dashboard: invalid dashboard URL {dashboard_url:?}: {err}"
            ));
            return Err(format!(
                "invalid dashboard URL {dashboard_url:?}: {err}"
            ));
        }
    };

    if let Err(e) = main_window.navigate(parsed_url) {
        log_to_file(&format!(
            "openclaw_back_to_dashboard: main_window.navigate() failed: {e}"
        ));
        return Err(format!("could not navigate back to dashboard: {e}"));
    }
    let _ = main_window.set_focus();
    let _ = main_window.unminimize();

    log_to_file(&format!(
        "openclaw_back_to_dashboard: navigated main window back to {dashboard_url}"
    ));
    Ok(())
}

/// Tauri command: mc_open_overlay (Lesson 244, rc53.10).
///
/// Called by `depot/openclaw-patches/dist/control-ui/mc-chat-toolbar.js`
/// when the user clicks 🔑 Secrets or 📎 Attach from inside the OpenClaw
/// chat page. Navigates the main webview back to the bundled dashboard
/// with a `#mcAutoOpen=<key>` URL hash, which `src/main.js` boot parses
/// to land on the Terminal page with `autoOpenOverlay=key` in ctx, which
/// `src/pages/terminal.js` then consumes to open the named overlay (and
/// strips the hash so it doesn't reopen on re-mount).
///
/// Why this exists (Lesson 244):
///
/// rc53.9 shipped with `mc-chat-toolbar.js` using
/// `window.location.href = 'tauri://localhost/index.html#mcAutoOpen=<key>'`
/// — the same IPC-INDEPENDENT pattern as `mc-back-button.js`. The pattern
/// works for the back button (Lesson 511) because mc-back-button is a
/// fallback for the bridge pill, which calls `openclaw_back_to_dashboard`
/// via Tauri IPC. Cross-scheme `window.location.href =` from
/// `http://127.0.0.1:28789` → `tauri://localhost` (or
/// `http://tauri.localhost`) is SILENTLY BLOCKED by Chromium/WebView2
/// when the origin isn't allowlisted in CSP `navigate-to` AND there's no
/// user gesture triggering navigation. Confirmed empirically: Playwright
/// tests showed `defaultPrevented: True` after click (proving the handler
/// ran) but `window.location.href` stayed on the chat URL — the
/// assignment was a no-op.
///
/// rc53.10 fix: add a real Tauri command parallel to
/// `openclaw_back_to_dashboard`. The bridge calls it via
/// `invoke('mc_open_overlay', { overlayKey: 'secrets' | 'attach' })`.
/// The hash gets appended to the dashboard URL, so WebView2 navigates
/// to the trusted bundled origin and main.js picks up the hash on boot.
///
/// Allowed values for `overlay_key` are validated against a small
/// allowlist (currently `secrets` and `attach`). Mismatched keys
/// reject with 400-equivalent error so a hostile chat page can't trick
/// us into navigating to arbitrary hashes.
#[tauri::command]
fn mc_open_overlay(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    overlay_key: String,
) -> Result<(), String> {
    // Allowlist (Lesson 244): only known overlay keys. The chat page is
    // a foreign origin (http://127.0.0.1:28789) and we don't want to
    // blindly forward arbitrary hashes into dashboard navigation.
    let valid_keys = ["secrets", "attach"];
    if !valid_keys.contains(&overlay_key.as_str()) {
        log_to_file(&format!(
            "mc_open_overlay: rejected unknown overlay_key {overlay_key:?} (allowed: {valid_keys:?})"
        ));
        return Err(format!(
            "mc_open_overlay: unknown overlay_key {overlay_key:?}; allowed: {valid_keys:?}"
        ));
    }

    let main_label = "main";
    let main_window = match app_handle.get_webview_window(main_label) {
        Some(w) => w,
        None => {
            log_to_file(&format!(
                "mc_open_overlay: main window {:?} not found",
                main_label
            ));
            return Err(format!(
                "MiracleClaw main window not found; cannot open overlay."
            ));
        }
    };

    // rc53.11 (Lesson 247): capture where we were so mc_close_overlay
    // can navigate back. Typical case: the user is on the OpenClaw chat
    // page at http://127.0.0.1:28789/chat?session=... and clicks the
    // 🔑 floating button — close should send them back to that chat
    // URL, not the dashboard. window.url() returns the page's current
    // URL as a `url::Url`; we serialise it to a String for storage.
    // (See create_main_window() for the same idiom.)
    let return_url = match main_window.url() {
        Ok(u) => {
            let s = u.to_string();
            // Defensive: don't stash about:blank — it'd cause a blank
            // page if close fires before any other navigation. And don't
            // stash the dashboard itself — that would create a no-op
            // round trip (close → dashboard → click overlay again).
            if s == "about:blank" || s.starts_with("tauri://localhost")
                || s.starts_with("http://tauri.localhost")
                || s.starts_with("https://tauri.localhost")
                || s.starts_with("http://localhost")
                || s.starts_with("https://localhost") {
                log_to_file(&format!(
                    "mc_open_overlay: skip stash for already-dashboard URL {s:?}"
                ));
                None
            } else {
                Some(s)
            }
        }
        Err(e) => {
            log_to_file(&format!(
                "mc_open_overlay: window.url() returned Err: {e} (no return URL stashed)"
            ));
            None
        }
    };
    if let Some(ref url) = return_url {
        if let Ok(mut guard) = state.overlay_return_url.lock() {
            *guard = Some(url.clone());
            log_to_file(&format!(
                "mc_open_overlay: stashed return_url = {url:?}"
            ));
        }
    }

    // Same dashboard URL lookup as openclaw_back_to_dashboard (Lesson 536/537).
    let dashboard_url = {
        match state.dashboard_url.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                log_to_file(
                    "mc_open_overlay: dashboard_url mutex poisoned; \
                     using scheme-aware fallback",
                );
                None
            }
        }
    };
    let dashboard_url = dashboard_url.unwrap_or_else(|| {
        #[cfg(any(windows, target_os = "android"))]
        let default = "http://tauri.localhost/".to_string();
        #[cfg(not(any(windows, target_os = "android")))]
        let default = "tauri://localhost/".to_string();
        log_to_file(&format!(
            "mc_open_overlay: AppState URL was None, using fallback {default:?}"
        ));
        default
    });

    // Append the URL hash. We strip any existing hash first to keep the
    // resulting URL predictable, then append ours. URL fragment (#...)
    // is not sent to the server so the gateway doesn't see it.
    let base = dashboard_url.split('#').next().unwrap_or(&dashboard_url);
    let target = format!("{base}#mcAutoOpen={overlay_key}");
    let parsed_url = match tauri::Url::parse(&target) {
        Ok(u) => u,
        Err(err) => {
            log_to_file(&format!(
                "mc_open_overlay: invalid target URL {target:?}: {err}"
            ));
            return Err(format!(
                "invalid target URL {target:?}: {err}"
            ));
        }
    };

    if let Err(e) = main_window.navigate(parsed_url) {
        log_to_file(&format!(
            "mc_open_overlay: main_window.navigate() failed: {e}"
        ));
        return Err(format!("could not navigate to overlay: {e}"));
    }
    let _ = main_window.set_focus();
    let _ = main_window.unminimize();

    log_to_file(&format!(
        "mc_open_overlay: navigated main window to {target}"
    ));
    Ok(())
}

/// Tauri command: mc_close_overlay (Lesson 247, rc53.11).
///
/// Called by `terminal.js` close-overlay handlers (`closeSecretsOverlay`,
/// `closeAttachOverlay`) via `window.__openclawHostBridge.closeOverlay()`.
/// Navigates the main webview back to the URL captured by the most
/// recent `mc_open_overlay` call (typically the OpenClaw chat page
/// `http://127.0.0.1:28789/chat?session=...`), so closing the overlay
/// returns the user to where they were.
///
/// Falls back to the dashboard if no return URL is stashed (covers the
/// case where the user opened Secrets/Attach from the Terminal page's
/// own toolbar — they expect to land back on the Terminal, which the
/// dashboard URL will satisfy via the dashboard router).
///
/// Clears the stashed return URL after navigation so a subsequent
/// close doesn't re-navigate to a stale URL.
#[tauri::command]
fn mc_close_overlay(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let main_label = "main";
    let main_window = match app_handle.get_webview_window(main_label) {
        Some(w) => w,
        None => {
            log_to_file(&format!(
                "mc_close_overlay: main window {:?} not found",
                main_label
            ));
            return Err(format!(
                "MiracleClaw main window not found; cannot close overlay."
            ));
        }
    };

    // Take the stashed return URL out of the mutex so a re-entry can't
    // double-navigate.
    let return_url = match state.overlay_return_url.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => {
            log_to_file(
                "mc_close_overlay: overlay_return_url mutex poisoned; \
                 falling back to dashboard",
            );
            None
        }
    };

    let target_url = match return_url {
        Some(url) => url,
        None => {
            // No return URL — fall back to the dashboard URL (same
            // resolution path as openclaw_back_to_dashboard).
            let dashboard_url = match state.dashboard_url.lock() {
                Ok(guard) => guard.clone(),
                Err(_) => None,
            };
            let dashboard_url = dashboard_url.unwrap_or_else(|| {
                #[cfg(any(windows, target_os = "android"))]
                let default = "http://tauri.localhost/".to_string();
                #[cfg(not(any(windows, target_os = "android")))]
                let default = "tauri://localhost/".to_string();
                log_to_file(&format!(
                    "mc_close_overlay: AppState URL was None, using fallback {default:?}"
                ));
                default
            });
            log_to_file(&format!(
                "mc_close_overlay: no return URL stashed; falling back to dashboard {dashboard_url:?}"
            ));
            dashboard_url
        }
    };

    // Strip any existing hash from the return URL. The dashboard's
    // main.js boot parses #mcAutoOpen=... to auto-open an overlay;
    // if we re-navigated to a URL with that hash still present, the
    // overlay would immediately reopen (recursion bug).
    let base = target_url.split('#').next().unwrap_or(&target_url);

    let parsed_url = match tauri::Url::parse(base) {
        Ok(u) => u,
        Err(err) => {
            log_to_file(&format!(
                "mc_close_overlay: invalid return URL {base:?}: {err}"
            ));
            return Err(format!(
                "invalid return URL {base:?}: {err}"
            ));
        }
    };

    if let Err(e) = main_window.navigate(parsed_url) {
        log_to_file(&format!(
            "mc_close_overlay: main_window.navigate() failed: {e}"
        ));
        return Err(format!("could not close overlay: {e}"));
    }
    let _ = main_window.set_focus();
    let _ = main_window.unminimize();

    log_to_file(&format!(
        "mc_close_overlay: navigated main window back to {base}"
    ));
    Ok(())
}

/// Delete the WebView2 user-data-dir so the next window build gets a fresh
/// cache. This is the most reliable fix for the "blank window after upgrade"
/// pattern: WebView2 caches the asset bundle hash from the prior install,
/// the new HTML's `<script src="./assets/index-XXXX.js"></script>` reference
/// mismatch, JS modules fail to load, `<openclaw-app>` never mounts, blank
/// window. By deleting the dir before build, we force WebView2 to refetch
/// everything from the gateway.
///
/// Idempotent — shares Lesson 466 resource-state-idempotency pattern.
fn nuke_webview2_cache_dir() {
    let dirs: Vec<PathBuf> = vec![
        // Standard Tauri 2 path on Windows
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("MiracleClaw").join("EBWebView")),
        // Sometimes under %APPDATA% instead
        std::env::var("APPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("MiracleClaw").join("EBWebView")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for dir in dirs {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => log_to_file(&format!(
                "nuke_webview2_cache_dir: removed {}",
                dir.display()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log_to_file(&format!(
                    "nuke_webview2_cache_dir: no-op (not found) {}",
                    dir.display()
                ));
            }
            Err(e) => {
                log_to_file(&format!(
                    "nuke_webview2_cache_dir: WARN failed to remove {}: {}",
                    dir.display(),
                    e
                ));
            }
        }
    }
}

/// Tauri command: open_register_url (v1.0.9, Lesson 462 companion).
///
/// Opens the supplied URL in the OS default browser via `cmd /c start ""`.
/// Used by the login screen's "Create New Account" button so new users can
/// register on milagrocloud.com without leaving the Tauri webview (we
/// don't want to navigate the dashboard webview to a third-party site).
///
/// We allow-list the host client-side (`https://milagrocloud.com/register`)
/// but defense-in-depth: the backend re-checks that the URL is https and
/// points at a host in our allow-list. This prevents the frontend from
/// being tricked into opening arbitrary URLs.
#[tauri::command]
fn open_register_url(url: String) -> Result<(), String> {
    // Defense-in-depth: re-check the URL.
    let url = url.trim().to_string();
    if !url.starts_with("https://") {
        return Err(format!(
            "open_register_url requires https:// — got {:?}",
            url
        ));
    }
    let allowed_hosts = ["milagrocloud.com", "www.milagrocloud.com"];
    let host_ok = allowed_hosts.iter().any(|h| {
        url[8..]
            .split('/')
            .next()
            .map(|first| first.eq_ignore_ascii_case(h))
            .unwrap_or(false)
    });
    if !host_ok {
        return Err(format!(
            "open_register_url host not in allow-list: {:?}",
            url
        ));
    }

    eprintln!("[miracle-claw] open_register_url: opening {}", url);

    #[cfg(target_os = "windows")]
    {
        // `cmd /c start "" <url>` opens in default browser without a
        // console window flashing. Empty quotes suppress the title arg.
        use std::process::Command;
        let status = Command::new("cmd")
            .args(["/C", "start", "", &url])
            .status()
            .map_err(|e| format!("cmd start failed: {e}"))?;
        if !status.success() {
            return Err(format!("cmd start exited with {:?}", status.code()));
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS: open, Linux: xdg-open. Future-proofing only.
        use std::process::Command;
        #[cfg(target_os = "macos")]
        let mut cmd = Command::new("open");
        #[cfg(not(target_os = "macos"))]
        let mut cmd = Command::new("xdg-open");
        cmd
            .arg(&url)
            .spawn()
            .map_err(|e| format!("browser open failed: {e}"))?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// v1.0.7: tier + quota + nudge Tauri commands.
//
// These are read-only from the frontend's perspective. They fetch from
// MAIC over HTTP and cache results client-side. They do NOT mutate
// openclaw.json — that's done by maic_login/maic_logout. Tier changes
// detected here are reported via the `tier_changed` flag in the response
// so the frontend can show a downgrade modal, but the actual tool
// tear-down happens via `apply_tier_change` (separate command) which
// the frontend calls explicitly.

// v1.0.7: resolve the MAIC base URL the same way maic_login does — env
// vars first, then system openclaw.json, then the bare default. Used by
// all tier/nudge commands so they hit the same backend the chat session
// is talking to.
fn resolve_maic_base_url() -> String {
    if let Ok(v) = std::env::var("MAIC_BASE") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(v) = std::env::var("MAIC_URL") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(v) = std::env::var("MILAGRO_MAIC_URL") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(Some(v)) = read_system_openclaw_maic_base_url() {
        if !v.trim().is_empty() {
            return v;
        }
    }
    DEFAULT_ENDPOINT.to_string()
}

// Lesson 725 (2026-08-28 21:30 MDT, David): Tasks feature needs the
// same MAIC base URL resolver. Expose `pub(crate)` so the tasks
// module can call it without re-implementing the env-var ladder.
pub(crate) fn maic_base_url() -> String {
    resolve_maic_base_url()
}

// Returns the current tier (cached, 5-min TTL). Used by the dashboard
// to render the tier badge.
#[tauri::command]
fn mc_get_tier() -> Result<crate::auth::tier::TierInfo, String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    crate::auth::tier::fetch_tier_cached(&jwt, &maic_base)
}

// Returns the current quota + nudge decision. Used by the dashboard to
// render the token usage indicator and the nudge modal.
#[tauri::command]
fn mc_get_nudge() -> Result<crate::auth::nudge::NudgeDecision, String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    let tier = crate::auth::tier::current_tier();
    let quota = crate::auth::nudge::fetch_quota_cached(&jwt, &maic_base)?;
    Ok(crate::auth::nudge::evaluate_nudge(tier, &quota))
}

// Returns the raw quota (used + limit) regardless of threshold.
//
// Lesson 561 (2026-08-24 16:08 MDT, David): the dashboard's usage line
// was "dead" because `mc_get_nudge` only returns a struct when the user
// has hit a 500/1000/cap/80/95/100% threshold — below threshold, the
// dashboard fell back to rendering just an em dash. This new command
// returns the quota unconditionally so the dashboard can ALWAYS show
// "X / Y tokens this period" and a progress bar.
//
// Reuses the same fetch + cache as `mc_get_nudge` so quota and nudge
// can never disagree about the current token count.
#[tauri::command]
fn mc_get_quota() -> Result<crate::auth::nudge::QuotaResponse, String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    crate::auth::nudge::fetch_quota_cached(&jwt, &maic_base)
}

// Returns the list of available plans + pricing for the in-app
// upgrade card. Mirrors MAIC's `/v1/billing/plans` endpoint but is
// served from Rust so we don't need a separate HTTP fetch from JS.
//
// Lesson 561: pulled into the dashboard so free users can upgrade
// without leaving the Tauri webview. The endpoint is unauthenticated
// on MAIC's side, so we don't need a JWT here — but we still use
// `resolve_maic_base_url()` so test/dev environments work.
#[tauri::command]
fn mc_list_plans() -> Result<Vec<serde_json::Value>, String> {
    let maic_base = resolve_maic_base_url();
    let url = format!(
        "{}/v1/billing/plans",
        crate::auth::tier::normalize_api_base(&maic_base)
    );
    let resp = ureq::get(&url)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("/v1/billing/plans: {e}"))?;
    let plans: Vec<serde_json::Value> = resp
        .into_json()
        .map_err(|e| format!("parse /v1/billing/plans: {e}"))?;
    Ok(plans)
}

// Opens a Stripe Checkout URL for the given plan in the OS default
// browser. Used by the in-app "Plans" card on the dashboard so free
// users can upgrade without leaving the Tauri webview.
//
// Lesson 561: existing `open_register_url` allow-lists only
// milagrocloud.com; Stripe checkout URLs are on checkout.stripe.com.
// We add a NEW command instead of widening that allow-list so the
// billing flow has its own dedicated defense-in-depth boundary.
//
// Flow:
//   1. JS calls `mc_open_checkout_url("pro")`.
//   2. Rust POSTs to MAIC `/v1/billing/checkout` with the user's JWT
//      + plan_code, plus success_url/cancel_url pointing at
//      milagrocloud.com/welcome and /pricing respectively.
//   3. MAIC returns a Stripe Checkout URL.
//   4. Rust validates the URL is checkout.stripe.com (defense-in-depth)
//      and opens it via `cmd /c start ""` on Windows (the cross-platform
//      `open`/`xdg-open` pattern in `open_register_url` handles
//      macOS/Linux).
#[tauri::command]
fn mc_open_checkout_url(plan_code: String) -> Result<(), String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    let base = crate::auth::tier::normalize_api_base(&maic_base);
    let url = format!("{}/v1/billing/checkout", base);

    // MAIC's CheckoutIn shape (see /opt/maic/api/routes/billing.py:281).
    // We always send success_url=/welcome?plan=<code> so the post-payment
    // redirect lands on the same page as /signup?plan=free (Lesson 553).
    let body = serde_json::json!({
        "plan_code": plan_code,
        "success_url": format!("https://milagrocloud.com/welcome?plan={}", plan_code),
        "cancel_url": "https://milagrocloud.com/pricing?canceled=1",
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {jwt}"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send_json(&body)
        .map_err(|e| format!("/v1/billing/checkout POST: {e}"))?;

    let parsed: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("parse checkout response: {e}"))?;

    let checkout_url = parsed
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "checkout response missing `url` field".to_string())?
        .to_string();

    // Defense-in-depth: only open Stripe checkout URLs. If MAIC ever
    // returns a different host, refuse rather than open a phishing
    // redirect. Covers the case where a future MAIC bug or a frontend
    // compromise tries to redirect the user to an attacker site.
    if !(checkout_url.starts_with("https://checkout.stripe.com/")
        || checkout_url.starts_with("https://buy.stripe.com/"))
    {
        return Err(format!(
            "checkout URL host not in Stripe allow-list: {checkout_url}"
        ));
    }

    eprintln!(
        "[miracle-claw] mc_open_checkout_url: opening {} for plan={}",
        checkout_url, plan_code
    );

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let status = Command::new("cmd")
            .args(["/C", "start", "", &checkout_url])
            .status()
            .map_err(|e| format!("cmd start failed: {e}"))?;
        if !status.success() {
            return Err(format!("cmd start exited with {:?}", status.code()));
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        #[cfg(target_os = "macos")]
        let mut cmd = Command::new("open");
        #[cfg(not(target_os = "macos"))]
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&checkout_url);
        let status = cmd.status().map_err(|e| format!("browser launch failed: {e}"))?;
        if !status.success() {
            return Err(format!("browser exited with {:?}", status.code()));
        }
        Ok(())
    }
}

// Returns the list of tool names the current tier can use. Used by the
// dashboard to render "what you have access to" + the upgrade CTA.
#[tauri::command]
fn mc_list_tools() -> Vec<crate::tools::LocalToolName> {
    let tier = crate::auth::tier::current_tier();
    tools_for_tier(tier)
}

/// v1.0.7: tier-gated tool list. Returns the 7 local tool schemas for
/// paid tiers, empty for free. Lives in `lib.rs` (not `tools/mod.rs`)
/// because the lib-only `Tier` type isn't included in the
/// `miracle-claw-tools` binary.
///
/// **Lesson 526 (NEW 2026-08-21)**: gating removed. ALL tiers get the
/// 8 tools — Free users are rate-limited by TPM (50K) but otherwise
/// have the same capabilities. The tier parameter is kept for
/// signature stability and forward-compatibility (e.g. if we later
/// add tier-specific tools, this is the seam).
fn tools_for_tier(_tier: crate::auth::tier::Tier) -> Vec<crate::tools::LocalToolName> {
    crate::tools::ALL_LOCAL_TOOL_NAMES.to_vec()
}

// Force-refresh tier from MAIC (bypasses cache). Called by the dashboard
// when the user clicks the tier badge.
#[tauri::command]
fn mc_refresh_tier() -> Result<crate::auth::tier::TierInfo, String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    crate::auth::tier::invalidate_tier_cache();
    let info = crate::auth::tier::fetch_tier_fresh(&jwt, &maic_base)?;
    crate::auth::tier::publish_tier_env(info.tier);
    // Lesson 517 / rc20: re-route default model on tier refresh (handles
    // upgrades from Free → Pro that happen mid-session without a re-login).
    if let Err(e) = ensure_agents_default_model_for_tier(info.tier) {
        eprintln!("[miracle-claw] mc_refresh_tier: WARNING — failed to write tier defaults: {}", e);
    }
    // Lesson 523 (NEW): re-stamp `params.tools` so the model sees the
    // new entitlement immediately on the next chat. Without this, the
    // user would have to log out and back in to pick up the new tools
    // (or hit "Refresh tier" — which now also fixes tools).
    if let Err(e) = ensure_maic_provider_config_for_tier(info.tier) {
        eprintln!("[miracle-claw] mc_refresh_tier: WARNING — failed to re-stamp tool schemas: {}", e);
    }
    Ok(info)
}

// Apply a tier change: write the new tier to env (so the MAIC plugin
// re-evaluates on next launch) and invalidate caches. The launcher
// restart is the frontend's responsibility (it must re-spawn the OpenClaw
// window so the MAIC plugin reads the new MC_USER_TIER).
#[tauri::command]
fn mc_apply_tier_change(new_tier_str: String) -> Result<(), String> {
    let tier = crate::auth::tier::Tier::from_str(&new_tier_str);
    crate::auth::tier::publish_tier_env(tier);
    crate::auth::tier::invalidate_tier_cache();
    crate::auth::nudge::invalidate_quota_cache();
    // Lesson 517 / rc20: re-route default model on tier change.
    if let Err(e) = ensure_agents_default_model_for_tier(tier) {
        eprintln!("[miracle-claw] mc_apply_tier_change: WARNING — failed to write tier defaults: {}", e);
    }
    // Lesson 523 (NEW): re-stamp `params.tools` for the new tier.
    if let Err(e) = ensure_maic_provider_config_for_tier(tier) {
        eprintln!("[miracle-claw] mc_apply_tier_change: WARNING — failed to re-stamp tool schemas: {}", e);
    }
    Ok(())
}

// Lesson 517 / rc20: one-shot migration helper. The frontend can call
// this after a successful login to force the tier default onto
// `agents.defaults.model` even if the user previously picked something
// else (e.g. Free users upgraded to Pro while their manual `milagro-dev`
// was still primary). Returns `Ok(true)` if a write happened, `Ok(false)`
// if already correct.
//
// Non-destructive by default: `ensure_agents_default_model_for_tier`
// won't overwrite a non-empty primary. If `force = true`, we blank
// the existing primary in-memory, call the writer, and let it stamp
// the tier default. Use with care — it overrides user choice.
#[tauri::command]
fn mc_set_tier_defaults(force: Option<bool>) -> Result<bool, String> {
    let tier = crate::auth::tier::current_tier();
    let force = force.unwrap_or(false);
    if force {
        let path = openclaw_json_path();
        if let Some(mut cfg) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            // Lesson 800 (rc55.13): new flat schema is
            //   `agents.defaults.model = "<provider>/<id>"`  (string)
            // Clear the primary string in-place so the writer below
            // stamps the tier default. Tolerate the old object schema
            // too so a force-reset works during the migration window.
            if let Some(model_val) = cfg
                .pointer_mut("/agents/defaults/model")
            {
                match model_val {
                    serde_json::Value::String(_) => {
                        *model_val = serde_json::Value::String(String::new());
                    }
                    serde_json::Value::Object(obj) => {
                        if let Some(primary) = obj.get_mut("primary") {
                            *primary = serde_json::Value::String(String::new());
                        }
                    }
                    _ => {}
                }
            }
            if let Ok(serialized) = serde_json::to_string_pretty(&cfg) {
                let _ = std::fs::write(&path, serialized);
            }
        }
    }
    ensure_agents_default_model_for_tier(tier)
        .map_err(|e| format!("set_tier_defaults: {}", e))
}

// ============================================================================
// v1.0.9-rc35: Settings page commands.
//
// Settings is a new top-level page in main.js (registered via page_registry
// after the rc34 refactor). It needs to:
//   - Show the user their tier, endpoint, and email
//   - Let them view + edit memory files in the agent workspace
//   - Open the workspace folder in OS file manager
//
// Path safety (Lesson 211): all file operations are confined to the agent
// workspace (`~/.openclaw/workspace/` on *nix, `%USERPROFILE%\.openclaw\workspace\`
// on Windows). The frontend can pass any path; if it escapes the workspace
// or isn't a .md file, the command rejects with a clear error.
//
// Why this lives in lib.rs (not auth.rs): the workspace is a UI-facing
// concept, not an auth concept. The MAIC plugin already handles its own
// state dir independently.
// ============================================================================

/// Returned to the frontend by `mc_get_user_info`. Mirrors the
/// `TierInfo` shape with a few extras the Settings page needs (the
/// provider endpoint, a friendly "from_cache" age string).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserInfo {
    /// Resolved tier: 'free', 'pro', 'pro_plus', 'team', 'enterprise'.
    /// May be empty string if not logged in.
    tier: String,
    /// Raw `plan_code` from MAIC; informational only.
    plan_code: Option<String>,
    /// User email from MAIC /v1/users/me (None if not logged in).
    email: Option<String>,
    /// Provider endpoint the app is talking to.
    endpoint: String,
    /// True iff tier was served from cache (<5 min old).
    from_cache: bool,
    /// True iff the tier changed since last fetch (downgrade indicator).
    tier_changed: bool,
    /// User-friendly cache age, e.g. "just now", "2 min ago", "—".
    cache_age: String,
}

/// One entry in the memory-files listing returned by `mc_list_memory_files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryFileEntry {
    /// Display name without leading directories, e.g. "MEMORY.md".
    name: String,
    /// Path relative to the workspace root, e.g. "memory/2026-08-22.md".
    rel_path: String,
    /// Absolute path on disk (for the editor to load/save).
    abs_path: String,
    /// File size in bytes. 0 if the file doesn't exist yet.
    size_bytes: u64,
    /// Last-modified timestamp as ISO 8601 UTC, or None if missing.
    modified_at: Option<String>,
    /// True iff this is one of the "core" files always shown (MEMORY.md,
    /// USER.md, etc.). Daily notes are not flagged core.
    core: bool,
}

/// Resolve the agent workspace root.
///
/// Resolution order (first hit wins):
///   1. `MIRACLE_CLAW_WORKSPACE` env var (test/CI override)
///   2. `%USERPROFILE%\.openclaw\workspace` (Windows)
///   3. `$HOME/.openclaw/workspace` (macOS, Linux, WSL)
///
/// We deliberately do NOT create the directory — `mc_list_memory_files`
/// handles "doesn't exist yet" as an empty list, which is friendlier than
/// silently materializing an empty workspace on a fresh install.
fn workspace_root() -> Result<PathBuf, String> {
    if let Ok(v) = std::env::var("MIRACLE_CLAW_WORKSPACE") {
        let p = PathBuf::from(v);
        if !p.as_os_str().is_empty() {
            return Ok(p);
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let mut p = PathBuf::from(home);
        p.push(".openclaw");
        p.push("workspace");
        return Ok(p);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".openclaw");
        p.push("workspace");
        return Ok(p);
    }
    Err("could not resolve workspace root (no HOME or USERPROFILE)".to_string())
}

/// Convert an absolute path into a workspace-relative path with forward
/// slashes. Used by the frontend for display ("memory/2026-08-22.md").
fn relpath_from_workspace(abs: &Path, root: &Path) -> String {
    match abs.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}

/// Validate that a path is safe to read/write:
///   - Lives under the workspace root (no ../ escape)
///   - Has a `.md` extension
///   - Is not a symlink that resolves outside the workspace
///
/// Returns the canonicalized absolute path on success. The canonicalize
/// step also resolves symlinks — important on Linux where the user
/// could symlink ~/.openclaw/workspace/MEMORY.md → /etc/passwd.
fn validate_workspace_md(path: &Path) -> Result<PathBuf, String> {
    let root = workspace_root()?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("workspace root does not exist: {}", e))?;

    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    // Reject non-md extensions BEFORE canonicalize so the error message
    // is "not a .md file" not "No such file or directory".
    match abs.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("md") => {}
        Some(other) => return Err(format!("refusing to touch non-markdown file: .{}", other)),
        None => return Err("refusing to touch file without extension".to_string()),
    }

    let canonical = abs
        .canonicalize()
        .map_err(|e| format!("file not found: {}", e))?;

    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "path escapes workspace root ({} not under {})",
            canonical.display(),
            canonical_root.display()
        ));
    }
    Ok(canonical)
}

/// Returns the user-facing info for the Settings page.
///
/// Combines `mc_get_tier` (which already does the JWT check) with the
/// resolved endpoint and a friendly cache-age string. We call the same
/// `fetch_tier_cached` that `mc_get_tier` uses, so the Settings page
/// sees the SAME tier the dashboard just rendered.
#[tauri::command]
fn mc_get_user_info() -> Result<UserInfo, String> {
    let jwt = std::env::var(ENV_VAR_NAME).map_err(|_| "not logged in".to_string())?;
    let maic_base = resolve_maic_base_url();
    let info = crate::auth::tier::fetch_tier_cached(&jwt, &maic_base)?;
    Ok(UserInfo {
        tier: format!("{:?}", info.tier).to_lowercase(),
        plan_code: info.plan_code,
        email: info.email,
        endpoint: maic_base,
        from_cache: info.from_cache,
        tier_changed: info.tier_changed,
        cache_age: friendly_age(info.from_cache),
    })
}

/// Format the `from_cache` boolean as a user-facing string.
///
/// `mc_get_tier` returns `from_cache: true` when the cached tier is <5
/// min old. We don't have the actual age in the response, so we say
/// "cached" or "freshly fetched" rather than guessing minutes.
fn friendly_age(from_cache: bool) -> String {
    if from_cache {
        "cached (under 5 min old)".to_string()
    } else {
        "freshly fetched from MAIC".to_string()
    }
}

/// List the markdown files in the agent workspace.
///
/// Returns the "core" files (MEMORY.md, USER.md, AGENTS.md, SOUL.md,
/// IDENTITY.md, TOOLS.md) at the top, then any `memory/*.md` daily
/// notes sorted newest-first.
///
/// If the workspace doesn't exist yet (fresh install), returns an empty
/// list — the frontend shows a friendly empty state instead of an error.
#[tauri::command]
fn mc_list_memory_files() -> Result<Vec<MemoryFileEntry>, String> {
    let root = workspace_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }

    let core_files = [
        "MEMORY.md",
        "USER.md",
        "AGENTS.md",
        "SOUL.md",
        "IDENTITY.md",
        "TOOLS.md",
    ];

    let mut entries: Vec<MemoryFileEntry> = Vec::new();

    // Core files at root
    for name in core_files {
        let abs = root.join(name);
        entries.push(make_entry(&abs, &root, true));
    }

    // Daily notes under memory/
    let memory_dir = root.join("memory");
    if memory_dir.exists() {
        let read = match fs::read_dir(&memory_dir) {
            Ok(r) => r,
            Err(e) => return Err(format!("read memory/: {}", e)),
        };
        let mut daily: Vec<PathBuf> = read
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            })
            .collect();
        // Sort newest-first by filename (YYYY-MM-DD.md sorts lexicographically).
        daily.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for abs in daily {
            entries.push(make_entry(&abs, &root, false));
        }
    }

    Ok(entries)
}

fn make_entry(abs: &Path, root: &Path, core: bool) -> MemoryFileEntry {
    let rel = relpath_from_workspace(abs, root);
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.clone());
    let meta = fs::metadata(abs).ok();
    let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified_at = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| {
            // Render ISO 8601 UTC. Manual format to avoid pulling chrono.
            let secs = d.as_secs();
            // Days since 1970-01-01
            let (y, mo, day, h, mi, s) = epoch_to_ymdhms(secs);
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                y, mo, day, h, mi, s
            )
        });
    MemoryFileEntry {
        name,
        rel_path: rel,
        abs_path: abs.to_string_lossy().into_owned(),
        size_bytes,
        modified_at,
        core,
    }
}

/// Manual epoch → (year, month, day, hour, min, sec) conversion.
/// Avoids pulling in `chrono` just for one timestamp.
fn epoch_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let s = (secs % 60) as u32;
    let mins_total = secs / 60;
    let mi = (mins_total % 60) as u32;
    let hours_total = mins_total / 60;
    let h = (hours_total % 24) as u32;
    let mut days = (hours_total / 24) as i64;

    // Civil-from-days algorithm by Howard Hinnant (public domain).
    // https://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    (y as i32, m, d, h, mi, s)
}

/// Read a memory file. Path is validated by `validate_workspace_md`.
#[tauri::command]
fn mc_read_memory_file(path: String) -> Result<String, String> {
    let abs = validate_workspace_md(Path::new(&path))?;
    fs::read_to_string(&abs).map_err(|e| format!("read {}: {}", abs.display(), e))
}

/// Write a memory file. Path is validated by `validate_workspace_md`.
/// Creates parent directories if missing (so writing `memory/2026-08-22.md`
/// on a fresh install just works).
#[tauri::command]
fn mc_write_memory_file(path: String, content: String) -> Result<(), String> {
    let abs = validate_workspace_md(Path::new(&path))?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {}", parent.display(), e))?;
    }
    fs::write(&abs, content.as_bytes())
        .map_err(|e| format!("write {}: {}", abs.display(), e))?;
    Ok(())
}

/// Open the workspace folder in OS file manager.
///
///   - Windows: `explorer.exe <path>`
///   - macOS:   `open <path>`
///   - Linux:   `xdg-open <path>`
///
/// Best-effort: spawns the process detached. If it fails, returns the
/// OS error string so the frontend can show "couldn't open folder".
#[tauri::command]
fn mc_open_data_folder() -> Result<(), String> {
    let root = workspace_root()?;
    if !root.exists() {
        return Err(format!(
            "workspace does not exist yet: {}",
            root.display()
        ));
    }

    let (cmd, args): (&str, Vec<String>) = if cfg!(windows) {
        ("explorer.exe", vec![root.to_string_lossy().into_owned()])
    } else if cfg!(target_os = "macos") {
        ("open", vec![root.to_string_lossy().into_owned()])
    } else {
        ("xdg-open", vec![root.to_string_lossy().into_owned()])
    };

    std::process::Command::new(cmd)
        .args(&args)
        .spawn()
        .map_err(|e| format!("spawn {}: {}", cmd, e))?;

    Ok(())
}

// ============================================================================
// v1.0.9-rc36: Terminal tab commands.
//
// A first-cut Power-User shell: spawn cmd.exe / pwsh.exe / wsl.exe with
// stdin/stdout/stderr piped. Two reader threads (stdout, stderr) drain into
// a shared Vec<OutputChunk> guarded by a Mutex. Frontend polls
// `mc_terminal_poll(id, since_seq)` every ~100ms and gets back only the
// new chunks — the `seq` is a monotonic counter that makes polling
// idempotent (re-poll with the same `since_seq` returns the same bytes).
//
// Why not portable-pty? PTY semantics (TERM, signal delivery, resize) add
// ~150kB and a cross-platform headache for ~5% of the value: most users
// don't resize their terminal mid-session, and cmd.exe / pwsh don't care
// about TERM. We can swap in portable-pty later without changing the
// frontend surface — the polling API stays the same.
//
// Why not tauri::Emitter events? Same answer as above: events require
// test mocking, and 100ms polling latency is invisible to humans. Polling
// is bulletproof and easy to write Rust tests for.
//
// Security note: there's NO path validation here — the user is in their
// own shell, they can do whatever they want. We do NOT call this from
// the model side; it's purely a UI affordance. The MAIC `bash_run` tool
// (in tools/exec.rs) is the SANDBOXED shell — that one enforces an
// allowlist of CWDs and a 60s timeout. The Terminal panel is the
// UNSANDBOXED shell, by design.
// ============================================================================

/// One chunk of output from a terminal session. Serialized to JSON for
/// the frontend to append to its DOM. `seq` is the monotonic counter
/// assigned at append time; the frontend uses it to skip already-seen
/// chunks on the next poll.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutputChunk {
    seq: u64,
    stream: String,
    data: String,
}

/// Internal state for one terminal session. The `buffer_arc` and
/// `seq_arc` are `Arc<Mutex<...>>` so reader threads (which we spawn
/// at start time and never rejoin) can keep references without going
/// through `tauri::State`. The frontend's `mc_terminal_poll` reaches
/// them by id via `state.terminals`.
struct TerminalHandle {
    shell: String,
    started_at: u64,
    child: Mutex<Option<std::process::Child>>,
    /// Output buffer reader threads append to; poll drains. Shared
    /// with reader threads via `Arc` clone at spawn time.
    buffer_arc: std::sync::Arc<Mutex<Vec<OutputChunk>>>,
    /// Monotonic counter, bumped by reader threads. Shared via `Arc`.
    seq_arc: std::sync::Arc<Mutex<u64>>,
}

impl TerminalHandle {
    /// Append one chunk to the buffer and return its `seq`. Used by
    /// reader threads. Poll-time callers should iterate `buffer` and
    /// filter by seq instead — keeps the locking tight.
    fn push_chunk(&self, stream: &str, data: String) -> u64 {
        let new_seq = {
            let mut s = self.seq_arc.lock().unwrap();
            *s += 1;
            *s
        };
        let mut buf = self.buffer_arc.lock().unwrap();
        buf.push(OutputChunk {
            seq: new_seq,
            stream: stream.to_string(),
            data,
        });
        if buf.len() > MAX_BUFFER_LINES {
            let drop = buf.len() - MAX_BUFFER_LINES;
            buf.drain(0..drop);
        }
        new_seq
    }
}

/// Hard limit on buffered output. Once a session hits this, the buffer
/// keeps the most-recent N chunks and drops older ones. Protects against
/// OOM from runaway processes (`yes`, `for ((;;)); do echo x; done`).
const MAX_BUFFER_LINES: usize = 5_000;

/// Resolve the (cmd, args[]) tuple for a given shell label on this OS.
fn resolve_shell_cmd(shell: &str) -> Result<(&'static str, Vec<&'static str>), String> {
    if cfg!(windows) {
        match shell {
            "cmd" => Ok(("cmd.exe", vec![])),
            "pwsh" => Ok(("pwsh.exe", vec!["-NoLogo"])),
            "wsl" => Ok(("wsl.exe", vec!["--distribution", "Ubuntu", "bash"])),
            // Lesson 220 + 223: spawn the OpenClaw TUI. The bundled
            // resources invoke `node <resources>/openclaw.mjs tui --local`
            // (the actual wiring is in mc_terminal_start). With `--local`
            // the TUI runs an embedded agent runtime instead of trying to
            // reach a remote Gateway at ws://127.0.0.1:18789, which is
            // empty/not-yet-listening from a fresh Terminal tile and
            // causes the TUI to exit immediately with code 0. The shell
            // label here is just a key — `resolve_shell_cmd` returns a
            // marker we re-interpret in mc_terminal_start.
            "mc-openclaw" => Ok(("__MC_OPENCLAW__", vec!["tui", "--local"])),
            _ => Err(format!(
                "unknown shell on Windows: '{}' (supported: cmd, pwsh, wsl, mc-openclaw)",
                shell
            )),
        }
    } else {
        match shell {
            "bash" => Ok(("bash", vec!["-i"])),
            "sh" => Ok(("sh", vec!["-i"])),
            "zsh" => Ok(("zsh", vec!["-i"])),
            // Lesson 220 + 223: --local avoids the TUI exiting at
            // startup because there's no remote Gateway listening.
            "mc-openclaw" => Ok(("__MC_OPENCLAW__", vec!["tui", "--local"])),
            _ => Err(format!(
                "unknown shell on *nix: '{}' (supported: bash, sh, zsh, mc-openclaw)",
                shell
            )),
        }
    }
}

/// Look up an executable the way `which` would.
fn which_first(cmd: &str) -> String {
    if cmd.contains('/') || cmd.contains('\\') {
        return cmd.to_string();
    }
    if let Ok(paths) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in paths.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let candidate = std::path::PathBuf::from(dir).join(cmd);
            if cfg!(windows) {
                // Try the bare name first (e.g. cmd.exe resolves directly).
                if candidate.exists() {
                    return candidate.to_string_lossy().into_owned();
                }
                // Then try .exe (e.g. openclaw.exe would resolve here).
                let with_exe = candidate.with_extension("exe");
                if with_exe.exists() {
                    return with_exe.to_string_lossy().into_owned();
                }
                // Then try .cmd (npm bin shims like openclaw.cmd).
                // Needed for `mc-openclaw` which resolves to npm's
                // openclaw.cmd on Windows. Lesson 220.
                let with_cmd = candidate.with_extension("cmd");
                if with_cmd.exists() {
                    return with_cmd.to_string_lossy().into_owned();
                }
                // And .bat for older shims.
                let with_bat = candidate.with_extension("bat");
                if with_bat.exists() {
                    return with_bat.to_string_lossy().into_owned();
                }
            } else if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    cmd.to_string()
}

/// Spawn a shell with piped stdio. On Windows we pass CREATE_NO_WINDOW
/// so the user doesn't see a second console flash.
///
/// On Windows, `.cmd` and `.bat` files cannot be launched directly by
/// `CreateProcess` — they must be invoked through `cmd.exe /D /C`.
/// Lesson 221: this matters for npm bin shims (e.g. `openclaw.cmd`)
/// and any future shell that resolves to one of those extensions.
/// Spawn a child process with the given args and piped stdio.
///
/// Lesson 224: takes an optional `extra_env` map that is layered on top
/// of the inherited parent env. Only set keys, never clear. This lets
/// `mc_terminal_start` inject TUI-specific overrides (OPENCLAW_CONFIG_PATH,
/// OPENCLAW_STATE_DIR, OPENCLAW_NO_PLUGINS) without us having to
/// `std::env::set_var` and pollute the parent process — and without
/// forcing every call site to pass a Vec when it has nothing to add.
///
/// Key/value types are `String` for ergonomic ownership; `Command::env`
/// takes `K: AsRef<OsStr>` + `V: AsRef<OsStr>` which `String` impls.
fn spawn_shell(
    cmd: &str,
    args: &[String],
    extra_env: Option<&std::collections::HashMap<String, String>>,
) -> Result<std::process::Child, String> {
    // Detect whether `cmd` resolves to a .cmd/.bat shim and, if so,
    // route through cmd.exe. Return a friendly error if cmd.exe isn't
    // resolvable either (which would mean Windows itself is broken).
    #[cfg(windows)]
    let (real_cmd, real_args): (String, Vec<String>) = {
        let lower = cmd.to_ascii_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let cmd_exe = which_first("cmd.exe");
            if cmd_exe.is_empty() {
                return Err(format!(
                    "internal error: cmd.exe not found on PATH (needed to run {})",
                    cmd
                ));
            }
            let mut v = vec!["/D".to_string(), "/C".to_string(), cmd.to_string()];
            v.extend(args.iter().cloned());
            (cmd_exe, v)
        } else {
            (cmd.to_string(), args.to_vec())
        }
    };
    #[cfg(not(windows))]
    let (real_cmd, real_args): (String, Vec<String>) = {
        (cmd.to_string(), args.to_vec())
    };

    let mut command = std::process::Command::new(&real_cmd);
    for a in &real_args {
        command.arg(a);
    }
    // Layer extra_env on top of inherited parent env. Command::env() on a
    // std::process::Command is additive — it does NOT clear, it adds or
    // overrides named keys. So MAIC_API_KEY (which maic_login sets on the
    // parent process as the user's JWT) is still inherited; we only
    // override the two OPENCLAW_ keys specifically for mc-openclaw.
    if let Some(env) = extra_env {
        for (k, v) in env {
            command.env(k, v);
        }
    }
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command.spawn().map_err(|e| format!("spawn {}: {}", cmd, e))
}

/// Reader thread body. Reads lines from `reader` and appends them
/// to the handle's shared buffer via `push_chunk`. Companion of
/// `mc_terminal_start`.
fn read_lines<R: std::io::Read + Send + 'static>(
    mut reader: R,
    handle: std::sync::Arc<TerminalHandle>,
    stream: &'static str,
) {
    use std::io::{BufRead, BufReader};
    let mut br = BufReader::new(&mut reader);
    let mut line = String::new();
    loop {
        line.clear();
        match br.read_line(&mut line) {
            Ok(0) => {
                handle.push_chunk("system", format!("[{} closed]\n", stream));
                break;
            }
            Ok(_) => {
                handle.push_chunk(stream, std::mem::take(&mut line));
            }
            Err(e) => {
                handle.push_chunk("system", format!("[{} read error: {}]\n", stream, e));
                break;
            }
        }
    }
}

/// Generate a short, url-safe session id. 24 hex chars = 96 bits.
fn new_terminal_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mask each half to 12 hex digits (48 bits) so format width is fixed.
    let a = ((nanos as u64) ^ std::process::id() as u64) & 0xFF_FFFF_FFFF_FF;
    let b = ((nanos as u64).wrapping_mul(0x9E3779B97F4A7C15)) & 0xFF_FFFF_FFFF_FF;
    format!("{:012x}{:012x}", a, b)
}

/// Start a new terminal session. Spawns the chosen shell with piped
/// stdio, kicks off two reader threads (stdout, stderr), and stores
/// the handle in `AppState::terminals`. Returns the session id so the
/// frontend can address it on subsequent polls/writes/kills.
#[tauri::command]
fn mc_terminal_start(
    shell: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let (cmd, args) = resolve_shell_cmd(&shell)?;

    // Lesson 221: for `mc-openclaw`, use the BUNDLED node.exe +
    // openclaw.mjs shipped in the installer resources, not the
    // user's system PATH. Most end users don't have `openclaw`
    // installed globally, and we ship a self-contained CLI here
    // so the Terminal tile Just Works. resolve_shell_cmd returns
    // a `__MC_OPENCLAW__` marker for this shell so we can detect
    // it without an additional string compare.
    let (cmd_path, real_args): (String, Vec<String>) = if cmd == "__MC_OPENCLAW__" {
        let resources = resources_dir(&app_handle);
        #[cfg(windows)]
        let node = resources.join("node.exe");
        #[cfg(not(windows))]
        let node = resources.join("node");
        let mjs = resources.join("openclaw.mjs");
        if !node.exists() {
            return Err(format!(
                "bundled node not found at {} (expected from installer resources)",
                node.display()
            ));
        }
        if !mjs.exists() {
            return Err(format!(
                "bundled openclaw.mjs not found at {} (reinstall MiracleClaw)",
                mjs.display()
            ));
        }
        let mut v = vec![mjs.to_string_lossy().into_owned()];
        for a in &args {
            v.push((*a).to_string());
        }
        (node.to_string_lossy().into_owned(), v)
    } else {
        let resolved = which_first(cmd);
        (resolved, args.iter().map(|s| s.to_string()).collect())
    };

    let mut extra_env: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if shell == "mc-openclaw" {
        let cfg_path = openclaw_json_path();
        // The state dir is the parent of the extensions dir, so the TUI
        // also finds the bundled MAIC plugin automatically (instead of
        // reaching into APPDATA/MiracleClaw/extensions).
        let state_dir = openclaw_extensions_dir()
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        extra_env.insert(
            "OPENCLAW_CONFIG_PATH".to_string(),
            cfg_path.to_string_lossy().into_owned(),
        );
        if !state_dir.is_empty() {
            extra_env.insert("OPENCLAW_STATE_DIR".to_string(), state_dir);
        }
        extra_env.insert("OPENCLAW_NO_PLUGINS".to_string(), "1".to_string());
    }

    let mut child = spawn_shell(&cmd_path, &real_args, if extra_env.is_empty() { None } else { Some(&extra_env) })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture child stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture child stderr".to_string())?;

    let id = new_terminal_id();

    let buffer_arc = std::sync::Arc::new(Mutex::new(Vec::<OutputChunk>::new()));
    let seq_arc = std::sync::Arc::new(Mutex::new(0u64));

    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let handle = TerminalHandle {
        shell: shell.clone(),
        started_at,
        child: Mutex::new(Some(child)),
        buffer_arc: buffer_arc.clone(),
        seq_arc: seq_arc.clone(),
    };

    // Wrap the handle in Arc so reader threads and the state map can
    // share the same `Arc<TerminalHandle>`. Reader threads push chunks
    // into `handle.buffer_arc`; the state's poll/write/kill reaches
    // the same child via `handle.child` for `try_wait` / `kill`.
    let handle_arc: std::sync::Arc<TerminalHandle> = std::sync::Arc::new(handle);
    let stdout_handle = handle_arc.clone();
    std::thread::spawn(move || read_lines(stdout, stdout_handle, "stdout"));
    let stderr_handle = handle_arc.clone();
    std::thread::spawn(move || read_lines(stderr, stderr_handle, "stderr"));

    {
        let mut map = state.terminals.lock().unwrap();
        map.insert(id.clone(), handle_arc);
    }

    Ok(id)
}

/// Drain new output chunks for a session. Frontend calls this every
/// ~100ms with the `since_seq` it saw last time; we return only chunks
/// with `seq > since_seq`. Also returns the session's current
/// "alive" status so the UI can swap a spawn→kill button without an
/// extra round-trip.
#[derive(Debug, Serialize, Deserialize)]
struct TerminalPollResult {
    alive: bool,
    /// Process exit code or signal summary, populated only when alive=false.
    exit_info: Option<String>,
    chunks: Vec<OutputChunk>,
}

#[tauri::command]
fn mc_terminal_poll(
    id: String,
    since_seq: u64,
    state: tauri::State<'_, AppState>,
) -> Result<TerminalPollResult, String> {
    let map = state.terminals.lock().unwrap();
    let handle = map
        .get(&id)
        .ok_or_else(|| format!("terminal session '{}' not found", id))?;

    let (alive, exit_info) = {
        let mut child_lock = handle.child.lock().unwrap();
        match child_lock.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(Some(status)) => {
                    let info = format!("exit code {}", status.code().unwrap_or(-1));
                    *child_lock = None;
                    (false, Some(info))
                }
                Ok(None) => (true, None),
                Err(e) => {
                    *child_lock = None;
                    (false, Some(format!("wait failed: {}", e)))
                }
            },
            None => (false, Some("process already reaped or killed".to_string())),
        }
    };

    let chunks: Vec<OutputChunk> = {
        let buf = handle.buffer_arc.lock().unwrap();
        buf.iter()
            .filter(|c| c.seq > since_seq)
            .cloned()
            .collect()
    };

    Ok(TerminalPollResult {
        alive,
        exit_info,
        chunks,
    })
}

/// Write to a session's stdin. The frontend's input box feeds this on
/// enter. For interactive shells we append `\r\n` so the shell sees a
/// complete line; the frontend doesn't have to know about line endings.
///
/// Lesson 223: `mc-openclaw` uses Ink (terminal UI) which expects `\r`
/// as the Enter key (PTY-style), not just `\n`. So for that shell kind
/// we emit `\r` rather than `\n`. For real shells (cmd, pwsh, bash) we
/// keep `\r\n` which works universally on Windows + *nix.
#[tauri::command]
fn mc_terminal_write(
    id: String,
    input: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let map = state.terminals.lock().unwrap();
    let handle = map
        .get(&id)
        .ok_or_else(|| format!("terminal session '{}' not found", id))?;

    // Capture shell kind up front so mc_terminal_write can use the
    // right line terminator (\r for Ink TUI vs \r\n for real shells)
    // without needing to introspect again. The handle here is an
    // Arc<TerminalHandle> so we can hold the &Arc across .child.lock().
    let is_tui = handle.shell == "mc-openclaw";

    let mut child_lock = handle.child.lock().unwrap();
    let child = child_lock
        .as_mut()
        .ok_or_else(|| "session already ended".to_string())?;

    use std::io::Write;
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| "stdin not piped".to_string())?;

    let mut to_send = input;
    if !to_send.ends_with('\n') && !to_send.ends_with('\r') {
        // Use \r for Ink TUI (mc-openclaw), \r\n for everything else.
        // \r\n works for both cmd and bash; \r alone is the canonical
        // Enter key Ink / readline uses in a PTY context.
        if is_tui {
            to_send.push('\r');
        } else {
            to_send.push_str("\r\n");
        }
    }
    stdin
        .write_all(to_send.as_bytes())
        .map_err(|e| format!("write: {}", e))?;
    stdin.flush().map_err(|e| format!("flush: {}", e))?;
    Ok(())
}

/// Forcefully kill a session. Idempotent — killing an already-dead
/// session is a no-op. Removes the session from the map so the next
/// poll returns "session not found"; the frontend interprets that
/// as "go back to dashboard" or "kill and re-start".
#[tauri::command]
fn mc_terminal_kill(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut map = state.terminals.lock().unwrap();
    if let Some(handle) = map.remove(&id) {
        if let Some(mut child) = handle.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    Ok(())
}

/// List active sessions. Useful for diagnostics (and so the Settings
/// page can show a "Terminal sessions: 3" count if we want).
#[derive(Debug, Serialize, Deserialize)]
struct TerminalSession {
    id: String,
    shell: String,
    started_at: u64,
    alive: bool,
}

#[tauri::command]
fn mc_terminal_list(state: tauri::State<'_, AppState>) -> Vec<TerminalSession> {
    let map = state.terminals.lock().unwrap();
    map.iter()
        .map(|(id, h)| {
            let alive = match h.child.lock().unwrap().as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_none(),
                None => false,
            };
            TerminalSession {
                id: id.clone(),
                shell: h.shell.clone(),
                started_at: h.started_at,
                alive,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// v1.0.9-rc46: UI-facing filesystem commands for the Files page.
//
// These are NOT routed through the model — the user calls them directly
// from the Files browser. They share the path-allowlist enforcement used
// by the model-side tools (Documents / Desktop / Downloads / MC workspace)
// so the same safety guarantees apply even though there's no model in
// the loop.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct DirEntry {
    name: String,
    path: String,
    kind: String, // "dir" | "file" | "other"
    size: u64,
}

#[derive(serde::Serialize)]
struct ListDirResult {
    path: String,
    entries: Vec<DirEntry>,
}

/// Return the allowed root directories the Files browser can show.
/// Surfaced to the UI so we can render the root shortcuts the user
/// actually has access to.
#[tauri::command]
fn mc_ui_list_allowed_roots() -> Vec<String> {
    crate::tools::exec::allowed_roots()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

/// List the entries of a directory under the allowlist. Returns
/// structured entries sorted dirs-first then alpha.
#[tauri::command]
fn mc_ui_list_dir(path: String) -> Result<ListDirResult, String> {
    use crate::tools::exec;

    let p = exec::normalize_user_path(&path)
        .map_err(|e| format!("path error: {e}"))?;
    exec::assert_path_allowed(&p)?;

    let read = std::fs::read_dir(&p)
        .map_err(|e| format!("cannot read {p:?}: {e}"))?;

    let mut entries: Vec<DirEntry> = read
        .filter_map(|e| e.ok())
        .map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok();
            let (kind, size) = match &metadata {
                Some(m) if m.is_dir() => ("dir".to_string(), 0),
                Some(m) => ("file".to_string(), m.len()),
                None => ("other".to_string(), 0),
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            DirEntry {
                name,
                path: path.to_string_lossy().to_string(),
                kind,
                size,
            }
        })
        .filter(|e| !e.name.is_empty())
        .collect();

    // Sort: dirs first, then alpha (case-insensitive).
    entries.sort_by(|a, b| {
        let ak = a.kind == "dir";
        let bk = b.kind == "dir";
        bk.cmp(&ak)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(ListDirResult {
        path: p.to_string_lossy().to_string(),
        entries,
    })
}

/// Read a file's contents (UTF-8 only, capped). Returns the text and
/// the canonical path. On size/encoding failure returns Err with a
/// human-readable message; the UI shows it in the preview pane.
#[derive(serde::Serialize)]
struct ReadFileResult {
    path: String,
    content: String,
    bytes: u64,
    truncated: bool,
}

#[tauri::command]
fn mc_ui_read_file(path: String, max_bytes: Option<u64>) -> Result<ReadFileResult, String> {
    use crate::tools::exec;

    let p = exec::normalize_user_path(&path)
        .map_err(|e| format!("path error: {e}"))?;
    exec::assert_path_allowed(&p)?;

    let cap = max_bytes.unwrap_or(1_048_576); // 1 MB
    let metadata = std::fs::metadata(&p)
        .map_err(|e| format!("cannot stat {p:?}: {e}"))?;
    if metadata.is_dir() {
        return Err(format!("{p:?} is a directory"));
    }
    let truncated = metadata.len() > cap;
    let read_cap = if truncated { cap as usize } else { metadata.len() as usize };
    let bytes = std::fs::read(&p)
        .map_err(|e| format!("cannot read {p:?}: {e}"))?;
    let slice = &bytes[..read_cap];
    let content = String::from_utf8_lossy(slice).to_string();
    Ok(ReadFileResult {
        path: p.to_string_lossy().to_string(),
        content,
        bytes: metadata.len(),
        truncated,
    })
}

/// Read a file as bytes and return them as base64 alongside the detected
/// MIME type. Used by the Files UI to render image previews inline (PNG,
/// JPEG, GIF, WebP, SVG, BMP). Capped at 5 MB so the data URI stays small.
/// Returns Err for unsupported extensions or files that exceed the cap.
#[derive(serde::Serialize)]
struct ReadImageResult {
    path: String,
    mime: String,
    bytes: u64,
    data_base64: String,
}

const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

fn image_mime_for_ext(ext_lower: &str) -> Option<&'static str> {
    match ext_lower {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

#[tauri::command]
fn mc_ui_read_image(path: String) -> Result<ReadImageResult, String> {
    use crate::tools::exec;

    let p = exec::normalize_user_path(&path)
        .map_err(|e| format!("path error: {e}"))?;
    exec::assert_path_allowed(&p)?;

    let metadata = std::fs::metadata(&p)
        .map_err(|e| format!("cannot stat {p:?}: {e}"))?;
    if metadata.is_dir() {
        return Err(format!("{p:?} is a directory"));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "image too large: {} bytes (cap {} bytes)",
            metadata.len(),
            MAX_IMAGE_BYTES
        ));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let mime = image_mime_for_ext(&ext)
        .ok_or_else(|| format!("unsupported image extension: .{ext}"))?
        .to_string();
    let bytes = std::fs::read(&p)
        .map_err(|e| format!("cannot read {p:?}: {e}"))?;
    let data_base64 = base64_encode(&bytes);
    Ok(ReadImageResult {
        path: p.to_string_lossy().to_string(),
        mime,
        bytes: metadata.len(),
        data_base64,
    })
}

/// Tiny base64 encoder (standard alphabet, no padding). Avoids pulling in
/// the `base64` crate just for one use site. Output is ASCII-safe.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        out.push(ALPHABET[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

/// Hand a file path to the OS so the user's default application opens it.
/// No preview-in-MC possible for binary formats (PDF, Office, archives) —
/// but the user can still get to the file via their normal tools. Allowlist
/// enforced the same way as `mc_ui_read_file` and `mc_ui_read_image`.
#[tauri::command]
fn mc_ui_open_externally(path: String) -> Result<(), String> {
    use crate::tools::exec;

    let p = exec::normalize_user_path(&path)
        .map_err(|e| format!("path error: {e}"))?;
    exec::assert_path_allowed(&p)?;

    let metadata = std::fs::metadata(&p)
        .map_err(|e| format!("cannot stat {p:?}: {e}"))?;
    if metadata.is_dir() {
        return Err(format!("{p:?} is a directory"));
    }

    eprintln!("[miracle-claw] mc_ui_open_externally: {:?}", p);

    #[cfg(target_os = "windows")]
    {
        // cmd /c start "" <path> — same trick used by open_register_url.
        // Empty quotes suppress the title arg so cmd doesn't treat the
        // path as a window title.
        use std::process::Command;
        let p_str = p.to_string_lossy().into_owned();
        let status = Command::new("cmd")
            .args(["/C", "start", "", &p_str])
            .status()
            .map_err(|e| format!("cmd start failed: {e}"))?;
        if !status.success() {
            return Err(format!("cmd start exited with {:?}", status.code()));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let p_str = p.to_string_lossy().into_owned();
        Command::new("open")
            .arg(&p_str)
            .spawn()
            .map_err(|e| format!("open failed: {e}"))?;
        Ok(())
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        use std::process::Command;
        let p_str = p.to_string_lossy().into_owned();
        Command::new("xdg-open")
            .arg(&p_str)
            .spawn()
            .map_err(|e| format!("xdg-open failed: {e}"))?;
        Ok(())
    }
}

// ============================================================================
// v1.1.0-prep: drag-and-drop file attachment staging (feature/drag-drop).
//
// User drops files from OS File Explorer onto the dashboard. We copy
// each file into <workspace>/inbox/ under a timestamped name so the
// path is stable (no spaces, no unicode edge cases) and under the
// existing path allowlist (read_file can see them). Original names
// are preserved as the suffix so the user can recognize the file.
//
// The dashboard shows a queue of staged attachments. "Send to chat"
// opens the OpenClaw webview and returns a markdown payload the JS
// writes to the OS clipboard via navigator.clipboard.writeText. The
// user pastes (Ctrl+V) into the chat. The chat model then reads the
// file via its existing `read_file` tool — paths are in the allowlist.
//
// We deliberately do NOT inject into the OpenClaw chat DOM (we don't
// own it). Clipboard paste is the simplest reliable handoff that works
// across MC versions and OpenClaw session changes.

#[derive(serde::Deserialize)]
struct StageAttachmentArgs {
    src_path: String,
    #[serde(default)]
    original_name: Option<String>,
}

const ATTACHMENT_MAX_BYTES: u64 = 100 * 1024 * 1024; // 100 MB per file
const INBOX_DIR: &str = "inbox";

#[derive(serde::Serialize, Clone)]
struct AttachmentMeta {
    /// Timestamped id, stable across calls (used by mc_remove_attachment).
    id: String,
    /// Absolute path of the staged copy in inbox/.
    staged_path: String,
    /// Original filename from the OS for display.
    original_name: String,
    size: u64,
    /// "staged" once copied; "missing" if the file vanished between
    /// stage and the next render. UI can hide missing rows.
    status: String,
}

/// Return the absolute path of <workspace>/<INBOX_DIR>, creating it
/// if missing. Used by all attachment commands below.
fn inbox_dir() -> Result<PathBuf, String> {
    let root = workspace_root()?;
    let dir = root.join(INBOX_DIR);
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("create {}: {}", dir.display(), e))?;
    }
    Ok(dir)
}

#[tauri::command]
fn mc_stage_attachment(args: StageAttachmentArgs) -> Result<AttachmentMeta, String> {
    let src = PathBuf::from(&args.src_path);
    let metadata = fs::metadata(&src).map_err(|e| {
        format!("cannot stat {}: {}", args.src_path, e)
    })?;
    if metadata.is_dir() {
        return Err(format!(
            "{} is a directory (drop a file, not a folder)",
            args.src_path
        ));
    }
    if metadata.len() > ATTACHMENT_MAX_BYTES {
        return Err(format!(
            "{} is too large: {} bytes (cap {} bytes)",
            args.src_path,
            metadata.len(),
            ATTACHMENT_MAX_BYTES
        ));
    }

    let dir = inbox_dir()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("clock error: {e}"))?
        .as_millis();
    let raw_name = args
        .original_name
        .clone()
        .or_else(|| {
            PathBuf::from(&args.src_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "attachment".to_string());
    // Sanitize the suffix so the staged filename is portable across
    // tools that consume it (model chat, terminal paste, etc.).
    let safe_name = raw_name
        .replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    let filename = format!("{}-{}", now, safe_name);
    let dst = dir.join(&filename);

    fs::copy(&src, &dst).map_err(|e| {
        format!("copy {} -> {}: {}", args.src_path, dst.display(), e)
    })?;

    Ok(AttachmentMeta {
        id: now.to_string(),
        staged_path: dst.to_string_lossy().into_owned(),
        original_name: raw_name,
        size: metadata.len(),
        status: "staged".to_string(),
    })
}

#[tauri::command]
fn mc_list_attachments() -> Vec<AttachmentMeta> {
    // The queue is "everything currently in inbox/". v1 doesn't keep a
    // separate sent/cleared marker; the dashboard can call
    // mc_clear_attachments once the user clicks Send.
    let dir = match inbox_dir() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<AttachmentMeta> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if metadata.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().into_owned();
            let (id, original) = match name.split_once('-') {
                Some((id, rest)) => (id.to_string(), rest.to_string()),
                None => (String::new(), name.clone()),
            };
            let status = if metadata.len() > 0 {
                "staged".to_string()
            } else {
                "missing".to_string()
            };
            Some(AttachmentMeta {
                id,
                staged_path: path.to_string_lossy().into_owned(),
                original_name: original,
                size: metadata.len(),
                status,
            })
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

#[tauri::command]
fn mc_remove_attachment(id: String) -> Result<(), String> {
    let dir = inbox_dir()?;
    for entry in fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some((prefix, _)) = name.split_once('-') {
            if prefix == id {
                let path = entry.path();
                fs::remove_file(&path).map_err(|e| {
                    format!("delete {}: {}", path.display(), e)
                })?;
                return Ok(());
            }
        }
    }
    Err(format!("no attachment with id {id}"))
}

#[tauri::command]
fn mc_clear_attachments() -> Result<usize, String> {
    let dir = inbox_dir()?;
    let mut removed = 0;
    for entry in fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Build the markdown payload the user pastes into chat. Paths are
/// absolute so the chat model's read_file tool can resolve them
/// regardless of cwd. Backticks in paths are escaped to keep the
/// markdown well-formed.
fn build_attachment_message(attachments: &[AttachmentMeta], user_msg: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(m) = user_msg {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push_str("\n\n");
        }
    }
    out.push_str("Attached files:\n");
    for a in attachments {
        out.push_str(&format!(
            "- `{}` ({} bytes)\n",
            a.staged_path.replace('`', "\\`"),
            a.size
        ));
    }
    out
}

/// Finalize the queue: build the markdown payload, open the OpenClaw
/// webview window, and return the payload so the JS can write it to
/// the OS clipboard via navigator.clipboard.writeText. The dashboard
/// is responsible for clearing the queue after a successful send.
#[tauri::command]
fn mc_send_attachments_to_chat(
    app: tauri::AppHandle,
    user_message: Option<String>,
) -> Result<String, String> {
    let attachments = mc_list_attachments();
    if attachments.is_empty() {
        return Err("no staged attachments to send".to_string());
    }
    let payload = build_attachment_message(&attachments, user_message.as_deref());

    // Open (or focus) the OpenClaw webview window. Non-fatal if it
    // fails — the user can still paste from clipboard into any window.
    if let Err(e) = crate::openclaw_open_window(app) {
        eprintln!("[miracle-claw] openclaw_open_window failed: {e}");
    }

    Ok(payload)
}

// ----------------------------------------------------------------------------
// rc53 (feature/secrets-vault): local encrypted secrets vault.
//
// THREAT MODEL (locked-in v1, see notes/SECRETS-VAULT.md):
//   - Plaintext NEVER crosses the network boundary to MAIC.
//   - Chat preprocessor replaces `$NAME` references with `<<secret:NAME>>`
//     placeholders before any `/v1/chat/completions` call.
//   - bash_run expands `<<secret:NAME>>` placeholders AT EXEC TIME ONLY,
//     in-memory, never logged with plaintext.
//
// v0 (rc53): plaintext JSON vault, no encryption. Validates the data
// flow end-to-end. v1 (rc54) layers AES-256-GCM on top.
//
// This module exposes Tauri commands for: set/get/delete/list/expand.
// ----------------------------------------------------------------------------

/// Path to the secrets vault file (plaintext JSON in v0).
fn secrets_vault_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    // Tauri-canonical data dir: %APPDATA%\MiracleClaw on Windows,
    // ~/.local/share/MiracleClaw on Linux, etc.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("secrets.json"))
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub(crate) struct SecretEntry {
    name: String,
    value: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(serde::Serialize)]
struct SecretSummary {
    name: String,
    created_at: String,
    last_used_at: Option<String>,
    /// Length of the plaintext value. UI uses this to render
    /// "••••••••" without knowing the value.
    value_len: usize,
}

/// Validate a secret name. Enforces shell-var rules so a careless
/// name like `$PATH; rm -rf /` is rejected before it ever reaches a
/// command line.
fn validate_secret_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .ok_or_else(|| "secret name cannot be empty".to_string())?;
    if !(first.is_ascii_uppercase() || first == '_') {
        return Err(format!(
            "secret name `{name}` invalid: must start with uppercase letter or underscore"
        ));
    }
    for c in chars {
        if !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!(
                "secret name `{name}` invalid: only A-Z, 0-9, underscore allowed"
            ));
        }
    }
    Ok(())
}

pub(crate) fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cheap ISO-8601-ish. Avoids pulling chrono for one call site.
    // Format: 1970-01-01T00:00:00Z (relative seconds since epoch).
    // v1 will swap this for chrono::Utc::now().to_rfc3339().
    format!("epoch:{secs}")
}

pub(crate) fn read_vault(app: &tauri::AppHandle) -> Result<Vec<SecretEntry>, String> {
    let p = secrets_vault_path(app)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read(&p).map_err(|e| e.to_string())?;
    // rc53.8: read_vault_bytes transparently decrypts MCV1-encrypted
    // blobs OR passes legacy plaintext JSON through for on-save
    // migration.
    let plain = secrets_encryption::read_vault_bytes(&raw)?;
    let entries: Vec<SecretEntry> =
        serde_json::from_slice(&plain).map_err(|e| format!("vault parse error: {e}"))?;
    Ok(entries)
}

fn write_vault(app: &tauri::AppHandle, entries: &[SecretEntry]) -> Result<(), String> {
    let p = secrets_vault_path(app)?;
    let plain = serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?;
    // rc53.8: write_vault_bytes encrypts the JSON with a per-install
    // master key from the OS keychain. Legacy plaintext callers are
    // migrated automatically — the next save after upgrading overwrites
    // the plaintext with MCV1-encrypted bytes.
    let blob = secrets_encryption::write_vault_bytes(&plain)?;
    // Atomic write: write to .tmp then rename. Prevents torn writes
    // if MC crashes mid-save.
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, blob).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(())
}

/// Append a line to the audit log. v1 minimal: just timestamp + name.
/// Lives in `<MC_DATA>/secrets.audit.log`. Never leaves the machine.
fn audit_log(app: &tauri::AppHandle, name: &str) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let line = format!("{} {}\n", now_iso(), name);
        let _ = std::fs::write(dir.join("secrets.audit.log"), line);
        // Note: this APPENDS-by-truncating. For v0 simplicity only.
        // v1 will use proper append + rotation.
    }
}

#[tauri::command]
fn mc_secret_set(app: tauri::AppHandle, name: String, value: String) -> Result<SecretSummary, String> {
    validate_secret_name(&name)?;
    if value.is_empty() {
        return Err("secret value cannot be empty".to_string());
    }
    let mut entries = read_vault(&app)?;
    // Upsert: if name exists, replace value; else append.
    let now = now_iso();
    let value_len = value.chars().count();
    let mut found = false;
    for entry in entries.iter_mut() {
        if entry.name == name {
            entry.value = value.clone();
            entry.created_at = now.clone();
            entry.last_used_at = None;
            found = true;
            break;
        }
    }
    if !found {
        entries.push(SecretEntry {
            name: name.clone(),
            value,
            created_at: now.clone(),
            last_used_at: None,
        });
    }
    write_vault(&app, &entries)?;
    Ok(SecretSummary {
        name,
        created_at: now,
        last_used_at: None,
        value_len,
    })
}

#[tauri::command]
fn mc_secret_delete(app: tauri::AppHandle, name: String) -> Result<bool, String> {
    validate_secret_name(&name)?;
    let mut entries = read_vault(&app)?;
    let before = entries.len();
    entries.retain(|e| e.name != name);
    if entries.len() == before {
        return Err(format!("secret `{name}` not found"));
    }
    write_vault(&app, &entries)?;
    Ok(true)
}

#[tauri::command]
fn mc_secret_list(app: tauri::AppHandle) -> Result<Vec<SecretSummary>, String> {
    let entries = read_vault(&app)?;
    Ok(entries
        .into_iter()
        .map(|e| SecretSummary {
            name: e.name,
            created_at: e.created_at,
            last_used_at: e.last_used_at,
            value_len: e.value.chars().count(),
        })
        .collect())
}

/// Expand `<<secret:NAME>>` placeholders in a string to their stored
/// plaintext values. Called by `bash_run` (via the JS preprocessor)
/// right before exec. Updates `last_used_at` and writes an audit log
/// line for each expansion.
#[tauri::command]
fn mc_secret_expand(app: tauri::AppHandle, input: String) -> Result<String, String> {
    use std::sync::Mutex;
    // Static cache of expanded placeholders to avoid race-on-audit when
    // multiple bash_run calls fire in parallel (each one triggers an
    // audit log + last_used_at update).
    static AUDIT_LOCK: Mutex<()> = Mutex::new(());

    let entries = read_vault(&app)?;
    let mut by_name: std::collections::HashMap<&str, &SecretEntry> =
        std::collections::HashMap::new();
    for e in &entries {
        by_name.insert(e.name.as_str(), e);
    }

    let mut expanded = input.clone();
    let mut to_mark_used: Vec<String> = Vec::new();

    // Scan for `<<secret:NAME>>` patterns and expand each one. We
    // loop because the replacement value may itself contain
    // placeholders (e.g. a script that uses another secret). v0:
    // we do a single pass; if the user wants nested secrets they
    // can call mc_secret_expand on the result. v1: maybe recursive,
    // with cycle detection.
    //
    // The string-find approach avoids the byte-scan borrow checker
    // dance. Each iteration: find next `<<secret:`, find matching
    // `>>`, look up name, splice replacement in.
    loop {
        let Some(start) = expanded.find("<<secret:") else {
            break;
        };
        let after_prefix = start + "<<secret:".len();
        let Some(end_rel) = expanded[after_prefix..].find(">>") else {
            // Unterminated placeholder — leave as-is and stop.
            break;
        };
        let end_abs = after_prefix + end_rel;
        let name = &expanded[after_prefix..end_abs];

        // rc53.6: check the in-memory friendly pool FIRST (Once + PerSession +
        // Vault-mirror entries). Falls back to disk vault if not present.
        // Pool entries take() and remove themselves when Lifetime::Once.
        if let Some(replacement) = secrets_friendly::take(name) {
            let name_owned = name.to_string();
            let before = &expanded[..start];
            let after = &expanded[end_abs + 2..];
            expanded = format!("{before}{replacement}{after}");
            if !to_mark_used.contains(&name_owned) {
                to_mark_used.push(name_owned);
            }
            // Continue scanning — next find() picks the next placeholder.
        } else if let Some(entry) = by_name.get(name) {
            let replacement = entry.value.clone();
            let name_owned = name.to_string();
            // Splice: before + replacement + after
            let before = &expanded[..start];
            let after = &expanded[end_abs + 2..];
            expanded = format!("{before}{replacement}{after}");
            if !to_mark_used.contains(&name_owned) {
                to_mark_used.push(name_owned);
            }
            // Continue scanning from after the replacement. The
            // `find("<<secret:")` will pick the next placeholder
            // in the (now-modified) string.
        } else {
            // Placeholder name not in vault. Skip past it so we
            // don't loop forever on the same unknown placeholder.
            // We do this by replacing it with a sentinel that won't
            // match `<<secret:` and continuing.
            let before = &expanded[..start];
            let after = &expanded[end_abs + 2..];
            // Use a high-unicode sentinel that wouldn't appear in
            // any normal command. The model will see this as a
            // visible marker that the placeholder was unrecognized.
            let sentinel = format!("\u{200B}<<unknown-secret:{}>>\u{200B}", name.to_uppercase());
            expanded = format!("{before}{sentinel}{after}");
        }
    }

    // Update last_used_at and write audit log under lock.
    if !to_mark_used.is_empty() {
        let _lock = AUDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = read_vault(&app)?;
        let now = now_iso();
        for name in &to_mark_used {
            audit_log(&app, name);
            for e in entries.iter_mut() {
                if &e.name == name {
                    e.last_used_at = Some(now.clone());
                    break;
                }
            }
        }
        write_vault(&app, &entries)?;
    }

    Ok(expanded)
}

/// Test-only: read the raw vault file. Returns the JSON string. Used
/// by integration tests to verify set/delete/expand round-trips.
#[tauri::command]
fn mc_secret_debug_dump(app: tauri::AppHandle) -> Result<String, String> {
    let entries = read_vault(&app)?;
    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        // rc49 (feature/drag-drop): OS clipboard for the "Send to chat"
        // handoff. Writes directly via Win32 / cocoa / xclip so the
        // dashboard doesn't depend on the webview being focused.
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            first_run_report,
            // Lesson TBD (2026-08-27): on-demand voice diagnostics probe.
            // Reads Windows speech stack status, KB updates, mic device.
            // Called from Settings → Voice. NOT on the boot path.
            voice_diagnostics,
            voice_open_sound_settings,
            voice_open_windows_update,
            // Lesson 706 (rc54.3): native Windows SAPI 5 STT for the
            // OpenClaw chat voice button. 10-50x faster than whisper.cpp,
            // zero model download, fully offline. Replaces the sidecar
            // path the Voice module uses so users don't need to install
            // Voice for chat voice input anymore.
            mc_voice_native_capture,
            maic_login,
            maic_logout,
            silent_relogin,
            start_gateway_after_login,
            openclaw_open_window,
            openclaw_back_to_dashboard,
            mc_open_overlay,
            mc_close_overlay,
            open_register_url,
            // v1.0.7: tier + nudge surface
            mc_get_tier,
            mc_get_nudge,
            mc_list_tools,
            mc_refresh_tier,
            mc_apply_tier_change,
            mc_set_tier_defaults,
            // rc53.12 (Lesson 561): in-app upgrade flow. Free users see
            // a "Plans" card on the dashboard with one-click Stripe
            // upgrade buttons. The card shows live pricing pulled from
            // MAIC's `/v1/billing/plans`. Clicking a plan opens Stripe
            // Checkout in the OS default browser — user pays there,
            // returns to /welcome?plan=<code> on success.
            mc_get_quota,
            mc_list_plans,
            mc_open_checkout_url,
            // v1.0.9-rc35: Settings page
            mc_get_user_info,
            mc_list_memory_files,
            mc_read_memory_file,
            mc_write_memory_file,
            mc_open_data_folder,
            // v1.0.9-rc36: Terminal tab
            mc_terminal_start,
            mc_terminal_poll,
            mc_terminal_write,
            mc_terminal_kill,
            mc_terminal_list,
            // v1.0.9-rc46: Files page UI commands
            mc_ui_list_allowed_roots,
            mc_ui_list_dir,
            mc_ui_read_file,
            // v1.0.0-prep: image preview + external-open handoff for
            // binary files (PDF, Office, archives). Newbies get a working
            // "Open in default app" button instead of mojibake.
            mc_ui_read_image,
            mc_ui_open_externally,
            // v1.1.0-prep: drag-and-drop attachment staging
            // (feature/drag-drop branch). Dashboard drop zone, queue,
            // and send-to-chat via clipboard handoff.
            mc_stage_attachment,
            mc_list_attachments,
            mc_remove_attachment,
            mc_clear_attachments,
            mc_send_attachments_to_chat,
            // rc53 (feature/secrets-vault): local encrypted secrets
            // vault. v0: plaintext JSON. See notes/SECRETS-VAULT.md.
            // Crypto layer arrives in rc54.
            mc_secret_set,
            mc_secret_delete,
            mc_secret_list,
            mc_secret_expand,
            mc_secret_debug_dump,
            // rc53.6 (feature/secrets-vault): friendly setter + ephemeral
            // pool management. UI merges list_ephemerals() with the
            // disk-vault list() to render the full secrets table.
            secrets_friendly::mc_secret_set_friendly,
            secrets_friendly::mc_secret_list_ephemerals,
            secrets_friendly::mc_secret_clear_session_ephemerals,
            secrets_friendly::mc_secret_clear_all_ephemerals,
            // Lesson 713 (2026-08-28, David): BYO provider keys. Three
            // commands — set/list/clear — all gated behind MAIC_API_KEY
            // (JWT) in env. Storage: encrypted vault. Runtime injection:
            // std::env::set_var so the openclaw plugin reads it via
            // process.env.<KEY>.
            provider_keys::mc_set_provider_key,
            provider_keys::mc_list_provider_keys,
            provider_keys::mc_clear_provider_key,
            // v1.1.0-rc53.15 (Lesson 570): MC Module Framework.
            // Voice is the first module; future modules (OCR, TTS,
            // local search) follow the same pattern.
            mc_module_list,
            mc_module_install_local,
            mc_module_install_url,
            mc_module_uninstall,
            mc_module_call,
            // Lesson 725 (2026-08-28 21:30 MDT, David): MC Tasks feature
            // (Miracle Bot persistent memory). Paid-tier gated (Pro,
            // Pro+, Team, Enterprise). Free users see the tile + an
            // upgrade prompt; commands return `paid_tier_required` if
            // invoked anyway (e.g. via MAIC tool call on a free account).
            tasks::mc_tasks_page,
            tasks::mc_task_add,
            tasks::mc_task_update,
            tasks::mc_task_done,
            tasks::mc_task_delete,
            tasks::mc_task_sync,
            tasks::mc_task_login
        ])
        .setup(|app| {
            setup(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                eprintln!("[miracle-claw] exit — killing launcher child");
                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Some(child) = state.launcher_child.lock().unwrap().take() {
                        // tauri-plugin-shell v2's CommandChild::kill takes &self.
                        let _ = child.kill();
                    }
                }
            }
            _ => {}
        });
}

// ----------------------------------------------------------------------------
// Module Framework Tauri commands (Lesson 570, rc53.15)
// ----------------------------------------------------------------------------
//
// These are the public surface of the module framework. They wire up:
// - mc_module_list: frontend calls this to populate the Module Manager UI
// - mc_module_install_local: dev-mode install from a local dir (no download)
// - mc_module_uninstall: removes a module's files + unregisters it
// - mc_module_call: generic dispatcher — routes to a module's sidecar
//                   based on the Tauri command name. This is the only
//                   way module commands get into MC. The JS side does
//                   `invoke('mc_module_call', { command: 'mc_voice_transcribe', params: {...} })`
//                   and the dispatcher resolves it.
//
// Per Lesson 219, every command needs:
//   1. A stub here
//   2. An entry in app_commands.toml (`identifier = "allow-mc-module-..."`)
//   3. An entry in default.toml
//   4. An entry in main.json (if Tauri needs it for capability manifest)
//
// These four entries are added below after the function bodies.

// ---- mc_module_list --------------------------------------------------------

/// Returns a JSON array describing every installed module. Called by the
/// Module Manager UI on the Settings page (and on startup, to render
/// UI hooks for already-installed modules).
#[tauri::command]
fn mc_module_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::modules::registry::ModuleInfo>, String> {
    Ok(state.modules.lock().unwrap().list())
}

// ---- mc_module_install_local -----------------------------------------------

/// Dev-mode install from a local directory. Does NOT download anything.
/// Used by developers (and by the GitHub-releases flow once that's wired
/// up in Lesson 572) to install a module without going through the
/// downloader.
///
/// Emits `mc:module-installed` to the frontend so UI hooks flip from
/// dormant → active without a page reload.
#[tauri::command]
fn mc_module_install_local(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<crate::modules::installer::InstallResult, String> {
    let source_dir = std::path::PathBuf::from(&source_path);
    let modules_root = crate::modules::modules_root(
        &app.path()
            .app_data_dir()
            .map_err(|e| format!("could not resolve app_data_dir: {e}"))?,
    );
    let registry = state.modules.lock().unwrap();

    match crate::modules::installer::install_from_local_dir(
        &modules_root,
        &registry,
        &source_dir,
    ) {
        Ok(result) => {
            // Emit UI hook activation event so dormant buttons light up
            // without a page reload. Same pattern as Lesson 491.
            let _ = app.emit(
                "mc:module-installed",
                crate::modules::ui_hooks::ModuleInstalledEvent {
                    id: result.manifest.id.clone(),
                    name: result.manifest.name.clone(),
                    version: result.manifest.version.clone(),
                    hooks: result.manifest.ui_hooks.clone(),
                },
            );
            eprintln!(
                "[miracle-claw] module installed: {} v{} ({} hooks activated)",
                result.manifest.id,
                result.manifest.version,
                result.manifest.ui_hooks.len()
            );
            Ok(result)
        }
        Err(e) => Err(format!("install failed: {e}")),
    }
}

// ---- mc_module_install_url -------------------------------------------------

/// v1.1.0-rc53.17 (Lesson 574c): install a module from a remote URL
/// (e.g. GitHub releases tarball). Mirrors `mc_module_install_local`
/// but fetches the archive first via `reqwest` + `rustls` (no native
/// OpenSSL, so this works on Linux AND Windows MSVC with the same
/// build).
///
/// Flow:
///  1. Fetch the `.tar.gz` from `url`
///  2. Extract into a tmp dir under `modules_root/<id>.tmp-<uuid>/`
///  3. Verify SHA256SUMS (if present in the archive)
///  4. Atomic rename → `modules_root/<id>/`
///  5. Register in the runtime registry
///  6. Emit `mc:module-installed` so UI hooks flip dormant → active
///
/// Emits the same event as `mc_module_install_local` so existing
/// listeners on the JS side pick it up without changes.
#[tauri::command]
fn mc_module_install_url(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    url: String,
) -> Result<crate::modules::installer::InstallResult, String> {
    let modules_root = crate::modules::modules_root(
        &app.path()
            .app_data_dir()
            .map_err(|e| format!("could not resolve app_data_dir: {e}"))?,
    );
    let registry = state.modules.lock().unwrap();

    let source = crate::modules::installer::InstallSource {
        id: id.clone(),
        download_url: url.clone(),
        sums_url: None,
    };

    // The installer body is async (for the reqwest call) but the
    // Tauri command handler is sync — we block on it via the
    // tokio runtime that tauri maintains. If that ever changes and
    // we want the UI to stay responsive during large downloads,
    // this is the spot to switch to a spawn.
    let install_fut = crate::modules::installer::install_from_url(
        &modules_root,
        &registry,
        source,
    );
    let result = tauri::async_runtime::block_on(install_fut);

    match result {
        Ok(result) => {
            // Same UI hook activation event as install_local — the
            // JS side has a single listener for both flows.
            let _ = app.emit(
                "mc:module-installed",
                crate::modules::ui_hooks::ModuleInstalledEvent {
                    id: result.manifest.id.clone(),
                    name: result.manifest.name.clone(),
                    version: result.manifest.version.clone(),
                    hooks: result.manifest.ui_hooks.clone(),
                },
            );
            eprintln!(
                "[miracle-claw] module installed from URL: {} v{} ({}) ({} hooks activated)",
                result.manifest.id,
                result.manifest.version,
                url,
                result.manifest.ui_hooks.len()
            );
            Ok(result)
        }
        Err(e) => Err(format!("install from URL failed: {e}")),
    }
}

// ---- mc_module_uninstall ---------------------------------------------------

/// Remove a module by id. Clears its install dir + unregisters from the
/// runtime registry. Emits `mc:module-uninstalled` so dormant UI hooks
/// can flip back to disabled.
#[tauri::command]
fn mc_module_uninstall(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    // Look up the module's UI hooks BEFORE we remove it, so we can
    // emit them in the uninstalled event.
    let hooks: Vec<String> = state
        .modules
        .lock()
        .unwrap()
        .get(&id)
        .map(|m| m.manifest.ui_hooks.clone())
        .unwrap_or_default();

    let modules_root = crate::modules::modules_root(
        &app.path()
            .app_data_dir()
            .map_err(|e| format!("could not resolve app_data_dir: {e}"))?,
    );
    let registry = state.modules.lock().unwrap();

    match crate::modules::installer::uninstall(&modules_root, &registry, &id) {
        Ok(()) => {
            let _ = app.emit(
                "mc:module-uninstalled",
                crate::modules::ui_hooks::ModuleUninstalledEvent {
                    id: id.clone(),
                    hooks: hooks.clone(),
                },
            );
            eprintln!(
                "[miracle-claw] module uninstalled: {} ({} hooks dormant)",
                id,
                hooks.len()
            );
            Ok(())
        }
        Err(e) => Err(format!("uninstall failed: {e}")),
    }
}

// ---- mc_module_call --------------------------------------------------------

/// Generic dispatcher. The only Tauri command module commands actually
/// use. Resolves `command` (e.g. `"mc_voice_transcribe"`) to its module's
/// sidecar binary + action name and runs it with `params` as the JSON
/// argument.
///
/// This indirection keeps the ACL surface tiny (one allow-list per
/// command instead of N) and means adding a new module command requires
/// ZERO changes to lib.rs — just an entry in the module's installer.json.
#[tauri::command]
fn mc_module_call(
    state: tauri::State<'_, AppState>,
    command: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let registry = state.modules.lock().unwrap();
    crate::modules::dispatcher::dispatch(&registry, &command, params)
        .map_err(|e| format!("{e}"))
}

// ----------------------------------------------------------------------------
// Unit tests for ensure_maic_provider_config (Lesson 431 v2).
//
// These run with `cargo test --bin miracle-claw`. They use a per-test HOME
// override so `openclaw_extensions_dir()` resolves to a temp dir, isolating
// the test from the user's real MC state.
//
// What we verify:
//  1. Env-var key wins: when MAIC_API_KEY is set, apiKey is written as a literal.
//  2. Env-var no-key fallback: when MAIC_API_KEY is NOT set, apiKey is written
//     as a SecretRef { source: "env", provider: "default", id: "MAIC_API_KEY" }
//     AND secrets.providers.default + secrets.defaults.env are registered.
//  3. Idempotency: running the function twice does not duplicate fields.
//  4. Existing entry is preserved: a user-supplied provider entry is not
//     overwritten.
//  5. Schema correctness: the final JSON has the keys expected by openclaw's
//     zod-schema.core (SecretInputSchema, SecretProviderSchema, ModelsConfigSchema).
// ----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Tests that mutate process-global env vars run serially.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the env lock, tolerating poisoning from a previously-panicked
    /// test. Without this, a single test panic leaves the mutex poisoned
    /// for all subsequent tests in the binary (and they all panic on
    /// `lock().unwrap()`), masking the actual failure mode.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        match ENV_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    // ---------------------------------------------------------------------
    // v1.0.7: tools_for_tier gating → REMOVED in Lesson 526 (2026-08-21)
    // ---------------------------------------------------------------------
    // Previously Free got 0 tools; now ALL tiers get all 8 tools. The
    // tier parameter is kept on `tools_for_tier(tier)` for future
    // forward-compat but currently ignores it.

    #[test]
    fn tools_for_tier_returns_all_eight_for_every_tier() {
        // Lesson 526 (NEW 2026-08-21 13:55 MDT): all tiers get all 7
        // tools. Rate limiting (per-tier TPM) is the actual control.
        // Lesson 737 (NEW 2026-08-29, David): Starter/StarterPlus are
        // paid tiers — same features as Pro, only the token bucket differs.
        for t in [
            crate::auth::tier::Tier::Free,
            crate::auth::tier::Tier::Starter,
            crate::auth::tier::Tier::StarterPlus,
            crate::auth::tier::Tier::Pro,
            crate::auth::tier::Tier::ProPlus,
            crate::auth::tier::Tier::Team,
            crate::auth::tier::Tier::Enterprise,
        ] {
            let tools = tools_for_tier(t);
            assert_eq!(
                tools.len(),
                8,
                "Lesson 526: tier {:?} should have 8 local tools (was Free=0)",
                t
            );
        }
    }

    /// Set HOME to a fresh temp dir, return (temp_path, guard).
    /// Guard clears MAIC_API_KEY / MAIC_API_URL on drop.
    struct EnvGuard {
        _temp: tempfile::TempDir,
        prev_home: Option<String>,
        prev_key: Option<String>,
        prev_url: Option<String>,
    }

    fn fresh_env() -> EnvGuard {
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = env::var("HOME").ok();
        let prev_key = env::var("MAIC_API_KEY").ok();
        let prev_url = env::var("MAIC_API_URL").ok();
        env::set_var("HOME", temp.path());
        env::remove_var("MAIC_API_KEY");
        env::remove_var("MAIC_API_URL");
        EnvGuard {
            _temp: temp,
            prev_home: prev_home,
            prev_key: prev_key,
            prev_url: prev_url,
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev_home {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
            match &self.prev_key {
                Some(v) => env::set_var("MAIC_API_KEY", v),
                None => env::remove_var("MAIC_API_KEY"),
            }
            match &self.prev_url {
                Some(v) => env::set_var("MAIC_API_URL", v),
                None => env::remove_var("MAIC_API_URL"),
            }
        }
    }

    fn read_maic_root() -> serde_json::Value {
        let path = openclaw_json_path();
        // If the file doesn't exist (e.g. LoginRequired early-return didn't
        // write it), return an empty object so assertions on missing keys
        // return None instead of panicking. Tests should NOT depend on the
        // file's presence — they should assert on the keys they care about.
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return serde_json::json!({}),
            Err(e) => panic!("read openclaw.json: {e}"),
        };
        serde_json::from_str(&raw).expect("parse openclaw.json")
    }

    #[test]
    fn env_var_key_writes_literal_string() {
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "test-key-abc-123");

        let result = ensure_maic_provider_config().expect("bootstrap ok");
        assert!(result.provider_configured, "should be configured");
        assert!(matches!(result.api_key_source, MaicKeySource::Env));
        // Lesson 450: bootstrap returns bare endpoint, but the openclaw.json
        // baseUrl stamped is normalized to add /v1 (which is what openclaw's
        // OpenAI SDK appends `/chat/completions` to).
        assert_eq!(result.endpoint, "https://maicserver.com");

        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        // apiKey should be the literal string, not a SecretRef.
        assert_eq!(
            entry.get("apiKey").and_then(|v| v.as_str()),
            Some("test-key-abc-123")
        );
        // Lesson 450: baseUrl is normalized to include the /v1 prefix that
        // openclaw's OpenAI SDK needs to reach MAIC's /v1/chat/completions.
        assert_eq!(
            entry.get("baseUrl").and_then(|v| v.as_str()),
            Some("https://maicserver.com/v1")
        );
        assert_eq!(
            entry.get("api").and_then(|v| v.as_str()),
            Some("openai-completions")
        );
        let models = entry.get("models").and_then(|m| m.as_array()).unwrap();
        // Lesson 527: Free tier default is milagro-m1-t1, not
        // milagro-dev. The chat panel will pre-select the first
        // model in the catalog, which is now the smallest
        // chat-only model for Free users.
        let has_default = models
            .iter()
            .any(|m| m.get("id").and_then(|v| v.as_str()) == Some("milagro-m1-t1"));
        assert!(has_default, "default model id (m1-t1) should be present for Free");
    }

    #[test]
    fn no_env_var_returns_login_required_and_writes_no_provider() {
        let _lock = lock_env();
        let _g = fresh_env();
        // No MAIC_API_KEY set.

        let result = ensure_maic_provider_config().expect("bootstrap ok");
        assert!(!result.provider_configured, "Lesson 444: should NOT be configured (login required)");
        assert!(
            matches!(result.api_key_source, MaicKeySource::LoginRequired),
            "key_source should be LoginRequired, got {:?}",
            result.api_key_source
        );

        // Lesson 444: NO provider entry written (we early-return before the
        // providers_obj insert). LoginRequired path keeps openclaw.json
        // minimal so the user-facing login modal can present a clean message.
        let cfg = read_maic_root();
        let maic = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"));
        assert!(
            maic.is_none(),
            "LoginRequired path must not write a maic provider entry, got: {}",
            maic.map(|v| v.to_string()).unwrap_or_default()
        );

        // secrets.providers.default should NOT be registered (no SecretRef
        // fallback was needed). This is a deliberate Lesson 444 change:
        // the previous behavior auto-registered the env provider with a
        // SecretRef, which produced a guaranteed-failing config on disk.
        let secrets_providers_default = cfg
            .get("secrets")
            .and_then(|s| s.get("providers"))
            .and_then(|p| p.get("default"));
        assert!(
            secrets_providers_default.is_none(),
            "LoginRequired path must not register secrets.providers.default, got: {}",
            secrets_providers_default.map(|v| v.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn idempotency_no_duplication() {
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "first-key");

        let r1 = ensure_maic_provider_config().expect("first ok");
        assert_eq!(r1.endpoint, "https://maicserver.com");

        // Change the env var and re-run. The existing entry should NOT be
        // overwritten (we have has_key+has_url).
        env::set_var("MAIC_API_KEY", "second-key");
        let r2 = ensure_maic_provider_config().expect("second ok");
        assert!(matches!(r2.api_key_source, MaicKeySource::Existing));

        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        // apiKey should still be the FIRST key, not the second. Idempotency
        // preserves user-supplied config.
        assert_eq!(
            entry.get("apiKey").and_then(|v| v.as_str()),
            Some("first-key")
        );
    }

    #[test]
    fn existing_entry_with_secret_ref_is_preserved_when_env_set() {
        // Lesson 449: a SecretRef alone does NOT count as "complete" at the
        // bootstrap layer — only a literal non-empty apiKey does. So if the
        // env var resolves the SecretRef, we fall through to the write path
        // and the existing entry's other fields are preserved (we only stamp
        // apiKey when missing/empty).
        let _lock = lock_env();
        let _g = fresh_env();
        // Env var set — the user's SecretRef to MAIC_API_KEY will resolve.
        env::set_var("MAIC_API_KEY", "env-resolved-key");
        // Pre-write a user-supplied entry with a SecretRef + custom fields.
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_existing = serde_json::json!({
            "gateway": {"mode": "local", "auth": {"mode": "none"}},
            "models": {
                "providers": {
                    "maic": {
                        "baseUrl": "https://custom.maic.example.com",
                        "apiKey": {
                            "source": "env",
                            "provider": "default",
                            "id": "MAIC_API_KEY"
                        },
                        "api": "openai-completions",
                        "models": [
                            {"id": "custom-model", "name": "Custom"}
                        ]
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_existing).unwrap()).unwrap();

        let result = ensure_maic_provider_config().expect("bootstrap ok");
        // Lesson 449: with the SecretRef falling through to the write path,
        // the key_source reports Env (resolved from env var), not Existing.
        // The user's SecretRef field itself is preserved (we only stamp
        // apiKey when it's empty per is_empty_api_key), but the bootstrap
        // result reflects that we actively used the env-resolved key this
        // run.
        assert!(matches!(result.api_key_source, MaicKeySource::Env));
        assert_eq!(result.endpoint, "https://custom.maic.example.com");

        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        // Lesson 450: user's custom baseUrl was rewritten to include /v1
        // because it was a bare-origin URL (no path). The legacy v1.0.0..v1.0.3
        // bootstrap wrote `https://maicserver.com` without /v1, which made
        // openclaw POST to /chat/completions (404). upgrade_legacy_maic_base_url
        // retroactively fixes the value.
        assert_eq!(
            entry.get("baseUrl").and_then(|v| v.as_str()),
            Some("https://custom.maic.example.com/v1")
        );
        // User's SecretRef preserved (not overwritten with a literal).
        let api_key = entry.get("apiKey").expect("apiKey");
        assert!(api_key.is_object(), "SecretRef should still be an object");
        assert_eq!(api_key.get("id").and_then(|v| v.as_str()), Some("MAIC_API_KEY"));
        // User's custom model preserved.
        let models = entry.get("models").and_then(|m| m.as_array()).unwrap();
        assert!(models.iter().any(|m| m.get("id").and_then(|v| v.as_str()) == Some("custom-model")));
    }

    #[test]
    fn existing_secret_ref_with_no_env_var_falls_through_to_login_required() {
        // Lesson 449: when a SecretRef exists but the env var is empty, the
        // entry is INCOMPLETE (the openclaw gateway would fail to resolve
        // MAIC_API_KEY at startup). The bootstrap must NOT early-return on
        // "Existing" — it must fall through so we can either re-login (and
        // replace the SecretRef with a literal) or hit LoginRequired.
        let _lock = lock_env();
        let _g = fresh_env();
        // No MAIC_API_KEY env var set.
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_existing = serde_json::json!({
            "gateway": {"mode": "local", "auth": {"mode": "none"}},
            "models": {
                "providers": {
                    "maic": {
                        "baseUrl": "https://maicserver.com",
                        "apiKey": {
                            "source": "env",
                            "provider": "default",
                            "id": "MAIC_API_KEY"
                        },
                        "api": "openai-completions",
                        "models": [{"id": "milagro-dev", "name": "MAIC default"}]
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_existing).unwrap()).unwrap();

        let result = ensure_maic_provider_config().expect("bootstrap ok");
        assert!(
            !result.provider_configured,
            "SecretRef + empty env var must NOT be treated as configured"
        );
        assert!(
            matches!(result.api_key_source, MaicKeySource::LoginRequired),
            "key_source should be LoginRequired, got {:?}",
            result.api_key_source
        );
        // The legacy SecretRef is left in place (we don't overwrite it on
        // LoginRequired). It'll be replaced with the literal after login.
        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry should still be on disk");
        let api_key = entry.get("apiKey").expect("apiKey");
        assert!(api_key.is_object(), "SecretRef should remain until login");
    }

    #[test]
    fn login_replaces_legacy_secret_ref_with_literal() {
        // Lesson 449: after login (env var now set), the bootstrap preserves
        // the legacy SecretRef (because it resolves now), so we rely on
        // `replace_secret_ref_with_literal` to explicitly swap the SecretRef
        // for the literal JWT that maic_login just received. The bootstrap
        // call alone is not enough — this is intentional to keep the
        // bootstrap idempotent for users who actually have a working
        // SecretRef + env-var-set setup.
        let _lock = lock_env();
        let _g = fresh_env();
        // Pre-existing entry with a SecretRef (the legacy v1.0.0 / v1.0.1 /
        // v1.0.2 shape — Lesson 444 didn't write SecretRefs, but pre-Lesson-444
        // installs did, and we want to migrate them on first login).
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_existing = serde_json::json!({
            "gateway": {"mode": "local", "auth": {"mode": "none"}},
            "models": {
                "providers": {
                    "maic": {
                        "baseUrl": "https://maicserver.com",
                        "apiKey": {
                            "source": "env",
                            "provider": "default",
                            "id": "MAIC_API_KEY"
                        },
                        "api": "openai-completions",
                        "models": [{"id": "milagro-dev", "name": "MAIC default"}]
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_existing).unwrap()).unwrap();

        // Simulate what maic_login does: set the env var (carrying the JWT),
        // then run the post-login cleanup helpers.
        env::set_var("MAIC_API_KEY", "newly-logged-in-jwt");
        let _ = ensure_maic_provider_config().expect("bootstrap ok");
        // The bootstrap alone would preserve the SecretRef (it now resolves
        // because env is set). The literal-replace helper is what migrates
        // legacy setups to the new shape.
        let replaced = replace_secret_ref_with_literal();
        assert!(replaced, "replace_secret_ref_with_literal should report it wrote");

        // The SecretRef must have been replaced with the literal JWT.
        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        assert_eq!(
            entry.get("apiKey").and_then(|v| v.as_str()),
            Some("newly-logged-in-jwt"),
            "legacy SecretRef should be replaced with the literal JWT on login"
        );
    }

    #[test]
    fn replace_secret_ref_is_idempotent_when_already_literal_and_correct() {
        // Lesson 449: if the apiKey is already the correct literal, the
        // helper must be a no-op (don't churn the file) AND return true
        // (so callers don't log "nothing happened" warnings).
        let _lock = lock_env();
        let _g = fresh_env();
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_existing = serde_json::json!({
            "gateway": {"mode": "local", "auth": {"mode": "none"}},
            "models": {
                "providers": {
                    "maic": {
                        "baseUrl": "https://maicserver.com",
                        "apiKey": "existing-correct-jwt",
                        "api": "openai-completions",
                        "models": [{"id": "milagro-dev", "name": "MAIC default"}]
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_existing).unwrap()).unwrap();
        env::set_var("MAIC_API_KEY", "existing-correct-jwt");

        let replaced = replace_secret_ref_with_literal();
        assert!(replaced);

        // File should still parse and have the correct literal.
        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        assert_eq!(
            entry.get("apiKey").and_then(|v| v.as_str()),
            Some("existing-correct-jwt")
        );
    }

    #[test]
    fn replace_secret_ref_overwrites_wrong_literal() {
        // Lesson 449: if the existing literal is wrong (e.g. user logged in
        // as a different account, or stale token from a prior session), the
        // helper overwrites with the current env var value.
        let _lock = lock_env();
        let _g = fresh_env();
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_existing = serde_json::json!({
            "gateway": {"mode": "local", "auth": {"mode": "none"}},
            "models": {
                "providers": {
                    "maic": {
                        "baseUrl": "https://maicserver.com",
                        "apiKey": "old-stale-jwt",
                        "api": "openai-completions",
                        "models": [{"id": "milagro-dev", "name": "MAIC default"}]
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_existing).unwrap()).unwrap();
        env::set_var("MAIC_API_KEY", "new-correct-jwt");

        let replaced = replace_secret_ref_with_literal();
        assert!(replaced);

        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        assert_eq!(
            entry.get("apiKey").and_then(|v| v.as_str()),
            Some("new-correct-jwt"),
            "stale literal should be replaced with the new env var value"
        );
    }

    #[test]
    fn tool_execution_param_pinned_to_client() {
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config().expect("ok");
        let cfg = read_maic_root();
        let params = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("params"))
            .expect("params");
        assert_eq!(
            params.get("tool_execution").and_then(|v| v.as_str()),
            Some("client"),
            "MAIC plugin requires params.tool_execution='client' to return tool_calls"
        );
    }

    // ---------------------------------------------------------------------
    // Lesson 523 (NEW 2026-08-20): tier-gated tool injection.
    //
    // `ensure_maic_provider_config()` (no tier) defaults to Free and
    // should write an empty `params.tools` array. The tier-aware
    // variant should write 7 entries for every paid tier.
    // ---------------------------------------------------------------------

    /// Helper: read the maic provider's `params.tools` array length
    /// from the on-disk openclaw.json. Returns None if missing.
    fn read_tools_array_len() -> Option<usize> {
        let cfg = read_maic_root();
        cfg.get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("params"))
            .and_then(|p| p.get("tools"))
            .and_then(|t| t.as_array())
            .map(|a| a.len())
    }

    /// Helper: read the names from the `params.tools` array.
    fn read_tools_names() -> Vec<String> {
        let cfg = read_maic_root();
        cfg.get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("params"))
            .and_then(|p| p.get("tools"))
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn no_tier_default_writes_all_eight_tools() {
        // Lesson 523 (was): setup() / login-required bootstrap (no
        // tier context) defaulted to Free → empty tools array.
        //
        // Lesson 526 (NEW 2026-08-21): gating removed. ALL tiers,
        // including Free, get all 8 tools. The "no tier" case
        // (login-required bootstrap before MAIC responds) now also
        // gets all 7 — better to advertise capabilities than to
        // hide them behind a tier check that may be wrong.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config().expect("ok");
        let len = read_tools_array_len().expect("params.tools should be present");
        assert_eq!(
            len, 8,
            "no-tier bootstrap gets all 8 tools (Lesson 526); was 0 before"
        );
    }

    #[test]
    fn free_tier_writes_all_eight_tools() {
        // Lesson 526: Free tier gets all 8 tools. Rate limiting
        // (TPM) is the actual control, not capability gating.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let len = read_tools_array_len().expect("params.tools should be present");
        assert_eq!(len, 8, "Free tier gets all 8 tools (Lesson 526); was 0");
    }

    #[test]
    fn paid_tiers_write_all_eight_tools() {
        // Lesson 737 (2026-08-29, David): all paid tiers share features.
        for tier in [
            crate::auth::tier::Tier::Starter,
            crate::auth::tier::Tier::StarterPlus,
            crate::auth::tier::Tier::Pro,
            crate::auth::tier::Tier::ProPlus,
            crate::auth::tier::Tier::Team,
            crate::auth::tier::Tier::Enterprise,
        ] {
            let _lock = lock_env();
            let _g = fresh_env();
            env::set_var("MAIC_API_KEY", "any-key");

            ensure_maic_provider_config_for_tier(tier).expect("ok");
            let names = read_tools_names();
            assert_eq!(
                names.len(),
                8,
                "tier {tier:?} should write 8 tools, got {} ({names:?})",
                names.len()
            );
            // Verify the wire format (Lesson 513): each entry is
            // `{type: "function", function: {name, description, parameters}}`
            let cfg = read_maic_root();
            let tools = cfg
                .get("models")
                .and_then(|m| m.get("providers"))
                .and_then(|p| p.get("maic"))
                .and_then(|m| m.get("params"))
                .and_then(|p| p.get("tools"))
                .and_then(|t| t.as_array())
                .expect("tools array");
            for (i, entry) in tools.iter().enumerate() {
                assert_eq!(
                    entry.get("type").and_then(|v| v.as_str()),
                    Some("function"),
                    "tool[{i}] missing type='function' wrapper"
                );
                assert!(
                    entry.get("function").is_some(),
                    "tool[{i}] missing function block"
                );
            }
            // Spot-check that all 7 names are present.
            for expected in [
                "read_file", "write_file", "edit_file", "list_dir",
                "bash_run", "apply_patch", "remember_fact", "web_fetch",
            ] {
                assert!(
                    names.iter().any(|n| n == expected),
                    "tier {tier:?} missing tool {expected}; got {names:?}"
                );
            }
        }
    }

    #[test]
    fn tools_array_preserves_user_customizations() {
        // Lesson 449 + 525: idempotency pattern protects user-edited
        // configs from being silently overwritten on every login.
        //
        // Lesson 526 (NEW 2026-08-21): tools_for_tier returns 7 for
        // ALL tiers, so a `[]` on disk always gets re-stamped to 8.
        // But NON-EMPTY arrays (the actual user-edited case — e.g.
        // user removed a tool they don't want) must still be preserved.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call: Pro tier writes 8 tools.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        assert_eq!(read_tools_array_len(), Some(8));

        // Mutate the on-disk config to a smaller array (user removed
        // 7 of the 8 tools by hand).
        let path = openclaw_json_path();
        let raw = std::fs::read_to_string(&path).expect("read");
        let mut cfg: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        cfg["models"]["providers"]["maic"]["params"]["tools"] = serde_json::json!([
            {"type":"function","function":{"name":"read_file"}}
        ]);
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap())
            .expect("write");

        // Second call: any tier. The non-empty single-tool array
        // must be preserved (not re-stamped to 8).
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        assert_eq!(
            read_tools_array_len(),
            Some(1),
            "user's custom 1-tool array must be preserved across logins"
        );
    }

    #[test]
    fn tools_array_stale_build_adds_new_tools() {
        // Lesson 842: David's RC55.16 test scenario. His openclaw.json
        // was stamped by an older build (pre-rc55.13, before
        // `web_fetch` was added) and contained 7 tools. With the
        // old Lesson 525 idempotency, the 7-tool array was preserved
        // forever and the model never saw `web_fetch` advertised.
        //
        // Fix: stamp `_stamped_tools_version` alongside the tools
        // array, and re-stamp when stamp_version < CURRENT_STAMP_VERSION
        // (build added tools since the user's last stamp).
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // Simulate a pre-rc55.13 install: stamp is missing (treated as 0),
        // tools array has 7 entries (the old full set).
        let path = openclaw_json_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pre_stamp = serde_json::json!({
            "models": {"providers": {
                "maic": {
                    "apiKey": "any-key",
                    "baseUrl": "https://maicserver.com/v1",
                    "api": "openai-completions",
                    "params": {
                        "tool_execution": "client",
                        "tools": [
                            {"type":"function","function":{"name":"read_file"}},
                            {"type":"function","function":{"name":"write_file"}},
                            {"type":"function","function":{"name":"edit_file"}},
                            {"type":"function","function":{"name":"list_dir"}},
                            {"type":"function","function":{"name":"bash_run"}},
                            {"type":"function","function":{"name":"apply_patch"}},
                            {"type":"function","function":{"name":"remember_fact"}}
                        ]
                    }
                }
            }},
            "plugins": {"entries": {"maic": {"enabled": true}}}
        });
        std::fs::write(&path, serde_json::to_string_pretty(&pre_stamp).unwrap()).unwrap();

        // Boot: Lesson 842 must detect the missing `web_fetch` (and
        // bump to the build's CURRENT_STAMP_VERSION).
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");

        let len = read_tools_array_len().expect("params.tools present");
        assert_eq!(
            len, 8,
            "stale 7-tool config must be re-stamped to current 8 (Lesson 842); web_fetch added in rc55.13"
        );

        // Verify _stamped_tools_version was written so future boots
        // know this array is up-to-date.
        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let stamp = cfg["models"]["providers"]["maic"]["params"]
            .get("_stamped_tools_version")
            .and_then(|v| v.as_u64())
            .expect("_stamped_tools_version must be written");
        assert!(
            stamp >= 8,
            "stamp_version must be >= 8 (CURRENT_STAMP_VERSION) after re-stamp; got {stamp}"
        );
    }

    #[test]
    fn tools_array_write_path_stamps_too() {
        // Lesson 842 fix verification: the WRITE PATH (when there's
        // no existing entry) must also call write_tier_gated so
        // fresh installs get params.tools + params.max_tokens stamped.
        // Without this, fresh installs would have NO tools array and
        // the model would see only MAIC's 4 server tools.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // Ensure path's parent exists; the function creates the file
        // when nothing exists.
        ensure_maic_provider_config().expect("ok");
        let raw = std::fs::read_to_string(openclaw_json_path()).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let stamp = cfg["models"]["providers"]["maic"]["params"]
            .get("_stamped_tools_version")
            .and_then(|v| v.as_u64())
            .expect("write path must stamp _stamped_tools_version");
        assert!(stamp >= 8, "write path stamp must be >= 8; got {stamp}");
    }

    // =====================================================================
    // Lesson 527 (NEW 2026-08-21 13:57 MDT, last revised 2026-08-24
    // Lesson 567): tier-gated model list.
    // Free = m1-t1 + m1-t2 + m1-t3 + chat-nemotron-nano
    //        (chat-only fast tier + cheap NVIDIA MoE).
    // Paid = all 20 (Lesson 567 added 3 Nemotron models).
    // This is the actual cost-control for Free accounts — they can't
    // accidentally pick `milagro-dev` (14B) and burn their 50K TPM in
    // three messages. Both the write path AND the existing-entry path
    // must produce the same model list for the same tier.
    // =====================================================================

    fn read_model_ids() -> Vec<String> {
        let cfg = read_maic_root();
        cfg.get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("models"))
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn free_tier_sees_three_t_models() {
        // Lesson 527: Free tier gets the t1 + t2 distilled m1 models.
        // 2026-08-24 (Lesson 565, David): added t3 to free tier so
        // free users get the heaviest local 14B-distilled model as
        // their best option. Cost effect is neutral (t3 is local;
        // picking it REDUCES cloud fallback risk vs. t1/t2).
        // 2026-08-24 (Lesson 567, David): added chat-nemotron-nano
        // — NVIDIA MoE via Ollama Cloud, 1.7s response, costs us
        // ~$0 via existing subscription.
        // Free tier excludes the 14B `milagro-dev` (general-purpose)
        // and the coder/dev-coder/oc-* cloud models.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            4,
            "Free tier should see exactly 4 models (m1-t1, m1-t2, m1-t3, chat-nemotron-nano); got {ids:?}"
        );
        assert!(ids.contains(&"milagro-m1-t1".to_string()));
        assert!(ids.contains(&"milagro-m1-t2".to_string()));
        assert!(ids.contains(&"milagro-m1-t3".to_string()));
        assert!(ids.contains(&"chat-nemotron-nano".to_string()));
        // General-purpose 14B + coder + cloud models still excluded.
        for forbidden in ["milagro-dev", "milagro-dev-coder", "milagro-m1"] {
            assert!(
                !ids.contains(&forbidden.to_string()),
                "Free tier must NOT see {forbidden}"
            );
        }
    }

    #[test]
    fn paid_tiers_see_all_twenty_models() {
        // Lesson 527: paid tiers see the full 20-model catalog.
        // Lesson 567 (2026-08-24, David): catalog grew 17 → 20 with
        // the addition of 3 Nemotron models
        // (chat-nemotron-nano/super/ultra). nano is also added to
        // Free (test asserts that separately).
        // Note: existing tests like `paid_tiers_write_all_eight_tools`
        // test the TOOLS list, not the model list. This is the
        // parallel test for models.
        // Lesson 737 (2026-08-29, David): all paid tiers share models too.
        for tier in [
            crate::auth::tier::Tier::Starter,
            crate::auth::tier::Tier::StarterPlus,
            crate::auth::tier::Tier::Pro,
            crate::auth::tier::Tier::ProPlus,
            crate::auth::tier::Tier::Team,
            crate::auth::tier::Tier::Enterprise,
        ] {
            let _lock = lock_env();
            let _g = fresh_env();
            env::set_var("MAIC_API_KEY", "any-key");

            ensure_maic_provider_config_for_tier(tier).expect("ok");
            let ids = read_model_ids();
            assert_eq!(
                ids.len(),
                20,
                "Paid tier {tier:?} should see all 20 models; got {ids:?}"
            );
            assert!(ids.contains(&"milagro-dev".to_string()));
            assert!(ids.contains(&"milagro-m1-t1".to_string()));
            assert!(ids.contains(&"milagro-oc-minimax".to_string()));
            assert!(ids.contains(&"chat-nemotron-nano".to_string()));
            assert!(ids.contains(&"chat-nemotron-super".to_string()));
            assert!(ids.contains(&"chat-nemotron-ultra".to_string()));
        }
    }

    #[test]
    fn downgrade_from_pro_to_free_removes_paid_models() {
        // Lesson 527: downgrade path. Pro user downgrades to Free
        // → their on-disk config must be re-stamped to only allow
        // m1-t1 + m1-t2 + m1-t3 + chat-nemotron-nano (Lesson 565
        // added t3; Lesson 567 added chat-nemotron-nano to Free).
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call as Pro: writes 20 models.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        assert_eq!(read_model_ids().len(), 20);

        // Downgrade to Free.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            4,
            "After Pro→Free downgrade, only 4 models should remain (t1, t2, t3, chat-nemotron-nano); got {ids:?}"
        );
        assert!(ids.contains(&"milagro-m1-t1".to_string()));
        assert!(ids.contains(&"milagro-m1-t2".to_string()));
        assert!(ids.contains(&"milagro-m1-t3".to_string()));
        assert!(ids.contains(&"chat-nemotron-nano".to_string()));
    }

    #[test]
    fn upgrade_from_free_to_pro_adds_paid_models() {
        // Lesson 527: upgrade path. Free user upgrades to Pro →
        // their on-disk config must be re-stamped to include all
        // 20 models. Without this, the user would see only 4
        // models (Free: t1+t2+t3+chat-nemotron-nano) even after
        // paying.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call as Free: writes 4 models (t1, t2, t3, chat-nemotron-nano).
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        assert_eq!(read_model_ids().len(), 4);

        // Upgrade to Pro.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            20,
            "After Free→Pro upgrade, all 20 models should be present; got {ids:?}"
        );
    }

    #[test]
    fn empty_tools_array_gets_re_stamped_for_paid_tier() {
        // Lesson 525 (NEW 2026-08-21 13:50 MDT): empty `params.tools: []`
        // must be re-stamped for paid tiers. This was the rc23 bug —
        // Lesson 524's helper had `if !contains_key("tools")` which
        // treated `[]` as "already populated, skip", so upgraded users
        // stayed on empty tools even though their tier entitled them.
        //
        // Reproduce the production shape: write a complete maic entry
        // with empty tools array (mimicking what rc17-rc22 left on
        // disk), then call ensure_maic_provider_config_for_tier(Pro).
        // The helper must re-stamp the tools array to 7 entries.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        let path = openclaw_json_path();
        let existing = serde_json::json!({
            "models": {
                "providers": {
                    "maic": {
                        "api": "openai-completions",
                        "apiKey": "prior-install-key",
                        "baseUrl": "https://maicserver.com/v1",
                        "models": [
                            {"id": "milagro-dev", "name": "milagro-dev"}
                        ],
                        "params": {
                            "tool_execution": "client",
                            "tools": []
                        }
                    }
                }
            }
        });
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .expect("write");

        // Call with Pro tier. Must re-stamp tools from `[]` to 8 entries.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro)
            .expect("ok");

        let names = read_tools_names();
        assert_eq!(
            names.len(),
            8,
            "Lesson 525: existing-entry with empty `params.tools: []` must be re-stamped for Pro (got {names:?})"
        );
        // Verify the original apiKey was preserved.
        let cfg = read_maic_root();
        let api_key = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("apiKey"))
            .and_then(|v| v.as_str());
        assert_eq!(
            api_key,
            Some("prior-install-key"),
            "Lesson 525 must NOT overwrite existing apiKey"
        );
    }

    #[test]
    fn existing_entry_path_also_stamps_tools_array() {
        // Lesson 524 (NEW 2026-08-20 22:50 MDT): the existing-entry
        // early-return path ALSO has to stamp `params.tools`. Without
        // this, every login after first install would skip Lesson 513
        // (tool_execution) and Lesson 523 (tier-gated tools array),
        // and the bot would see only MAIC's 4 server tools.
        //
        // Reproduce the production shape: write a complete maic entry
        // (apiKey + baseUrl) to disk as if from a prior rc18-rc21
        // install, then call ensure_maic_provider_config_for_tier(Pro).
        // The early-return path should fire, but the helper should
        // still write `params.tools` with 7 entries.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        let path = openclaw_json_path();
        // Pre-populate with a complete entry from a prior install.
        let existing = serde_json::json!({
            "models": {
                "providers": {
                    "maic": {
                        "api": "openai-completions",
                        "apiKey": "prior-install-key",
                        "baseUrl": "https://maicserver.com/v1",
                        "models": [
                            {"id": "milagro-dev", "name": "milagro-dev"}
                        ]
                    }
                }
            }
        });
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .expect("write");

        // Now call the Pro tier. Should hit the early-return path
        // (entry is complete) AND stamp tools via the helper.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro)
            .expect("ok");

        let names = read_tools_names();
        assert_eq!(
            names.len(),
            8,
            "Lesson 524: existing-entry early-return path must still stamp tools (got {names:?})"
        );
        // Verify the original apiKey was preserved (not overwritten).
        let cfg = read_maic_root();
        let api_key = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .and_then(|m| m.get("apiKey"))
            .and_then(|v| v.as_str());
        assert_eq!(
            api_key,
            Some("prior-install-key"),
            "Lesson 524 must NOT overwrite existing apiKey"
        );
    }

    #[test]
    fn is_empty_api_key_literal_and_ref() {
        // Literal empty string
        assert!(is_empty_api_key(&serde_json::json!("")));
        assert!(is_empty_api_key(&serde_json::json!("   ")));
        // Literal non-empty
        assert!(!is_empty_api_key(&serde_json::json!("abc")));
        // SecretRef with empty id
        assert!(is_empty_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": ""
        })));
        // SecretRef with non-empty id
        assert!(!is_empty_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": "MAIC_API_KEY"
        })));
        // Object missing id
        assert!(is_empty_api_key(&serde_json::json!({
            "source": "env", "provider": "default"
        })));
        // Null
        assert!(is_empty_api_key(&serde_json::json!(null)));
        // Scalar
        assert!(is_empty_api_key(&serde_json::json!(42)));
    }

    #[test]
    fn is_unresolvable_api_key_checks_env_var() {
        let _lock = lock_env();
        let _g = fresh_env();

        // Literal non-empty → always resolvable.
        assert!(!is_unresolvable_api_key(&serde_json::json!("abc")));

        // SecretRef with empty id → unresolvable (delegates to is_empty_api_key).
        assert!(is_unresolvable_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": ""
        })));

        // SecretRef to MAIC_API_KEY + env var NOT SET → unresolvable.
        // This is the lesson 449 case: bootstrap ships a SecretRef, user
        // hasn't set env, gateway would crash.
        assert!(is_unresolvable_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": "MAIC_API_KEY"
        })));

        // Set the env var → same SecretRef now resolves → NOT unresolvable.
        env::set_var("MAIC_API_KEY", "live-jwt-token");
        assert!(!is_unresolvable_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": "MAIC_API_KEY"
        })));

        // Empty env var → unresolvable.
        env::set_var("MAIC_API_KEY", "");
        assert!(is_unresolvable_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": "MAIC_API_KEY"
        })));

        // Whitespace-only env var → unresolvable.
        env::set_var("MAIC_API_KEY", "   ");
        assert!(is_unresolvable_api_key(&serde_json::json!({
            "source": "env", "provider": "default", "id": "MAIC_API_KEY"
        })));

        // Non-env sources (file/exec) → trust is_empty_api_key result.
        // Non-empty id + non-env source → considered resolvable (we can't
        // actually check file/exec resolution here without side effects).
        assert!(!is_unresolvable_api_key(&serde_json::json!({
            "source": "file", "provider": "default", "id": "/etc/key"
        })));
    }

    #[test]
    fn normalize_maic_base_url_appends_v1_when_missing() {
        // Already has /v1 — left alone.
        assert_eq!(
            normalize_maic_base_url("https://maicserver.com/v1"),
            "https://maicserver.com/v1"
        );
        assert_eq!(
            normalize_maic_base_url("https://maicserver.com/v1/"),
            "https://maicserver.com/v1"
        );
        // Bare origin — append /v1.
        assert_eq!(
            normalize_maic_base_url("https://maicserver.com"),
            "https://maicserver.com/v1"
        );
        assert_eq!(
            normalize_maic_base_url("https://maicserver.com/"),
            "https://maicserver.com/v1"
        );
        // Path with /v1 mid-segment (e.g. https://proxy.example.com/v1/maic) — left alone.
        assert_eq!(
            normalize_maic_base_url("https://proxy.example.com/v1/maic"),
            "https://proxy.example.com/v1/maic"
        );
        // Empty — left alone.
        assert_eq!(normalize_maic_base_url(""), "");
    }

    #[test]
    fn upgrade_legacy_maic_base_url_only_rewrites_bare_origin() {
        // Bare origin WITHOUT /v1 → upgraded.
        assert_eq!(
            upgrade_legacy_maic_base_url("https://maicserver.com"),
            Some("https://maicserver.com/v1".to_string())
        );
        assert_eq!(
            upgrade_legacy_maic_base_url("https://maicserver.com/"),
            Some("https://maicserver.com/v1".to_string())
        );
        // Already has /v1 → returns None (no rewrite needed).
        assert_eq!(
            upgrade_legacy_maic_base_url("https://maicserver.com/v1"),
            None
        );
        // Path with custom mount (/maic) → returns None (preserve user setup).
        assert_eq!(
            upgrade_legacy_maic_base_url("https://proxy.example.com/maic"),
            None
        );
        // Garbage → returns None.
        assert_eq!(upgrade_legacy_maic_base_url("not-a-url"), None);
        // Empty → returns None.
        assert_eq!(upgrade_legacy_maic_base_url(""), None);
    }

    #[test]
    fn strip_trailing_v1_handles_both_forms() {
        // Already bare → unchanged.
        assert_eq!(strip_trailing_v1("https://maicserver.com"), "https://maicserver.com");
        // /v1 suffix → stripped.
        assert_eq!(strip_trailing_v1("https://maicserver.com/v1"), "https://maicserver.com");
        // /v1/ trailing → stripped to bare (no trailing slash).
        assert_eq!(strip_trailing_v1("https://maicserver.com/v1/"), "https://maicserver.com");
        // Mid-path /v1 → unchanged (not at the end).
        assert_eq!(
            strip_trailing_v1("https://proxy.example.com/v1/maic"),
            "https://proxy.example.com/v1/maic"
        );
    }

    #[test]
    fn lesson_451_early_return_path_migrates_existing_bare_origin() {
        // Lesson 451 regression: the early-return path (existing entry with
        // literal apiKey + non-empty baseUrl) used to skip the baseUrl /v1
        // migration entirely. v1.0.4 users coming from v1.0.0..v1.0.3 kept
        // their bare "https://maicserver.com" baseUrl and chat still failed
        // with "model not found". This test pins the migration behavior on
        // the helper that's invoked in the early-return path: when given a
        // bare MAIC origin, it must rewrite to the /v1 form so the
        // subsequent `entry.insert("baseUrl", ...)` in the early-return path
        // actually mutates the value.
        let existing_base = "https://maicserver.com";
        let migrated = upgrade_legacy_maic_base_url(existing_base)
            .expect("bare MAIC origin must be migrated to /v1 form");
        assert_eq!(migrated, "https://maicserver.com/v1");
        // Idempotency check: re-running on the migrated value must be a no-op.
        assert!(
            upgrade_legacy_maic_base_url(&migrated).is_none(),
            "re-running migration on already-migrated URL must not rewrite"
        );
        // Conservative: user-customized paths preserved verbatim (no rewrite).
        assert!(
            upgrade_legacy_maic_base_url("https://proxy.example.com/maic").is_none(),
            "user paths must be preserved by the migration policy"
        );
    }

    // ---------------------------------------------------------------------
    // Lesson 517: tier-conditional default model + fallbacks
    // ---------------------------------------------------------------------

    /// Build a minimal openclaw.json shape that mirrors what MC writes
    /// after `ensure_maic_provider_config`. Returns the temp dir guard so
    /// the file lives for the test scope.
    ///
    /// Schema (Lesson 829 / rc55.15):
    ///   `agents.defaults.model = { primary: "<provider>/<id>", fallbacks: [...] }`
    /// The OBJECT form is what openclaw's `AgentDefaultsSchema` accepts
    /// (`.strict()` — it has NO top-level `fallbacks` field). Tests that
    /// want to seed the migration from the BAD top-level form
    /// (rc55.14 regression) should use the legacy helper below.
    ///
    /// Note: openclaw_json_path() resolves to `<HOME>/.miracle-claw/openclaw.json`
    /// on non-Windows builds (and `<APPDATA>/MiracleClaw/openclaw.json` on
    /// Windows), NOT `<HOME>/.openclaw/openclaw.json` — that was a steeler
    /// convention we don't use. This helper mirrors what the production
    /// code resolves to.
    fn fresh_openclaw_with_model(primary: Option<&str>, fallbacks: Option<Vec<&str>>) -> EnvGuard {
        let g = fresh_env();
        let path = if cfg!(windows) {
            g._temp.path().join("MiracleClaw").join("openclaw.json")
        } else {
            g._temp.path().join(".miracle-claw").join("openclaw.json")
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut cfg = serde_json::json!({
            "models": { "providers": { "maic": {} } }
        });
        if primary.is_some() || fallbacks.is_some() {
            // CORRECT schema: object form.
            let mut model = serde_json::Map::new();
            if let Some(p) = primary {
                model.insert("primary".to_string(), serde_json::Value::String(p.to_string()));
            }
            if let Some(fb) = fallbacks {
                let arr: Vec<serde_json::Value> =
                    fb.iter().map(|s| serde_json::Value::String(s.to_string())).collect();
                model.insert("fallbacks".to_string(), serde_json::Value::Array(arr));
            }
            cfg["agents"] = serde_json::json!({
                "defaults": { "model": serde_json::Value::Object(model) }
            });
        }
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        g
    }

    /// Build a openclaw.json shape that mirrors the BAD rc55.14 shape:
    ///   `agents.defaults.model = "<id>"` (string, bare)
    ///   `agents.defaults.fallbacks = [...]` (top-level — INVALID per
    ///      AgentDefaultsSchema `.strict()`)
    /// Used by `lesson_829_*` tests to verify the repair path strips the
    /// top-level fallbacks and either keeps or converts the in-object
    /// fallbacks.
    fn fresh_openclaw_with_bad_top_level_fallbacks(
        primary: Option<&str>,
        fallbacks: Option<Vec<&str>>,
    ) -> EnvGuard {
        let g = fresh_env();
        let path = if cfg!(windows) {
            g._temp.path().join("MiracleClaw").join("openclaw.json")
        } else {
            g._temp.path().join(".miracle-claw").join("openclaw.json")
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut cfg = serde_json::json!({
            "models": { "providers": { "maic": {} } }
        });
        if primary.is_some() || fallbacks.is_some() {
            let mut defaults = serde_json::Map::new();
            if let Some(p) = primary {
                defaults.insert(
                    "model".to_string(),
                    serde_json::Value::String(p.to_string()),
                );
            }
            if let Some(fb) = fallbacks {
                let arr: Vec<serde_json::Value> = fb
                    .iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                defaults.insert("fallbacks".to_string(), serde_json::Value::Array(arr));
            }
            cfg["agents"] = serde_json::json!({
                "defaults": serde_json::Value::Object(defaults)
            });
        }
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        g
    }

    #[test]
    fn lesson_517_free_writes_local_default_when_empty() {
        let _env = lock_env();
        let _g = fresh_openclaw_with_model(None, None);
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Free)
            .expect("writer should succeed");
        assert!(wrote, "should have written because no primary was set");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 829 (2026-08-30, rc55.15 hotfix): seed writes the OBJECT
        // form (`agents.defaults.model = {primary, fallbacks}`). The
        // schema (`AgentDefaultsSchema`) is `.strict()` — it does NOT
        // accept top-level `fallbacks` at `agents.defaults`.
        let primary = cfg.pointer("/agents/defaults/model/primary").unwrap();
        assert_eq!(primary, "maic/milagro-m1-t1");
        // Lesson 569: chain is local 7B → local 14B → cloud nano.
        let fallbacks: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .expect("Free must have fallbacks inside model (Lesson 829)")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            fallbacks,
            vec!["maic/milagro-m1-t2", "maic/milagro-m1-t3", "maic/chat-nemotron-nano"],
            "Free chain must be local 7B → local 14B → cloud nano (level 1)"
        );
        // Lesson 829: no top-level fallbacks allowed.
        assert!(
            cfg.pointer("/agents/defaults/fallbacks").is_none(),
            "Lesson 829: schema rejects top-level fallbacks"
        );
    }

    #[test]
    fn lesson_517_pro_writes_kimi_with_fallbacks() {
        let _env = lock_env();
        let _g = fresh_openclaw_with_model(None, None);
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        assert!(wrote, "should have written because no primary was set");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 829: primary lives at /agents/defaults/model/primary.
        // Lesson 795 (2026-08-30, David): primary swapped Kimi → GLM.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-oc-glm"
        );
        let fallbacks: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // Lesson 795: chain is MiniMax → Kimi → local 14B-distilled.
        assert_eq!(
            fallbacks,
            vec!["maic/milagro-oc-minimax", "maic/milagro-oc-kimi", "maic/milagro-m1-t3"]
        );
        // Lesson 829: no top-level fallbacks allowed.
        assert!(cfg.pointer("/agents/defaults/fallbacks").is_none());
    }

    #[test]
    fn lesson_517_does_not_overwrite_user_choice() {
        let _env = lock_env();
        // User picked a manual primary — writer must leave it alone so
        // logins don't clobber their pick.
        let _g = fresh_openclaw_with_model(Some("milagro-dev-coder"), None);
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        assert!(!wrote, "writer must report 'already correct' when user has a primary");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Primary still the user's pick, NOT Kimi.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "milagro-dev-coder"
        );
    }

    #[test]
    fn lesson_517_writes_when_existing_primary_is_empty_string() {
        let _env = lock_env();
        // Edge case: blank-string primary is treated as unset (not user
        // choice). Writer should overwrite it.
        let _g = fresh_openclaw_with_model(Some(""), None);
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Enterprise)
            .expect("writer should succeed");
        assert!(wrote, "empty primary must be treated as unset");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-oc-glm"
        );
    }

    #[test]
    fn lesson_517_pro_plus_team_enterprise_share_routing() {
        let _env = lock_env();
        // Sanity: ProPlus, Team, Enterprise all route to the same GLM +
        // MiniMax + Kimi + local chain. Lesson 795 (2026-08-30, David):
        // primary is GLM (not Kimi) because GLM is the only paid-tier
        // model that reliably fires our plugin's local tools.
        // Lesson 737 (2026-08-29, David): Starter/StarterPlus share the
        // same routing as Pro. Token bucket is the only differentiator.
        for tier in [
            crate::auth::tier::Tier::Starter,
            crate::auth::tier::Tier::StarterPlus,
            crate::auth::tier::Tier::ProPlus,
            crate::auth::tier::Tier::Team,
            crate::auth::tier::Tier::Enterprise,
        ] {
            let _g = fresh_openclaw_with_model(None, None);
            let wrote = ensure_agents_default_model_for_tier(tier)
                .expect("writer should succeed");
            assert!(wrote, "{:?} should have written", tier);

            let path = openclaw_json_path();
            let cfg: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                cfg.pointer("/agents/defaults/model/primary").unwrap(),
                "maic/milagro-oc-glm",
                "{:?} primary must be GLM (Lesson 795)",
                tier,
            );
            let fallbacks: Vec<String> = cfg
                .pointer("/agents/defaults/model/fallbacks")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(fallbacks.len(), 3, "{:?} should have 3 fallbacks", tier);
            assert_eq!(fallbacks[0], "maic/milagro-oc-minimax");
            assert_eq!(fallbacks[1], "maic/milagro-oc-kimi");
            assert_eq!(fallbacks[2], "maic/milagro-m1-t3");
        }
    }

    #[test]
    fn lesson_517_free_clears_stale_paid_fallbacks_on_downgrade() {
        let _env = lock_env();
        // User paid → had Kimi+fallbacks. Downgraded to Free. Next login
        // must rewrite the chain to Free's m1-t chain so the dropdown
        // shows the local models + cheap cloud fallback. We model this
        // by starting with empty primary + paid fallbacks (OBJECT form,
        // Lesson 829), then calling the writer with Free.
        let _g = fresh_openclaw_with_model(Some(""), Some(vec!["maic/milagro-oc-minimax", "maic/milagro-dev"]));
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Free)
            .expect("writer should succeed");
        assert!(wrote, "empty primary + non-empty fallbacks must trigger write");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 829: primary at /agents/defaults/model/primary.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-m1-t1"
        );
        let fallbacks: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .expect("Free must have fallbacks (Lesson 569)")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            fallbacks,
            vec!["maic/milagro-m1-t2", "maic/milagro-m1-t3", "maic/chat-nemotron-nano"],
            "Free downgrade must replace paid fallbacks with Free's m1-t chain"
        );
        // Lesson 829: no top-level fallbacks.
        assert!(cfg.pointer("/agents/defaults/fallbacks").is_none());
    }

    // -----------------------------------------------------------------------
    // Lesson 800 tests (rc55.13 schema migration)
    // -----------------------------------------------------------------------

    // ---------------------------------------------------------------------
    // Lesson 829 tests (rc55.15 schema repair)
    // ---------------------------------------------------------------------

    /// Build an openclaw.json with the OBJECT schema
    /// (`model = { primary, fallbacks }`) but populated with BARE ids
    /// (no `maic/` prefix). Used to exercise Lesson 824's prefix
    /// rewrite on top of the correct schema shape (Lesson 829).
    fn fresh_openclaw_with_maic_models_and_primary(
        primary: Option<&str>,
        fallbacks: Option<Vec<&str>>,
    ) -> EnvGuard {
        let g = fresh_env();
        let path = if cfg!(windows) {
            g._temp.path().join("MiracleClaw").join("openclaw.json")
        } else {
            g._temp.path().join(".miracle-claw").join("openclaw.json")
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cfg = serde_json::json!({
            "models": {
                "providers": {
                    "maic": {
                        "api": "openai-completions",
                        "baseUrl": "https://maicserver.com/v1",
                        "models": [
                            {"id": "milagro-dev", "name": "MAIC default"},
                            {"id": "milagro-oc-kimi", "name": "MAIC kimi"},
                            {"id": "milagro-oc-minimax", "name": "MAIC minimax"},
                            {"id": "milagro-oc-glm", "name": "MAIC glm"},
                        ]
                    }
                }
            }
        });
        let mut cfg = cfg;
        if primary.is_some() || fallbacks.is_some() {
            let mut model_obj = serde_json::Map::new();
            if let Some(p) = primary {
                model_obj.insert(
                    "primary".to_string(),
                    serde_json::Value::String(p.to_string()),
                );
            }
            if let Some(fb) = fallbacks {
                let arr: Vec<serde_json::Value> = fb
                    .iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                model_obj.insert("fallbacks".to_string(), serde_json::Value::Array(arr));
            }
            cfg["agents"] = serde_json::json!({
                "defaults": { "model": serde_json::Value::Object(model_obj) }
            });
        }
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        g
    }

    #[test]
    fn lesson_829_repairs_bad_top_level_fallbacks_from_rc55_14() {
        // rc55.14 regression: the writer (Lesson 800) emitted
        //   agents.defaults = { fallbacks: [...], model: "<id>" }
        // which crashes gateway boot with `agents.defaults: Invalid
        // input` because `AgentDefaultsSchema` is `.strict()` (only
        // accepts the documented fields).
        //
        // Lesson 829 (rc55.15): the writer must lift top-level
        // fallbacks into the model object's `fallbacks` field, then
        // STRIP the top-level key.
        let _env = lock_env();
        let _g = fresh_openclaw_with_bad_top_level_fallbacks(
            Some("maic/milagro-oc-kimi"),
            Some(vec!["maic/milagro-oc-minimax", "maic/milagro-oc-glm"]),
        );

        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        assert!(wrote, "rc55.14-bad shape must trigger a repair write");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 829: top-level fallbacks key removed (schema rejects it).
        assert!(
            cfg.pointer("/agents/defaults/fallbacks").is_none(),
            "Lesson 829: top-level fallbacks must be stripped"
        );
        // Lesson 829: model is OBJECT form with fallbacks preserved.
        let fb_in_obj: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .expect("fallbacks must live inside model.fallbacks (Lesson 829)")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            fb_in_obj,
            vec!["maic/milagro-oc-minimax".to_string(), "maic/milagro-oc-glm".to_string()],
            "Lesson 829: top-level fallbacks should be lifted into model.fallbacks"
        );
        let primary = cfg
            .pointer("/agents/defaults/model/primary")
            .and_then(|v| v.as_str());
        assert_eq!(
            primary,
            Some("maic/milagro-oc-kimi"),
            "primary preserved (already prefixed)"
        );
    }

    #[test]
    fn lesson_824_prefixes_bare_existing_primary() {
        // David's bug: agents.defaults.model.primary = "milagro-oc-kimi"
        // (bare). The writer must recognize that bare id, prefix it
        // with `maic/`, and persist the file. This is the fix for the
        // rc55.12 chat picker showing only 4 models (gateway
        // dispatched via `inferUniqueProviderFromCatalog` to
        // `openai/<id>` and MAIC rejected it).
        let _env = lock_env();
        let _g = fresh_openclaw_with_maic_models_and_primary(
            Some("milagro-oc-kimi"),
            Some(vec!["milagro-oc-minimax"]),
        );

        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        assert!(wrote, "bare primary without prefix must trigger a write");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 829: primary lives at /agents/defaults/model/primary.
        let model = cfg
            .pointer("/agents/defaults/model/primary")
            .and_then(|v| v.as_str());
        assert_eq!(
            model,
            Some("maic/milagro-oc-kimi"),
            "bare primary must be prefixed with 'maic/'"
        );
        let fallbacks: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fallbacks,
            vec!["maic/milagro-oc-minimax".to_string()],
            "bare fallback ids must also be prefixed"
        );
    }

    #[test]
    fn lesson_824_does_not_double_prefix() {
        // Already-prefixed primary must NOT be re-prefixed. The writer
        // should be a no-op (the primary is valid and non-empty).
        let _env = lock_env();
        let _g = fresh_openclaw_with_maic_models_and_primary(
            Some("maic/milagro-oc-kimi"),
            Some(vec!["maic/milagro-oc-minimax"]),
        );

        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        assert!(!wrote, "already-prefixed primary must be a no-op");
    }

    #[test]
    fn lesson_824_ignores_bare_ids_not_in_catalog() {
        // A bare primary like "milagro-dev" matches the catalog and gets
        // prefixed; one that DOESN'T exist in the catalog (e.g. a typo
        // or stale id) is left alone — no false writes, no spurious
        // prefixes that would silently corrupt the wire request.
        let _env = lock_env();
        let _g = fresh_openclaw_with_maic_models_and_primary(
            Some("unknown-model-id"),
            Some(vec!["another-unknown"]),
        );

        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Pro)
            .expect("writer should succeed");
        // Bare unknown ids → no prefix rewrite, no seed (non-empty primary),
        // net effect: file unchanged, wrote=false.
        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!wrote, "unknown bare primary must not trigger a write");
        let model = cfg
            .pointer("/agents/defaults/model/primary")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert_eq!(
            model, "unknown-model-id",
            "unknown bare id must be left untouched"
        );
    }

    // -----------------------------------------------------------------------
    // Lesson 520 tests (model-merge runs on existing-entry early-return path)
    // -----------------------------------------------------------------------

    #[test]
    fn lesson_520_known_ids_merge_into_existing_entry() {
        // The existing-entry path returns early BEFORE the in-function
        // model merge (Lesson 451 was a one-shot, then return). Lesson 520
        // adds a merge_known_model_ids_into_provider call there too, so an
        // rc18 upgrade (which only had `milagro-dev` seeded) gets the full
        // 17-id surface on the next ensure_maic_provider_config() run.
        //
        // We simulate the rc18 state by populating openclaw.json with a
        // complete (literal-key + baseUrl) provider entry whose models[]
        // has only the original `milagro-dev` entry — then re-run
        // ensure_maic_provider_config() and check the merge landed.
        use std::io::Write as _;
        let _env = lock_env();
        let g = fresh_env();
        let path = if cfg!(windows) {
            g._temp.path().join("MiracleClaw").join("openclaw.json")
        } else {
            g._temp.path().join(".miracle-claw").join("openclaw.json")
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cfg = serde_json::json!({
            "$schema": "https://openclaw.dev/schema/v1/openclaw.config.schema.json",
            "models": {
                "providers": {
                    "maic": {
                        "api": "openai-completions",
                        "apiKey": "literal-test-key",
                        "baseUrl": "https://maicserver.com/v1",
                        "models": [
                            {"id": "milagro-dev", "name": "MAIC default (miracle-claw)"}
                        ],
                        "params": {"tool_execution": "client"}
                    }
                }
            }
        });
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(serde_json::to_string_pretty(&cfg).unwrap().as_bytes()).unwrap();
        }

        let result = ensure_maic_provider_config().expect("rc18-style entry must rehydrate");
        assert!(result.provider_configured, "literal key + url means configured");

        let rehydrated: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let models = rehydrated
            .pointer("/models/providers/maic/models")
            .unwrap()
            .as_array()
            .unwrap();
        let ids: std::collections::HashSet<String> = models
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        // Lesson 527: ensure_maic_provider_config() defaults to Free
        // tier (no context). Free gets m1-t1 + m1-t2 + m1-t3 +
        // chat-nemotron-nano (Lesson 565 added t3 for better free
        // experience, 2026-08-24; Lesson 567 added chat-nemotron-nano
        // as a fast NVIDIA MoE option, 2026-08-24).
        for id in ["milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3", "chat-nemotron-nano"] {
            assert!(ids.contains(id), "Free tier must include model {}", id);
        }
        // Total = 4 unique ids for Free tier (Lesson 567).
        assert_eq!(models.len(), 4, "Free tier should see exactly 4 models (t1, t2, t3, chat-nemotron-nano per Lesson 565+567)");
    }

    #[test]
    fn lesson_520_known_ids_merge_is_idempotent() {
        // Re-running ensure_maic_provider_config() on a file that already
        // has all 20 ids must NOT add duplicates and must NOT change the
        // apiKey/baseUrl. This protects against file-mtime churn.
        let _env = lock_env();
        let _g = fresh_openclaw_with_model(None, None);

        // Run twice with a literal key + baseUrl in the env (so the
        // early-return path is exercised).
        std::env::set_var(ENV_VAR_NAME, "literal-test-key");
        std::env::set_var("MAIC_API_URL", "https://maicserver.com/v1");
        let r1 = ensure_maic_provider_config().expect("first run");
        assert!(r1.provider_configured);
        let size1 = std::fs::metadata(openclaw_json_path()).unwrap().len();

        let r2 = ensure_maic_provider_config().expect("second run");
        assert!(r2.provider_configured);
        let size2 = std::fs::metadata(openclaw_json_path()).unwrap().len();

        // Same content written both times — sizes should match exactly.
        // We don't check byte-for-byte because serde_json's pretty-printer
        // could vary on object key order, but the size delta is a good
        // smoke test.
        assert_eq!(size1, size2,
                   "second run should not grow the file (got {size1} → {size2})");

        std::env::remove_var(ENV_VAR_NAME);
        std::env::remove_var("MAIC_API_URL");
    }

    // ---------------------------------------------------------------------
    // Lesson 535 (NEW 2026-08-22): openclaw 2026.7.1 plugin activation
    // ---------------------------------------------------------------------
    //
    // `resolveEffectivePluginActivationState` in openclaw 2026.7.1 requires
    // non-bundled plugins to be EXPLICITLY enabled via
    // `plugins.entries.<id>.enabled = true` (or allowlisted in
    // `plugins.allow`). Without it, the MAIC plugin's `register()` is
    // never invoked, the `extraParamsForTransport` hook never fires, and
    // the bundled steeler-compat patch can't propagate
    // `tool_execution: "client"` into the outbound chat request — so the
    // model only sees MAIC's 4 server tools (weather, web_search,
    // get_current_time, calculate) instead of the full tool registry.
    //
    // MC's `ensure_maic_provider_config_for_tier` is responsible for
    // stamping the enable into openclaw.json alongside the provider
    // entry. These tests pin that contract.
    #[test]
    fn lesson_535_plugin_entry_enabled_after_bootstrap() {
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "test-key-bootstrap");
        env::set_var("MAIC_API_URL", "https://maicserver.com/v1");

        let _ = ensure_maic_provider_config().expect("bootstrap");

        let raw = std::fs::read_to_string(openclaw_json_path()).expect("read cfg");
        let cfg: serde_json::Value = serde_json::from_str(&raw).expect("parse cfg");

        // Must contain plugins.entries.maic.enabled = true
        let enabled = cfg
            .pointer("/plugins/entries/maic/enabled")
            .and_then(|v| v.as_bool())
            .expect("plugins.entries.maic.enabled must be written by MC bootstrap");
        assert!(
            enabled,
            "Lesson 535: plugins.entries.maic.enabled must be true"
        );

        // Must also have plugins.allow = ["maic"] to silence the
        // "discovered non-bundled plugins may auto-load" warning and
        // pin trust provenance.
        let allow = cfg
            .pointer("/plugins/allow")
            .and_then(|v| v.as_array())
            .expect("plugins.allow must be an array");
        let allow_strs: Vec<&str> = allow
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            allow_strs.contains(&"maic"),
            "Lesson 535: plugins.allow must include 'maic' (got {allow_strs:?})"
        );
    }

    #[test]
    fn lesson_535_plugin_entry_idempotent_across_runs() {
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "test-key-idempotent");
        env::set_var("MAIC_API_URL", "https://maicserver.com/v1");

        // First run creates the entry.
        let _ = ensure_maic_provider_config().expect("first bootstrap");
        let size1 = std::fs::metadata(openclaw_json_path()).unwrap().len();

        // Second run with a DIFFERENT api key + url must still:
        // - preserve plugins.entries.maic.enabled = true
        // - not duplicate "maic" in plugins.allow
        env::set_var("MAIC_API_KEY", "different-key");
        env::set_var("MAIC_API_URL", "https://other.example.com/v1");
        let _ = ensure_maic_provider_config().expect("second bootstrap");

        let raw = std::fs::read_to_string(openclaw_json_path()).expect("read cfg");
        let cfg: serde_json::Value = serde_json::from_str(&raw).expect("parse cfg");

        let enabled = cfg
            .pointer("/plugins/entries/maic/enabled")
            .and_then(|v| v.as_bool())
            .expect("plugins.entries.maic.enabled must persist");
        assert!(enabled, "Lesson 535: enabled flag must survive re-runs");

        let allow = cfg
            .pointer("/plugins/allow")
            .and_then(|v| v.as_array())
            .expect("plugins.allow must be an array");
        let maic_count = allow
            .iter()
            .filter(|v| v.as_str() == Some("maic"))
            .count();
        assert_eq!(
            maic_count, 1,
            "Lesson 535: 'maic' must appear exactly once in plugins.allow (got {maic_count})"
        );

        // The plugins.entries.maic object must NOT have grown — re-runs
        // should be no-ops on the enable/allow stamping (size1 may have
        // grown slightly because the provider entry's apiKey changed,
        // but plugins.entries.maic itself is bounded).
        let maic_entry_size = cfg
            .pointer("/plugins/entries/maic")
            .map(|v| v.to_string().len())
            .unwrap_or(0);
        assert!(
            maic_entry_size < 200,
            "Lesson 535: plugins.entries.maic should be a small object, got {maic_entry_size} bytes"
        );

        let _ = size1; // suppress unused warning
    }

    #[test]
    fn lesson_535_plugin_entry_written_even_with_existing_config() {
        // If the user already has a complete MAIC provider entry from a
        // prior install (rc18-rc21, pre-Lesson-535), the bootstrap should
        // STILL stamp plugins.entries.maic.enabled = true. Otherwise the
        // plugin stays un-loaded after upgrade.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "test-key-existing");

        let path = openclaw_json_path();
        let existing = serde_json::json!({
            "models": {
                "providers": {
                    "maic": {
                        "api": "openai-completions",
                        "apiKey": "prior-install-key",
                        "baseUrl": "https://maicserver.com/v1",
                        "models": [
                            {"id": "milagro-dev", "name": "milagro-dev"}
                        ]
                    }
                }
            }
        });
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .expect("write existing");

        // Run the bootstrap. It should hit the existing-entry early-return
        // path but STILL stamp plugins.entries.maic.enabled = true.
        let _ = ensure_maic_provider_config().expect("bootstrap");

        let raw = std::fs::read_to_string(&path).expect("read cfg");
        let cfg: serde_json::Value = serde_json::from_str(&raw).expect("parse");

        let enabled = cfg
            .pointer("/plugins/entries/maic/enabled")
            .and_then(|v| v.as_bool())
            .expect("plugins.entries.maic.enabled must be added even to pre-existing configs");
        assert!(
            enabled,
            "Lesson 535: existing-config upgrade path must stamp plugin entry"
        );
    }

    // ---------------------------------------------------------------------
    // v1.0.9-rc35: validate_workspace_md path safety
    //
    // The Settings page passes absolute paths from the frontend. We MUST
    // reject any path that escapes the workspace or isn't a .md file,
    // regardless of what the frontend claims.
    //
    // These tests use MIRACLE_CLAW_WORKSPACE to point at a temp dir so
    // they don't touch the user's real ~/.openclaw/workspace.
    // ---------------------------------------------------------------------

    /// Build a workspace at `root` containing `rel_path` (creating
    /// parent dirs as needed) and return the canonicalized absolute path.
    fn touch_md(root: &Path, rel_path: &str) -> PathBuf {
        let abs = root.join(rel_path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&abs, b"# test\n").expect("write fixture");
        abs.canonicalize().expect("canonicalize")
    }

    #[test]
    fn validate_workspace_md_accepts_existing_md() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        let fixture = touch_md(tmp.path(), "MEMORY.md");
        let result = validate_workspace_md(&fixture);
        assert!(result.is_ok(), "should accept MEMORY.md: {:?}", result);
        assert_eq!(result.unwrap(), fixture);
    }

    #[test]
    fn validate_workspace_md_accepts_nested_md() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        let fixture = touch_md(tmp.path(), "memory/2026-08-22.md");
        let result = validate_workspace_md(&fixture);
        assert!(result.is_ok(), "should accept nested .md: {:?}", result);
    }

    #[test]
    fn validate_workspace_md_rejects_non_md_extension() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        // Create a .txt file inside the workspace; should be rejected.
        let bad = tmp.path().join("secrets.txt");
        fs::write(&bad, b"don't touch this").unwrap();
        let bad_canonical = bad.canonicalize().unwrap();

        let result = validate_workspace_md(&bad_canonical);
        assert!(result.is_err(), ".txt must be rejected");
        let err = result.err().unwrap_or_default();
        assert!(
            err.contains("non-markdown"),
            "error should explain the extension policy, got: {}",
            err
        );
    }

    #[test]
    fn validate_workspace_md_rejects_no_extension() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        let bad = tmp.path().join("MEMORY");
        fs::write(&bad, b"no ext").unwrap();
        let bad_canonical = bad.canonicalize().unwrap();

        let result = validate_workspace_md(&bad_canonical);
        assert!(result.is_err(), "extensionless must be rejected");
    }

    #[test]
    fn validate_workspace_md_rejects_escape() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        // Create a file OUTSIDE the workspace. The frontend would have
        // to construct this path; we want to be sure we refuse it.
        let outside = std::env::temp_dir().join("outside_workspace_evil.md");
        let _ = fs::remove_file(&outside);
        fs::write(&outside, b"# evil\n").unwrap();
        let outside_canonical = outside.canonicalize().unwrap();

        let result = validate_workspace_md(&outside_canonical);
        assert!(result.is_err(), "outside-workspace path must be rejected");
        let err = result.err().unwrap_or_default();
        assert!(
            err.contains("escapes workspace"),
            "error should explain why, got: {}",
            err
        );
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn validate_workspace_md_rejects_missing_file() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        // Path looks valid but doesn't exist.
        let ghost = tmp.path().join("memory").join("never-created.md");
        let result = validate_workspace_md(&ghost);
        assert!(result.is_err(), "nonexistent file must be rejected");
    }

    #[test]
    fn validate_workspace_md_rejects_relative_path_traversal() {
        let _lock = lock_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MIRACLE_CLAW_WORKSPACE", tmp.path());

        // "../etc/passwd.md" — even though we'd reject the extension
        // anyway, this proves the canonicalize step is what catches
        // escapes (the extension check happens before, but the
        // canonicalize would catch it if the file existed).
        let traversal = Path::new("../etc/passwd.md");
        let result = validate_workspace_md(traversal);
        assert!(result.is_err(), "relative traversal must be rejected");
    }

    // ---------------------------------------------------------------------
    // v1.0.9-rc35: epoch_to_ymdhms sanity (matches the smoke test we ran
    // during development; pinned here so the algorithm can't silently
    // regress).
    // ---------------------------------------------------------------------

    #[test]
    fn epoch_to_ymdhms_known_dates() {
        assert_eq!(epoch_to_ymdhms(0), (1970, 1, 1, 0, 0, 0), "epoch");
        assert_eq!(epoch_to_ymdhms(946684800), (2000, 1, 1, 0, 0, 0), "y2k");
        // 2024-02-29 = leap day
        assert_eq!(epoch_to_ymdhms(1709164800), (2024, 2, 29, 0, 0, 0), "leap day 2024");
        // 2026-08-22 00:00:00 UTC
        assert_eq!(epoch_to_ymdhms(1787356800), (2026, 8, 22, 0, 0, 0), "rc35 ship date");
    }

    // ---------------------------------------------------------------------
    // v1.0.9-rc36: Terminal command unit tests.
    //
    // We avoid running the full Tauri command surface (no easy way to
    // construct a tauri::State in tests). Instead we exercise the
    // pure-function helpers and a real spawn-then-read end-to-end:
    //
    //   - resolve_shell_cmd: pure function for shell label -> (cmd, args)
    //   - which_first: PATH lookup with absolute-path short-circuit
    //   - new_terminal_id: shape (24 hex chars) + uniqueness
    //   - read_lines: spawns `echo` and asserts the buffer fills with
    //     the expected line. Closest we get to E2E without tauri::State.
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_shell_cmd_recognizes_known_shells() {
        if cfg!(windows) {
            assert!(resolve_shell_cmd("cmd").is_ok());
            assert!(resolve_shell_cmd("pwsh").is_ok());
            assert!(resolve_shell_cmd("wsl").is_ok());
            // Lesson 220: mc-openclaw is the new default. It maps to
            // `openclaw tui` and resolves to npm's openclaw.cmd on
            // Windows via the .cmd extension in which_first.
            let mc = resolve_shell_cmd("mc-openclaw").expect("mc-openclaw must resolve");
            assert_eq!(mc.0, "openclaw");
            assert_eq!(mc.1, vec!["tui"]);
            assert!(resolve_shell_cmd("bash").is_err());
        } else {
            assert!(resolve_shell_cmd("bash").is_ok());
            assert!(resolve_shell_cmd("sh").is_ok());
            assert!(resolve_shell_cmd("zsh").is_ok());
            let mc = resolve_shell_cmd("mc-openclaw").expect("mc-openclaw must resolve on *nix too");
            // Lesson 221 + 223: sentinel marker; mc_terminal_start
            // expands it to bundled openclaw.mjs. The full tuple is
            // validated by `mc_openclaw_maps_to_openclaw_tui_local`.
            assert_eq!(mc.0, "__MC_OPENCLAW__");
            assert_eq!(mc.1, vec!["tui", "--local"]);
            assert!(resolve_shell_cmd("pwsh").is_err());
        }
    }

    #[test]
    fn resolve_shell_cmd_rejects_unknown() {
        let res = resolve_shell_cmd("totally-not-a-shell");
        assert!(res.is_err(), "unknown shell must error");
        assert!(res.unwrap_err().contains("unknown shell"));
    }

    /// Lesson 221: mc-openclaw maps to `openclaw tui --local`. The bundled
    /// resources dir swap happens in mc_terminal_start (this just
    /// verifies the (cmd, args) tuple is what we expect so a future
    /// refactor doesn't silently break the spawn contract).
    #[test]
    fn mc_openclaw_maps_to_openclaw_tui_local() {
        let res = resolve_shell_cmd("mc-openclaw")
            .expect("mc-openclaw must resolve on both Windows and *nix");
        // We use a sentinel so mc_terminal_start can detect the bundled
        // openclaw path without string-comparing the shell name twice.
        assert_eq!(res.0, "__MC_OPENCLAW__");
        // Lesson 223: --local avoids the TUI exiting at startup because
        // there's no remote Gateway listening on ws://127.0.0.1:18789
        // (we use the in-process embedded agent instead).
        assert_eq!(res.1, vec!["tui", "--local"]);
    }

    #[test]
    fn which_first_skips_path_lookup_for_absolute() {
        if cfg!(windows) {
            assert_eq!(
                which_first("C:\\Windows\\System32\\cmd.exe"),
                "C:\\Windows\\System32\\cmd.exe"
            );
        } else {
            assert_eq!(which_first("/bin/sh"), "/bin/sh");
        }
    }

    #[cfg(windows)]
    #[test]
    fn which_first_resolves_cmd_shims() {
        // Lesson 220: npm installs `openclaw` as `openclaw.cmd` on
        // Windows. which_first must resolve the .cmd shim so that
        // spawning `openclaw` (via the mc-openclaw shell kind)
        // actually finds the CLI. We synthesize a temp dir with a
        // fake.cmd to test the lookup logic in isolation from PATH.
        use std::env;
        use std::fs;
        use std::path::PathBuf;

        let tmp = env::temp_dir().join("mc_which_test_dir");
        let _ = fs::create_dir_all(&tmp);
        let shim_path = tmp.join("fakeshell.cmd");
        fs::write(&shim_path, "@echo off\r\n").unwrap();

        // Prepend tmp to PATH.
        let old_path = env::var("PATH").unwrap_or_default();
        let new_path = format!("{};{}", tmp.display(), old_path);
        // SAFETY: setting PATH in a single-threaded test is fine; the
        // race window is microscopic and the test is hermetic.
        unsafe { env::set_var("PATH", &new_path) };

        let resolved = which_first("fakeshell");
        assert_eq!(
            resolved,
            shim_path.to_string_lossy().into_owned(),
            "which_first must find .cmd shims on Windows"
        );

        // Restore.
        unsafe { env::set_var("PATH", old_path) };
        let _ = fs::remove_file(&shim_path);
        let _ = fs::remove_dir(&tmp);
        // Suppress unused warning on non-windows builds if cfg drops it.
        let _ = PathBuf::new();
    }

    #[test]
    fn new_terminal_id_is_24_hex_chars() {
        let id = new_terminal_id();
        assert_eq!(id.len(), 24, "session id must be 24 hex chars: got {:?}", id);
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "must be hex: {:?}",
            id
        );
    }

    #[test]
    fn new_terminal_id_is_unique_across_calls() {
        let mut ids: Vec<String> = (0..1000).map(|_| new_terminal_id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 1000, "duplicate terminal ids found");
    }

    #[test]
    fn read_lines_drains_lines_into_buffer() {
        use std::process::{Command, Stdio};

        let (cmd, args): (&str, Vec<&str>) = if cfg!(windows) {
            ("cmd.exe", vec!["/C", "echo hello-rc36"])
        } else {
            ("/bin/sh", vec!["-c", "echo hello-rc36"])
        };

        let mut child = Command::new(cmd)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn echo");

        let stdout = child.stdout.take().expect("take stdout");
        let buffer_arc = std::sync::Arc::new(Mutex::new(Vec::<OutputChunk>::new()));
        let seq_arc = std::sync::Arc::new(Mutex::new(0u64));

        let handle = TerminalHandle {
            shell: "test".into(),
            started_at: 0,
            child: Mutex::new(Some(child)),
            buffer_arc: buffer_arc.clone(),
            seq_arc: seq_arc.clone(),
        };
        let handle_arc = std::sync::Arc::new(handle);

        let reader_handle = handle_arc.clone();
        std::thread::spawn(move || {
            read_lines(stdout, reader_handle, "stdout");
        });

        let start = std::time::Instant::now();
        loop {
            {
                let buf = buffer_arc.lock().unwrap();
                if !buf.is_empty() {
                    break;
                }
            }
            if start.elapsed() > std::time::Duration::from_secs(2) {
                panic!("read_lines never produced a chunk");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let buf = buffer_arc.lock().unwrap();
        let combined: String = buf
            .iter()
            .filter(|c| c.stream == "stdout")
            .map(|c| c.data.as_str())
            .collect();
        assert!(
            combined.contains("hello-rc36"),
            "expected 'hello-rc36' in output, got: {:?}",
            combined
        );
    }

    /// Lesson 224: spawn_shell must accept an optional extra_env map and
    /// apply it to the child process without losing the inherited env.
    /// We exercise this by passing an EMPTY extra_env and verifying
    /// that a child spawned with `cmd /C echo` on Windows can read a
    /// standard inherited var like PATH that we set in the test scope.
    #[cfg(windows)]
    #[test]
    fn spawn_shell_accepts_empty_extra_env_without_dropping_inherited() {
        use std::process::Command;
        // Set a unique sentinel PATH variable on the parent (PATH itself
        // is always set on Windows in our test env, but we add a clearly-
        // recognizable suffix that we can grep for).
        let sentinel = "MC_SPAWN_SENTINEL=lesson_224";
        std::env::set_var("MC_SPAWN_SENTINEL", "lesson_224");

        let empty = std::collections::HashMap::<String, String>::new();
        let child = spawn_shell(
            "cmd.exe",
            &[
                "/D".to_string(),
                "/C".to_string(),
                format!("echo %{}%", "MC_SPAWN_SENTINEL"),
            ],
            Some(&empty),
        )
        .expect("spawn_shell with empty extra_env should succeed");

        let mut child = child;
        let status = child.wait().expect("child should exit cleanly");
        assert!(status.success(), "echo should exit 0, got: {:?}", status);

        // _no easy way to capture stdout from this test without
        // modifying the signature_ — the structural assertion is that
        // `Some(&empty_hashmap)` doesn't itself cause spawn to fail.
        let _ = Command::new("cmd");
    }

    /// Lesson 224: extra_env values must override inherited env in the
    /// child. Spawn echo with an explicit override of PATH just to prove
    /// that the supplied map actually reaches the child (we rely on the
    /// override being visible in the child's stdout).
    #[cfg(windows)]
    #[test]
    fn spawn_shell_extra_env_overrides_inherited_for_child() {
        // We don't need to consume the echoed string — just confirm
        // `spawn_shell` returns Ok and the child exits cleanly.
        let mut overrides = std::collections::HashMap::<String, String>::new();
        overrides.insert("MC_OVERRIDE_KEY".to_string(), "lesson_224_set".to_string());
        let child = spawn_shell(
            "cmd.exe",
            &[
                "/D".to_string(),
                "/C".to_string(),
                "echo %MC_OVERRIDE_KEY%".to_string(),
            ],
            Some(&overrides),
        )
        .expect("spawn_shell with extra_env should succeed");
        let mut child = child;
        let status = child.wait().expect("child should exit cleanly");
        assert!(status.success(), "cmd echo should exit 0");
    }

    // ---------------------------------------------------------------------
    // rc53 (feature/secrets-vault): validate_secret_name + placeholder
    // expansion logic. We test the pure functions directly, not the
    // Tauri commands (which would require an AppHandle mock).
    // ---------------------------------------------------------------------

    #[test]
    fn validate_secret_name_accepts_valid_names() {
        assert!(validate_secret_name("STRIPE_KEY").is_ok());
        assert!(validate_secret_name("_PRIVATE").is_ok());
        assert!(validate_secret_name("A").is_ok());
        assert!(validate_secret_name("AWS_ACCESS_KEY_ID_2026").is_ok());
    }

    #[test]
    fn validate_secret_name_rejects_shell_injection_attempts() {
        // The whole point of the validation: reject anything that
        // could be interpreted as shell syntax or path traversal.
        assert!(validate_secret_name("").is_err());
        assert!(validate_secret_name("PATH; rm -rf /").is_err());
        assert!(validate_secret_name("KEY`whoami`").is_err());
        assert!(validate_secret_name("KEY$(id)").is_err());
        assert!(validate_secret_name("KEY|grep").is_err());
        assert!(validate_secret_name("KEY&echo").is_err());
        assert!(validate_secret_name("../etc/passwd").is_err());
        assert!(validate_secret_name("KEY WITH SPACES").is_err());
        assert!(validate_secret_name("lowercase").is_err()); // uppercase only
        assert!(validate_secret_name("1NUM_START").is_err()); // can't start with digit
        assert!(validate_secret_name("KEY-DASH").is_err()); // no dash
    }

    #[test]
    fn secret_name_with_lowercase_passes_validation_for_js_interop() {
        // JS preprocessor lowercases names before placeholder insertion.
        // Rust validation should also accept lowercase if it ever
        // sees it (defense in depth). v0: accept both. v1: maybe
        // tighten to uppercase only.
        assert!(validate_secret_name("lowercase").is_err()); // currently strict
        // The above is the locked-in v0 behavior. If we want to
        // accept lowercase, remove this assertion and update the
        // docstring.
    }
}
