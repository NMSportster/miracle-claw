// tests/pairing_identity_test.rs — round-trip + edge cases for pairing_identity.
//
// Run from main-adeal: cargo test --test pairing_identity_test --manifest-path src-tauri/Cargo.toml

use miracle_claw_lib::pairing_identity::{
    fingerprint_from_pubkey, generate, generate_instance_id, generate_keypair, load,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!(
        "mc-pairing-identity-test-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        n
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn fingerprint_is_stable() {
    let key = [42u8; 32];
    assert_eq!(fingerprint_from_pubkey(&key), fingerprint_from_pubkey(&key));
    let fp = fingerprint_from_pubkey(&key);
    assert_eq!(fp.len(), 14); // 12 hex + 2 dashes
    assert!(fp.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
}

#[test]
fn fingerprint_changes_with_input() {
    let k1 = [0u8; 32];
    let mut k2 = [0u8; 32];
    k2[0] = 1;
    assert_ne!(fingerprint_from_pubkey(&k1), fingerprint_from_pubkey(&k2));
}

#[test]
fn instance_id_format() {
    let id = generate_instance_id();
    assert!(id.starts_with("inst_"));
    assert_eq!(id.len(), 5 + 16);
}

#[test]
fn keypair_is_random() {
    let (p1, _s1) = generate_keypair();
    let (p2, _s2) = generate_keypair();
    assert_ne!(p1, p2);
}

#[test]
fn save_load_roundtrip() {
    let tmp = unique_tmp();
    let original = generate(&tmp).expect("generate");
    let loaded = load(&tmp).expect("load").expect("should exist");
    assert_eq!(original.instance_id, loaded.instance_id);
    assert_eq!(original.desktop_pubkey_b64, loaded.desktop_pubkey_b64);
    assert_eq!(original.fingerprint, loaded.fingerprint);
    assert_eq!(original.desktop_secret_key, loaded.desktop_secret_key);
}

#[test]
fn load_returns_none_when_missing() {
    let tmp = unique_tmp();
    let result = load(&tmp).expect("no error on missing file");
    assert!(result.is_none());
}

#[test]
fn generate_creates_distinct_ids_on_repeat() {
    let tmp1 = unique_tmp();
    let tmp2 = unique_tmp();
    let a = generate(&tmp1).unwrap();
    let b = generate(&tmp2).unwrap();
    assert_ne!(a.instance_id, b.instance_id);
    assert_ne!(a.desktop_pubkey_b64, b.desktop_pubkey_b64);
}
