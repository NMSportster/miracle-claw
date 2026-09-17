//! workspace_commands.rs — Tauri command surface for the workspace
//! sync layer (Phase 3).
//!
//! Spec: memory/projects/workspace_architecture.md
//! MAIC backend: api/routes/workspace.py
//!
//! The desktop side of the workspace model is mostly a background
//! sync loop (see workspace_sync.rs). The Tauri commands are a thin
//! status surface for the dashboard + a hook for the future
//! "show me notes needing approval" UI.
//!
//! Commands:
//!   mc_workspace_status       — running flag + counters for the dashboard
//!   mc_workspace_refresh_jwt  — re-read MAIC_API_KEY after silent_relogin
//!                               (call this when the user signs in mid-session)
//!
//! The actual claim/execute work runs on its own thread and doesn't
//! need a Tauri command per note — that's the point of MAIC being the
//! message bus.

use std::sync::Arc;

use tauri::State;

use crate::workspace_sync::{WorkspaceClient, WorkspaceStatus};

// ─────────────────────────────────────────────────────────────────────
// Commands
// ─────────────────────────────────────────────────────────────────────

/// Snapshot of the workspace sync state. Cheap to call (atomic loads).
#[tauri::command]
pub fn mc_workspace_status(
    client: State<'_, Arc<WorkspaceClient>>,
) -> Result<WorkspaceStatus, String> {
    Ok(client.status())
}

/// Re-read MAIC_API_KEY from env into the running client. Call after
/// `silent_relogin` (or any other login flow) so the bg loop picks up
/// the new token without a full restart.
#[tauri::command]
pub fn mc_workspace_refresh_jwt(
    client: State<'_, Arc<WorkspaceClient>>,
) -> Result<(), String> {
    let jwt = std::env::var("MAIC_API_KEY").unwrap_or_default();
    client.set_jwt(jwt);
    Ok(())
}
