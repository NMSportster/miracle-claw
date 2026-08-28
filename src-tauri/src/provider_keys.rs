// src-tauri/src/provider_keys.rs
//
// Lesson 713 (2026-08-28 06:48 MDT, David): BYO provider keys for
// Miracle Claw. Lets a user bring their own OpenAI / Anthropic / Ollama
// Cloud / etc. key instead of always routing through MAIC's billing.
//
// Why this lives in its own module (not lib.rs):
//   - lib.rs is already 9000+ lines. Provider-keys logic is small but
//     distinct (env var + vault + openclaw.json patching).
//   - Tests live next to the code so a future refactor doesn't lose them.
//   - The openclaw plugin manifest declares per-provider env var names
//     (e.g. "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OLLAMA_API_KEY"),
//     and we hard-code the same names here so the two stay in sync.
//
// Threat model:
//   - The user's API key is stored in the encrypted vault (rc53.8
//     AES-256-GCM, master key in OS keychain). It is NEVER written
//     to openclaw.json as a literal — only as a SecretRef so a casual
//     `cat openclaw.json` doesn't leak it.
//   - The key IS set in the Tauri process env so the openclaw runtime
//     (which reads `process.env.OPENAI_API_KEY` etc.) picks it up.
//     The env var does NOT persist across restarts; the vault entry
//     does, and on every restart we re-set the env from the vault.
//
// Public API:
//   - mc_set_provider_key(provider_id, api_key, base_url) -> ProviderKeySummary
//   - mc_list_provider_keys() -> Vec<ProviderKeySummary>
//   - mc_clear_provider_key(provider_id) -> bool
//
// All three commands require the user to be logged in (JWT in env).
// See check_jwt_required() below — the same gate MAIC login uses.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Summary returned to the UI. Never carries the plaintext key — only
/// its length and the canonical env var name.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProviderKeySummary {
    /// Provider id as known by the openclaw runtime, e.g. "openai",
    /// "anthropic", "ollama-cloud". Lowercase, no spaces.
    pub provider_id: String,
    /// Canonical env var name the openclaw runtime reads. e.g.
    /// "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OLLAMA_API_KEY".
    pub env_var: String,
    /// Friendly display name for the UI, e.g. "OpenAI", "Anthropic",
    /// "Ollama Cloud". Pulled from PROVIDER_REGISTRY below.
    pub display_name: String,
    /// Whether the vault has a key stored. UI uses this to render
    /// "✓ Set" vs "Not set" pills.
    pub is_set: bool,
    /// Length of the stored value. UI renders "•••• (39 chars)".
    pub value_len: usize,
    /// Optional base_url override. None = use openclaw default. e.g.
    /// "https://api.openai.com/v1" or "https://ollama.com/v1".
    pub base_url: Option<String>,
    /// ISO-ish creation timestamp.
    pub created_at: String,
}

/// What we return from `mc_set_provider_key`.
#[derive(Serialize, Clone, Debug)]
pub struct SetProviderKeyResult {
    pub provider_id: String,
    pub env_var: String,
    pub summary: ProviderKeySummary,
}

/// Provider registry. Maps provider_id -> (env_var, display_name,
/// default_base_url). Keep this in sync with the openclaw plugin
/// manifests under src-tauri/resources/dist/extensions/<id>/openclaw.plugin.json
/// — when a plugin manifest adds a new api-key method, add a row here.
///
/// To add a new provider:
///   1. Find the plugin's setup.envVars[0] in openclaw.plugin.json
///      (e.g. "ANTHROPIC_API_KEY").
///   2. Add a row here.
///   3. Done — UI auto-discovers from this map.
pub const PROVIDER_REGISTRY: &[(&str, &str, &str, &str)] = &[
    // (provider_id, env_var, display_name, default_base_url)
    ("openai", "OPENAI_API_KEY", "OpenAI", "https://api.openai.com/v1"),
    ("anthropic", "ANTHROPIC_API_KEY", "Anthropic", "https://api.anthropic.com"),
    ("ollama-cloud", "OLLAMA_API_KEY", "Ollama Cloud", "https://ollama.com/v1"),
    ("mistral", "MISTRAL_API_KEY", "Mistral", "https://api.mistral.ai/v1"),
    ("cohere", "COHERE_API_KEY", "Cohere", "https://api.cohere.ai/v1"),
    ("openrouter", "OPENROUTER_API_KEY", "OpenRouter", "https://openrouter.ai/api/v1"),
    ("groq", "GROQ_API_KEY", "Groq", "https://api.groq.com/openai/v1"),
    ("together", "TOGETHER_API_KEY", "Together AI", "https://api.together.xyz/v1"),
    ("xai", "XAI_API_KEY", "xAI (Grok)", "https://api.x.ai/v1"),
    ("google", "GOOGLE_API_KEY", "Google AI", "https://generativelanguage.googleapis.com/v1beta"),
    ("voyage", "VOYAGE_API_KEY", "Voyage AI", "https://api.voyageai.com/v1"),
    ("deepgram", "DEEPGRAM_API_KEY", "Deepgram", "https://api.deepgram.com"),
    ("elevenlabs", "ELEVENLABS_API_KEY", "ElevenLabs", "https://api.elevenlabs.io"),
];

/// Look up a provider's metadata. Returns None for unknown providers.
pub fn provider_meta(provider_id: &str) -> Option<(&'static str, &'static str, &'static str)> {
    PROVIDER_REGISTRY
        .iter()
        .find(|(id, _, _, _)| *id == provider_id)
        .map(|(_, env_var, display_name, default_base_url)| (*env_var, *display_name, *default_base_url))
}

/// Return the canonical env var name for a provider.
#[allow(dead_code)] // Public helper, used by tests + future UI bridges.
pub fn env_var_for(provider_id: &str) -> Option<&'static str> {
    provider_meta(provider_id).map(|(env_var, _, _)| env_var)
}

/// Map env var -> provider_id (reverse lookup). Used when restoring
/// vault entries on startup so we know which provider each key belongs
/// to without hard-coding a parallel list.
pub fn provider_for_env_var(env_var: &str) -> Option<&'static str> {
    PROVIDER_REGISTRY
        .iter()
        .find(|(_, ev, _, _)| *ev == env_var)
        .map(|(id, _, _, _)| *id)
}

/// Validate a provider_id against the registry. Returns the canonical
/// metadata tuple or an error string for the UI to surface.
pub fn validate_provider_id(provider_id: &str) -> Result<(&'static str, &'static str, &'static str), String> {
    provider_meta(provider_id).ok_or_else(|| {
        format!(
            "unknown provider `{provider_id}`; supported: {}",
            PROVIDER_REGISTRY
                .iter()
                .map(|(id, _, _, _)| *id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Validate an API key value. Rejects empty strings and trivially
/// malformed inputs. Provider-specific format checks (e.g. "sk-" prefix
/// for OpenAI) live in the UI for nice error messages — we do the
/// generic "not empty, no whitespace, reasonable length" check here.
pub fn validate_api_key(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("API key cannot be empty".to_string());
    }
    if value.len() > 4096 {
        return Err("API key too long (>4096 chars)".to_string());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("API key cannot contain whitespace".to_string());
    }
    Ok(())
}

/// Validate a base_url. Rejects empty / malformed / non-https URLs
/// (with one exception for http://localhost-style local Ollama).
pub fn validate_base_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        // Empty means "use provider default" — that's fine.
        return Ok(());
    }
    let lower = trimmed.to_ascii_lowercase();
    let allowed_http = lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://0.0.0.0");
    if !trimmed.starts_with("https://") && !allowed_http {
        return Err(format!(
            "base_url must start with https:// (or http://localhost for local Ollama); got `{trimmed}`"
        ));
    }
    Ok(())
}

// ---------- Tauri commands ----------

/// `mc_set_provider_key(provider_id, api_key, base_url)` — store the
/// user's API key in the encrypted vault AND set the corresponding
/// env var in the Tauri process so the openclaw runtime picks it up
/// on the next request.
///
/// Requires JWT in env (callers go through MAIC login first — see
/// require_jwt()).
#[tauri::command]
pub fn mc_set_provider_key(
    app: tauri::AppHandle,
    provider_id: String,
    api_key: String,
    base_url: Option<String>,
) -> Result<SetProviderKeyResult, String> {
    require_jwt()?;

    let (env_var, display_name, default_base_url) = validate_provider_id(&provider_id)?;
    validate_api_key(&api_key)?;
    let normalized_base = base_url.unwrap_or_else(|| default_base_url.to_string());
    validate_base_url(&normalized_base)?;

    // Step 1: upsert into the encrypted vault under the canonical env var name.
    // Reuse the existing mc_secret_set path so encryption + persistence are
    // identical to the user-typed Secrets vault UI.
    let summary = crate::mc_secret_set(app.clone(), env_var.to_string(), api_key.clone())?;

    // Step 2: set the env var in THIS process so the openclaw runtime
    // (which reads process.env.X via index.js:483) sees the new key on
    // the next chat request. The env var does NOT survive process
    // restart — vault + restore_provider_keys_on_boot() handle that.
    // SAFETY: std::env::set_var is unsafe in Rust 2024. We accept the
    // unsafety because MC is single-process and the openclaw runtime
    // relies on process.env reads to resolve API keys.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var(env_var, &api_key);
    }

    // Step 3: also set the provider-specific base_url env var if the
    // plugin uses one. Some plugins (Anthropic, Ollama Cloud) read a
    // separate BASE_URL env. We use the openclaw plugin manifest
    // convention here: if base_url differs from default, also set
    // <ENV_VAR>_BASE_URL so the plugin can pick it up.
    let base_env_var = format!("{env_var}_BASE_URL");
    let should_set_base = !normalized_base.is_empty() && normalized_base != default_base_url;
    if should_set_base {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(&base_env_var, &normalized_base);
        }
    } else {
        // Clear any stale override.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(&base_env_var);
        }
    }

    // Step 4: fire-and-forget telemetry ping to MAIC so we can measure
    // BYO adoption. Best-effort, never blocks the user, never surfaces
    // errors. See `ping_byok_event` for the full design.
    ping_byok_event(
        "byok_key_saved",
        provider_id.clone(),
        env_var.to_string(),
        should_set_base,
    );

    Ok(SetProviderKeyResult {
        provider_id: provider_id.clone(),
        env_var: env_var.to_string(),
        summary: ProviderKeySummary {
            provider_id,
            env_var: env_var.to_string(),
            display_name: display_name.to_string(),
            is_set: true,
            value_len: summary.value_len,
            base_url: if normalized_base == default_base_url {
                None
            } else {
                Some(normalized_base)
            },
            created_at: summary.created_at,
        },
    })
}

/// `mc_list_provider_keys()` — return summaries for every provider in
/// the registry. Used by the UI to render the Provider Keys page.
#[tauri::command]
pub fn mc_list_provider_keys(app: tauri::AppHandle) -> Result<Vec<ProviderKeySummary>, String> {
    require_jwt()?;

    // Fetch the full vault once, index by name.
    let entries = crate::read_vault(&app)?;
    let vault_index: HashMap<String, &crate::SecretEntry> =
        entries.iter().map(|e| (e.name.clone(), e)).collect();

    let now = crate::now_iso();
    let mut out = Vec::with_capacity(PROVIDER_REGISTRY.len());
    for (provider_id, env_var, display_name, default_base_url) in PROVIDER_REGISTRY {
        let entry = vault_index.get(*env_var);
        let base_env_var = format!("{env_var}_BASE_URL");
        let base_url = std::env::var(&base_env_var)
            .ok()
            .filter(|s| !s.is_empty() && s != *default_base_url);
        out.push(ProviderKeySummary {
            provider_id: provider_id.to_string(),
            env_var: env_var.to_string(),
            display_name: display_name.to_string(),
            is_set: entry.is_some(),
            value_len: entry.map(|e| e.value.chars().count()).unwrap_or(0),
            base_url,
            created_at: entry.map(|e| e.created_at.clone()).unwrap_or_else(|| now.clone()),
        });
    }
    Ok(out)
}

/// `mc_clear_provider_key(provider_id)` — wipe the vault entry AND
/// the env var. Returns true if a vault entry was deleted.
#[tauri::command]
pub fn mc_clear_provider_key(
    app: tauri::AppHandle,
    provider_id: String,
) -> Result<bool, String> {
    require_jwt()?;

    let (env_var, _, _) = validate_provider_id(&provider_id)?;
    let deleted = crate::mc_secret_delete(app, env_var.to_string())?;

    // Also clear the env var so subsequent requests don't pick up a
    // stale value.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(env_var);
        std::env::remove_var(format!("{env_var}_BASE_URL"));
    }

    // Step: fire-and-forget telemetry ping. Same pattern as save --
    // we tell MAIC the user cleared their key so adoption analytics
    // can compute net-BYOs over time (saves minus clears).
    ping_byok_event(
        "byok_key_cleared",
        provider_id.clone(),
        env_var.to_string(),
        false,
    );

    Ok(deleted)
}

/// `mc_restore_provider_keys_on_boot(app)` — re-set env vars from the
/// vault. Called once at Tauri setup() so a returning user (who
/// already saved keys last session) doesn't have to re-enter them.
///
/// This is intentionally NOT a #[tauri::command] — it's called from
/// inside the setup hook with the AppHandle directly.
pub fn restore_provider_keys_on_boot(app: &tauri::AppHandle) -> Result<usize, String> {
    let entries = crate::read_vault(app)?;
    let mut restored = 0usize;
    for entry in entries {
        if let Some(provider_id) = provider_for_env_var(&entry.name) {
            // Only restore keys we know about. Skip unknown vault entries.
            let _ = provider_id;
            #[allow(unused_unsafe)]
            unsafe {
                std::env::set_var(&entry.name, &entry.value);
            }
            restored += 1;
        }
    }
    Ok(restored)
}

// ---------- Telemetry pings to MAIC (Lesson 713, 2026-08-28) ----------
//
// Why this exists:
//   We need adoption analytics for client-side BYO provider keys,
//   but the openclaw runtime never talks to MAIC when a user has
//   set their own key — it bypasses our gateway entirely to save
//   the user per-token cost. So MAIC's chat traffic logs show zero
//   BYO activity, even when adoption is high.
//
// Fix: when MC saves/clears a provider key, fire a fire-and-forget
// POST to MAIC's /v1/telemetry/byok-event endpoint. The payload
// has NO key material — only metadata (which provider, whether a
// custom base_url was set, app version, etc). MAIC stores it in
// the byok_events table (migration 032) for the admin dashboard.
//
// Why fire-and-forget:
//   The user is staring at the Provider Keys UI waiting for their
//   save to complete. We MUST NOT block that on a MAIC gateway
//   round-trip. The vault write (which IS the source of truth for
//   the user) is sync; the telemetry POST happens in the background
//   via `tauri::async_runtime::spawn`. If MAIC is down, the event
//   is lost — that's fine, it's analytics, not billing.

const MAIC_TELEMETRY_PATH: &str = "/v1/telemetry/byok-event";

/// Resolve the MAIC base URL at runtime. Mirrors the same logic
/// lib.rs uses at lines 1009-1013 (MAIC_API_URL env override,
/// then DEFAULT_ENDPOINT). Kept inline so we don't depend on
/// lib.rs's internal ordering.
fn maic_telemetry_endpoint() -> String {
    let raw = std::env::var("MAIC_API_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::DEFAULT_ENDPOINT.to_string());
    // Strip a trailing /v1 if the user set MAIC_API_URL=https://.../v1
    // so we don't end up POSTing to /v1/v1/telemetry/...
    let trimmed = raw.trim().trim_end_matches('/');
    let without_v1 = trimmed.trim_end_matches("/v1");
    format!("{}/{}", without_v1, MAIC_TELEMETRY_PATH.trim_start_matches('/'))
}

/// MC's app version, formatted as "v<cargo_version>". Used in the
// `app_version` field of every telemetry event so we can later
// segment adoption by client version.
const APP_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// Fire a telemetry ping to MAIC in the background. NEVER blocks
/// the caller. NEVER panics. NEVER surfaces errors to the user.
///
/// Failure modes we silently swallow:
///   - MAIC gateway unreachable (network error, timeout)
///   - MAIC returns non-2xx (auth expired, gateway down, etc.)
///   - reqwest internal error
///
/// If MAIC is having a bad day, the user keeps their saved key
/// (the vault IS the source of truth); we just lose one row of
/// analytics. Acceptable trade-off.
fn ping_byok_event(event_name: &'static str, provider_id: String, env_var: String, has_base_url_override: bool) {
    let url = maic_telemetry_endpoint();
    let jwt = std::env::var(crate::ENV_VAR_NAME).unwrap_or_default();
    let app_version = APP_VERSION.to_string();

    // Build the JSON body. We hand-roll a tiny serializer here
    // because reqwest::json is overkill for 6 fields and we want
    // to keep the spawn closure Send-safe without depending on
    // serde_json in the closure.
    let body = format!(
        r#"{{"event":"{}","provider_id":"{}","env_var":"{}","has_base_url_override":{},"app_version":"{}"}}"#,
        event_name,
        provider_id.replace('"', r#"\""#),
        env_var.replace('"', r#"\""#),
        if has_base_url_override { "true" } else { "false" },
        app_version.replace('"', r#"\""#),
    );

    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[miracle-claw] byok telemetry: client build failed: {e}");
                return;
            }
        };
        let req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body);
        let req = if !jwt.is_empty() {
            req.header("Authorization", format!("Bearer {jwt}"))
        } else {
            req
        };
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                // Quiet success — the user doesn't need to know
                // telemetry landed. (They'll see it in MAIC's
                // dashboard.)
            }
            Ok(resp) => {
                eprintln!(
                    "[miracle-claw] byok telemetry: MAIC returned {} for {} {}",
                    resp.status(),
                    event_name,
                    provider_id
                );
            }
            Err(e) => {
                eprintln!(
                    "[miracle-claw] byok telemetry: send failed for {} {}: {e}",
                    event_name, provider_id
                );
            }
        }
    });
}

/// Gate every provider-keys command behind a valid JWT. Mirrors the
/// pattern other MAIC-aware commands use: env var must be set to a
/// non-empty JWT-shaped string. We deliberately don't *verify* the
/// JWT here — that's the gateway's job. We just refuse to act on
/// provider keys when the user isn't logged in.
fn require_jwt() -> Result<(), String> {
    let jwt = std::env::var(crate::ENV_VAR_NAME).unwrap_or_default();
    if jwt.is_empty() {
        return Err(
            "Sign in to MiracleClaw before managing provider keys. \
             The login flow sets MAIC_API_KEY in the process env; we \
             refuse to store BYO keys until that's present."
                .to_string(),
        );
    }
    if jwt.len() < 16 {
        return Err("MAIC_API_KEY in env is too short to be a valid JWT".to_string());
    }
    Ok(())
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_known_providers() {
        assert!(provider_meta("openai").is_some());
        assert!(provider_meta("anthropic").is_some());
        assert!(provider_meta("ollama-cloud").is_some());
        assert_eq!(env_var_for("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(env_var_for("anthropic"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(env_var_for("ollama-cloud"), Some("OLLAMA_API_KEY"));
    }

    #[test]
    fn unknown_provider_rejected() {
        let err = validate_provider_id("not-a-real-provider").unwrap_err();
        assert!(err.contains("unknown provider"));
        assert!(err.contains("openai")); // lists supported ones
    }

    #[test]
    fn api_key_validation() {
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key("sk-abc123def456").is_ok());
        assert!(validate_api_key("has spaces").is_err());
        assert!(validate_api_key("\twith\ttabs").is_err());
        let long = "x".repeat(4097);
        assert!(validate_api_key(&long).is_err());
    }

    #[test]
    fn base_url_validation() {
        // Empty = use default, allowed.
        assert!(validate_base_url("").is_ok());
        assert!(validate_base_url("   ").is_ok());
        // HTTPS always allowed.
        assert!(validate_base_url("https://api.openai.com/v1").is_ok());
        // HTTP only for localhost loopback (Ollama local).
        assert!(validate_base_url("http://localhost:11434/v1").is_ok());
        assert!(validate_base_url("http://127.0.0.1:11434/v1").is_ok());
        // HTTP to a remote host is rejected.
        assert!(validate_base_url("http://api.openai.com").is_err());
        // Non-URL is rejected.
        assert!(validate_base_url("not-a-url").is_err());
    }

    #[test]
    fn reverse_env_var_lookup() {
        assert_eq!(provider_for_env_var("OPENAI_API_KEY"), Some("openai"));
        assert_eq!(provider_for_env_var("ANTHROPIC_API_KEY"), Some("anthropic"));
        assert_eq!(provider_for_env_var("TOTALLY_MADE_UP_KEY"), None);
    }

    #[test]
    fn require_jwt_blocks_empty_env() {
        // Snapshot, clear, check, restore. Other tests in the suite
        // (e.g. maic_login tests) set MAIC_API_KEY and would collide
        // without this.
        let saved = std::env::var(crate::ENV_VAR_NAME).ok();
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(crate::ENV_VAR_NAME);
        }
        let result = require_jwt();
        #[allow(unused_unsafe)]
        unsafe {
            if let Some(v) = saved {
                std::env::set_var(crate::ENV_VAR_NAME, v);
            }
        }
        assert!(result.is_err(), "should reject empty JWT");
    }

    #[test]
    fn maic_telemetry_endpoint_handles_v1_suffix_and_overrides() {
        // Snapshot the env, set up scenarios, restore.
        let saved_url = std::env::var("MAIC_API_URL").ok();
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var("MAIC_API_URL");
        }
        // Default endpoint (no override)
        let url = maic_telemetry_endpoint();
        assert_eq!(url, "https://maicserver.com/v1/telemetry/byok-event");

        // User set /v1 explicitly (common mistake)
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("MAIC_API_URL", "https://maicserver.com/v1");
        }
        let url = maic_telemetry_endpoint();
        assert_eq!(url, "https://maicserver.com/v1/telemetry/byok-event");

        // User set with trailing slash
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("MAIC_API_URL", "https://maicserver.com/");
        }
        let url = maic_telemetry_endpoint();
        assert_eq!(url, "https://maicserver.com/v1/telemetry/byok-event");

        // User set a self-hosted endpoint
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("MAIC_API_URL", "http://localhost:8080");
        }
        let url = maic_telemetry_endpoint();
        assert_eq!(url, "http://localhost:8080/v1/telemetry/byok-event");

        // Restore
        #[allow(unused_unsafe)]
        unsafe {
            match saved_url {
                Some(v) => std::env::set_var("MAIC_API_URL", v),
                None => std::env::remove_var("MAIC_API_URL"),
            }
        }
    }

    #[test]
    fn app_version_is_nonempty() {
        // Sanity check: APP_VERSION must produce a non-empty string.
        // The exact format depends on CARGO_PKG_VERSION, but it
        // should always start with 'v'.
        assert!(APP_VERSION.starts_with('v'), "APP_VERSION should start with 'v', got {APP_VERSION}");
        assert!(APP_VERSION.len() > 1, "APP_VERSION should not be just 'v'");
    }
}
