//! pairing_commands.rs — Tauri command surface for mobile↔desktop pairing (Phase 2.2).
//!
//! Spec: docs/specs/mobile-desktop-pairing.md.
//!
//! Six Tauri commands that the Settings page (and any internal automation)
//! invokes. They cover the full lifecycle:
//!
//!   mc_register_desktop    — register this MC instance with MAIC; get instance_id
//!   mc_unregister_desktop  — graceful shutdown; MAIC DELETE + revoke all sessions
//!   mc_initiate_pair       — desktop-side handshake (called from HTTP server handler)
//!   mc_paired_devices      — list phones currently paired with this desktop
//!   mc_revoke_device       — remove a phone (MAIC DELETE + local session revoke)
//!   mc_set_drop_folder     — set/clear the phone's filesystem sandbox root
//!   mc_get_pairing_status  — quick status snapshot for the dashboard
//!
//! Persistence: MAIC is the source of truth. Local session state is in
//! `pairing_state::PairingState` (process memory only). On app restart,
//! all session_secrets are lost — phones must re-handshake. This is the
//! intended behavior: session_secrets should never persist.
//!
//! Test strategy: unit tests with a fake MAIC HTTP layer (no live network).
//! See tests/pairing_commands_test.rs.

use std::sync::{Arc, Mutex};
use std::path::Path;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::pairing_crypto::{
    desktop_issue_handshake, new_session_id, new_session_secret,
};
use crate::pairing_state::{
    now_unix, Capability, PairingState,
};

// ─────────────────────────────────────────────────────────────────────
// Public types (returned by the Tauri commands to the JS side)
// ─────────────────────────────────────────────────────────────────────

/// Returned by `mc_register_desktop` on first-time registration.
#[derive(Debug, Clone, Serialize)]
pub struct DesktopRegistration {
    /// Stable identifier for this MC install. Persisted on disk by the
    /// caller so subsequent startups can reuse the same instance_id.
    pub instance_id: String,
    /// Base64-encoded X25519 public key the desktop advertises to phones.
    pub desktop_pubkey_b64: String,
    /// Stable fingerprint for human display in the Settings UI and on the
    /// phone. NOT used for crypto — just a stable identifier.
    pub fingerprint: String,
    /// Capabilities this desktop instance is willing to grant (v1 scope).
    pub capabilities: Vec<String>,
    /// Server-assigned drop-folder (empty string if none set).
    pub drop_folder: String,
}

/// Returned by `mc_initiate_pair` to the phone-side HTTP handler.
#[derive(Debug, Clone, Serialize)]
pub struct HandshakeResponse {
    pub session_id: String,
    pub ephemeral_pub_b64: String,
    pub encrypted_secret_b64: String,
    pub capabilities: Vec<String>,
    pub expires_at_unix: u64,
}

/// Returned by `mc_get_pairing_status` for the Settings page.
#[derive(Debug, Clone, Serialize)]
pub struct PairingStatus {
    /// True if this desktop is registered with MAIC.
    pub registered: bool,
    pub instance_id: String,
    /// Active sessions in process memory (those that have completed
    /// a handshake recently). May be > paired_devices because the
    /// phone could have re-paired without MAIC re-sync yet.
    pub active_session_count: usize,
    /// Phones currently paired (per MAIC). May be < active_session_count
    /// for the same reason.
    pub paired_device_count: usize,
    /// Drop-folder set on MAIC (empty string if not set).
    pub drop_folder: String,
}

/// Returned by `mc_paired_devices` for the Settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct PairedDeviceInfo {
    pub device_id: i64,
    pub device_name: String,
    pub phone_pubkey_b64: String,
    pub paired_at_unix: u64,
    pub last_seen_at_unix: u64,
}

// ─────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("user is not logged in to MAIC (no JWT in env)")]
    NotLoggedIn,
    #[error("MAIC request failed: {0}")]
    MaicRequest(String),
    #[error("MAIC returned malformed JSON: {0}")]
    MaicJson(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("session storage error: {0}")]
    State(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}

impl serde::Serialize for PairingError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type PairingResult<T> = Result<T, PairingError>;

// ─────────────────────────────────────────────────────────────────────
// MAIC HTTP layer (abstracted for testability)
// ─────────────────────────────────────────────────────────────────────

/// Trait that the Tauri commands use to talk to MAIC. In production this is
/// `LiveMaicHttp` (uses `ureq` like the rest of the app). In tests, we plug
/// in `FakeMaicHttp` that returns canned responses.
pub trait MaicHttp: Send + Sync {
    fn put_desktop(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value>;
    fn delete_desktop(&self, instance_id: &str) -> PairingResult<()>;
    fn post_paired_device(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value>;
    fn get_paired_devices(&self, instance_id: &str) -> PairingResult<Vec<serde_json::Value>>;
    fn delete_paired_device(
        &self,
        instance_id: &str,
        device_id: i64,
    ) -> PairingResult<()>;

    /// Phase 2.3: PUT /v1/users/me/desktops/{id}/heartbeat (60s ticker).
    /// Body shape: {endpoint?, endpoint_kind?, capabilities_hash?}.
    /// MAIC returns 204 on success; 404 means "desktop_not_registered"
    /// (cold-boot race, caller hasn't registered yet — heartbeat loop
    /// tolerates and retries).
    fn put_heartbeat(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<()>;

    fn post_drop_folder(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value>;
}

/// Production implementation: HTTPS POST/GET against `MAIC_API_URL` with the
/// `MAIC_API_KEY` JWT in `Authorization: Bearer`.
pub struct LiveMaicHttp {
    endpoint: String,
    jwt: String,
}

impl LiveMaicHttp {
    pub fn new(endpoint: String, jwt: String) -> Self {
        Self { endpoint, jwt }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.endpoint.trim_end_matches('/'), path)
    }

    fn put_json(&self, path: &str, body: &str) -> PairingResult<String> {
        use std::time::Duration;
        let url = self.url(path);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        match agent
            .put(&url)
            .set("Authorization", &format!("Bearer {}", self.jwt))
            .set("Content-Type", "application/json")
            .send_string(body)
        {
            Ok(resp) => resp.into_string().map_err(|e| PairingError::MaicRequest(format!("read body: {}", e))),
            Err(ureq::Error::Status(code, response)) => Err(PairingError::MaicRequest(format!(
                "PUT {} -> HTTP {}: {}",
                url,
                code,
                response.into_string().unwrap_or_default()
            ))),
            Err(e) => Err(PairingError::MaicRequest(format!("PUT {} -> {}", url, e))),
        }
    }

    fn get_json(&self, path: &str) -> PairingResult<String> {
        use std::time::Duration;
        let url = self.url(path);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        match agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.jwt))
            .send_bytes(&[])
        {
            Ok(resp) => resp.into_string().map_err(|e| PairingError::MaicRequest(format!("read body: {}", e))),
            Err(ureq::Error::Status(code, response)) => Err(PairingError::MaicRequest(format!(
                "GET {} -> HTTP {}: {}",
                url,
                code,
                response.into_string().unwrap_or_default()
            ))),
            Err(e) => Err(PairingError::MaicRequest(format!("GET {} -> {}", url, e))),
        }
    }

    fn delete_json(&self, path: &str) -> PairingResult<String> {
        use std::time::Duration;
        let url = self.url(path);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        match agent
            .delete(&url)
            .set("Authorization", &format!("Bearer {}", self.jwt))
            .send_bytes(&[])
        {
            Ok(resp) => resp.into_string().map_err(|e| PairingError::MaicRequest(format!("read body: {}", e))),
            Err(ureq::Error::Status(code, response)) => Err(PairingError::MaicRequest(format!(
                "DELETE {} -> HTTP {}: {}",
                url,
                code,
                response.into_string().unwrap_or_default()
            ))),
            Err(e) => Err(PairingError::MaicRequest(format!("DELETE {} -> {}", url, e))),
        }
    }
}

impl MaicHttp for LiveMaicHttp {
    fn put_desktop(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value> {
        let path = format!("/v1/users/me/desktops/{}", instance_id);
        let resp = self.put_json(&path, &body.to_string())?;
        serde_json::from_str(&resp).map_err(|e| PairingError::MaicJson(e.to_string()))
    }
    fn delete_desktop(&self, instance_id: &str) -> PairingResult<()> {
        let path = format!("/v1/users/me/desktops/{}", instance_id);
        self.delete_json(&path)?;
        Ok(())
    }
    fn post_paired_device(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value> {
        let path = format!("/v1/users/me/desktops/{}/paired-devices", instance_id);
        let resp = self.put_json(&path, &body.to_string())?;
        serde_json::from_str(&resp).map_err(|e| PairingError::MaicJson(e.to_string()))
    }
    fn get_paired_devices(&self, instance_id: &str) -> PairingResult<Vec<serde_json::Value>> {
        let path = format!("/v1/users/me/desktops/{}/paired-devices", instance_id);
        let resp = self.get_json(&path)?;
        let parsed: serde_json::Value =
            serde_json::from_str(&resp).map_err(|e| PairingError::MaicJson(e.to_string()))?;
        let arr = parsed
            .get("paired_devices")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr)
    }
    fn delete_paired_device(
        &self,
        instance_id: &str,
        device_id: i64,
    ) -> PairingResult<()> {
        let path = format!(
            "/v1/users/me/desktops/{}/paired-devices/{}",
            instance_id, device_id
        );
        self.delete_json(&path)?;
        Ok(())
    }
    fn put_heartbeat(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<()> {
        let path = format!("/v1/users/me/desktops/{}/heartbeat", instance_id);
        // MAIC returns 204 No Content; we ignore the (empty) response.
        self.put_json(&path, &body.to_string())?;
        Ok(())
    }

    fn post_drop_folder(
        &self,
        instance_id: &str,
        body: &serde_json::Value,
    ) -> PairingResult<serde_json::Value> {
        let path = format!("/v1/users/me/desktops/{}/drop-folder", instance_id);
        let resp = self.put_json(&path, &body.to_string())?;
        serde_json::from_str(&resp).map_err(|e| PairingError::MaicJson(e.to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────────
// Wrapper that the Tauri commands hang off. Holds the in-memory state
// and the MAIC HTTP layer. Stored as managed Tauri state.
// ─────────────────────────────────────────────────────────────────────

pub struct PairingCommands {
    pub state: Arc<PairingState>,
    pub maic: Arc<dyn MaicHttp>,
    /// This desktop instance's stable identity. Generated on first register
    /// call, then loaded from disk on subsequent startups.
    pub instance_id: Mutex<Option<String>>,
}

impl PairingCommands {
    pub fn new(state: Arc<PairingState>, maic: Arc<dyn MaicHttp>) -> Self {
        Self {
            state,
            maic,
            instance_id: Mutex::new(None),
        }
    }

    /// Load a saved instance_id from disk on startup (caller invokes this
    /// from the Tauri setup hook after the data dir is known).
    pub fn restore_instance_id(&self, instance_id: String) {
        *self.instance_id.lock().unwrap() = Some(instance_id);
    }

    fn require_instance_id(&self) -> PairingResult<String> {
        self.instance_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                PairingError::InvalidInput(
                    "desktop is not registered yet (call mc_register_desktop first)".into(),
                )
            })
    }
}

// ─────────────────────────────────────────────────────────────────────
// The 7 Tauri commands. They take the PairingCommands via `tauri::State`
// in the actual handler, but here we expose plain functions for testing.
// ─────────────────────────────────────────────────────────────────────

/// `mc_register_desktop` — PUT to MAIC, get back instance_id + initial state.
///
/// First call: generates a UUID + keypair, persists, PUTs.
/// Subsequent calls (idempotent): re-PUTs to refresh fingerprint/pubkey
/// (e.g. after a key rotation).
pub fn register_desktop(
    cmds: &PairingCommands,
    desktop_pubkey_b64: String,
    fingerprint: String,
) -> PairingResult<DesktopRegistration> {
    if desktop_pubkey_b64.is_empty() {
        return Err(PairingError::InvalidInput("desktop_pubkey_b64 is empty".into()));
    }
    if fingerprint.is_empty() {
        return Err(PairingError::InvalidInput("fingerprint is empty".into()));
    }

    // Reuse existing instance_id if we have one; otherwise mint a fresh UUID.
    let instance_id = {
        let guard = cmds.instance_id.lock().unwrap();
        guard.clone().unwrap_or_else(|| Uuid::new_v4().to_string())
    };

    let body = json!({
        "instance_name": hostname_or_default(),
        "public_key": desktop_pubkey_b64,
        "fingerprint": fingerprint,
        "endpoint_kind": "lan_or_relay",
    });

    let response = cmds.maic.put_desktop(&instance_id, &body)?;
    let drop_folder = response
        .get("mobile_drop_folder")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    *cmds.instance_id.lock().unwrap() = Some(instance_id.clone());

    Ok(DesktopRegistration {
        instance_id,
        desktop_pubkey_b64,
        fingerprint,
        capabilities: Capability::locked_v1_scope()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        drop_folder,
    })
}

/// `mc_register_desktop_self` — server-side keypair generation.
///
/// Phase 2.4 (NEW 2026-09-08): the frontend calls this without passing
/// a pubkey. The X25519 keypair is generated (and persisted) entirely
/// inside Rust so the secret never crosses the JS-Rust boundary.
/// See `pairing_identity.rs` for the on-disk format.
///
/// `frontend_fingerprint` is optional — if empty, falls back to the
/// fingerprint derived from the generated pubkey. (The frontend typically
/// has nothing useful to put here since it doesn't see the keypair.)
pub fn register_desktop_self(
    cmds: &PairingCommands,
    app_data_dir: &Path,
    frontend_fingerprint: String,
) -> PairingResult<DesktopRegistration> {
    let identity = match crate::pairing_identity::load(app_data_dir) {
        Ok(Some(id)) => id,
        Ok(None) => crate::pairing_identity::generate(app_data_dir).map_err(|e| {
            PairingError::State(format!("could not create pairing identity: {e}"))
        })?,
        Err(e) => {
            return Err(PairingError::State(format!(
                "pairing identity file is corrupt: {e}"
            )));
        }
    };

    let fingerprint = if frontend_fingerprint.trim().is_empty() {
        identity.fingerprint.clone()
    } else {
        frontend_fingerprint
    };

    let instance_id = {
        let guard = cmds.instance_id.lock().unwrap();
        guard.clone().unwrap_or_else(|| identity.instance_id.clone())
    };

    let body = json!({
        "instance_name": hostname_or_default(),
        "public_key": identity.desktop_pubkey_b64,
        "fingerprint": fingerprint,
        "endpoint_kind": "lan_or_relay",
    });

    let response = cmds.maic.put_desktop(&instance_id, &body)?;
    let drop_folder = response
        .get("mobile_drop_folder")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    *cmds.instance_id.lock().unwrap() = Some(instance_id.clone());

    Ok(DesktopRegistration {
        instance_id,
        desktop_pubkey_b64: identity.desktop_pubkey_b64,
        fingerprint,
        capabilities: Capability::locked_v1_scope()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        drop_folder,
    })
}

/// Restore a saved identity at app startup. Idempotent — if no identity
/// file exists yet, returns Ok(None) and the user can call
/// `register_desktop_self` later to generate one.
pub fn restore_identity(
    cmds: &PairingCommands,
    app_data_dir: &Path,
) -> PairingResult<Option<String>> {
    match crate::pairing_identity::load(app_data_dir) {
        Ok(Some(id)) => {
            *cmds.instance_id.lock().unwrap() = Some(id.instance_id.clone());
            Ok(Some(id.instance_id))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(PairingError::State(format!(
            "pairing identity file is corrupt: {e}"
        ))),
    }
}

/// `mc_unregister_desktop` — DELETE on MAIC, revoke all sessions.
pub fn unregister_desktop(cmds: &PairingCommands) -> PairingResult<()> {
    let instance_id = cmds.require_instance_id()?;
    cmds.maic.delete_desktop(&instance_id)?;
    let _ = cmds.state.revoke_instance(&instance_id);
    *cmds.instance_id.lock().unwrap() = None;
    Ok(())
}

/// `mc_initiate_pair` — desktop-side handshake. Returns the handshake
/// response to be sent back to the phone over HTTP (or MAIC relay).
///
/// `phone_pubkey_b64`: base64-encoded X25519 public key from the phone.
/// `device_name`: human-readable name (e.g. "David's Pixel 8").
pub fn initiate_pair(
    cmds: &PairingCommands,
    phone_pubkey_b64: String,
    device_name: String,
) -> PairingResult<HandshakeResponse> {
    let instance_id = cmds.require_instance_id()?;

    if phone_pubkey_b64.is_empty() {
        return Err(PairingError::InvalidInput("phone_pubkey_b64 is empty".into()));
    }
    let phone_pubkey_bytes = B64
        .decode(&phone_pubkey_b64)
        .map_err(|e| PairingError::InvalidInput(format!("phone_pubkey_b64 not base64: {}", e)))?;
    if phone_pubkey_bytes.len() != 32 {
        return Err(PairingError::InvalidInput(format!(
            "phone_pubkey must be 32 bytes, got {}",
            phone_pubkey_bytes.len()
        )));
    }
    if device_name.is_empty() {
        return Err(PairingError::InvalidInput("device_name is empty".into()));
    }

    // Tell MAIC we have a new paired device. MAIC returns the device_id.
    let phone_pubkey_fp = compute_fingerprint(&phone_pubkey_bytes);
    let body = json!({
        "device_name": device_name,
        "phone_pubkey_fp": phone_pubkey_fp,
    });
    let response = cmds.maic.post_paired_device(&instance_id, &body)?;
    let device_id = response
        .get("device_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            PairingError::MaicJson("MAIC did not return a device_id".into())
        })?;

    // Mint session + handshake response.
    let now = now_unix();
    let session_id = new_session_id_string();
    let session_secret = new_session_secret();

    let caps = Capability::locked_v1_scope();
    let capabilities_strings: Vec<String> =
        caps.iter().map(|c| c.as_str().to_string()).collect();

    let handshake = desktop_issue_handshake(
        &phone_pubkey_bytes,
        session_id_raw_from_string(&session_id),
        session_secret,
        capabilities_strings.clone(),
        DEFAULT_SESSION_TTL_SECONDS,
        now,
    )
    .map_err(|e| PairingError::Crypto(e.to_string()))?;

    let expires_at = now + DEFAULT_SESSION_TTL_SECONDS;
    cmds.state
        .mint(
            session_id.clone(),
            session_secret,
            instance_id,
            Some(device_id),
            device_name,
            caps,
            DEFAULT_SESSION_TTL_SECONDS,
            now,
        )
        .map_err(|e| PairingError::State(e.to_string()))?;

    Ok(HandshakeResponse {
        session_id,
        ephemeral_pub_b64: handshake.ephemeral_pub_b64,
        encrypted_secret_b64: handshake.encrypted_secret_b64,
        capabilities: capabilities_strings,
        expires_at_unix: expires_at,
    })
}

/// `mc_paired_devices` — list phones currently paired (per MAIC).
pub fn paired_devices(
    cmds: &PairingCommands,
) -> PairingResult<Vec<PairedDeviceInfo>> {
    let instance_id = cmds.require_instance_id()?;
    let raw = cmds.maic.get_paired_devices(&instance_id)?;
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let device_id = entry
            .get("device_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let device_name = entry
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let phone_pubkey_b64 = entry
            .get("phone_pubkey_fp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let paired_at_unix = entry
            .get("paired_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as u64;
        let last_seen_at_unix = entry
            .get("last_seen_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as u64;
        out.push(PairedDeviceInfo {
            device_id,
            device_name,
            phone_pubkey_b64,
            paired_at_unix,
            last_seen_at_unix,
        });
    }
    Ok(out)
}

/// `mc_revoke_device` — DELETE on MAIC + revoke local sessions.
pub fn revoke_device(cmds: &PairingCommands, device_id: i64) -> PairingResult<()> {
    let instance_id = cmds.require_instance_id()?;
    cmds.maic.delete_paired_device(&instance_id, device_id)?;
    let _ = cmds.state.revoke_device(device_id);
    Ok(())
}

/// `mc_set_drop_folder` — set/clear the phone's filesystem sandbox root.
/// Empty string clears (sets NULL on MAIC).
pub fn set_drop_folder(
    cmds: &PairingCommands,
    folder: String,
) -> PairingResult<String> {
    let instance_id = cmds.require_instance_id()?;
    let trimmed = folder.trim().to_string();
    let body = json!({ "mobile_drop_folder": trimmed });
    let response = cmds.maic.post_drop_folder(&instance_id, &body)?;
    let normalized = response
        .get("mobile_drop_folder")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    Ok(normalized)
}

/// `mc_get_pairing_status` — quick status for the Settings page.
pub fn get_pairing_status(cmds: &PairingCommands) -> PairingResult<PairingStatus> {
    let instance_id = cmds
        .instance_id
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();
    let now = now_unix();
    let active_session_count = cmds.state.active_count(now);
    let registered = !instance_id.is_empty();
    let drop_folder = String::new();
    let mut paired_device_count = 0;
    if registered {
        if let Ok(devices) = cmds.maic.get_paired_devices(&instance_id) {
            paired_device_count = devices.len();
        }
        // Pull drop_folder from a single PUT (idempotent; just to read
        // current state). Cheaper: do a dedicated GET endpoint in MAIC
        // v2 if this becomes a hot path.
        if let Ok(list) = cmds.maic.get_paired_devices(&instance_id) {
            let _ = list; // currently paired_devices returns array; drop_folder is on the desktop record
        }
        // Note: v1 doesn't expose GET /desktops/{id}, so we infer drop_folder
        // is empty unless the JS side already loaded it. The Settings page
        // does its own bootstrap from a prior register_desktop response.
    }
    Ok(PairingStatus {
        registered,
        instance_id,
        active_session_count,
        paired_device_count,
        drop_folder,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────

const DEFAULT_SESSION_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;

/// New session_id as a hex string (32 chars from 16 random bytes).
/// The wire format is identical to what the Rust compat test fixtures use.
pub fn new_session_id_string() -> String {
    let bytes = new_session_id();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Convert a hex session_id string back into the 16-byte array for HKDF.
pub fn session_id_raw_from_string(s: &str) -> [u8; 16] {
    let bytes = hex_decode(s);
    let mut out = [0u8; 16];
    let len = bytes.len().min(16);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

fn hex_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let (Some(a), Some(b)) = (
            (bytes[i] as char).to_digit(16),
            (bytes[i + 1] as char).to_digit(16),
        ) {
            out.push(((a << 4) | b) as u8);
        }
        i += 2;
    }
    out
}

/// Compute a stable fingerprint for a public key (hex of SHA256[0..8]).
/// Used for human display and as the `phone_pubkey_fp` field sent to MAIC.
pub fn compute_fingerprint(pubkey_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(pubkey_bytes);
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Miracle Claw Desktop".to_string())
}

// ─────────────────────────────────────────────────────────────────────
// Re-export PairingState types for lib.rs to wire up
// ─────────────────────────────────────────────────────────────────────

pub use crate::pairing_state::SessionSnapshot as _ReexportSessionSnapshot;

// ─────────────────────────────────────────────────────────────────────
// Tests — see tests/pairing_commands_test.rs for the FakeMaicHttp-based
// integration tests. The functions themselves are pure orchestration.
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_id_string_is_32_hex_chars() {
        let id = new_session_id_string();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn session_id_string_roundtrip() {
        let id = new_session_id_string();
        let raw = session_id_raw_from_string(&id);
        assert_eq!(raw.len(), 16);
        let id2: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(id, id2);
    }

    #[test]
    fn compute_fingerprint_is_16_hex_chars() {
        let pubkey = vec![0xABu8; 32];
        let fp = compute_fingerprint(&pubkey);
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_fingerprint_changes_with_input() {
        let a = compute_fingerprint(&[0xAA; 32]);
        let b = compute_fingerprint(&[0xBB; 32]);
        assert_ne!(a, b);
    }
}
