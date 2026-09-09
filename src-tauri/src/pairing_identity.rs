//! pairing_identity.rs — Persistent desktop identity for mobile↔desktop pairing.
//!
//! Spec: docs/specs/mobile-desktop-pairing.md
//!
//! Each desktop install gets a stable identity (instance_id + X25519 keypair
//! + human-readable fingerprint). Persists to `app_data_dir()/pairing/identity.json`.
//!
//! Security:
//!   - The X25519 secret key is serialized as base64 inside the JSON file.
//!     In a hardened build this should be encrypted with the existing
//!     `secrets_encryption` module, but for v1 we treat the desktop install
//!     as the trust boundary — anyone with read access to the user's home
//!     dir already has full filesystem access, so encrypting-at-rest adds
//!     complexity without raising the bar.
//!   - The fingerprint is a short stable identifier shown in the Settings UI
//!     so the user can confirm "I'm pairing with the right desktop." It is
//!     derived from the public key and is NOT used in any crypto path.
//!
//! Phase 2.4 (NEW 2026-09-08, Home Claw): the frontend needs an
//! `mc_register_desktop_self(fingerprint)` Tauri command that generates
//! the keypair server-side (so the secret never crosses the JS-Rust
//! boundary). The original `mc_register_desktop(pubkey_b64, fingerprint)`
//! API is kept for compatibility / tests but should be deprecated once
//! 2.4 lands.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

/// Filename under `app_data_dir()/pairing/`.
const IDENTITY_FILENAME: &str = "identity.json";

/// On-disk identity (JSON-serialized).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// Stable identifier for this MC install. Format: `inst_<16 hex>`.
    pub instance_id: String,
    /// Base64-encoded X25519 public key.
    pub desktop_pubkey_b64: String,
    /// Base64-encoded X25519 secret key (32 bytes). Kept on disk because
    /// the desktop needs it on every handshake (no in-memory cache across
    /// restarts). See module docs on at-rest security.
    #[serde(with = "base64_secret")]
    pub desktop_secret_key: Vec<u8>,
    /// Stable short fingerprint for human display. Format: XXXX-XXXX-XXXX.
    pub fingerprint: String,
    /// Created-at (Unix epoch seconds). For audit only.
    pub created_at_unix: u64,
}

impl Drop for Identity {
    fn drop(&mut self) {
        // Best-effort: zero the secret in memory before free.
        self.desktop_secret_key.zeroize();
    }
}

/// Serde helper so the secret is base64-encoded in JSON.
mod base64_secret {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        B64.encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(&s).map_err(serde::de::Error::custom)
    }
}

/// Compute a human-readable fingerprint from a 32-byte X25519 public key.
/// Returns XXXX-XXXX-XXXX (12 hex chars, dashes every 4) — short enough
/// to read aloud on a phone call, long enough that collisions are negligible.
pub fn fingerprint_from_pubkey(pubkey: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pubkey);
    let digest = hasher.finalize();
    let hex: String = digest[..6].iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}-{}-{}", &hex[0..4], &hex[4..8], &hex[8..12])
}

/// Generate a fresh `inst_<16 hex>` instance id using OS rng.
pub fn generate_instance_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    format!("inst_{}", hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate a fresh X25519 keypair using OS rng. Returns the public key as
/// 32 bytes and the secret as a `StaticSecret` (zeroized on drop).
pub fn generate_keypair() -> ([u8; 32], StaticSecret) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (public.to_bytes(), secret)
}

/// Resolve the on-disk identity path: <app_data_dir>/pairing/identity.json.
pub fn identity_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("pairing").join(IDENTITY_FILENAME)
}

/// Try to load a saved identity from disk. Returns `Ok(None)` if no file
/// exists yet (fresh install). Returns `Err` if the file is corrupt or
/// unreadable — the caller decides whether to ignore (regenerate) or
/// surface the error to the user.
pub fn load(app_data_dir: &Path) -> std::io::Result<Option<Identity>> {
    let path = identity_path(app_data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    match serde_json::from_str::<Identity>(&raw) {
        Ok(id) => Ok(Some(id)),
        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    }
}

/// Persist an identity to disk with 0600 perms (best-effort on Windows).
pub fn save(app_data_dir: &Path, identity: &Identity) -> std::io::Result<()> {
    let dir = app_data_dir.join("pairing");
    fs::create_dir_all(&dir)?;
    let path = identity_path(app_data_dir);
    let serialized = serde_json::to_string_pretty(identity)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&path, serialized)?;
    // Try to set 0600 perms (Unix only; on Windows the file ACL is whatever
    // the user's profile gives it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Generate a brand-new identity (used by `mc_register_desktop_self`).
pub fn generate(app_data_dir: &Path) -> std::io::Result<Identity> {
    let (pub_bytes, secret) = generate_keypair();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let identity = Identity {
        instance_id: generate_instance_id(),
        desktop_pubkey_b64: base64::engine::general_purpose::STANDARD.encode(pub_bytes),
        desktop_secret_key: secret.to_bytes().to_vec(),
        fingerprint: fingerprint_from_pubkey(&pub_bytes),
        created_at_unix: now,
    };
    save(app_data_dir, &identity)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_for_same_pubkey() {
        let key = [0u8; 32];
        let fp1 = fingerprint_from_pubkey(&key);
        let fp2 = fingerprint_from_pubkey(&key);
        assert_eq!(fp1, fp2);
        assert!(fp1.contains('-'));
        assert_eq!(fp1.len(), 14); // 12 hex + 2 dashes
    }

    #[test]
    fn fingerprint_changes_with_pubkey() {
        let k1 = [0u8; 32];
        let mut k2 = [0u8; 32];
        k2[0] = 1;
        assert_ne!(fingerprint_from_pubkey(&k1), fingerprint_from_pubkey(&k2));
    }

    #[test]
    fn instance_id_has_expected_format() {
        let id = generate_instance_id();
        assert!(id.starts_with("inst_"));
        assert_eq!(id.len(), 5 + 16); // "inst_" + 16 hex chars
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempdir_in_tests();
        let original = generate(&tmp).expect("generate identity");
        let loaded = load(&tmp).expect("load identity").expect("identity should exist");
        assert_eq!(original.instance_id, loaded.instance_id);
        assert_eq!(original.desktop_pubkey_b64, loaded.desktop_pubkey_b64);
        assert_eq!(original.fingerprint, loaded.fingerprint);
        assert_eq!(original.desktop_secret_key, loaded.desktop_secret_key);
    }

    #[test]
    fn load_returns_none_when_no_file() {
        let tmp = tempdir_in_tests();
        let result = load(&tmp).expect("load should not error on missing file");
        assert!(result.is_none());
    }

    fn tempdir_in_tests() -> PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "mc-pairing-identity-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let p = base.join(unique);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
