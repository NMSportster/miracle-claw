//! AES-256-GCM encryption-at-rest for the secrets vault.
//!
//! # rc53.8 design (David requested 2026-08-23 ~18:38 MDT)
//!
//! Before this module, the secrets vault stored `SecretEntry { name, value,
//! ... }` records as **plaintext JSON** in `<MC_DATA>/secrets.json`. David
//! called out that "it's encrypted and safe" was wrong — anyone with disk
//! access (or a `file_recovery` tool, or a backup viewer) could read the
//! vault.
//!
//! This module:
//!
//! 1. **Generates a per-install 32-byte master key** on first save,
//!    stored in the **OS keychain** (Windows Credential Manager, macOS
//!    Keychain, Linux Secret Service) under service
//!    `com.adealauto.miracle-claw`, user `secrets-vault-v1`.
//! 2. **Encrypts the entire vault** as a single AES-256-GCM blob.
//!    On-disk format: `nonce (12B) || ciphertext || tag (16B)` (auto_relogin's
//!    `encrypt_blob` already produces this shape).
//! 3. **Authenticates with associated data** `"miracle-claw/vault-v1"` so a
//!    stolen ciphertext from a different app can't be replayed against
//!    our keychain.
//! 4. **Migrates legacy plaintext vaults on first save**: if
//!    `secrets.json` is valid plaintext JSON (no nonce prefix), we
//!    read it normally, then write the encrypted form. The plaintext
//!    file is overwritten atomically by the encrypted write — no
//!    transient plaintext lingers.
//!
//! # Threat model
//!
//! - **Disk theft / backup snooping**: solved. Without the OS keychain
//!   entry, the blob is unauthenticated AES-GCM ciphertext = random
//!   noise.
//! - **Process memory dump while app is running**: NOT solved. While
//!   `mc_secret_expand` decrypts in memory to substitute the
//!   placeholder, the plaintext is in the process. The Expand path
//!   is short-lived (substitute + send to shell), but the model is
//!   "memory-at-rest" — not "memory-in-use". Future lesson: clear
//!   after substitution.
//! - **OS keychain compromise**: out of scope. If the keychain is
//!   compromised, everything is.
//!
//! # Tests
//!
//! The pure helpers (`encrypt_vault_blob`, `decrypt_vault_blob`,
//! `is_encrypted_vault_blob`) are testable without a keychain. The
//! `vault_master_key` integration with `keyring::Entry` requires an
//! OS keychain — we mark those tests `#[ignore]` and run them
//! manually on each platform before shipping a release.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use keyring::Entry;
use rand::RngCore;
use std::sync::OnceLock;

// -------------------------------------------------------------------------
// Constants
// -------------------------------------------------------------------------

/// OS keychain service identifier. Matches `auto_relogin::SERVICE` so the
/// keychain entry groups with cached creds in Credential Manager.
const SERVICE: &str = "com.adealauto.miracle-claw";

/// Keychain "user" string for the vault master key. Versioned so a future
/// algorithm migration can read the v1 key without breaking (yet), then
/// write a `secrets-vault-v2` key on first save.
const KEY_USER: &str = "secrets-vault-v1";

/// AAD bound to every encryption. v1 = the string below. If we ever
/// rotate to v2 encryption, change this string and bump the keychain
/// user — old ciphertexts will fail to authenticate and force a clean
/// slate (with the user's permission).
const AAD: &[u8] = b"miracle-claw/vault-v1";

/// Header prefix for the on-disk encrypted blob. We use a 4-byte magic
/// + version so we can detect plaintext migration cleanly without
/// false positives on random JSON that happens to start with `[`.
const MAGIC: &[u8; 4] = b"MCV1";
const HEADER_LEN: usize = 4 + 1; // MAGIC + version byte

// -------------------------------------------------------------------------
// Pure helpers (testable without OS keychain)
// -------------------------------------------------------------------------

/// Encrypt a JSON-serialized vault (`plaintext`) under `key`. Returns
/// `MCV1 || 0x01 || nonce || ciphertext || tag`.
pub fn encrypt_vault_blob(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);

    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-Gcm key is 32 bytes");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: AAD,
            },
        )
        .expect("AES-GCM encrypt should not fail for valid inputs");

    let mut out = Vec::with_capacity(HEADER_LEN + nonce.len() + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(0x01); // version
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

/// Inverse of `encrypt_vault_blob`. Returns the plaintext JSON, or an
/// error string on truncation, bad magic, wrong key, or tamper.
pub fn decrypt_vault_blob(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, &'static str> {
    if blob.len() < HEADER_LEN + 12 + 16 {
        return Err("encrypted vault too short");
    }
    if &blob[..4] != MAGIC {
        return Err("not a MCV1 vault blob");
    }
    if blob[4] != 0x01 {
        return Err("unknown vault version");
    }
    let (header, rest) = blob.split_at(HEADER_LEN);
    let _ = header; // not used past validation
    let (nonce_bytes, ciphertext) = rest.split_at(12);

    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-Gcm key is 32 bytes");
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| "vault decrypt failed (wrong key, tampered, or wrong AAD)")
}

/// Returns true if `blob` looks like an MCV1-encrypted vault (starts
/// with the magic). Used by `read_vault` to auto-migrate legacy
/// plaintext.
pub fn is_encrypted_vault_blob(blob: &[u8]) -> bool {
    blob.len() >= 4 && &blob[..4] == MAGIC
}

// -------------------------------------------------------------------------
// OS keychain integration
// -------------------------------------------------------------------------

/// Lazily-cached master key for the running process. We don't cache
/// forever — the keychain entry is the source of truth. But repeated
/// `vault_master_key()` calls within a single save/load burst hit the
/// keychain once.
static MASTER_KEY_CACHE: OnceLock<Result<[u8; 32], String>> = OnceLock::new();

/// Get the vault's master key from the OS keychain, generating a new
/// one if this is the first run. Idempotent.
///
/// Returns the raw 32 bytes on success, or an error string if the
/// keychain is unavailable (no D-Bus on Linux, locked macOS Keychain,
/// etc.).
pub fn vault_master_key() -> Result<[u8; 32], String> {
    MASTER_KEY_CACHE
        .get_or_init(|| {
            let entry = Entry::new(SERVICE, KEY_USER)
                .map_err(|e| format!("keychain unavailable: {e}"))?;

            // Try to read existing key first.
            if let Ok(b64) = entry.get_password() {
                if let Ok(bytes) = B64.decode(b64.trim()) {
                    if bytes.len() == 32 {
                        let mut key = [0u8; 32];
                        key.copy_from_slice(&bytes);
                        return Ok(key);
                    }
                }
                // Corrupt entry — fall through to regenerate. The
                // old (broken) entry will be overwritten below.
            }

            // Generate a new key and stash it.
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            let encoded = B64.encode(key);
            entry
                .set_password(&encoded)
                .map_err(|e| format!("failed to store master key: {e}"))?;
            Ok(key)
        })
        .clone()
}

// -------------------------------------------------------------------------
// High-level read/write (call from lib.rs read_vault / write_vault)
// -------------------------------------------------------------------------

/// Read the vault, transparently handling both encrypted (rc53.8+) and
/// plaintext (rc53.6) on-disk formats. Plaintext is detected by the
/// lack of `MCV1` magic at byte 0.
///
/// Returns the parsed `Vec<SecretEntry>` JSON on success.
pub fn read_vault_bytes(blob: &[u8]) -> Result<Vec<u8>, String> {
    if blob.is_empty() {
        return Ok(b"[]".to_vec());
    }
    if is_encrypted_vault_blob(blob) {
        let key = vault_master_key()?;
        decrypt_vault_blob(&key, blob)
            .map_err(|e| format!("vault decrypt: {e}"))
    } else {
        // Legacy plaintext JSON. Returned as-is so the caller can
        // parse it. The caller should re-save with `write_vault_bytes`
        // to migrate to the encrypted form on the next mutation.
        Ok(blob.to_vec())
    }
}

/// Encrypt and write the vault JSON. Always writes the MCV1 form,
/// migrating plaintext callers on first save.
pub fn write_vault_bytes(plaintext_json: &[u8]) -> Result<Vec<u8>, String> {
    let key = vault_master_key()?;
    Ok(encrypt_vault_blob(&key, plaintext_json))
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = [42u8; 32];
        let plaintext = br#"[{"name":"FOO","value":"bar","created_at":"epoch:1"}]"#;
        let blob = encrypt_vault_blob(&key, plaintext);
        assert_eq!(&blob[..4], MAGIC, "blob should start with magic");
        assert_eq!(blob[4], 0x01, "version byte should be 1");
        let pt = decrypt_vault_blob(&key, &blob).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn nonce_uniqueness_same_plaintext_different_ciphertexts() {
        let key = [7u8; 32];
        let plaintext = b"[]";
        let blob_1 = encrypt_vault_blob(&key, plaintext);
        let blob_2 = encrypt_vault_blob(&key, plaintext);
        // Same plaintext + same key + random nonces MUST produce different
        // ciphertexts (otherwise the nonce is broken).
        assert_ne!(blob_1, blob_2);
        assert_eq!(decrypt_vault_blob(&key, &blob_1).unwrap(), plaintext);
        assert_eq!(decrypt_vault_blob(&key, &blob_2).unwrap(), plaintext);
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let key = [1u8; 32];
        let plaintext = b"hello";
        let blob = encrypt_vault_blob(&key, plaintext);
        let other_key = [2u8; 32];
        let result = decrypt_vault_blob(&other_key, &blob);
        assert!(result.is_err(), "wrong key must fail");
    }

    #[test]
    fn truncated_blob_fails() {
        let key = [3u8; 32];
        let plaintext = b"x";
        let blob = encrypt_vault_blob(&key, plaintext);
        for cut in 0..blob.len() {
            if cut >= HEADER_LEN + 12 + 16 {
                continue; // valid full ciphertext
            }
            let truncated = &blob[..cut];
            assert!(
                decrypt_vault_blob(&key, truncated).is_err(),
                "truncated to {} bytes must fail",
                cut
            );
        }
    }

    #[test]
    fn wrong_magic_fails() {
        let key = [4u8; 32];
        // Plain JSON starts with `[`, not `MCV1`.
        let plaintext = b"[{\"name\":\"X\",\"value\":\"y\"}]";
        assert!(!is_encrypted_vault_blob(plaintext));
        let result = decrypt_vault_blob(&key, plaintext);
        assert!(result.is_err(), "plaintext must not decrypt");
    }

    #[test]
    fn bit_flip_in_ciphertext_fails() {
        let key = [5u8; 32];
        let plaintext = b"secret value here";
        let mut blob = encrypt_vault_blob(&key, plaintext);
        // Flip a bit in the ciphertext portion (after header + nonce).
        let idx = HEADER_LEN + 12 + 2;
        blob[idx] ^= 0x01;
        let result = decrypt_vault_blob(&key, &blob);
        assert!(result.is_err(), "tampered ciphertext must fail AES-GCM auth");
    }

    #[test]
    fn is_encrypted_vault_blob_detects_magic() {
        assert!(is_encrypted_vault_blob(b"MCV1..."));
        assert!(is_encrypted_vault_blob(b"MCV1"));
        assert!(!is_encrypted_vault_blob(b"[]"));
        assert!(!is_encrypted_vault_blob(b""));
        assert!(!is_encrypted_vault_blob(b"MCV2"));
    }

    #[test]
    fn plaintext_read_vault_returns_as_is_for_migration() {
        let plaintext = b"[{\"name\":\"FOO\",\"value\":\"old\",\"created_at\":\"epoch:1\"}]";
        let out = read_vault_bytes(plaintext).unwrap();
        assert_eq!(out, plaintext, "legacy plaintext must pass through");
    }

    #[test]
    fn empty_blob_read_vault_returns_empty_array() {
        let out = read_vault_bytes(b"").unwrap();
        assert_eq!(out, b"[]");
    }

    // -----------------------------------------------------------------
    // Keychain integration tests (#[ignore] — require a real keychain)
    //
    // Run with: cargo test --lib -- --ignored secrets_encryption
    // -----------------------------------------------------------------

    #[test]
    #[ignore = "requires OS keychain (Credential Manager / Keychain / Secret Service)"]
    fn keychain_round_trip_generates_and_reads_same_key() {
        let key_1 = vault_master_key().expect("first read should succeed");
        let key_2 = vault_master_key().expect("second read should hit cache");
        assert_eq!(key_1, key_2, "OnceLock cache must return identical key");
        // 32 bytes of entropy — must NOT be all zeros.
        assert!(key_1.iter().any(|&b| b != 0));
    }
}