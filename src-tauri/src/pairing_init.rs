//! pairing_init.rs — Wires PairingCommands into Tauri managed state.
//!
//! Spec: docs/specs/mobile-desktop-pairing.md.
//!
//! Called from lib.rs's `.setup()` hook via `pairing_init::build_pairing_state()`.
//!
//! Behavior:
//!   1. Read JWT from `MAIC_API_KEY` env var (set by `silent_relogin` after
//!      a successful MAIC login).
//!   2. Read MAIC endpoint from `MAIC_BASE` / `MAIC_URL` / `MILAGRO_MAIC_URL`
//!      env vars (same precedence as `resolve_maic_base_url`).
//!   3. Build a `LiveMaicHttp` and wrap it in a `PairingCommands`.
//!   4. Return the PairingCommands. The Tauri state-management machinery
//!      puts it in the `tauri::State<PairingCommands>` accessible from
//!      every command handler.
//!
//! If the JWT is missing (user not logged in), the returned `PairingCommands`
//! has a `LiveMaicHttp` with an empty JWT. Every command will fail with
//! `PairingError::NotLoggedIn` until the user logs in. This is intentional —
//! the Tauri commands are user-scoped; they don't work without a session.

use std::sync::Arc;

use crate::pairing_commands::{LiveMaicHttp, PairingCommands};
use crate::pairing_state::PairingState;

/// Build a `PairingCommands` ready to be managed by Tauri state.
///
/// Called once at app startup from `lib.rs::run`. Reads JWT from env on
/// every call — if the user logs in mid-session, a fresh PairingCommands
/// would pick that up. (In practice, the user logs in before MC starts,
/// so this is a one-shot at startup.)
pub fn build_pairing_state() -> Arc<PairingCommands> {
    let jwt = std::env::var("MAIC_API_KEY").unwrap_or_default();
    let endpoint = resolve_maic_base_url_local();
    let maic = Arc::new(LiveMaicHttp::new(endpoint, jwt));
    let state = Arc::new(PairingState::new());
    Arc::new(PairingCommands::new(state, maic))
}

/// Local copy of `resolve_maic_base_url` to avoid a circular dependency with
/// lib.rs (we're already inside the same crate, but we want to keep the
/// pairing module reusable as a library).
///
/// Precedence (first match wins):
///   1. `MAIC_BASE`
///   2. `MAIC_URL`
///   3. `MILAGRO_MAIC_URL`
///   4. System OpenClaw config (skipped here — lib.rs handles that)
///   5. `DEFAULT_ENDPOINT` ("https://maicserver.com")
fn resolve_maic_base_url_local() -> String {
    for var in &["MAIC_BASE", "MAIC_URL", "MILAGRO_MAIC_URL"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    "https://maicserver.com".to_string()
}
