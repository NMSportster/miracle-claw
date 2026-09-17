//! MAIC HTTP client — thin reqwest wrapper around the OpenAI-compatible endpoints
//!
//! All requests go through `Client::request` which handles:
//! - Bearer token attachment (from keychain or `MAIC_JWT` env var)
//! - 401 → silent refresh → retry once
//! - JSON request/response
//!
//! The Tauri app uses the same endpoints; we deliberately mirror the
//! request shape so that any MAIC-side change benefits both clients.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::auth;
use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    http: reqwest::Client,
    /// If set, `get_token` returns this instead of calling the auth layer.
    /// Only used by tests (via `with_base_url_and_token`).
    token_override: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("authentication required: run `mclaw login` or set MAIC_JWT")]
    NoAuth,
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("rate limited (try again in {0:?})")]
    RateLimited(Duration),
    #[error("MAIC error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

impl Client {
    pub fn new(config: &Config) -> Result<Self> {
        // The base URL is the only thing we need from Config; the JWT comes
        // from auth::load_token() in resolve_token(). We can't use
        // with_base_url_and_token here because we don't have the token yet.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("mclaw/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            base_url: config.resolved_base_url().trim_end_matches('/').to_string(),
            http,
            token_override: None,
        })
    }

    /// Build a client pointed at a specific base URL with a specific JWT,
    /// ignoring Config and the env var. Used by integration tests that need
    /// to talk to wiremock servers without racing on global state.
    pub fn with_base_url_and_token(base_url: &str, token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("mclaw/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            // We override at request time via `with_token_override`; see `get_token()`.
            token_override: Some(token.to_string()),
        })
    }

    /// Async version: prefer the override (test fixture), else call
    /// `auth::load_token()` (production: keychain + env var).
    async fn resolve_token(&self) -> Result<String, Error> {
        if let Some(t) = &self.token_override {
            return Ok(t.clone());
        }
        auth::load_token()
            .await
            .map_err(|e| Error::Api { status: 0, message: e.to_string() })?
            .ok_or(Error::NoAuth)
    }

    /// Fetch `/v1/auth/me` — current user info (tier, plan, email).
    pub async fn whoami(&self) -> Result<WhoAmI, Error> {
        let resp = self.get("/v1/auth/me").await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: WhoAmI = resp.json().await?;
        Ok(body)
    }

    /// Fetch `/v1/models` — list of available models.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, Error> {
        let resp = self.get("/v1/models").await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: ModelsResp = resp.json().await?;
        Ok(body.data)
    }

    // ─────────────────────────────────────────────────────────────────
    // Module endpoints (Week 5 — rc56.0 Module-Driven Surfaces)
    //
    // These back `mclaw run <module>.<tool>`, `mclaw tools`, and
    // `mclaw marketplace`. Spec: docs/mc-module-driven-surfaces-2026-09.md
    // §3.4 (endpoints) + §5.3 (mclaw CLI).
    // ─────────────────────────────────────────────────────────────────

    /// Fetch `/v1/categories` — public, no auth.
    pub async fn categories(&self) -> Result<Vec<Category>, Error> {
        let resp = self.get("/v1/categories").await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: CategoriesResp = resp.json().await?;
        Ok(body.categories)
    }

    /// Fetch `/v1/marketplace/modules` — full marketplace catalog.
    /// Optional `category` filter (e.g. "data", "content").
    pub async fn marketplace(&self, category: Option<&str>) -> Result<Vec<ModuleSummary>, Error> {
        let mut path = "/v1/marketplace/modules".to_string();
        if let Some(cat) = category {
            // Server expects ?category=foo, url-encode the value just in case.
            path.push_str(&format!("?category={}", urlencode(cat)));
        }
        let resp = self.get(&path).await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: MarketplaceResp = resp.json().await?;
        Ok(body.modules)
    }

    /// Fetch `/v1/modules/installed` — modules the current user has installed.
    pub async fn installed_modules(&self) -> Result<InstalledResponse, Error> {
        let resp = self.get("/v1/modules/installed").await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: InstalledResponse = resp.json().await?;
        Ok(body)
    }

    /// POST `/v1/modules/install` — install a module for the current user.
    /// Idempotent: re-installing returns success with the current manifest.
    pub async fn install_module(&self, module_id: &str) -> Result<InstallResponse, Error> {
        let body = serde_json::json!({ "module_id": module_id });
        let resp = self.post("/v1/modules/install", &body).await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: InstallResponse = resp.json().await?;
        Ok(body)
    }

    /// POST `/v1/modules/uninstall` — uninstall a module. Idempotent.
    pub async fn uninstall_module(&self, module_id: &str) -> Result<UninstallResponse, Error> {
        let body = serde_json::json!({ "module_id": module_id });
        let resp = self.post("/v1/modules/uninstall", &body).await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: UninstallResponse = resp.json().await?;
        Ok(body)
    }

    /// POST `/v1/modules/{module_id}/run` — invoke a tool on a first-party module.
    /// `surface` is one of "tauri", "tui", "mclaw" — MAIC tags the run
    /// for analytics. We always pass "mclaw" from this CLI.
    pub async fn run_module(
        &self,
        module_id: &str,
        tool: &str,
        inputs: &serde_json::Value,
    ) -> Result<RunResponse, Error> {
        let path = format!("/v1/modules/{}/run", urlencode(module_id));
        let body = serde_json::json!({
            "tool": tool,
            "inputs": inputs,
            "surface": "mclaw",
        });
        let resp = self.post(&path, &body).await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: RunResponse = resp.json().await?;
        Ok(body)
    }

    /// Fetch `/v1/dashboard/usage` — current usage stats.
    #[allow(dead_code)]
    pub async fn usage(&self) -> Result<UsageResp, Error> {
        let resp = self.get("/v1/dashboard/usage").await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let body: UsageResp = resp.json().await?;
        Ok(body)
    }

    /// Call `POST /v1/chat/completions` with a single user message.
    /// Returns the full response including any tool_calls the model made.
    ///
    /// `model`: "milagro-dev" or whatever the user requested (or empty for default).
    /// `tools_enabled`: if true, sets `tool_execution: "server"` so MAIC runs
    ///   server-side tools (weather, web_search, etc.). If false, sets
    ///   `tool_execution: "client"` with empty tools (text-only response).
    pub async fn chat_completion(
        &self,
        prompt: &str,
        model: Option<&str>,
        tools_enabled: bool,
    ) -> Result<ChatResp, Error> {
        let model = model.unwrap_or("milagro-dev");
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": false,
            "tool_execution": if tools_enabled { "server" } else { "client" },
            "tools": [],
        });
        let resp = self.post("/v1/chat/completions", &body).await?;
        if !resp.status().is_success() {
            return Err(self.translate_error(resp).await);
        }
        let wire: OpenAIChatResp = resp.json().await?;
        Ok(wire.into())
    }

    // -- internal helpers --

    /// GET that auto-attaches the bearer token. On 401, clears the local
    /// token and returns the raw response (caller surfaces the auth error).
    /// MAIC's 7-day sliding window means a refresh is server-side (Lesson 194);
    /// no client refresh logic needed.
    async fn get(&self, path: &str) -> Result<reqwest::Response, Error> {
        let jwt = self.resolve_token().await?;
        let resp = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .bearer_auth(&jwt)
            .send()
            .await?;
        if resp.status() == 401 {
            // Server says token is bad. Clear local copy so the next call
            // tells the user to re-login instead of looping.
            let _ = auth::clear_token().await;
        }
        Ok(resp)
    }

    /// POST that auto-attaches the bearer token. Same 401 handling as `get`.
    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response, Error> {
        let jwt = self.resolve_token().await?;
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(&jwt)
            .json(body)
            .send()
            .await?;
        if resp.status() == 401 {
            let _ = auth::clear_token().await;
        }
        Ok(resp)
    }

    async fn translate_error(&self, resp: reqwest::Response) -> Error {
        let status = resp.status();
        // Read the body BEFORE matching on status so we can include it in the error.
        let message = resp.text().await.unwrap_or_default();
        // Headers are no longer accessible after `text()`, so capture Retry-After above by status only.

        match status.as_u16() {
            401 => {
                // Clear the bad token so the user gets a clean "run mclaw login" next time
                let _ = auth::clear_token().await;
                Error::AuthFailed(message)
            }
            429 => {
                // Without access to headers here (consumed by text()), default to 60s.
                // The Tauri app gets Retry-After from headers; for CLI, 60s is fine.
                Error::RateLimited(Duration::from_secs(60))
            }
            _ => Error::Api {
                status: status.as_u16(),
                message,
            },
        }
    }
}

// -- Response types (mirrors MAIC's wire format) --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoAmI {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub is_master: bool,
    pub tier: String,
    pub plan_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResp {
    pub data: Vec<ModelInfo>,
}

// Usage* structs are reserved for rc55.27's `mclaw usage` command.
// MAIC's /v1/dashboard/usage endpoint shape is still in flux, so we
// kept the structs but don't deserialize them yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct UsageResp {
    pub tier: String,
    pub quota: UsageQuota,
    pub used: UsageUsed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct UsageQuota {
    pub requests: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct UsageUsed {
    pub requests: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResp {
    pub model: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// Parse the OpenAI-compatible `{"choices": [...]}` envelope into our flat shape.
impl From<OpenAIChatResp> for ChatResp {
    fn from(o: OpenAIChatResp) -> Self {
        let (content, tool_calls) = o
            .choices
            .into_iter()
            .next()
            .map(|c| (c.message.content, c.message.tool_calls))
            .unwrap_or_default();
        ChatResp {
            model: o.model,
            content,
            tool_calls: tool_calls.unwrap_or_default(),
            usage: o.usage,
            finish_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatResp {
    model: String,
    choices: Vec<OpenAIChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatChoice {
    message: OpenAIChatMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default)]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// JSON output shape for `mclaw ask --json`. Includes duration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskJsonOut {
    pub model: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<ChatUsage>,
    pub duration_ms: u128,
}

/// Time a chat call and return both the response and the wall-clock duration.
pub async fn timed_chat(
    client: &Client,
    prompt: &str,
    model: Option<&str>,
    tools_enabled: bool,
) -> Result<(ChatResp, Duration), Error> {
    let start = Instant::now();
    let resp = client.chat_completion(prompt, model, tools_enabled).await?;
    let elapsed = start.elapsed();
    Ok((resp, elapsed))
}

// ─────────────────────────────────────────────────────────────────────
// Module wire types (Week 5 — rc56.0 Module-Driven Surfaces)
// ─────────────────────────────────────────────────────────────────────

/// A category as returned by `/v1/categories`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub label: String,
    pub emoji: String,
    pub sort_order: i32,
    #[serde(default)]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CategoriesResp {
    categories: Vec<Category>,
}

/// A module as it appears in the marketplace listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSummary {
    pub module_id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: String,
    #[serde(default)]
    pub subcategory: Option<String>,
    pub tier_required: String,
    pub author: String,
    #[serde(default)]
    pub rating: f64,
    #[serde(default)]
    pub install_count: i64,
    pub is_first_party: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MarketplaceResp {
    modules: Vec<ModuleSummary>,
}

/// A module as it appears in the user's "Your Tools" grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModule {
    pub module_id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: String,
    #[serde(default)]
    pub subcategory: Option<String>,
    pub installed_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub client_manifest: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledResponse {
    pub modules: Vec<InstalledModule>,
    pub grouped_by_category: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResponse {
    pub success: bool,
    pub client_manifest: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResponse {
    pub outputs: String,
    pub duration_ms: i64,
    pub module_id: String,
    pub tool: String,
    pub run_id: i64,
}

/// JSON output shape for `mclaw run --json`. Includes the raw MAIC run_id
/// for tracing and the wall-clock duration the CLI observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunJsonOut {
    pub module_id: String,
    pub tool: String,
    pub outputs: String,
    pub run_id: i64,
    pub duration_ms: i64,
    pub cli_duration_ms: u128,
}

/// Minimal RFC 3986 percent-encoder for path/query values. Avoids pulling
/// in the `urlencoding` crate — the inputs we encode are short module IDs
/// and category slugs made of `[a-z0-9_-]`, so this is just defense in depth.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod urlencode_tests {
    use super::urlencode;

    #[test]
    fn passthrough_for_safe_chars() {
        assert_eq!(urlencode("hello-world_1.0"), "hello-world_1.0");
    }

    #[test]
    fn encodes_spaces_and_slashes() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn encodes_module_id_with_specials() {
        // Tildes are safe (RFC 3986 unreserved), so we expect pass-through.
        assert_eq!(urlencode("foo~bar"), "foo~bar");
        // Question mark should encode (it's a delimiter).
        assert_eq!(urlencode("a?b"), "a%3Fb");
    }
}
