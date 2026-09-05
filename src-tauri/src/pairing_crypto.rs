//! pairing_crypto.rs — Mobile ↔ Desktop pairing handshake primitives (Rust side).
//!
//! Spec: see docs/specs/mobile-desktop-pairing.md § "Handshake protocol".
//! Source of truth for the protocol. This module is the wire-compatible
//! counterpart to the Dart `lib/core/pairing/key_store.dart` + handshake code
//! using the `cryptography 2.9.0` package.
//!
//! The flow:
//!   1. Phone generates X25519 keypair (one-time, persisted in secure storage).
//!   2. Phone sends its pubkey + JWT + device_name to `POST {endpoint}/pair/initiate`.
//!   3. Desktop (this file) generates an EPHEMERAL X25519 keypair.
//!   4. Desktop computes shared_secret = X25519(ephemeral_priv, phone_pub).
//!   5. Desktop derives session_key = HKDF-SHA256(shared_secret, info="miracle-claw-pair-v1", salt=session_id).
//!   6. Desktop generates a random 32-byte session_secret.
//!   7. Desktop encrypts session_secret with AES-256-GCM(key=session_key, nonce=random).
//!   8. Desktop sends {session_id, ephemeral_pub, encrypted_session_secret, capabilities, expires_at} to phone.
//!   9. Phone does the same X25519+HKDF+AES-GCM dance, recovers session_secret, stores it.
//!
//! Wire compatibility is proven by `tests/pairing_crypto_compat_test.rs`, which
//! hardcodes fixed inputs and checks both sides derive the same session_secret.
//!
//! IMPORTANT: changes here MUST be mirrored in the Dart counterpart before
//! shipping. Run the compat test after any change.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

/// Domain separator for the session key derivation. **DO NOT CHANGE without
/// coordinating with the Dart side — breaking this is a wire-incompat.** Stable
/// since 2026-09-05.
pub const PAIR_INFO: &[u8] = b"miracle-claw-pair-v1";

/// Session IDs are 16 random bytes (UUID v4 worth of entropy). The Dart side
/// uses UUIDs directly; the bytes representation must match.
pub const SESSION_ID_LEN: usize = 16;

/// AES-256-GCM key length. The Dart side uses `AesGcm.with256bits()`.
pub const SESSION_KEY_LEN: usize = 32;

/// Session secret length (random bytes). 32 bytes = 256 bits of entropy.
pub const SESSION_SECRET_LEN: usize = 32;

/// Errors from the handshake. All map to a HTTP 4xx on the wire side.
#[derive(Debug, Error)]
pub enum PairingError {
    #[error("session_secret must be {SESSION_SECRET_LEN} bytes")]
    InvalidSecretLength,
    #[error("session_id must be {SESSION_ID_LEN} bytes")]
    InvalidSessionIdLength,
    #[error("phone public key must be 32 bytes")]
    InvalidPhonePubkey,
    #[error("AES-GCM encryption failed")]
    AesGcmEncrypt,
    #[error("AES-GCM decryption failed (wrong key, wrong ciphertext, or wrong session_id?)")]
    AesGcmDecrypt,
}

type PairingResult<T> = Result<T, PairingError>;

/// Handshake response sent to the phone after successful `POST /pair/initiate`.
#[derive(Debug, Clone)]
pub struct HandshakeResponse {
    /// Random session ID (also used as the HKDF salt). The phone uses this as
    /// the cache key for storing the session_secret in `flutter_secure_storage`.
    pub session_id: [u8; SESSION_ID_LEN],
    /// Base64-encoded ephemeral X25519 public key (32 bytes raw).
    pub ephemeral_pub_b64: String,
    /// Base64-encoded AES-256-GCM(ciphertext + 16-byte tag) of the session_secret.
    /// Format: `nonce(12) || ciphertext || tag(16)`, base64-encoded as one string.
    pub encrypted_secret_b64: String,
    /// Capabilities granted to this device session (caller fills this in; the
    /// crypto module doesn't know about the capability matrix).
    pub capabilities: Vec<String>,
    /// Unix timestamp (seconds) when the session expires.
    pub expires_at_unix: u64,
}

/// The desktop-side handshake. Given the phone's long-lived pubkey, returns
/// (HandshakeResponse, session_secret_in_memory). The caller is responsible
/// for keeping `session_secret` in process memory only (never on disk) and
/// inserting session_id → session_secret into the in-memory map.
///
/// Wire format produced here MUST match what the Dart side expects in
/// `Handshake.decrypt()`:
///   - ephemeral_pub_b64: base64(32 raw bytes)
///   - encrypted_secret_b64: base64(nonce(12) || ciphertext || tag(16))
pub fn desktop_issue_handshake(
    phone_pubkey_bytes: &[u8],
    session_id: [u8; SESSION_ID_LEN],
    session_secret: [u8; SESSION_SECRET_LEN],
    capabilities: Vec<String>,
    ttl_seconds: u64,
    now_unix: u64,
) -> PairingResult<HandshakeResponse> {
    if phone_pubkey_bytes.len() != 32 {
        return Err(PairingError::InvalidPhonePubkey);
    }

    // Generate ephemeral X25519 keypair (this desktop instance, this handshake only).
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);

    // Decode the phone's long-lived pubkey.
    let phone_public = PublicKey::from(<[u8; 32]>::try_from(phone_pubkey_bytes).unwrap());

    // Compute the shared secret.
    let shared_secret = ephemeral_secret.diffie_hellman(&phone_public);

    // Derive the session key via HKDF-SHA256.
    //   salt = session_id (16 bytes)
    //   info = "miracle-claw-pair-v1" (PAIR_INFO)
    //   output = 32 bytes
    let hk = Hkdf::<Sha256>::new(Some(&session_id), shared_secret.as_bytes());
    let mut session_key = [0u8; SESSION_KEY_LEN];
    hk.expand(PAIR_INFO, &mut session_key)
        .expect("HKDF expand with 32-byte output is always safe");

    // Encrypt the session_secret with AES-256-GCM.
    //   key = session_key (32 bytes)
    //   nonce = 12 random bytes (NEVER reuse with the same key)
    //   aad = empty (we bind to session_id via the HKDF salt, not via aad;
    //        the wire-level per-request HMAC binds each request to the session)
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&session_key));
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // aes-gcm 0.10 returns ciphertext||tag(16) concatenated as `encrypt()` output.
    let ciphertext_with_tag = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &session_secret,
                aad: &[],
            },
        )
        .map_err(|_| PairingError::AesGcmEncrypt)?;

    // Wire format: nonce(12) || ciphertext_with_tag(32 + 16)
    let mut wire = Vec::with_capacity(12 + ciphertext_with_tag.len());
    wire.extend_from_slice(&nonce_bytes);
    wire.extend_from_slice(&ciphertext_with_tag);

    Ok(HandshakeResponse {
        session_id,
        ephemeral_pub_b64: B64.encode(ephemeral_public.as_bytes()),
        encrypted_secret_b64: B64.encode(&wire),
        capabilities,
        expires_at_unix: now_unix + ttl_seconds,
    })
}

/// Test-only helper: lets a test re-derive the session_secret from the
/// handshake response, given the phone's static private key. This is what
/// the Dart phone code does after receiving the response. We expose it here
/// to enable wire-compat tests that prove both sides derive the same secret.
#[doc(hidden)]
pub fn phone_decrypt_handshake(
    phone_static_secret_bytes: &[u8; 32],
    session_id: &[u8; SESSION_ID_LEN],
    ephemeral_pub_b64: &str,
    encrypted_secret_b64: &str,
) -> PairingResult<[u8; SESSION_SECRET_LEN]> {
    let ephemeral_pub_bytes = B64
        .decode(ephemeral_pub_b64)
        .map_err(|_| PairingError::AesGcmDecrypt)?;
    if ephemeral_pub_bytes.len() != 32 {
        return Err(PairingError::AesGcmDecrypt);
    }
    let ephemeral_public =
        PublicKey::from(<[u8; 32]>::try_from(ephemeral_pub_bytes).unwrap());

    let phone_static = StaticSecret::from(*phone_static_secret_bytes);
    let shared_secret = phone_static.diffie_hellman(&ephemeral_public);

    let hk = Hkdf::<Sha256>::new(Some(session_id), shared_secret.as_bytes());
    let mut session_key = [0u8; SESSION_KEY_LEN];
    hk.expand(PAIR_INFO, &mut session_key)
        .expect("HKDF expand with 32-byte output is always safe");

    let wire = B64
        .decode(encrypted_secret_b64)
        .map_err(|_| PairingError::AesGcmDecrypt)?;
    if wire.len() < 12 + 16 {
        return Err(PairingError::AesGcmDecrypt);
    }
    let (nonce_bytes, ciphertext_with_tag) = wire.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&session_key));
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext_with_tag,
                aad: &[],
            },
        )
        .map_err(|_| PairingError::AesGcmDecrypt)?;

    if plaintext.len() != SESSION_SECRET_LEN {
        return Err(PairingError::AesGcmDecrypt);
    }
    let mut out = [0u8; SESSION_SECRET_LEN];
    out.copy_from_slice(&plaintext);
    Ok(out)
}

/// Generate a random session_id (16 bytes = UUID v4 worth of entropy).
#[doc(hidden)]
pub fn new_session_id() -> [u8; SESSION_ID_LEN] {
    let mut id = [0u8; SESSION_ID_LEN];
    OsRng.fill_bytes(&mut id);
    id
}

/// Generate a random session_secret (32 bytes = AES-256 key worth of entropy).
#[doc(hidden)]
pub fn new_session_secret() -> [u8; SESSION_SECRET_LEN] {
    let mut s = [0u8; SESSION_SECRET_LEN];
    OsRng.fill_bytes(&mut s);
    s
}

/// Per-request HMAC-SHA256 binding. Phone computes `HMAC(session_secret, body)`
/// and sends it as the `X-Miracle-Pair-Session: <session_id>:<hmac_hex>` header.
/// Desktop validates: HMAC matches, session is not revoked/expired, capability
/// is allowed.
///
/// This is exposed so the compat test can verify the same body produces the
/// same HMAC on both sides (the Dart side computes HMAC-SHA256, the Rust side
/// uses hmac + sha2).
#[doc(hidden)]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
        .expect("HMAC accepts any key length");
    mac.update(message);
    let result = mac.finalize();
    let bytes = result.into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}
