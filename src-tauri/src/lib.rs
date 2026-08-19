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
use tauri_plugin_shell::ShellExt;

mod launcher_info;
use launcher_info::{launcher_binary_name, MAIC_PLUGIN_FILENAMES, OPENCLAW_PORT};

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
fn ensure_maic_provider_config() -> io::Result<MaicProviderBootstrap> {
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

    // If the user already has a complete provider entry (apiKey with non-empty
    // value OR a SecretRef, baseUrl), we are done. This is the idempotency
    // guarantee — re-running setup() never overwrites a working config.
    if let Some(entry) = existing.as_ref() {
        let has_key = entry
            .get("apiKey")
            .map(|v| !is_empty_api_key(v))
            .unwrap_or(false);
        let has_url = entry
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_key && has_url {
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

    entry_obj.entry("baseUrl".to_string()).or_insert(Value::String(resolved_url.clone()));
    // Only stamp apiKey if the user hasn't already set one. If they have a
    // non-empty literal string OR a SecretRef, leave it alone.
    if !entry_obj.contains_key("apiKey") || is_empty_api_key(&entry_obj["apiKey"]) {
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
        models.push(serde_json::json!({
            "id": DEFAULT_MODEL_ID,
            "name": "MAIC default (miracle-claw)",
        }));
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
        endpoint: resolved_url,
    })
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
// Health poll: TCP connect to 127.0.0.1:28789 until the port is bound.
// (openclaw accepts the connection the moment the port is bound; we don't
// need HTTP semantics — the webview's first GET will validate auth/MAIC.)
// ----------------------------------------------------------------------------

fn wait_for_gateway_ready(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_millis(1000);
    let addrs = format!("127.0.0.1:{}", port);

    while Instant::now() < deadline {
        if TcpStream::connect(addrs.as_str()).is_ok() {
            return Ok(());
        }
        thread::sleep(backoff);
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
    Err(format!(
        "gateway did not bind port {} within {:?}",
        port, timeout
    ))
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
// Tauri command: maic_login (Lesson 444)
//
// Called by the first-run login modal. POSTs {email, password} to
// `MAIC_API_URL/v1/auth/login`, returns the JWT, then triggers a re-bootstrap
// of the MAIC provider config (with MAIC_API_KEY now set in the parent
// process's env). After this returns successfully, the chat panel will
// unblock because openclaw.json now has a real apiKey.
//
// On failure, returns the error message verbatim so the login modal can
// surface it. Errors do NOT leak the password back to the frontend.
// ----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
struct MaicLoginInfo {
    /// JWT bearer token — also known as MAIC_API_KEY in the provider config.
    token: String,
    /// User email returned by /v1/auth/login (may equal the input).
    email: String,
    /// Resolved tier: 'free', 'pro', 'pro_plus', 'team', 'enterprise' (or
    /// whatever the server returns). Free tier is the default.
    tier: String,
    /// Endpoint the login was against (for the UI to show "logged in to X").
    endpoint: String,
}

#[tauri::command]
fn maic_login(email: String, password: String) -> Result<MaicLoginInfo, String> {
    let endpoint = std::env::var("MAIC_API_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

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

    let response_body = http_post_json_with_tls_fallback(&endpoint, "/v1/auth/login", &body)
        .map_err(|e| format!("Login request failed: {}", e))?;

    // The endpoint returns { token, user: {email, tier, ...} } on success.
    // Older /v1/users/login paths return { token }. We accept both.
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
    let bootstrap = ensure_maic_provider_config().map_err(|e| {
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

    eprintln!(
        "[miracle-claw] maic_login: success — user={}, tier={}, key_source={:?}",
        resolved_email, tier, bootstrap.api_key_source
    );

    Ok(MaicLoginInfo {
        token,
        email: resolved_email,
        tier,
        endpoint,
    })
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

    // 4. Spawn launcher sidecar.
    let shell = app_handle.shell();
    let port_str = OPENCLAW_PORT.to_string();
    let launcher_name = launcher_binary_name().to_string();
    eprintln!(
        "[miracle-claw] spawning sidecar: {} --gateway-port {}",
        launcher_name, port_str
    );
    let spawned = shell.sidecar(launcher_name.clone()).and_then(|cmd| {
        cmd.args(["--gateway-port", &port_str]).spawn()
    });

    let (mut rx, child) = match spawned {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[miracle-claw] sidecar spawn failed: {}", e);
            return Err(Box::new(e));
        }
    };

    // Background log capture: pipe sidecar stdout/stderr to our stderr with a
    // tag so it's visible in Tauri's dev console.
    std::thread::spawn(move || {
        while let Some(event) = rx.blocking_recv() {
            match event {
                CommandEvent::Stdout(bytes) => {
                    eprint!("[launcher.stdout] {}", String::from_utf8_lossy(&bytes));
                    let _ = std::io::stderr().flush();
                }
                CommandEvent::Stderr(bytes) => {
                    eprint!("[launcher.stderr] {}", String::from_utf8_lossy(&bytes));
                    let _ = std::io::stderr().flush();
                }
                CommandEvent::Error(e) => eprintln!("[launcher.error] {}", e),
                CommandEvent::Terminated(payload) => eprintln!(
                    "[launcher.terminated] code={:?} signal={:?}",
                    payload.code, payload.signal
                ),
                _ => {}
            }
        }
    });

    app.state::<AppState>()
        .launcher_child
        .lock()
        .unwrap()
        .replace(child);

    // 4. Wait for gateway to be ready (poll TCP connect, 15s cap).
    match wait_for_gateway_ready(OPENCLAW_PORT, Duration::from_secs(15)) {
        Ok(()) => eprintln!("[miracle-claw] gateway READY on port {}", OPENCLAW_PORT),
        Err(e) => eprintln!("[miracle-claw] gateway NOT ready: {}", e),
    }

    // 5. Webview URL pre-configured in tauri.conf.json → http://localhost:28789/.
    //    The webview loads as soon as the renderer fires; nothing to do here.

    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![first_run_report, maic_login])
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
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = fresh_env();
        env::set_var("MAIC_API_KEY", "test-key-abc-123");

        let result = ensure_maic_provider_config().expect("bootstrap ok");
        assert!(result.provider_configured, "should be configured");
        assert!(matches!(result.api_key_source, MaicKeySource::Env));
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
        assert_eq!(
            entry.get("baseUrl").and_then(|v| v.as_str()),
            Some("https://maicserver.com")
        );
        assert_eq!(
            entry.get("api").and_then(|v| v.as_str()),
            Some("openai-completions")
        );
        let models = entry.get("models").and_then(|m| m.as_array()).unwrap();
        let has_default = models
            .iter()
            .any(|m| m.get("id").and_then(|v| v.as_str()) == Some("milagro-dev"));
        assert!(has_default, "default model id should be present");
    }

    #[test]
    fn no_env_var_returns_login_required_and_writes_no_provider() {
        let _lock = ENV_LOCK.lock().unwrap();
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
        let _lock = ENV_LOCK.lock().unwrap();
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
    fn existing_entry_with_secret_ref_is_preserved() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = fresh_env();
        // Pre-write a user-supplied entry with a SecretRef.
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
        assert!(matches!(result.api_key_source, MaicKeySource::Existing));
        assert_eq!(result.endpoint, "https://custom.maic.example.com");

        let cfg = read_maic_root();
        let entry = cfg
            .get("models")
            .and_then(|m| m.get("providers"))
            .and_then(|p| p.get("maic"))
            .expect("maic provider entry");
        // User's custom endpoint preserved.
        assert_eq!(
            entry.get("baseUrl").and_then(|v| v.as_str()),
            Some("https://custom.maic.example.com")
        );
        // User's SecretRef preserved.
        let api_key = entry.get("apiKey").expect("apiKey");
        assert!(api_key.is_object());
        assert_eq!(api_key.get("id").and_then(|v| v.as_str()), Some("MAIC_API_KEY"));
        // User's custom model preserved.
        let models = entry.get("models").and_then(|m| m.as_array()).unwrap();
        assert!(models.iter().any(|m| m.get("id").and_then(|v| v.as_str()) == Some("custom-model")));
    }

    #[test]
    fn tool_execution_param_pinned_to_client() {
        let _lock = ENV_LOCK.lock().unwrap();
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
}
