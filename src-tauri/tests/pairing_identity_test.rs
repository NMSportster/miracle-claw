// tests/pairing_identity_test.rs — round-trip + edge cases for pairing_identity.
//
// Run from main-adeal: cargo test --test pairing_identity_test --manifest-path src-tauri/Cargo.toml

use miracle_claw_lib::pairing_identity::{
    fingerprint_from_pubkey, generate, generate_instance_id, generate_keypair, load, load_from,
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
    // Phase 2.4.1 fix: MAIC's DesktopRegisterIn.fingerprint has
    // minLength: 16. Old format was 12 hex + 2 dashes = 14 chars,
    // hit 422 `string_too_short`. New format is 18 hex + 2 dashes
    // = 20 chars, comfortably above the 16-char minimum.
    assert_eq!(fp.len(), 20, "expected 18 hex + 2 dashes, got: {fp:?}");
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
    // Phase 2.4.1: instance_id is a UUID v4, not `inst_<16 hex>`.
    // 36 chars total: 8-4-4-4-12 hex with 4 hyphens, version nibble
    // is '4', variant nibble is one of [89ab].
    let id = generate_instance_id();
    let s = id.to_string();
    assert_eq!(s.len(), 36, "UUID v4 string is always 36 chars, got: {}", s);
    assert!(s.chars().nth(14).unwrap() == '4', "version nibble must be 4, got: {}", s);
    let variant = s.chars().nth(19).unwrap();
    assert!(matches!(variant, '8' | '9' | 'a' | 'b'), "variant nibble must be 8/9/a/b, got: {}", s);
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

#[test]
fn load_from_migrates_v0_0_3_inst_hex_to_uuid() {
    // Homebot mcmobile.txt 2026-09-10: without this, the CLI
    // (`load_from`) couldn't read v0.0.3-era identity files and
    // bailed with "UUID parsing failed". `load()` already migrated;
    // this pins the same behavior in `load_from` so future refactors
    // don't silently drop the migration in one of the two paths.
    use std::fs;

    let tmp = unique_tmp();
    let path = tmp.join("identity.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    // Write a v0.0.3-era file (instance_id: "inst_<16 hex>")
    let legacy = r#"{
        "instance_id": "inst_4bcab9f1eaeab082",
        "desktop_pubkey_b64": "EAmcr3jRTU/qshwvO2nRrW2PrvgZZr784X1hMPkqbjo=",
        "desktop_secret_key": [80,222,8,44,40,182,97,191,182,102,138,192,193,234,42,68,224,222,184,40,82,168,21,181,206,198,196,86,67,125,75,27],
        "fingerprint": "e6b4-f05d-b684",
        "created_at_unix": 1789015456
    }"#;
    fs::write(&path, legacy).unwrap();

    // load_from should migrate (and rewrite) the file.
    let id = load_from(&path).expect("load_from should migrate v0.0.3").expect("file exists");
    let s = id.instance_id.to_string();
    assert_eq!(s.len(), 36, "migrated instance_id must be a UUID v4 string, got: {s}");
    assert!(!s.starts_with("inst_"), "migrated instance_id must NOT have inst_ prefix, got: {s}");

    // The migrated file should now be re-readable as Identity
    // (proves the migration rewrote the file, not just returned
    // an in-memory converted value).
    let reread = load_from(&path).expect("re-read after migration").expect("still exists");
    assert_eq!(reread.instance_id, id.instance_id);
}

#[test]
fn load_from_regenerates_short_fingerprint() {
    // Phase 2.4.1 (Homebot 2026-09-10 00:35): MAIC's DesktopRegisterIn
    // requires fingerprint minLength 16. v0.0.4-era identities had
    // 14-char fingerprints (12 hex + 2 dashes). On load, if the
    // fingerprint is too short, regenerate it from the pubkey and
    // rewrite the file. The regenerated fingerprint is a pure
    // function of the pubkey, so this is safe and deterministic.
    use std::fs;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    let tmp = unique_tmp();
    let path = tmp.join("identity.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    // Use the same pubkey from `fingerprint_is_stable` test
    // so we can predict the new fingerprint.
    let key = [42u8; 32];
    let pubkey_b64 = STANDARD.encode(key);
    // desktop_secret_key is base64-encoded (see base64_secret mod)
    let secret_key = [80u8, 222, 8, 44, 40, 182, 97, 191, 182, 102, 138, 192, 193, 234, 42, 68, 224, 222, 184, 40, 82, 168, 21, 181, 206, 198, 196, 86, 67, 125, 75, 27];
    let secret_b64 = STANDARD.encode(secret_key);

    // Write a Phase 2.4.1-shape file with a too-short fingerprint.
    let short_fp_json = format!(r#"{{
        "instance_id": "29999ed1-e074-4001-aa6b-a3a8fb4e1d92",
        "desktop_pubkey_b64": "{pubkey_b64}",
        "desktop_secret_key": "{secret_b64}",
        "fingerprint": "ab70-ff89-43d7",
        "created_at_unix": 1789015456
    }}"#);
    fs::write(&path, &short_fp_json).unwrap();

    // load_from should detect the short fingerprint, regenerate it
    // from the pubkey, and rewrite the file.
    let id = load_from(&path).expect("load_from should regenerate short fingerprint").expect("file exists");
    let fp = fingerprint_from_pubkey(&key);
    assert_eq!(id.fingerprint, fp, "fingerprint should be re-derived from pubkey");
    assert_eq!(id.fingerprint.len(), 20, "new fingerprint must be 18 hex + 2 dashes = 20 chars");

    // The file should now be rewritten with the new fingerprint.
    let reread = load_from(&path).expect("re-read after fingerprint regen").expect("still exists");
    assert_eq!(reread.fingerprint, fp, "re-read fingerprint must match");
}
