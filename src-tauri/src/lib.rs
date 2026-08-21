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
use tauri::{Manager, RunEvent};
use tauri_plugin_shell::process::CommandEvent;

mod launcher_info;
use launcher_info::{launcher_binary_name, MAIC_PLUGIN_FILENAMES, OPENCLAW_PORT};

// Lesson 458 / v1.0.6: silent-relogin via cached creds in OS keychain.
// See src/auto_relogin.rs for the full design.
mod auto_relogin;

// v1.0.7: tier fetching + token-quota nudges.
pub mod auth;
// v1.0.7: 7 local tool schemas (paid tier only). Marked `pub` so the
// `miracle-claw-tools` binary can `use` them via `crate::tools::...`.
pub mod tools;

// ----------------------------------------------------------------------------
// Constants
// ----------------------------------------------------------------------------

/// Default MAIC endpoint. Overridable via MAIC_API_URL env var at runtime;
/// the login UI shows whatever this resolves to so the customer knows
/// which server they're signing into. Lesson 444.
const DEFAULT_ENDPOINT: &str = "https://maicserver.com";

/// MAIC API key env var. Used by `maic_login`, `needs_maic_login_from_state`,
/// and the openclaw.json SecretRef path. Lesson 444.
const ENV_VAR_NAME: &str = "MAIC_API_KEY";

// ----------------------------------------------------------------------------
// State we hold for the lifetime of the process
// ----------------------------------------------------------------------------

#[derive(Default)]
struct AppState {
    /// Handle to the launcher sidecar child process (if started).
    /// Mutex because RunEvent handlers + setup() cross thread boundaries.
    launcher_child:
        Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
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
// Lesson 520 helper: idempotently merge the 17 known MAIC model ids into
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
// UX). Paid tiers (Pro, ProPlus, Team, Enterprise) get the full 17-
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
    // Full catalog (17 models). Free users see only the 2 in
    // FREE_MODEL_IDS; paid users see all 17.
    const ALL_MODEL_IDS: &[&str] = &[
        "milagro-dev", "milagro-dev-coder", "milagro-m1",
        "milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3",
        "milagro-chat", "milagro-coder", "milagro-stock",
        "milagro-oc-minimax", "milagro-oc-glm", "milagro-oc-qwen",
        "milagro-oc-deepseek", "milagro-oc-kimi",
        "chat-glm", "chat-deepseek", "chat-qwen",
    ];
    // Lesson 527: Free tier gets only the two smallest distilled
    // m1 models. These are the chat-only fast tier — ~7B ternary,
    // fast response, low TPM cost. Picking anything else would
    // blow through the 50K TPM ceiling in a few messages.
    const FREE_MODEL_IDS: &[&str] = &[
        "milagro-m1-t1",
        "milagro-m1-t2",
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
            let needs_tools_stamp = match params.get("tools") {
                None => true,
                Some(Value::Array(a)) => a.is_empty(),
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
    // Lesson 527 (NEW 2026-08-21 13:57 MDT): tier-gated model list.
    // Free gets m1-t1 + m1-t2 only (chat-only fast tier). Paid gets
    // the full 17-model catalog. The `seeds` array drives the
    // first-install write path (when models list is empty); for
    // upgrades the `else` branch below does the same tier gating.
    let seeds: &[(&str, &str)] = match tier {
        crate::auth::tier::Tier::Free => &[
            ("milagro-m1-t1",  "MAIC m1-t1 — 7B LoRA-distilled (fast)"),
            ("milagro-m1-t2",  "MAIC m1-t2 — 7B LoRA-distilled (mid)"),
        ],
        _ => &[
            ("milagro-dev",            "MAIC default (miracle-claw) — 14B local generalist"),
            ("milagro-dev-coder",      "MAIC coder — 14B local code-tuned"),
            ("milagro-m1",             "MAIC m1 — base"),
            ("milagro-m1-t1",          "MAIC m1-t1 — 7B LoRA-distilled (fast)"),
            ("milagro-m1-t2",          "MAIC m1-t2 — 7B LoRA-distilled (mid)"),
            ("milagro-m1-t3",          "MAIC m1-t3 — 7B LoRA-distilled (top of t-series)"),
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
        // m1-t1 + m1-t2 (chat-only fast tier). Paid gets all 17.
        // Match the gating in merge_known_model_ids_into_provider
        // (the existing-entry early-return path) so both code paths
        // produce the same model list for the same tier.
        let known_ids: &[&str] = match tier {
            crate::auth::tier::Tier::Free => &[
                "milagro-m1-t1",
                "milagro-m1-t2",
            ],
            _ => &[
                "milagro-dev", "milagro-dev-coder", "milagro-m1",
                "milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3",
                "milagro-chat", "milagro-coder", "milagro-stock",
                "milagro-oc-minimax", "milagro-oc-glm", "milagro-oc-qwen",
                "milagro-oc-deepseek", "milagro-oc-kimi",
                "chat-glm", "chat-deepseek", "chat-qwen",
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

    Ok(MaicProviderBootstrap {
        provider_configured: true,
        provider_id: PROVIDER_ID.to_string(),
        api_key_source: key_source,
        endpoint: final_endpoint,
    })
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
    let primary = format!("{PROVIDER_PREFIX}{}", tier_default_model_id(tier));
    let fallbacks: Vec<String> = tier_default_fallbacks(tier)
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

    // Walk to `agents.defaults.model`.
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

    // Read the existing model entry to decide whether to write.
    let existing_primary = defaults
        .get("model")
        .and_then(|m| m.get("primary"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Non-destructive: only write if no primary is set OR primary is empty.
    // We deliberately do NOT overwrite an existing non-empty primary — that
    // means the user picked one and we should respect it across logins.
    let needs_write = match existing_primary.as_deref() {
        None | Some("") => true,
        Some(_) => false, // user already chose; don't clobber
    };
    if !needs_write {
        return Ok(false);
    }

    let model_obj = defaults
        .as_object_mut()
        .unwrap()
        .entry("model".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !model_obj.is_object() {
        *model_obj = Value::Object(Default::default());
    }
    let model_obj = model_obj.as_object_mut().unwrap();

    model_obj.insert("primary".to_string(), Value::String(primary.to_string()));
    if fallbacks.is_empty() {
        // Free: clear any stale fallback array left over from a paid
        // account's downgrade (so the dropdown shows just `milagro-dev`).
        model_obj.remove("fallbacks");
    } else {
        let fb: Vec<Value> = fallbacks.iter().map(|s| Value::String(s.to_string())).collect();
        model_obj.insert("fallbacks".to_string(), Value::Array(fb));
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
        "[miracle-claw] tier: wrote agents.defaults.model primary={} fallbacks={:?} (tier={})",
        primary, fallbacks, tier.as_str()
    );
    Ok(true)
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

    Ok(())
}

/// Lesson 491 (rc13): create the main dashboard window programmatically
/// instead of via tauri.conf.json's `app.windows[]`. Reason: we need to
/// attach `initialization_script()` so the bridge JS runs on EVERY page
/// the main window navigates to (dashboard + chat).
fn create_main_window(app_handle: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // Embed the bridge script at compile time so it ships in the binary.
    // The script lives in `src-tauri/src/openclaw-host-bridge.js` and is
    // referenced via `include_str!` so cargo recompiles when it changes.
    let bridge_js = include_str!("openclaw-host-bridge.js");

    WebviewWindowBuilder::new(
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
    .build()
    .map_err(|e| format!("WebviewWindowBuilder::build() failed for main window: {e}"))?;

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

/// Tauri command: openclaw_back_to_dashboard (Lesson 491, rc13).
///
/// Called by the chat page's "← Dashboard" overlay (injected by
/// `src/openclaw-host-bridge.js` when the main window is at the chat
/// gateway). Navigates the main webview back to the bundled dashboard
/// (`index.html` served via the `tauri://localhost` scheme).
///
/// Why this lives in Rust rather than JS:
///
/// The chat page is loaded from `http://127.0.0.1:28789/` (the openclaw
/// gateway), not from the bundled dashboard assets. The bridge script has
/// no way to know the dashboard's Tauri URL except by asking the Rust
/// side via a Tauri command. Hardcoding `tauri://localhost/index.html`
/// in the bridge would couple it to Tauri's scheme — better to have Rust
/// own the navigation logic.
#[tauri::command]
fn openclaw_back_to_dashboard(app_handle: tauri::AppHandle) -> Result<(), String> {
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

    // The bundled dashboard is served at `index.html` under the tauri://
    // localhost scheme. tauri.conf.json `app.windows[0].url: "index.html"`
    // is the source of truth — we replicate that here so a future change
    // to the URL is a single-edit thing.
    const DASHBOARD_URL: &str = "tauri://localhost/index.html";
    if let Err(e) = main_window.navigate(DASHBOARD_URL.parse().map_err(|err| {
        format!("invalid dashboard URL {DASHBOARD_URL:?}: {err}")
    })?) {
        log_to_file(&format!(
            "openclaw_back_to_dashboard: main_window.navigate() failed: {e}"
        ));
        return Err(format!("could not navigate back to dashboard: {e}"));
    }
    let _ = main_window.set_focus();
    let _ = main_window.unminimize();

    log_to_file("openclaw_back_to_dashboard: navigated main window back to dashboard");
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
/// 7 tools — Free users are rate-limited by TPM (50K) but otherwise
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
            if let Some(primary) = cfg.pointer_mut("/agents/defaults/model/primary") {
                *primary = serde_json::Value::String(String::new());
            }
            if let Ok(serialized) = serde_json::to_string_pretty(&cfg) {
                let _ = std::fs::write(&path, serialized);
            }
        }
    }
    ensure_agents_default_model_for_tier(tier)
        .map_err(|e| format!("set_tier_defaults: {}", e))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            first_run_report,
            maic_login,
            maic_logout,
            silent_relogin,
            start_gateway_after_login,
            openclaw_open_window,
            openclaw_back_to_dashboard,
            open_register_url,
            // v1.0.7: tier + nudge surface
            mc_get_tier,
            mc_get_nudge,
            mc_list_tools,
            mc_refresh_tier,
            mc_apply_tier_change,
            mc_set_tier_defaults
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
    // Previously Free got 0 tools; now ALL tiers get all 7 tools. The
    // tier parameter is kept on `tools_for_tier(tier)` for future
    // forward-compat but currently ignores it.

    #[test]
    fn tools_for_tier_returns_all_seven_for_every_tier() {
        // Lesson 526 (NEW 2026-08-21 13:55 MDT): all tiers get all 7
        // tools. Rate limiting (per-tier TPM) is the actual control.
        for t in [
            crate::auth::tier::Tier::Free,
            crate::auth::tier::Tier::Pro,
            crate::auth::tier::Tier::ProPlus,
            crate::auth::tier::Tier::Team,
            crate::auth::tier::Tier::Enterprise,
        ] {
            let tools = tools_for_tier(t);
            assert_eq!(
                tools.len(),
                7,
                "Lesson 526: tier {:?} should have 7 local tools (was Free=0)",
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
    fn no_tier_default_writes_all_seven_tools() {
        // Lesson 523 (was): setup() / login-required bootstrap (no
        // tier context) defaulted to Free → empty tools array.
        //
        // Lesson 526 (NEW 2026-08-21): gating removed. ALL tiers,
        // including Free, get all 7 tools. The "no tier" case
        // (login-required bootstrap before MAIC responds) now also
        // gets all 7 — better to advertise capabilities than to
        // hide them behind a tier check that may be wrong.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config().expect("ok");
        let len = read_tools_array_len().expect("params.tools should be present");
        assert_eq!(
            len, 7,
            "no-tier bootstrap gets all 7 tools (Lesson 526); was 0 before"
        );
    }

    #[test]
    fn free_tier_writes_all_seven_tools() {
        // Lesson 526: Free tier gets all 7 tools. Rate limiting
        // (TPM) is the actual control, not capability gating.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let len = read_tools_array_len().expect("params.tools should be present");
        assert_eq!(len, 7, "Free tier gets all 7 tools (Lesson 526); was 0");
    }

    #[test]
    fn paid_tiers_write_all_seven_tools() {
        for tier in [
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
                7,
                "tier {tier:?} should write 7 tools, got {} ({names:?})",
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
                "bash_run", "apply_patch", "remember_fact",
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
        // ALL tiers, so a `[]` on disk always gets re-stamped to 7.
        // But NON-EMPTY arrays (the actual user-edited case — e.g.
        // user removed a tool they don't want) must still be preserved.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call: Pro tier writes 7 tools.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        assert_eq!(read_tools_array_len(), Some(7));

        // Mutate the on-disk config to a smaller array (user removed
        // 6 of the 7 tools by hand).
        let path = openclaw_json_path();
        let raw = std::fs::read_to_string(&path).expect("read");
        let mut cfg: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        cfg["models"]["providers"]["maic"]["params"]["tools"] = serde_json::json!([
            {"type":"function","function":{"name":"read_file"}}
        ]);
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap())
            .expect("write");

        // Second call: any tier. The non-empty single-tool array
        // must be preserved (not re-stamped to 7).
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        assert_eq!(
            read_tools_array_len(),
            Some(1),
            "user's custom 1-tool array must be preserved across logins"
        );
    }

    // =====================================================================
    // Lesson 527 (NEW 2026-08-21 13:57 MDT): tier-gated model list.
    // Free = m1-t1 + m1-t2 only (chat-only fast tier). Paid = all 17.
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
    fn free_tier_sees_only_two_models() {
        // Lesson 527: Free tier sees only m1-t1 + m1-t2 in the
        // model picker. These are 7B distilled ternary models —
        // chat-only fast tier. Picking anything else would blow
        // through the 50K TPM ceiling in a few messages.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            2,
            "Free tier should see exactly 2 models (m1-t1, m1-t2); got {ids:?}"
        );
        assert!(ids.contains(&"milagro-m1-t1".to_string()));
        assert!(ids.contains(&"milagro-m1-t2".to_string()));
        // No 14B models allowed for Free.
        for forbidden in ["milagro-dev", "milagro-dev-coder", "milagro-m1", "milagro-m1-t3"] {
            assert!(
                !ids.contains(&forbidden.to_string()),
                "Free tier must NOT see {forbidden}"
            );
        }
    }

    #[test]
    fn paid_tiers_see_all_seventeen_models() {
        // Lesson 527: paid tiers see the full 17-model catalog.
        // Note: existing tests like `paid_tiers_write_all_seven_tools`
        // test the TOOLS list, not the model list. This is the
        // parallel test for models.
        for tier in [
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
                17,
                "Paid tier {tier:?} should see all 17 models; got {ids:?}"
            );
            assert!(ids.contains(&"milagro-dev".to_string()));
            assert!(ids.contains(&"milagro-m1-t1".to_string()));
            assert!(ids.contains(&"milagro-oc-minimax".to_string()));
        }
    }

    #[test]
    fn downgrade_from_pro_to_free_removes_paid_models() {
        // Lesson 527: downgrade path. Pro user downgrades to Free
        // → their on-disk config must be re-stamped to only allow
        // m1-t1 + m1-t2. Without this, the user would still see
        // 14B models in the picker and could burn TPM.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call as Pro: writes 17 models.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        assert_eq!(read_model_ids().len(), 17);

        // Downgrade to Free.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            2,
            "After Pro→Free downgrade, only 2 models should remain; got {ids:?}"
        );
        assert!(ids.contains(&"milagro-m1-t1".to_string()));
        assert!(ids.contains(&"milagro-m1-t2".to_string()));
    }

    #[test]
    fn upgrade_from_free_to_pro_adds_paid_models() {
        // Lesson 527: upgrade path. Free user upgrades to Pro →
        // their on-disk config must be re-stamped to include all
        // 17 models. Without this, the user would see only 2
        // models even after paying.
        let _lock = lock_env();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "any-key");

        // First call as Free: writes 2 models.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Free).expect("ok");
        assert_eq!(read_model_ids().len(), 2);

        // Upgrade to Pro.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro).expect("ok");
        let ids = read_model_ids();
        assert_eq!(
            ids.len(),
            17,
            "After Free→Pro upgrade, all 17 models should be present; got {ids:?}"
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

        // Call with Pro tier. Must re-stamp tools from `[]` to 7 entries.
        ensure_maic_provider_config_for_tier(crate::auth::tier::Tier::Pro)
            .expect("ok");

        let names = read_tools_names();
        assert_eq!(
            names.len(),
            7,
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
            7,
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
        // Lesson 521: primary carries the `maic/` provider prefix so the
        // openclaw gateway dispatches via MAIC's baseUrl regardless of
        // catalog state (defends against the rc18→rc19/20 upgrade gap
        // fixed in Lesson 520).
        let primary = cfg.pointer("/agents/defaults/model/primary").unwrap();
        assert_eq!(primary, "maic/milagro-dev");
        // Free must NOT have fallbacks.
        assert!(cfg.pointer("/agents/defaults/model/fallbacks").is_none(),
                "Free must not have a fallbacks array");
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
        // Lesson 521: paid primary + fallbacks all carry `maic/` prefix.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-oc-kimi"
        );
        let fallbacks: Vec<String> = cfg
            .pointer("/agents/defaults/model/fallbacks")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            fallbacks,
            vec!["maic/milagro-oc-minimax", "maic/milagro-oc-glm", "maic/milagro-dev"]
        );
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
        // Lesson 521: provider-prefixed.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-oc-kimi"
        );
    }

    #[test]
    fn lesson_517_pro_plus_team_enterprise_share_routing() {
        let _env = lock_env();
        // Sanity: ProPlus, Team, Enterprise all route to the same Kimi +
        // MiniMax + GLM + local chain. This is the invariant the user's
        // request ("paid accounts use Kimi, fallback Minimax-m3, fallback
        // glm") pins.
        for tier in [
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
            // Lesson 521: provider-prefixed primary + fallbacks.
            assert_eq!(
                cfg.pointer("/agents/defaults/model/primary").unwrap(),
                "maic/milagro-oc-kimi",
                "{:?} primary must be Kimi",
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
            assert_eq!(fallbacks[1], "maic/milagro-oc-glm");
            assert_eq!(fallbacks[2], "maic/milagro-dev");
        }
    }

    #[test]
    fn lesson_517_free_clears_stale_paid_fallbacks_on_downgrade() {
        let _env = lock_env();
        // User paid → had Kimi+fallbacks. Downgraded to Free. Next login
        // must clean up the stale fallbacks so the dropdown only shows
        // `milagro-dev`. We model this by starting with Free default
        // + a fallback array, then calling the writer with Free.
        let _g = fresh_openclaw_with_model(Some(""), Some(vec!["maic/milagro-oc-minimax", "maic/milagro-dev"]));
        let wrote = ensure_agents_default_model_for_tier(crate::auth::tier::Tier::Free)
            .expect("writer should succeed");
        assert!(wrote, "empty primary + non-empty fallbacks must trigger write");

        let path = openclaw_json_path();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Lesson 521: provider-prefixed.
        assert_eq!(
            cfg.pointer("/agents/defaults/model/primary").unwrap(),
            "maic/milagro-dev"
        );
        assert!(
            cfg.pointer("/agents/defaults/model/fallbacks").is_none(),
            "Free downgrade must clear the fallbacks array (was: {:?})",
            cfg.pointer("/agents/defaults/model/fallbacks")
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
        // tier (no context). Free gets only m1-t1 + m1-t2 — 2 models,
        // not 17. The Lesson 520 test ran with the old assumption
        // (default = all 17). New default for this code path is Free.
        for id in ["milagro-m1-t1", "milagro-m1-t2"] {
            assert!(ids.contains(id), "Free tier must include model {}", id);
        }
        // Total = 2 unique ids for Free tier.
        assert_eq!(models.len(), 2, "Free tier should see exactly 2 models (m1-t1, m1-t2)");
    }

    #[test]
    fn lesson_520_known_ids_merge_is_idempotent() {
        // Re-running ensure_maic_provider_config() on a file that already
        // has all 17 ids must NOT add duplicates and must NOT change the
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
}
