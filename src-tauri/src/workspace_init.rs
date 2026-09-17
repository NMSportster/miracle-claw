//! workspace_init.rs — Wire the workspace sync layer into Tauri state.
//!
//! Spec: memory/projects/workspace_architecture.md
//! MAIC backend: api/routes/workspace.py
//!
//! Replaces the role of pairing_init.rs for the new model. The desktop
//! no longer needs to be "paired" — it just needs a valid MAIC JWT and
//! the endpoint URL, and it'll discover work via the workspace feed.
//!
//! Pattern mirrors pairing_init.rs so the call site in lib.rs is
//! identical:
//!   1. Read JWT from `MAIC_API_KEY` (set by `silent_relogin`).
//!   2. Read MAIC endpoint from `MAIC_BASE` / `MAIC_URL` /
//!      `MILAGRO_MAIC_URL` env vars (same precedence as pairing).
//!   3. Build a `WorkspaceClient` and wrap it in `Arc`.
//!   4. Return. Tauri's state-management puts it in
//!      `tauri::State<WorkspaceClient>`.

use std::sync::Arc;

use crate::workspace_sync::WorkspaceClient;

/// Build a `WorkspaceClient` ready to be managed by Tauri state.
///
/// Called from lib.rs::setup (or wherever we want to bootstrap). Reads
/// JWT from env on every call — the user logs in via the Tauri command
/// surface, then we re-init the client with the new JWT. (Actually, the
/// PairingCommands pattern did this; for now we just read once at
/// startup and rely on silent_relogin to update MAIC_API_KEY.)
pub fn build_workspace_state() -> Arc<WorkspaceClient> {
    let jwt = std::env::var("MAIC_API_KEY").unwrap_or_default();
    let endpoint = resolve_maic_base_url_local();
    Arc::new(WorkspaceClient::new(endpoint, jwt))
}

/// Local copy of `resolve_maic_base_url` to avoid a circular dependency
/// with lib.rs. Precedence (first match wins):
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
