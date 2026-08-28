//! auto_relogin — silent re-login on 401 using cached credentials
//!
//! Lesson 458 / v1.0.6: when the user checks "Stay signed in" on the login
//! form, we encrypt their email + password with a per-install AES-256-GCM
//! key and stash both the encrypted blob and the encryption key in the OS
//! keychain (Windows Credential Manager / macOS Keychain / Linux Secret
//! Service).
//!
//! When a chat request later returns 401, the OpenClaw child window calls
//! `silent_relogin` (Tauri command in lib.rs), which loads and decrypts the
//! cached creds and mints a fresh JWT. OpenClaw then retries the original
//! request once with the new token. If the user didn't check "Stay signed
//! in" — or if relogin fails — the original 401 propagates up to the
//! frontend so the login form can be shown again.
//!
//! ## Storage layout
//!
//! Two keychain entries, both namespaced under `com.milagrocloud.miracle-claw`:
//! - `cached-creds`        : JSON envelope containing `endpoint` + base64(
//!                            nonce (12 bytes) || ciphertext || tag (16) )
//! - `cached-creds-key`    : base64( 32 random bytes — the AES-256 key )
//!
//! We split the key from the ciphertext into separate keychain entries so
//! neither alone is enough to recover the secret. Both must be present and
//! decryptable for silent relogin to fire.
//!
//! ## Security properties
//!
//! - AES-256-GCM authenticated encryption. A wrong key or any ciphertext
//!   tamper makes `decrypt_blob` return `Err`, not silently produce garbage.
//! - Random 12-byte nonce per stash. Same plaintext + same key produces
//!   different ciphertexts each time.
//! - `Zeroize` on `CachedCreds::Drop` so decrypted secrets don't linger in
//!   process memory after the login round-trip.
//! - Keychain entries are scoped to the OS user account. Other OS users
//!   on the same machine cannot read them.
//!
//! ## What this is NOT
//!
//! - Not a substitute for proper MFA. The cached creds are the same
//!   email+password the user typed; if MAIC requires TOTP, relogin will
//!   fail with the same error as a fresh login.
//! - Not a backup. The keychain entries are tied to the OS user profile;
//!   moving to a new machine requires the user to log in again.
//! - Not a token. We don't cache the JWT (which expires per MAIC's 7-day
//!   sliding window); we cache the *credentials* so we can mint a new one.
//!
//! ## Testability
//!
//! The pure crypto helpers (`generate_key`, `generate_nonce`,
//! `encrypt_blob`, `decrypt_blob`) take a key as input and don't touch
//! the OS keychain. They're unit-testable without any platform backend.
//! The keychain integration (`stash_cached_credentials`,
//! `load_cached_credentials`, `clear_cached_credentials`) is exercised
//! end-to-end on David's machine before each release — there is no
//! keychain mock in v3.x of the `keyring` crate.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use keyring::Entry;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const SERVICE: &str = "com.milagrocloud.miracle-claw";
const KEY_USER: &str = "cached-creds-key";
const CREDS_USER: &str = "cached-creds";

/// Cached credentials for silent re-login.
///
/// `email` and `password` are `Zeroize`-on-drop so any heap copy is wiped
/// when this struct falls out of scope. We never log them, never include
/// them in error messages, and never serialize them to disk.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CachedCreds {
    pub email: String,
    pub password: String,
}

/// Serializable envelope stored in the keychain. We use a JSON envelope
/// (not raw bytes) so future fields (e.g. a TOTP secret, a token issuer
/// hint) can be added without breaking old entries.
#[derive(Serialize, Deserialize, Debug)]
struct CredsEnvelope {
    /// MAIC endpoint this credential was issued against. We check this on
    /// reload so a creds stash for `https://staging.maicserver.com` doesn't
    /// silently log into production.
    endpoint: String,
    /// Base64( nonce (12 bytes) || ciphertext || tag (16 bytes) )
    blob: String,
}

/// Errors that can come out of the auto_relogin module. We don't leak the
/// password back to the caller; messages describe the failure mode, not
/// the contents.
#[derive(Debug)]
pub enum AutoReloginError {
    /// No cached creds present in the keychain. The user didn't check
    /// "Stay signed in" on a previous login (or they explicitly cleared them).
    NoCachedCreds,
    /// The keychain entry exists but is corrupt or decrypts to garbage.
    /// This can happen if the user manually deleted one of the two entries,
    /// or if the OS keychain was restored from a different machine backup.
    KeychainCorrupt,
    /// The OS keychain itself returned an error (DBus not running, no
    /// credential manager service, etc.).
    KeychainUnavailable(String),
    /// AES-GCM decrypt failed. Wrong key, wrong nonce, or tampered
    /// ciphertext. We don't distinguish which one because the caller
    /// shouldn't act differently based on the specific failure.
    DecryptFailed,
    /// The stored endpoint doesn't match the endpoint we'd be logging into
    /// right now. This protects against accidental cross-environment
    /// reuse (e.g. a staging creds stash hitting production).
    EndpointMismatch { expected: String, cached: String },
    /// JSON envelope failed to parse. Should never happen in practice;
    /// indicates the entry was written by an incompatible future version.
    InvalidEnvelope(String),
}

impl std::fmt::Display for AutoReloginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoReloginError::NoCachedCreds => f.write_str("no cached credentials"),
            AutoReloginError::KeychainCorrupt => f.write_str("keychain entry corrupt"),
            AutoReloginError::KeychainUnavailable(s) => write!(f, "keychain unavailable: {}", s),
            AutoReloginError::DecryptFailed => f.write_str("cached credentials failed to decrypt"),
            AutoReloginError::EndpointMismatch { expected, cached } => write!(
                f,
                "cached credentials were issued for {}, not {}",
                cached, expected
            ),
            AutoReloginError::InvalidEnvelope(s) => write!(f, "invalid creds envelope: {}", s),
        }
    }
}

impl std::error::Error for AutoReloginError {}

// -------------------------------------------------------------------------
// Pure crypto helpers (testable without an OS keychain)
// -------------------------------------------------------------------------

/// Generate a fresh 32-byte AES-256 key.
pub fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Generate a fresh 12-byte AES-GCM nonce.
pub fn generate_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

/// Encrypt a plaintext under `key` with the given `nonce`, binding to `aad`
/// (associated authenticated data — used here as the endpoint string).
///
/// Returns `nonce || ciphertext || tag` as a single `Vec<u8>`.
pub fn encrypt_blob(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-Gcm key length is fixed at 32");
    let nonce_ref = Nonce::from_slice(nonce);
    let ciphertext = cipher
        .encrypt(
            nonce_ref,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("AES-GCM encrypt should not fail for valid inputs");
    let mut blob = Vec::with_capacity(nonce.len() + ciphertext.len());
    blob.extend_from_slice(nonce);
    blob.extend_from_slice(&ciphertext);
    blob
}

/// Inverse of `encrypt_blob`. Returns the plaintext on success, or an
/// error string describing the failure (wrong key, wrong AAD, tampered
/// ciphertext, or truncation).
pub fn decrypt_blob(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, &'static str> {
    if blob.len() < 12 + 16 {
        return Err("blob too short");
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-Gcm key length is fixed at 32");
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| "decrypt failed")
}

// -------------------------------------------------------------------------
// Keychain integration
// -------------------------------------------------------------------------

/// Get a keychain entry, mapping keyring errors into AutoReloginError.
fn entry(user: &str) -> Result<Entry, AutoReloginError> {
    Entry::new(SERVICE, user).map_err(|e| AutoReloginError::KeychainUnavailable(e.to_string()))
}

/// Stash the user's email + password in the keychain, encrypted with a
/// freshly-generated per-install AES-256 key.
///
/// If creds were already cached (from a prior "Stay signed in" login), they
/// are overwritten — last write wins. Idempotent and safe to call on every
/// login.
///
/// `endpoint` is recorded alongside the ciphertext so we can refuse to use
/// creds that were issued for a different MAIC instance.
pub fn stash_cached_credentials(
    email: &str,
    password: &str,
    endpoint: &str,
) -> Result<(), AutoReloginError> {
    let key = generate_key();
    let nonce = generate_nonce();
    let plaintext = format!("{}\0{}", email, password);
    let blob = encrypt_blob(&key, &nonce, endpoint.as_bytes(), plaintext.as_bytes());

    let envelope = CredsEnvelope {
        endpoint: endpoint.to_string(),
        blob: B64.encode(&blob),
    };
    let envelope_json = serde_json::to_string(&envelope)
        .map_err(|e| AutoReloginError::InvalidEnvelope(e.to_string()))?;

    // Write both keychain entries. If the second write fails, roll back
    // the first so we don't leave a half-stashed state.
    entry(KEY_USER)?
        .set_password(&B64.encode(&key))
        .map_err(|e| AutoReloginError::KeychainUnavailable(e.to_string()))?;
    if let Err(e) = entry(CREDS_USER)?.set_password(&envelope_json) {
        // Best-effort rollback. If this also fails, the user will see
        // DecryptFailed on the next reload — which is recoverable.
        let _ = entry(KEY_USER).and_then(|e| {
            e.delete_credential()
                .map_err(|ke| AutoReloginError::KeychainUnavailable(ke.to_string()))
        });
        return Err(AutoReloginError::KeychainUnavailable(e.to_string()));
    }

    eprintln!(
        "[miracle-claw] auto_relogin: stashed cached creds for endpoint={}",
        endpoint
    );
    Ok(())
}

/// Load and decrypt the cached creds. Returns `NoCachedCreds` if either
/// keychain entry is missing — that's the normal "user didn't check
/// Stay signed in" path, not an error condition.
///
/// `current_endpoint` is checked against the stashed endpoint. A mismatch
/// returns `EndpointMismatch` so the caller can prompt the user to log in
/// again rather than silently logging them into the wrong server.
pub fn load_cached_credentials(
    current_endpoint: &str,
) -> Result<CachedCreds, AutoReloginError> {
    let key_b64 = match entry(KEY_USER)?.get_password() {
        Ok(s) => s,
        Err(keyring::Error::NoEntry) => return Err(AutoReloginError::NoCachedCreds),
        Err(e) => return Err(AutoReloginError::KeychainUnavailable(e.to_string())),
    };
    let envelope_json = match entry(CREDS_USER)?.get_password() {
        Ok(s) => s,
        Err(keyring::Error::NoEntry) => return Err(AutoReloginError::NoCachedCreds),
        Err(e) => return Err(AutoReloginError::KeychainUnavailable(e.to_string())),
    };

    let envelope: CredsEnvelope = serde_json::from_str(&envelope_json)
        .map_err(|e| AutoReloginError::InvalidEnvelope(e.to_string()))?;

    if envelope.endpoint != current_endpoint {
        return Err(AutoReloginError::EndpointMismatch {
            expected: current_endpoint.to_string(),
            cached: envelope.endpoint.clone(),
        });
    }

    let key_bytes = B64
        .decode(&key_b64)
        .map_err(|_| AutoReloginError::KeychainCorrupt)?;
    if key_bytes.len() != 32 {
        return Err(AutoReloginError::KeychainCorrupt);
    }
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);
    let key_arr = key_arr; // immutable after this point

    let blob = B64
        .decode(&envelope.blob)
        .map_err(|_| AutoReloginError::KeychainCorrupt)?;

    let plaintext = decrypt_blob(&key_arr, envelope.endpoint.as_bytes(), &blob)
        .map_err(|_| AutoReloginError::DecryptFailed)?;

    let plaintext_str = std::str::from_utf8(&plaintext)
        .map_err(|_| AutoReloginError::KeychainCorrupt)?;

    let (email, password) = plaintext_str
        .split_once('\0')
        .ok_or(AutoReloginError::KeychainCorrupt)?;

    Ok(CachedCreds {
        email: email.to_string(),
        password: password.to_string(),
    })
}

/// Clear all cached credentials from the keychain. Idempotent: a no-op
/// if no entries exist. Called from `maic_logout` regardless of whether
/// the user had "Stay signed in" checked, so a user who un-checks the box
/// and re-logs-in has their prior stash wiped.
pub fn clear_cached_credentials() -> Result<(), AutoReloginError> {
    // We don't care if the entries don't exist; that's success.
    for user in [KEY_USER, CREDS_USER] {
        match entry(user)?.delete_credential() {
            Ok(()) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(AutoReloginError::KeychainUnavailable(e.to_string())),
        }
    }
    eprintln!("[miracle-claw] auto_relogin: cleared cached creds");
    Ok(())
}

// -------------------------------------------------------------------------
// Tests (pure helpers; keychain integration is exercised on David's box)
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = generate_key();
        let nonce = generate_nonce();
        let blob = encrypt_blob(&key, &nonce, b"https://maicserver.com", b"hello world");
        let pt = decrypt_blob(&key, b"https://maicserver.com", &blob).unwrap();
        assert_eq!(pt, b"hello world");
    }

    #[test]
    fn nonce_uniqueness_same_plaintext_different_ciphertexts() {
        let key = generate_key();
        let blob_1 = encrypt_blob(&key, &generate_nonce(), b"ep", b"same");
        let blob_2 = encrypt_blob(&key, &generate_nonce(), b"ep", b"same");
        assert_ne!(
            blob_1, blob_2,
            "same plaintext must produce different ciphertexts (random nonce)"
        );
        // Both must decrypt to the same plaintext.
        assert_eq!(decrypt_blob(&key, b"ep", &blob_1).unwrap(), b"same");
        assert_eq!(decrypt_blob(&key, b"ep", &blob_2).unwrap(), b"same");
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let key = generate_key();
        let blob = encrypt_blob(&key, &generate_nonce(), b"ep", b"hello");
        let other_key = generate_key();
        let result = decrypt_blob(&other_key, b"ep", &blob);
        assert_eq!(result, Err("decrypt failed"));
    }

    #[test]
    fn tampered_ciphertext_fails_decrypt() {
        let key = generate_key();
        let mut blob = encrypt_blob(&key, &generate_nonce(), b"ep", b"hello");
        // Flip a byte deep in the ciphertext (well past the nonce).
        blob[20] ^= 0xFF;
        let result = decrypt_blob(&key, b"ep", &blob);
        assert_eq!(result, Err("decrypt failed"));
    }

    #[test]
    fn truncated_blob_rejected() {
        let key = generate_key();
        // Shorter than 12 (nonce) + 16 (tag) bytes = minimum valid blob.
        let result = decrypt_blob(&key, b"ep", &[0u8, 1, 2, 3]);
        assert_eq!(result, Err("blob too short"));
    }

    #[test]
    fn wrong_aad_fails_decrypt() {
        let key = generate_key();
        let blob = encrypt_blob(&key, &generate_nonce(), b"endpoint-a", b"hello");
        let result = decrypt_blob(&key, b"endpoint-b", &blob);
        assert_eq!(result, Err("decrypt failed"));
    }

    #[test]
    fn ascii_password_round_trips() {
        let key = generate_key();
        // Edge cases: empty password, unicode, very long, password
        // containing the separator character (\0).
        for pw in &["", "p\u{2603}ssword", &"x".repeat(1024), "abc\0def"] {
            let plaintext = format!("user@example.com\0{}", pw);
            let blob = encrypt_blob(&key, &generate_nonce(), b"ep", plaintext.as_bytes());
            let recovered = decrypt_blob(&key, b"ep", &blob).unwrap();
            assert_eq!(recovered, plaintext.as_bytes(), "round-trip failed for {:?}", pw);
        }
    }

    #[test]
    fn envelope_round_trips_through_json() {
        let env = CredsEnvelope {
            endpoint: "https://maicserver.com".to_string(),
            blob: B64.encode(&[1u8, 2, 3, 4]),
        };
        let json = serde_json::to_string(&env).unwrap();
        let parsed: CredsEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.endpoint, env.endpoint);
        assert_eq!(parsed.blob, env.blob);
    }
}
