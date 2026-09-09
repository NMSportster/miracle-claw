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
