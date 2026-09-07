//! pairing_state.rs — In-memory pairing session map (Phase 2.2).
//!
//! Spec: docs/specs/mobile-desktop-pairing.md § "Session secret handling".
//!
//! The session_secret (32 bytes) is the symmetric key the phone uses to
//! authenticate per-request HMACs after the handshake. **It MUST live in
//! process memory only** — never on disk, never logged, never sent over the
//! wire after the handshake completes. The on-disk record (paired_sessions.json)
//! holds only metadata for audit + UI display.
//!
//! Invariants:
//!   - One session per phone (unique session_id)
//!   - session_secret is zeroized when the session is revoked or expires
//!   - TTL default = 30 days, configurable per-session
//!   - Cleanup task runs every 5 minutes to evict expired sessions
//!
//! On crash, all sessions are lost — phones must re-pair. This is the
//! intended behavior: a session_secret should never persist beyond the
//! process that minted it.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Default TTL for a pairing session. 30 days matches typical "remember me"
/// patterns; long enough that users don't re-pair constantly, short enough
/// that a forgotten phone has a hard cap on how long it can stay connected.
pub const DEFAULT_SESSION_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Hard cap to prevent accidental "forever" sessions.
pub const MAX_SESSION_TTL_SECONDS: u64 = 365 * 24 * 60 * 60;

/// Capabilities the desktop can grant. These MUST match the locked capability
/// scope in the spec — see "Locked capability scope" section.
///
/// v1 (this implementation) is the v1 trust model: any session with the
/// right scope passes. v2 would add per-desktop client_credentials and a
/// per-session scope certificate signed by the desktop's long-lived key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Send messages to the active chat session.
    Chat,
    /// Read + write the user's session list (start/resume/rename/delete).
    #[serde(rename = "sessions:rw")]
    SessionsRw,
    /// List available modules.
    #[serde(rename = "modules:list")]
    ModulesList,
    /// Invoke a module (only the drop-folder-restricted ones from desktop owner).
    #[serde(rename = "modules:invoke")]
    ModulesInvoke,
    /// Use the desktop's voice passthrough (mic capture + speaker playback
    /// over the active gateway).
    #[serde(rename = "voice:passthrough")]
    VoicePassthrough,
    /// Read files inside the desktop owner's chosen drop-folder only.
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    /// Write/delete files inside the drop-folder only.
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
}

impl Capability {
    /// The locked v1 scope — every granted session gets exactly these.
    /// `filesystem:read` + `filesystem:write` are sandboxed to the drop-folder
    /// at the gateway (see `api/routes/pairing.py::filesystem_proxy`).
    pub fn locked_v1_scope() -> Vec<Capability> {
        vec![
            Capability::Chat,
            Capability::SessionsRw,
            Capability::ModulesList,
            Capability::ModulesInvoke,
            Capability::VoicePassthrough,
            Capability::FilesystemRead,
            Capability::FilesystemWrite,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Chat => "chat",
            Capability::SessionsRw => "sessions:rw",
            Capability::ModulesList => "modules:list",
            Capability::ModulesInvoke => "modules:invoke",
            Capability::VoicePassthrough => "voice:passthrough",
            Capability::FilesystemRead => "filesystem:read",
            Capability::FilesystemWrite => "filesystem:write",
        }
    }
}

/// What we know about an active pairing session.
///
/// The `session_secret` field is `Zeroize`-able so we can wipe it before
/// dropping the entry — see `Self::revoke`.
#[derive(Debug)]
pub struct SessionEntry {
    pub session_id: String,
    pub session_secret: [u8; 32],
    pub instance_id: String,
    pub device_id: Option<i64>,
    pub device_name: String,
    pub capabilities: Vec<Capability>,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
    pub last_used_at_unix: u64,
}

impl SessionEntry {
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix >= self.expires_at_unix
    }
}

impl Drop for SessionEntry {
    fn drop(&mut self) {
        // Zeroize the session_secret before the entry is freed. This means a
        // revoked/expired session leaves no plaintext residue in heap memory.
        self.session_secret.zeroize();
    }
}

/// The full pairing state, managed as Tauri state.
///
/// Tauri requires `Send + Sync` for managed state, so we wrap the HashMap
/// in a `Mutex`. Reads (lookup) are far more common than writes (mint/revoke),
/// so contention should be minimal. If profiling shows contention, switch
/// to `RwLock` or `dashmap`.
#[derive(Debug, Default)]
pub struct PairingState {
    sessions: Mutex<HashMap<String, SessionEntry>>,
}

impl PairingState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a new session. The caller is responsible for encrypting
    /// `session_secret` into the handshake response and storing
    /// `session_id → SessionEntry` via this function.
    ///
    /// `ttl_seconds` is clamped to `[1, MAX_SESSION_TTL_SECONDS]`.
    pub fn mint(
        &self,
        session_id: String,
        session_secret: [u8; 32],
        instance_id: String,
        device_id: Option<i64>,
        device_name: String,
        capabilities: Vec<Capability>,
        ttl_seconds: u64,
        now_unix: u64,
    ) -> Result<MintResult, PairingStateError> {
        if session_id.is_empty() {
            return Err(PairingStateError::EmptySessionId);
        }
        if session_secret.len() != 32 {
            return Err(PairingStateError::InvalidSecretLength);
        }
        let ttl = ttl_seconds.clamp(1, MAX_SESSION_TTL_SECONDS);

        let entry = SessionEntry {
            session_id: session_id.clone(),
            session_secret,
            instance_id,
            device_id,
            device_name,
            capabilities,
            created_at_unix: now_unix,
            expires_at_unix: now_unix.saturating_add(ttl),
            last_used_at_unix: now_unix,
        };

        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| PairingStateError::LockPoisoned)?;

        // Idempotency: if the session_id is already present (re-mint), the
        // existing session_secret is dropped (zeroized via Drop impl) before
        // we overwrite. This is intentional — caller should generate a new
        // session_id for each handshake. If they reuse, it's their bug and we
        // don't double-allocate memory.
        guard.insert(session_id.clone(), entry);

        let expires_at_unix = guard.get(&session_id).unwrap().expires_at_unix;
        Ok(MintResult {
            session_id,
            expires_at_unix,
        })
    }

    /// Look up a session by ID. Returns `None` if it doesn't exist OR is
    /// expired. Expired entries are removed as a side effect (cheap GC).
    pub fn lookup(&self, session_id: &str, now_unix: u64) -> Option<SessionSnapshot> {
        let mut guard = self.sessions.lock().ok()?;
        let entry = guard.get_mut(session_id)?;

        if entry.is_expired(now_unix) {
            // Drop impl zeroizes the secret.
            guard.remove(session_id);
            return None;
        }

        // Update last_used_at for the per-request liveness tracking. The
        // phone's HMAC is the actual auth check; this is just observability.
        entry.last_used_at_unix = now_unix;

        Some(SessionSnapshot {
            session_id: entry.session_id.clone(),
            instance_id: entry.instance_id.clone(),
            device_id: entry.device_id,
            device_name: entry.device_name.clone(),
            capabilities: entry.capabilities.clone(),
            expires_at_unix: entry.expires_at_unix,
        })
    }

    /// Revoke a session by ID. Idempotent — revoking an unknown ID is a no-op.
    /// Returns true if a session was actually removed.
    pub fn revoke(&self, session_id: &str) -> bool {
        let Ok(mut guard) = self.sessions.lock() else {
            return false;
        };
        // Drop impl zeroizes the secret.
        guard.remove(session_id).is_some()
    }

    /// Revoke all sessions belonging to a specific paired-device. Used when
    /// the desktop owner clicks "revoke phone" — we revoke both the MAIC-side
    /// record AND all in-memory sessions for that device, so the phone can't
    /// keep sending HMACs.
    pub fn revoke_device(&self, device_id: i64) -> usize {
        let Ok(mut guard) = self.sessions.lock() else {
            return 0;
        };
        let before = guard.len();
        guard.retain(|_, entry| entry.device_id != Some(device_id));
        before - guard.len()
    }

    /// Revoke all sessions for a given desktop instance. Used on graceful
    /// shutdown or when the desktop owner's MAIC auth expires.
    pub fn revoke_instance(&self, instance_id: &str) -> usize {
        let Ok(mut guard) = self.sessions.lock() else {
            return 0;
        };
        let before = guard.len();
        guard.retain(|_, entry| entry.instance_id != instance_id);
        before - guard.len()
    }

    /// GC pass — remove all expired sessions. Called every 5 minutes by the
    /// cleanup task spawned at app startup.
    pub fn cleanup_expired(&self, now_unix: u64) -> usize {
        let Ok(mut guard) = self.sessions.lock() else {
            return 0;
        };
        let before = guard.len();
        guard.retain(|_, entry| !entry.is_expired(now_unix));
        before - guard.len()
    }

    /// List all current (non-expired) sessions. Used by the Settings page.
    pub fn list_active(&self, now_unix: u64) -> Vec<SessionSnapshot> {
        let Ok(mut guard) = self.sessions.lock() else {
            return vec![];
        };
        guard
            .values_mut()
            .filter(|e| !e.is_expired(now_unix))
            .map(|e| {
                e.last_used_at_unix = now_unix;
                SessionSnapshot {
                    session_id: e.session_id.clone(),
                    instance_id: e.instance_id.clone(),
                    device_id: e.device_id,
                    device_name: e.device_name.clone(),
                    capabilities: e.capabilities.clone(),
                    expires_at_unix: e.expires_at_unix,
                }
            })
            .collect()
    }

    /// Count of active (non-expired) sessions. Cheap.
    pub fn active_count(&self, now_unix: u64) -> usize {
        self.list_active(now_unix).len()
    }
}

/// Returned by `mint()` — caller uses these to build the handshake response.
#[derive(Debug, Clone)]
pub struct MintResult {
    pub session_id: String,
    pub expires_at_unix: u64,
}

/// Returned by `lookup()` — the caller can verify capabilities + identity
/// without holding a reference to the actual `session_secret`. The secret
/// stays inside the `PairingState` (zeroized on drop).
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub instance_id: String,
    pub device_id: Option<i64>,
    pub device_name: String,
    pub capabilities: Vec<Capability>,
    pub expires_at_unix: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingStateError {
    #[error("session_id cannot be empty")]
    EmptySessionId,
    #[error("session_secret must be exactly 32 bytes")]
    InvalidSecretLength,
    #[error("pairing state mutex was poisoned by a panic; restart required")]
    LockPoisoned,
}

/// Current Unix timestamp (seconds). Wrapped so tests can swap it.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_secret(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    #[test]
    fn mint_then_lookup_roundtrip() {
        let s = PairingState::new();
        s.mint(
            "sess-1".into(),
            fake_secret(0xAB),
            "inst-1".into(),
            Some(42),
            "Pixel 8".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();

        let snap = s.lookup("sess-1", 1_700_000_000 + 10).unwrap();
        assert_eq!(snap.session_id, "sess-1");
        assert_eq!(snap.instance_id, "inst-1");
        assert_eq!(snap.device_id, Some(42));
        assert_eq!(snap.device_name, "Pixel 8");
        assert_eq!(snap.capabilities.len(), 7); // v1 scope
        assert_eq!(snap.expires_at_unix, 1_700_003_600);
    }

    #[test]
    fn lookup_returns_none_for_expired() {
        let s = PairingState::new();
        s.mint(
            "sess-1".into(),
            fake_secret(0),
            "inst-1".into(),
            None,
            "d".into(),
            Capability::locked_v1_scope(),
            60,
            1_700_000_000,
        )
        .unwrap();
        assert!(s.lookup("sess-1", 1_700_000_100).is_none());
    }

    #[test]
    fn revoke_removes_session() {
        let s = PairingState::new();
        s.mint(
            "sess-1".into(),
            fake_secret(0),
            "inst-1".into(),
            None,
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();
        assert!(s.revoke("sess-1"));
        assert!(!s.revoke("sess-1"), "second revoke is no-op");
        assert!(s.lookup("sess-1", 1_700_000_001).is_none());
    }

    #[test]
    fn revoke_device_removes_all_sessions_for_device() {
        let s = PairingState::new();
        // Two sessions for device 42, one for device 99
        s.mint(
            "s1".into(),
            fake_secret(0),
            "inst-1".into(),
            Some(42),
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();
        s.mint(
            "s2".into(),
            fake_secret(0),
            "inst-1".into(),
            Some(42),
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();
        s.mint(
            "s3".into(),
            fake_secret(0),
            "inst-1".into(),
            Some(99),
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();

        let removed = s.revoke_device(42);
        assert_eq!(removed, 2);
        assert!(s.lookup("s1", 1_700_000_001).is_none());
        assert!(s.lookup("s2", 1_700_000_001).is_none());
        assert!(s.lookup("s3", 1_700_000_001).is_some(), "device 99 must survive");
    }

    #[test]
    fn revoke_instance_removes_all_sessions_for_desktop() {
        let s = PairingState::new();
        for i in 0..3 {
            s.mint(
                format!("s{}", i),
                fake_secret(0),
                "inst-A".into(),
                Some(i as i64),
                "d".into(),
                Capability::locked_v1_scope(),
                3600,
                1_700_000_000,
            )
            .unwrap();
        }
        s.mint(
            "sB".into(),
            fake_secret(0),
            "inst-B".into(),
            Some(99),
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();

        let removed = s.revoke_instance("inst-A");
        assert_eq!(removed, 3);
        assert_eq!(s.active_count(1_700_000_001), 1);
    }

    #[test]
    fn cleanup_expired_removes_only_expired() {
        let s = PairingState::new();
        s.mint(
            "old".into(),
            fake_secret(0),
            "inst-1".into(),
            None,
            "d".into(),
            Capability::locked_v1_scope(),
            60,
            1_700_000_000,
        )
        .unwrap();
        s.mint(
            "new".into(),
            fake_secret(0),
            "inst-1".into(),
            None,
            "d".into(),
            Capability::locked_v1_scope(),
            3_600,
            1_700_000_000,
        )
        .unwrap();

        let removed = s.cleanup_expired(1_700_000_100);
        assert_eq!(removed, 1);
        assert_eq!(s.active_count(1_700_000_100), 1);
    }

    #[test]
    fn ttl_is_clamped_to_max() {
        let s = PairingState::new();
        let r = s
            .mint(
                "s".into(),
                fake_secret(0),
                "i".into(),
                None,
                "d".into(),
                Capability::locked_v1_scope(),
                u64::MAX,
                1_700_000_000,
            )
            .unwrap();
        let max_futures = 1_700_000_000 + MAX_SESSION_TTL_SECONDS;
        assert_eq!(r.expires_at_unix, max_futures);
    }

    #[test]
    fn ttl_is_clamped_to_min() {
        let s = PairingState::new();
        let r = s
            .mint(
                "s".into(),
                fake_secret(0),
                "i".into(),
                None,
                "d".into(),
                Capability::locked_v1_scope(),
                0,
                1_700_000_000,
            )
            .unwrap();
        // ttl=0 → clamped to 1 second
        assert_eq!(r.expires_at_unix, 1_700_000_001);
    }

    #[test]
    fn mint_overwrites_with_zeroize() {
        // If a session_id is re-minted, the old session_secret must be
        // zeroized before drop. We can't observe zeroize directly, but we
        // can verify the operation succeeds and returns the new entry.
        let s = PairingState::new();
        s.mint(
            "s".into(),
            fake_secret(0xAA),
            "i".into(),
            None,
            "d".into(),
            Capability::locked_v1_scope(),
            3600,
            1_700_000_000,
        )
        .unwrap();
        let r = s
            .mint(
                "s".into(),
                fake_secret(0xBB),
                "i".into(),
                None,
                "d".into(),
                Capability::locked_v1_scope(),
                3600,
                1_700_000_001,
            )
            .unwrap();
        assert_eq!(r.session_id, "s");
        assert!(s.lookup("s", 1_700_000_002).is_some());
    }

    #[test]
    fn empty_session_id_rejected() {
        let s = PairingState::new();
        let err = s
            .mint(
                "".into(),
                fake_secret(0),
                "i".into(),
                None,
                "d".into(),
                Capability::locked_v1_scope(),
                3600,
                1_700_000_000,
            )
            .unwrap_err();
        assert!(matches!(err, PairingStateError::EmptySessionId));
    }

    #[test]
    fn capability_strings_are_stable() {
        // Locked capability strings — DO NOT change without coordinating
        // with the Dart side (pairing_compat_test.dart) and the MAIC
        // /v1/users/me/desktops/{id}/paired-devices response format.
        assert_eq!(Capability::Chat.as_str(), "chat");
        assert_eq!(Capability::SessionsRw.as_str(), "sessions:rw");
        assert_eq!(Capability::ModulesList.as_str(), "modules:list");
        assert_eq!(Capability::ModulesInvoke.as_str(), "modules:invoke");
        assert_eq!(Capability::VoicePassthrough.as_str(), "voice:passthrough");
        assert_eq!(Capability::FilesystemRead.as_str(), "filesystem:read");
        assert_eq!(Capability::FilesystemWrite.as_str(), "filesystem:write");
    }

    #[test]
    fn v1_scope_has_seven_capabilities() {
        // Pin the scope size so a future addition is a deliberate choice.
        // To add: bump a constant, update MEMORY, update Dart, update spec.
        assert_eq!(Capability::locked_v1_scope().len(), 7);
    }
}
