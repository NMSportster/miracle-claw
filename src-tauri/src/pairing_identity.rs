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
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

/// Filename under `app_data_dir()/pairing/`.
const IDENTITY_FILENAME: &str = "identity.json";

/// On-disk identity (JSON-serialized).
///
/// Phase 2.4.1 (Homebot 2026-09-10): `instance_id` switched from
/// `inst_<16 hex>` to a proper UUID v4 because MAIC's
/// `/v1/users/me/desktops/{instance_id}` path parameter is typed
/// `UUID` and 422s with `uuid_parsing` on the old format. Backward
/// compat: `load()` detects the old format on disk and migrates
/// deterministically (8 random bytes -> UUID v4 with marker bits
/// set) so existing v0.0.3 users keep the same identity and don't
/// have to re-pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// Stable identifier for this MC install. Format: UUID v4
    /// (e.g. `550e8400-e29b-41d4-a716-446655440000`).
    pub instance_id: Uuid,
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

/// Generate a fresh UUID v4 instance id using OS rng.
///
/// Phase 2.4.1 (Homebot 2026-09-10): switched from `inst_<16 hex>`
/// to UUID v4 because MAIC's `instance_id` path param is typed
/// `UUID`. The old format would 422 with `uuid_parsing`.
pub fn generate_instance_id() -> Uuid {
    Uuid::new_v4()
}

/// Convert a v0.0.3-era `inst_<16 hex>` instance id into a UUID v4
/// deterministically. The 8 hex bytes were already OS-rng random;
/// we just add the version (4) and variant (10xx) marker bits so
/// it parses as a valid UUID v4. This means existing v0.0.3 users
/// keep the same `instance_id` across the upgrade and don't have
/// to re-pair.
///
/// Returns `None` if the input doesn't match the v0.0.3 format.
pub fn migrate_inst_hex_to_uuid(inst_hex: &str) -> Option<Uuid> {
    let hex = inst_hex.strip_prefix("inst_")?;
    if hex.len() != 16 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let bytes_vec = hex_decode(hex)?;
    let mut bytes = [0u8; 16];
    // Fill the first 8 bytes with the v0.0.3 random bytes; the rest
    // are deterministic padding so the same `inst_<...>` always maps
    // to the same UUID. (Padding the lower 8 bytes with zeros would
    // also be deterministic; we use a constant so that two different
    // v0.0.3 installs with similar high bits are unlikely to collide.)
    bytes[..8].copy_from_slice(&bytes_vec);
    bytes[8..].copy_from_slice(&bytes_vec);
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // variant 10xx
    Some(Uuid::from_bytes(bytes))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
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
///
/// Phase 2.4.1 (Homebot 2026-09-10): the on-disk format changed from
/// `instance_id: String` (in `inst_<16 hex>` form) to `instance_id: Uuid`.
/// To avoid breaking existing v0.0.3 installs, we accept the old shape
/// as a deserialization error and then re-read the raw JSON, lift out
/// the string, and migrate it. If migration fails (e.g. file is from a
/// future version with an unknown format), we return the original error.
pub fn load(app_data_dir: &Path) -> std::io::Result<Option<Identity>> {
    let path = identity_path(app_data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    match serde_json::from_str::<Identity>(&raw) {
        Ok(id) => Ok(Some(id)),
        Err(parse_err) => {
            // Try the v0.0.3 shape: { "instance_id": "inst_<16 hex>", ... }
            #[derive(Deserialize)]
            struct LegacyIdentity {
                instance_id: String,
                desktop_pubkey_b64: String,
                desktop_secret_key: Vec<u8>,
                fingerprint: String,
                created_at_unix: u64,
            }
            match serde_json::from_str::<LegacyIdentity>(&raw) {
                Ok(legacy) => {
                    let uuid = migrate_inst_hex_to_uuid(&legacy.instance_id)
                        .ok_or_else(|| std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "instance_id '{}' is neither a UUID nor a v0.0.3 \
                                 inst_<16 hex>: {}",
                                legacy.instance_id, parse_err
                            ),
                        ))?;
                    // Re-serialize in the new shape and rewrite the file.
                    let migrated = Identity {
                        instance_id: uuid,
                        desktop_pubkey_b64: legacy.desktop_pubkey_b64,
                        desktop_secret_key: legacy.desktop_secret_key,
                        fingerprint: legacy.fingerprint,
                        created_at_unix: legacy.created_at_unix,
                    };
                    write_identity_to(&path, &migrated)?;
                    Ok(Some(migrated))
                }
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData, parse_err,
                )),
            }
        }
    }
}

/// Persist an identity to disk with 0600 perms (best-effort on Windows).
pub fn save(app_data_dir: &Path, identity: &Identity) -> std::io::Result<()> {
    let dir = app_data_dir.join("pairing");
    fs::create_dir_all(&dir)?;
    let path = identity_path(app_data_dir);
    write_identity_to(&path, identity)
}

/// Load an identity from a caller-specified path. Returns `Ok(None)` if
/// the file does not exist. Returns `Err(InvalidData)` if the file is
/// present but unparseable.
///
/// Added for `miracle-claw-pair --identity-file <path>` (Week 2.5).
/// The XDG-default `load()` / `save()` above still handle the GUI
/// path; this is the custom-path variant for the headless CLI.
pub fn load_from(path: &Path) -> std::io::Result<Option<Identity>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    match serde_json::from_str::<Identity>(&raw) {
        Ok(id) => Ok(Some(id)),
        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    }
}

/// Persist an identity to a caller-specified path. Creates parent
/// directories if missing, sets 0600 perms on Unix (best-effort on
/// Windows — inherits the user's profile ACL).
///
/// Added for `miracle-claw-pair --identity-file <path>` (Week 2.5).
pub fn save_to(path: &Path, identity: &Identity) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    write_identity_to(path, identity)
}

/// Shared writer used by both `save` (XDG default) and `save_to`
/// (custom path). Format and perms are identical — single source of
/// truth for on-disk shape.
fn write_identity_to(path: &Path, identity: &Identity) -> std::io::Result<()> {
    let serialized = serde_json::to_string_pretty(identity)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(path, serialized)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
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
    fn instance_id_is_uuid_v4() {
        let id = generate_instance_id();
        // Uuid::new_v4() guarantees version 4 and variant 10xx.
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    }

    #[test]
    fn migrate_inst_hex_to_uuid_round_trips() {
        // A v0.0.3-format string: inst_<16 hex>
        let legacy = "inst_a1b2c3d4e5f60718";
        let uuid = migrate_inst_hex_to_uuid(legacy).expect("should migrate");
        // Same input always maps to same UUID (deterministic).
        let uuid2 = migrate_inst_hex_to_uuid(legacy).expect("should migrate");
        assert_eq!(uuid, uuid2);
        // Output is a valid v4 UUID.
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        // The high 8 bytes are preserved (we copy them in).
        // The low 4 nibbles of byte 6 = 4 (version).
        // The high 2 bits of byte 8 = 10 (variant).
        // Don't assert exact byte values; the inputs were random hex.
    }

    #[test]
    fn migrate_inst_hex_to_uuid_rejects_bad_input() {
        assert!(migrate_inst_hex_to_uuid("not-an-inst-id").is_none());
        assert!(migrate_inst_hex_to_uuid("inst_short").is_none());
        assert!(migrate_inst_hex_to_uuid("inst_zzz_not_hex_xxxxx").is_none());
        assert!(migrate_inst_hex_to_uuid("").is_none());
    }

    #[test]
    fn load_migrates_v0_0_3_legacy_identity() {
        let tmp = tempdir_in_tests();
        // Write a v0.0.3-format identity.json to disk by hand.
        let legacy = serde_json::json!({
            "instance_id": "inst_a1b2c3d4e5f60718",
            "desktop_pubkey_b64": "MCowBQYDK2VwAyEARXhhcm0hZWZmamtsbW5vcDpzdHV2d3h5eno7",
            "desktop_secret_key": [1u8, 2, 3, 4, 5, 6, 7, 8],
            "fingerprint": "abcd-1234-ef56",
            "created_at_unix": 1700000000u64,
        });
        let path = identity_path(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        // Load should detect legacy shape and migrate.
        let loaded = load(&tmp).expect("load should not error")
            .expect("identity should exist after migration");
        // The migrated UUID should match the deterministic migration.
        let expected = migrate_inst_hex_to_uuid("inst_a1b2c3d4e5f60718").unwrap();
        assert_eq!(loaded.instance_id, expected);
        // The other fields are preserved unchanged.
        assert_eq!(loaded.desktop_pubkey_b64, "MCowBQYDK2VwAyEARXhhcm0hZWZmamtsbW5vcDpzdHV2d3h5eno7");
        assert_eq!(loaded.desktop_secret_key, vec![1u8, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(loaded.fingerprint, "abcd-1234-ef56");
        assert_eq!(loaded.created_at_unix, 1700000000u64);

        // After migration, the file on disk should be in the new shape.
        let raw = fs::read_to_string(&path).unwrap();
        let again: Identity = serde_json::from_str(&raw).expect("post-migration should parse as new shape");
        assert_eq!(again.instance_id, expected);
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

    // ── Tests for load_from / save_to (Week 2.5 miracle-claw-pair) ──

    #[test]
    fn load_from_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.json");
        let r = load_from(&path).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn save_to_then_load_from_round_trips_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("custom-identity.json");
        // Build a deterministic identity via the public API.
        let saved = generate(dir.path()).expect("generate identity");
        save_to(&path, &saved).expect("save_to custom path");

        let loaded = load_from(&path).unwrap().expect("file should exist");
        assert_eq!(loaded.instance_id, saved.instance_id);
        assert_eq!(loaded.desktop_pubkey_b64, saved.desktop_pubkey_b64);
        assert_eq!(loaded.desktop_secret_key, saved.desktop_secret_key);
        assert_eq!(loaded.fingerprint, saved.fingerprint);
        assert_eq!(loaded.created_at_unix, saved.created_at_unix);
    }

    #[test]
    fn save_to_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b").join("c").join("id.json");
        let id = generate(dir.path()).expect("generate");
        save_to(&nested, &id).expect("save_to nested path");
        assert!(nested.exists());
        // Round-trip reads back what we wrote.
        let loaded = load_from(&nested).unwrap().expect("file should exist");
        assert_eq!(loaded.instance_id, id.instance_id);
    }

    #[test]
    fn save_to_sets_unix_0600_perms() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("perm.json");
            let id = generate(dir.path()).expect("generate");
            save_to(&path, &id).expect("save_to");
            let perms = fs::metadata(&path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600,
                "custom-path save should set 0600 like the XDG default");
        }
        // On non-Unix the perms test is a no-op (Windows uses profile ACL).
    }
}
