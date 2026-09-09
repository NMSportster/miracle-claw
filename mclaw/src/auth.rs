//! Auth — JWT storage in OS keychain
//!
//! MAIC issues a JWT on `POST /v1/auth/login` that we use as a bearer token
//! on every authenticated call. We store that token in the OS-native
//! credential store so users don't have to manage it manually:
//!
//! - macOS:   Keychain
//! - Windows: Credential Manager
//! - Linux:   Secret Service (gnome-keyring, kwallet, etc.)
//!
//! ## Fallback for headless servers
//!
//! Some environments (CI, Docker, headless Linux without a keyring) can't
//! store secrets in the OS credential store. For those, the user can set
//! the `MAIC_JWT` env var and we'll use that instead. The keychain takes
//! precedence if both are present — the env var is a fallback.
//!
//! ## Why no client-side refresh
//!
//! MAIC's JWT has a 7-day sliding window (Lesson 194) — the server
//! transparently extends the expiry on every authenticated call. So
//! client-side refresh logic is unnecessary; the JWT just works until
//! it's been idle for 7 days, at which point `mclaw login` re-mints it.
//!
//! If the JWT becomes invalid before MAIC refreshes it (admin revoke,
//! logout from another device, 7-day cap hit), we just clear the local
//! token and require re-login.
//!
//! ## Why keyring v3
//!
//! Keyring v3 split the API into async-only for Linux Secret Service
//! (because the dbus calls are async). v2 had a sync API that was simpler
//! but is no longer the recommended path. We use the async API throughout
//! this module.

use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "com.milagrocloud.mclaw";
const JWT_KEY: &str = "jwt";

/// What we store in the keychain. Email is just for display in `mclaw whoami`;
/// it's not used for auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuth {
    pub jwt: String,
    pub email: String,
}

/// Resolve the active JWT: env var first, then keychain.
pub async fn load_token() -> Result<Option<String>> {
    // 1. Env var fallback (for CI/headless)
    if let Ok(env_jwt) = std::env::var("MAIC_JWT") {
        if !env_jwt.is_empty() {
            log::debug!("using MAIC_JWT env var");
            return Ok(Some(env_jwt));
        }
    }

    // 2. OS keychain (keyring is sync; run in spawn_blocking to avoid blocking the runtime)
    let handle = tokio::task::spawn_blocking(|| -> Result<Option<String>> {
        let entry = Entry::new(SERVICE_NAME, JWT_KEY).context("opening keychain entry for JWT")?;
        match entry.get_password() {
            Ok(serialized) => {
                let auth: StoredAuth = serde_json::from_str(&serialized)
                    .context("parsing stored auth blob (keychain data is corrupt)")?;
                Ok(Some(auth.jwt))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("reading JWT from keychain: {}", e)),
        }
    });
    handle.await?
}

/// Same as `load_token` but also returns the email for whoami display.
#[allow(dead_code)]
pub async fn load_auth() -> Result<Option<StoredAuth>> {
    if let Ok(env_jwt) = std::env::var("MAIC_JWT") {
        if !env_jwt.is_empty() {
            return Ok(Some(StoredAuth {
                jwt: env_jwt,
                email: std::env::var("MAIC_EMAIL").unwrap_or_else(|_| "(from env)".to_string()),
            }));
        }
    }

    let handle = tokio::task::spawn_blocking(|| -> Result<Option<StoredAuth>> {
        let entry = Entry::new(SERVICE_NAME, JWT_KEY).context("opening keychain entry for JWT")?;
        match entry.get_password() {
            Ok(serialized) => {
                let auth: StoredAuth = serde_json::from_str(&serialized)
                    .context("parsing stored auth blob (keychain data is corrupt)")?;
                Ok(Some(auth))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("reading JWT from keychain: {}", e)),
        }
    });
    handle.await?
}

/// Store JWT + email in keychain.
pub async fn store_token(jwt: &str, email: &str) -> Result<()> {
    let auth = StoredAuth {
        jwt: jwt.to_string(),
        email: email.to_string(),
    };
    let serialized = serde_json::to_string(&auth).context("serializing auth blob")?;
    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let entry = Entry::new(SERVICE_NAME, JWT_KEY).context("opening keychain entry for JWT")?;
        entry
            .set_password(&serialized)
            .context("storing JWT in keychain")?;
        Ok(())
    });
    handle.await?
}

/// Wipe JWT from keychain. Used by `mclaw logout` and on 401.
pub async fn clear_token() -> Result<()> {
    let handle = tokio::task::spawn_blocking(|| -> Result<()> {
        let entry = Entry::new(SERVICE_NAME, JWT_KEY).context("opening keychain entry for JWT")?;
        match entry.delete_credential() {
            Ok(()) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(anyhow::anyhow!("deleting JWT from keychain: {}", e)),
        }
        Ok(())
    });
    handle.await?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keyring tests need a real Secret Service running on Linux.
    /// We mark them as ignored by default and only run them on macOS/Windows
    /// (where the credential store is always present) or when the user
    /// explicitly opts in with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires OS credential store; run with `cargo test -- --ignored`"]
    async fn keychain_round_trip() {
        let test_jwt = "test-jwt-abc123";
        let test_email = "test@example.com";

        store_token(test_jwt, test_email).await.expect("store should succeed");

        let loaded = load_token().await.expect("load should succeed");
        assert_eq!(loaded, Some(test_jwt.to_string()));

        clear_token().await.expect("clear should succeed");

        let loaded = load_token().await.expect("load should succeed");
        assert_eq!(loaded, None);
    }
}
