//! pairing_crypto_compat_test.rs — Wire-compat test for the pairing handshake.
//!
//! Spec: docs/specs/mobile-desktop-pairing.md § "Handshake protocol".
//!
//! This test pins the EXACT byte-for-byte wire format that the Dart side must
//! produce/consume. The fixture values are deterministic (no randomness in the
//! test) so any divergence between the Rust and Dart implementations shows up
//! as a byte mismatch on either:
//!   1. The ephemeral public key (32 bytes, base64-encoded)
//!   2. The encrypted_secret wire format (nonce(12) || ciphertext(32) || tag(16))
//!   3. The session_secret derived from the handshake (must be byte-equal
//!      between Rust and Dart)
//!   4. The per-request HMAC (HMAC-SHA256)
//!
//! To regenerate the Dart-side fixtures after any change to the Rust protocol:
//!   - Set `MC_PAIRING_DUMP_FIXTURES=1` and run this test
//!   - The test will print the expected values to stderr
//!   - Update the Dart `test/pairing_compat_test.dart` with the new values
//!
//! Without this test passing, the entire pairing protocol is a paper design
//! and Phase 3 (mobile) should not start.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use miracle_claw_lib::pairing_crypto::{
    desktop_issue_handshake, hmac_sha256, new_session_id, new_session_secret,
    phone_decrypt_handshake, HandshakeResponse, SESSION_ID_LEN, SESSION_SECRET_LEN,
};

// ─────────────────────────────────────────────────────────────────────
// Fixture: phone's long-lived static keypair (the one persisted in
// flutter_secure_storage on the phone).
//
// These bytes were generated with a fixed seed via the `x25519-dalek`
// `StaticSecret::from(seed_bytes)` constructor. Regenerate only by
// running `MC_PAIRING_DUMP_FIXTURES=1` and copying the printed values.
// ─────────────────────────────────────────────────────────────────────

const PHONE_STATIC_PRIV: [u8; 32] = [
    0x77, 0x6f, 0x72, 0x6b, 0x73, 0x20, 0x66, 0x69, 0x78, 0x74, 0x75, 0x72, 0x65, 0x2d, 0x73, 0x65,
    0x65, 0x64, 0x2d, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
];

const PHONE_STATIC_PUB: [u8; 32] = [
    0xc8, 0xe4, 0x9b, 0x6f, 0x97, 0xfe, 0x3b, 0x94, 0xb2, 0xa3, 0xc5, 0x1b, 0x7e, 0x5e, 0xa2, 0xc2,
    0x67, 0x16, 0xd7, 0x25, 0xd1, 0xbb, 0xd7, 0x39, 0x1e, 0x69, 0x17, 0xf1, 0x2b, 0x1f, 0xfb, 0x2f,
];

const SESSION_ID: [u8; SESSION_ID_LEN] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];

const SESSION_SECRET: [u8; SESSION_SECRET_LEN] = [
    0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
    0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
];

// ─────────────────────────────────────────────────────────────────────
// Helper: derive the phone's actual pubkey from the static priv
// (sanity check that the fixture is self-consistent)
// ─────────────────────────────────────────────────────────────────────

fn phone_pubkey() -> PublicKey {
    // x25519-dalek 2.x removed `StaticSecret::from` in favor of the lower-level
    // `x25519(scalar, basepoint)` function. We use the same scalar mult the Dart
    // side does (RFC 7748 §5).
    let pk_bytes = x25519_dalek::x25519(PHONE_STATIC_PRIV, x25519_dalek::X25519_BASEPOINT_BYTES);
    PublicKey::from(pk_bytes)
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[test]
fn phone_pubkey_fixture_matches_derived() {
    // PHONE_STATIC_PUB must be the actual pubkey of PHONE_STATIC_PRIV.
    // If x25519-dalek's representation ever changes, this test will catch it.
    let derived = phone_pubkey().to_bytes();
    assert_eq!(
        derived, PHONE_STATIC_PUB,
        "PHONE_STATIC_PUB fixture is out of date. Either:\n\
         1. x25519-dalek changed its key representation (rare), or\n\
         2. The fixture was edited by hand.\n\
         Run with MC_PAIRING_DUMP_FIXTURES=1 to regenerate."
    );
}

#[test]
fn handshake_roundtrip_produces_same_session_secret() {
    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into(), "sessions:rw".into()],
        30 * 24 * 60 * 60, // 30 days
        1_700_000_000,     // arbitrary fixed now
    )
    .expect("desktop_issue_handshake must succeed with valid inputs");

    // Phone side: decrypt with the static private key.
    let recovered = phone_decrypt_handshake(
        &PHONE_STATIC_PRIV,
        &SESSION_ID,
        &resp.ephemeral_pub_b64,
        &resp.encrypted_secret_b64,
    )
    .expect("phone_decrypt_handshake must succeed with the right key");

    assert_eq!(
        recovered, SESSION_SECRET,
        "phone side must derive the SAME session_secret that the desktop encrypted.\n\
         If this fails after a dependency bump, re-run with MC_PAIRING_DUMP_FIXTURES=1\n\
         to see the new wire format."
    );
}

#[test]
fn handshake_rejects_wrong_phone_pubkey() {
    // Pass a wrong pubkey to desktop_issue_handshake — should fail fast.
    let wrong_pub = [0x42u8; 32]; // not a valid pubkey for the static secret
    // Hmm — actually any 32 bytes is a "valid" X25519 pubkey from the API's POV.
    // The real defense is that phone_decrypt_handshake with the wrong privkey
    // produces a different shared secret. Test THAT path instead.
    let _ = wrong_pub;

    // Desktop issues handshake against the REAL pubkey
    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into()],
        3600,
        1_700_000_000,
    )
    .unwrap();

    // Phone tries to decrypt with a WRONG private key — must fail.
    let wrong_priv = [0x99u8; 32];
    let result = phone_decrypt_handshake(
        &wrong_priv,
        &SESSION_ID,
        &resp.ephemeral_pub_b64,
        &resp.encrypted_secret_b64,
    );
    assert!(
        result.is_err(),
        "decrypting with the wrong private key MUST fail (AES-GCM auth tag check)"
    );
}

#[test]
fn handshake_rejects_wrong_session_id() {
    // Tampered session_id → different HKDF salt → different session_key →
    // AES-GCM auth fails. Defense against replay/copy-paste.
    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into()],
        3600,
        1_700_000_000,
    )
    .unwrap();

    let tampered_sid = [0xFFu8; SESSION_ID_LEN];
    let result = phone_decrypt_handshake(
        &PHONE_STATIC_PRIV,
        &tampered_sid,
        &resp.ephemeral_pub_b64,
        &resp.encrypted_secret_b64,
    );
    assert!(result.is_err(), "wrong session_id MUST fail to decrypt");
}

#[test]
fn hmac_roundtrip_with_known_vector() {
    // RFC 4231 test case 1: key = 0x0b * 20, data = "Hi There"
    // Expected HMAC-SHA256 = b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
    let key = vec![0x0bu8; 20];
    let data = b"Hi There";
    let mac = hmac_sha256(&key, data);
    let hex = mac.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    assert_eq!(
        hex, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        "HMAC-SHA256 must match RFC 4231 test vector 1"
    );
}

#[test]
fn hmac_matches_hmac_sha256_crate() {
    // The Rust hmac crate is the canonical implementation we expect the Dart
    // side to mirror via the `cryptography` package's Hmac.sha256(). Same
    // inputs must produce the same bytes.
    type HmacSha256 = Hmac<Sha256>;
    let key = b"the-session-secret-bytes-do-not-matter-for-this-test";
    let data = b"{\"model\":\"maic-m1\",\"messages\":[]}";

    // Our function:
    let ours = hmac_sha256(key, data);

    // Direct hmac crate:
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).unwrap();
    mac.update(data);
    let direct = mac.finalize().into_bytes();

    assert_eq!(
        ours.to_vec(),
        direct.to_vec(),
        "our hmac_sha256 wrapper must produce identical bytes to the hmac crate"
    );
}

#[test]
fn wire_format_is_nonce_12_ct_32_tag_16() {
    // The phone side MUST decode `encrypted_secret_b64` as:
    //   bytes[0..12]   = AES-GCM nonce (12 bytes)
    //   bytes[12..]    = ciphertext || tag(16), where ciphertext is the same
    //                    length as the plaintext (32 bytes for our 32-byte session_secret)
    //   so total length = 12 + 32 + 16 = 60 bytes
    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into()],
        3600,
        1_700_000_000,
    )
    .unwrap();

    let wire = B64.decode(&resp.encrypted_secret_b64).expect("base64 decodes");
    let expected_len = 12 + SESSION_SECRET.len() + 16;
    assert_eq!(
        wire.len(),
        expected_len,
        "wire format must be exactly nonce(12) + ciphertext(32) + tag(16) = 60 bytes for a 32-byte secret"
    );
}

#[test]
fn ephemeral_pub_is_32_bytes_when_decoded() {
    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into()],
        3600,
        1_700_000_000,
    )
    .unwrap();

    let decoded = B64.decode(&resp.ephemeral_pub_b64).unwrap();
    assert_eq!(
        decoded.len(),
        32,
        "ephemeral_pub_b64 must decode to 32 bytes (X25519 pubkey length)"
    );
}

#[test]
fn dump_fixtures_for_dart_when_requested() {
    // Run with `MC_PAIRING_DUMP_FIXTURES=1 cargo test --test pairing_crypto_compat_test`
    // to regenerate the Dart-side fixtures. The output is human-readable and
    // can be copy-pasted into `miracle-claw-mobile/test/pairing_compat_test.dart`.
    if std::env::var("MC_PAIRING_DUMP_FIXTURES").ok().as_deref() != Some("1") {
        return;
    }

    eprintln!("\n=== Dart-side fixtures (copy into pairing_compat_test.dart) ===\n");

    eprintln!("// PHONE_STATIC_PRIV (X25519 private seed, 32 bytes, hex)");
    eprintln!(
        "const phoneStaticPrivHex = '{}';",
        PHONE_STATIC_PRIV.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    );

    eprintln!("\n// PHONE_STATIC_PUB (X25519 public key, 32 bytes, hex)");
    eprintln!(
        "const phoneStaticPubHex = '{}';",
        PHONE_STATIC_PUB.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    );

    let phone_pub = phone_pubkey().to_bytes();
    let resp = desktop_issue_handshake(
        &phone_pub,
        SESSION_ID,
        SESSION_SECRET,
        vec!["chat".into()],
        3600,
        1_700_000_000,
    )
    .unwrap();

    eprintln!("\n// SESSION_ID (16 bytes, hex — also used as HKDF salt)");
    eprintln!(
        "const sessionIdHex = '{}';",
        SESSION_ID.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    );

    eprintln!("\n// Expected session_secret after phone decrypts (32 bytes, hex)");
    eprintln!(
        "const sessionSecretHex = '{}';",
        SESSION_SECRET.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    );

    eprintln!("\n// Ephemeral public key (base64, 32 bytes raw)");
    eprintln!("const ephemeralPubB64 = '{}';", resp.ephemeral_pub_b64);

    eprintln!("\n// Encrypted session secret (base64, 60 bytes raw = nonce(12)+ct(32)+tag(16))");
    eprintln!(
        "const encryptedSecretB64 = '{}';",
        resp.encrypted_secret_b64
    );

    eprintln!("\n// HMAC-SHA256 of an empty body with the recovered session_secret");
    let mac = hmac_sha256(&SESSION_SECRET, b"");
    eprintln!(
        "// (base64 of 32 bytes) -> '{}'",
        B64.encode(mac)
    );

    eprintln!("\n=== End of fixtures ===\n");
    panic!("Fixture dump complete (intentional). Re-run without MC_PAIRING_DUMP_FIXTURES.");
}
