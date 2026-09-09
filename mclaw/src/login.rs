//! `mclaw login` / `mclaw logout` — interactive auth
//!
//! `mclaw login` prompts for username + password, calls `/v1/auth/login`,
//! and stores the JWT in the OS keychain. `mclaw logout` wipes it.
//!
//! ## Note on field names
//!
//! MAIC's login endpoint uses `name` (not `email`) for the username field.
//! This is a quirk of the dashboard code (the field is the principal's
//! display name, which doubles as a login identifier). We accept either
//! `--username` or `--email` for caller convenience; both map to the
//! `name` field in the request body.

use anyhow::{Context, Result};
use clap::Args;
use dialoguer::Password;
use serde::Deserialize;

use crate::auth;

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Username (skip the prompt). MAIC uses `name` as the login identifier.
    #[arg(long, alias = "email")]
    username: Option<String>,

    /// Password (skip the prompt; not recommended — visible in shell history)
    #[arg(long)]
    password: Option<String>,

    /// MAIC base URL (override config + env)
    #[arg(long, value_name = "URL")]
    maic_url: Option<String>,
}

pub async fn login(args: LoginArgs) -> Result<i32> {
    // Resolve base URL — flag > env > config > default
    if let Some(url) = args.maic_url.as_deref() {
        // SAFETY: documented; the user explicitly passed --maic-url
        unsafe { std::env::set_var("MAIC_BASE_URL", url) };
    }
    let config = crate::config::Config::load()?;

    let username = match args.username {
        Some(u) => u,
        None => dialoguer::Input::<String>::new()
            .with_prompt("Username (MAIC `name` field)")
            .interact_text()
            .context("reading username")?,
    };

    let password = match args.password {
        Some(p) => p,
        None => Password::new()
            .with_prompt("Password")
            .interact()
            .context("reading password")?,
    };

    let base_url = config.resolved_base_url();
    eprintln!("Logging in to {}...", base_url);

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;
    let resp = http
        .post(format!("{}/v1/auth/login", base_url.trim_end_matches('/')))
        .json(&serde_json::json!({
            "name": username,
            "password": password,
        }))
        .send()
        .await
        .context("calling /v1/auth/login")?;

    let status = resp.status();
    // Try to parse as JSON, but tolerate plain-text errors (e.g. 401 "Invalid credentials")
    let body_text = resp.text().await.unwrap_or_default();
    let body: LoginResp = match serde_json::from_str(&body_text) {
        Ok(b) => b,
        Err(_) => LoginResp {
            token: None,
            name: None,
            detail: Some(body_text),
        },
    };

    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "login failed ({}): {}",
            status,
            body.detail.unwrap_or_else(|| "(no detail)".to_string())
        ));
    }

    let jwt = body
        .token
        .ok_or_else(|| anyhow::anyhow!("login succeeded but no `token` in response"))?;
    // We don't actually use the name from the response for auth, but storing
    // it makes `mclaw whoami` work without re-fetching.
    let display_name = body.name.unwrap_or(username);

    auth::store_token(&jwt, &display_name)
        .await
        .context("storing JWT in keychain")?;

    eprintln!("Logged in as {}", display_name);
    Ok(0)
}

#[derive(Deserialize)]
struct LoginResp {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Args, Debug)]
pub struct LogoutArgs {}

pub async fn logout(_args: LogoutArgs) -> Result<i32> {
    auth::clear_token().await.context("clearing keychain")?;
    eprintln!("Logged out.");
    Ok(0)
}
