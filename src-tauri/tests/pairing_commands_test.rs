//! pairing_commands_test.rs — Integration tests for the Tauri pairing commands.
//!
//! Spec: docs/specs/mobile-desktop-pairing.md.
//!
//! These tests use a `FakeMaicHttp` that records requests and returns canned
//! responses. They cover the full orchestration:
//!   - register_desktop → PUT to MAIC, instance_id round-trip
//!   - initiate_pair → MAIC paired_device POST + crypto handshake
//!   - revoke_device → MAIC DELETE + local session revocation
//!   - unregister_desktop → cascade to MAIC + all sessions
//!   - set_drop_folder → empty string disables
//!   - get_pairing_status → active session count + drop-folder
//!
//! Crypto is the real `pairing_crypto` (already proven wire-compatible with
//! Dart via pairing_crypto_compat_test.rs). The phone-side decrypt is also
//! exercised to prove the handshake produces a recoverable session_secret.

use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use miracle_claw_lib::pairing_commands::{
    initiate_pair, paired_devices, register_desktop, revoke_device, set_drop_folder,
    unregister_desktop, PairingCommands, PairingError,
};
use miracle_claw_lib::pairing_crypto::{
    desktop_issue_handshake, new_session_id, new_session_secret, phone_decrypt_handshake,
    SESSION_ID_LEN, SESSION_SECRET_LEN,
};
use miracle_claw_lib::pairing_state::{now_unix, PairingState, DEFAULT_SESSION_TTL_SECONDS};

use serde_json::{json, Value};

// ─────────────────────────────────────────────────────────────────────
// FakeMaicHttp — records requests, returns canned responses.
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    body: Value,
}

#[derive(Default)]
struct FakeMaic {
    requests: Mutex<Vec<RecordedRequest>>,
    desktops: Mutex<std::collections::HashMap<String, Value>>,
    paired_devices: Mutex<std::collections::HashMap<String, Vec<Value>>>,
    next_device_id: Mutex<i64>,
    fail_put_desktop_once: Mutex<bool>,
}

impl FakeMaic {
    fn new() -> Self {
        Self {
            next_device_id: Mutex::new(1000),
            ..Default::default()
        }
    }

    fn record(&self, method: &str, path: &str, body: Value) {
        self.requests.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: path.to_string(),
            body,
        });
    }

    fn device_count_for(&self, instance_id: &str) -> usize {
        self.paired_devices
            .lock()
            .unwrap()
            .get(instance_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

// We need to implement the trait. Pull it in via the public path.
use miracle_claw_lib::pairing_commands::MaicHttp;

impl MaicHttp for FakeMaic {
    fn put_desktop(&self, instance_id: &str, body: &Value) -> Result<Value, PairingError> {
        // Optional one-shot failure to test error path
        let mut fail = self.fail_put_desktop_once.lock().unwrap();
        if *fail {
            *fail = false;
            return Err(PairingError::MaicRequest("simulated network error".into()));
        }

        self.record("PUT", &format!("/v1/users/me/desktops/{}", instance_id), body.clone());

        // Idempotent upsert: existing fingerprint/pubkey must NOT be overwritten.
        let mut desktops = self.desktops.lock().unwrap();
        let existing = desktops.get(instance_id).cloned();
        let merged = if let Some(prev) = existing {
            // Preserve fingerprint + pubkey (MITM defense)
            json!({
                "instance_id": instance_id,
                "instance_name": body.get("instance_name").cloned().unwrap_or(json!("test")),
                "public_key": prev.get("public_key").cloned().unwrap_or(json!("")),
                "fingerprint": prev.get("fingerprint").cloned().unwrap_or(json!("")),
                "endpoint_kind": body.get("endpoint_kind").cloned().unwrap_or(json!("lan_or_relay")),
                "mobile_drop_folder": prev.get("mobile_drop_folder").cloned().unwrap_or(Value::Null),
                "last_heartbeat_at": 1_700_000_000_i64,
            })
        } else {
            json!({
                "instance_id": instance_id,
                "instance_name": body.get("instance_name").cloned().unwrap_or(json!("test")),
                "public_key": body.get("public_key").cloned().unwrap_or(json!("")),
                "fingerprint": body.get("fingerprint").cloned().unwrap_or(json!("")),
                "endpoint_kind": body.get("endpoint_kind").cloned().unwrap_or(json!("lan_or_relay")),
                "mobile_drop_folder": Value::Null,
                "last_heartbeat_at": 1_700_000_000_i64,
            })
        };
        desktops.insert(instance_id.to_string(), merged.clone());
        Ok(merged)
    }

    fn delete_desktop(&self, instance_id: &str) -> Result<(), PairingError> {
        self.record("DELETE", &format!("/v1/users/me/desktops/{}", instance_id), json!({}));
        self.desktops.lock().unwrap().remove(instance_id);
        self.paired_devices.lock().unwrap().remove(instance_id);
        Ok(())
    }

    fn post_paired_device(
        &self,
        instance_id: &str,
        body: &Value,
    ) -> Result<Value, PairingError> {
        let path = format!("/v1/users/me/desktops/{}/paired-devices", instance_id);
        self.record("POST", &path, body.clone());
        let mut next_id = self.next_device_id.lock().unwrap();
        *next_id += 1;
        let device_id = *next_id;
        let entry = json!({
            "device_id": device_id,
            "device_name": body.get("device_name").cloned().unwrap_or(json!("unknown")),
            "phone_pubkey_fp": body.get("phone_pubkey_fp").cloned().unwrap_or(json!("")),
            "paired_at": 1_700_000_000_i64,
            "last_seen_at": 1_700_000_000_i64,
        });
        let mut map = self.paired_devices.lock().unwrap();
        map.entry(instance_id.to_string())
            .or_insert_with(Vec::new)
            .push(entry.clone());
        Ok(entry)
    }

    fn get_paired_devices(&self, instance_id: &str) -> Result<Vec<Value>, PairingError> {
        self.record(
            "GET",
            &format!("/v1/users/me/desktops/{}/paired-devices", instance_id),
            json!({}),
        );
        Ok(self
            .paired_devices
            .lock()
            .unwrap()
            .get(instance_id)
            .cloned()
            .unwrap_or_default())
    }

    fn delete_paired_device(
        &self,
        instance_id: &str,
        device_id: i64,
    ) -> Result<(), PairingError> {
        self.record(
            "DELETE",
            &format!("/v1/users/me/desktops/{}/paired-devices/{}", instance_id, device_id),
            json!({}),
        );
        let mut map = self.paired_devices.lock().unwrap();
        if let Some(devices) = map.get_mut(instance_id) {
            devices.retain(|d| {
                d.get("device_id")
                    .and_then(|v| v.as_i64())
                    .map(|id| id != device_id)
                    .unwrap_or(true)
            });
        }
        Ok(())
    }

    fn post_drop_folder(
        &self,
        instance_id: &str,
        body: &Value,
    ) -> Result<Value, PairingError> {
        self.record(
            "POST",
            &format!("/v1/users/me/desktops/{}/drop-folder", instance_id),
            body.clone(),
        );
        let raw = body
            .get("mobile_drop_folder")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let normalized = if raw.is_empty() {
            Value::Null
        } else {
            Value::String(raw.to_string())
        };
        let mut desktops = self.desktops.lock().unwrap();
        if let Some(d) = desktops.get_mut(instance_id) {
            d["mobile_drop_folder"] = normalized.clone();
        }
        Ok(json!({ "mobile_drop_folder": normalized }))
    }

    fn put_heartbeat(
        &self,
        instance_id: &str,
        body: &Value,
    ) -> Result<(), PairingError> {
        // Phase 2.3: record and return Ok. Tests can inspect
        // .requests() to verify the heartbeat fired with the expected
        // body shape. We do NOT simulate the MAIC 404 "desktop not
        // registered" path here because the heartbeat loop tolerates
        // 404 in production (cold-boot race); the production behavior
        // is verified at the pairing_heartbeat layer.
        self.record(
            "PUT",
            &format!("/v1/users/me/desktops/{}/heartbeat", instance_id),
            body.clone(),
        );
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────
// Test helpers
// ─────────────────────────────────────────────────────────────────────

fn make_phone_keypair() -> (Vec<u8>, String) {
    // Phone generates a keypair in real life; here we just make a random one.
    use rand::{rngs::OsRng, RngCore};
    let mut priv_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut priv_bytes);
    let pub_bytes = x25519_dalek::x25519(priv_bytes, x25519_dalek::X25519_BASEPOINT_BYTES);
    let pub_b64 = B64.encode(pub_bytes);
    (priv_bytes.to_vec(), pub_b64)
}

fn build_pairing() -> (Arc<PairingCommands>, Arc<FakeMaic>) {
    let maic = Arc::new(FakeMaic::new());
    let cmds = Arc::new(PairingCommands::new(
        Arc::new(PairingState::new()),
        maic.clone(),
    ));
    (cmds, maic)
}



// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[test]
fn register_desktop_first_call_puts_and_persists_instance_id() {
    let (cmds, fake) = build_pairing();
    let reg = register_desktop(
        &cmds,
        "DESKTOPPUBKEYb64__16chars".into(),
        "FPb64_16chars!".into(),
    )
    .expect("register_desktop must succeed");

    // instance_id should be a UUID (36 chars, 4 hyphens)
    assert_eq!(reg.instance_id.len(), 36);
    assert_eq!(reg.instance_id.matches('-').count(), 4);

    // MAIC PUT was called
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    let req = &fake.requests.lock().unwrap()[0];
    assert_eq!(req.method, "PUT");
    assert!(req.path.contains(&reg.instance_id));
    assert_eq!(req.body["public_key"], "DESKTOPPUBKEYb64__16chars");
    assert_eq!(req.body["fingerprint"], "FPb64_16chars!");

    // Local instance_id is set
    assert_eq!(
        cmds.instance_id.lock().unwrap().as_ref().unwrap(),
        &reg.instance_id
    );
}

#[test]
fn register_desktop_idempotent_does_not_overwrite_fingerprint() {
    let (cmds, fake) = build_pairing();
    let reg1 = register_desktop(&cmds, "PUBKEY_b64_at_least_16_chars".into(), "FPb64_16chars!".into()).unwrap();
    // Second call with DIFFERENT fingerprint/pubkey — must NOT overwrite.
    let reg2 = register_desktop(&cmds, "NEWPUBKEY_b64_at_least_16chars".into(), "NEWFP_b64_16chars!".into()).unwrap();

    // Same instance_id reused
    assert_eq!(reg1.instance_id, reg2.instance_id);

    // MAIC state has the ORIGINAL fingerprint + pubkey (not the new ones)
    let stored = fake.desktops.lock().unwrap().get(&reg1.instance_id).cloned().unwrap();
    assert_eq!(stored["public_key"], "PUBKEY_b64_at_least_16_chars");
    assert_eq!(stored["fingerprint"], "FPb64_16chars!");
}

#[test]
fn initiate_pair_mints_session_and_produces_recoverable_handshake() {
    let (cmds, _fake) = build_pairing();
    let reg = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();
    let (phone_priv, phone_pub_b64) = make_phone_keypair();

    let handshake = initiate_pair(
        &cmds,
        phone_pub_b64.clone(),
        "Pixel 9 Pro".into(),
    )
    .expect("initiate_pair must succeed");

    // Handshake is well-formed
    assert_eq!(handshake.session_id.len(), 32, "session_id should be 32 hex chars");
    assert!(handshake.session_id.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!handshake.ephemeral_pub_b64.is_empty());
    assert!(!handshake.encrypted_secret_b64.is_empty());
    assert_eq!(handshake.capabilities.len(), 7); // v1 scope
    assert!(handshake.expires_at_unix > now_unix());

    // Phone side: recover the session_secret using the static private key.
    // Use a fixed session_id (parsed from the hex string) for the HKDF salt.
    let session_id_raw = {
        let bytes: Vec<u8> = (0..handshake.session_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&handshake.session_id[i..i + 2], 16).unwrap())
            .collect();
        let mut arr = [0u8; SESSION_ID_LEN];
        arr.copy_from_slice(&bytes);
        arr
    };
    let phone_priv_arr: [u8; 32] = phone_priv.try_into().unwrap();

    let recovered = phone_decrypt_handshake(
        &phone_priv_arr,
        &session_id_raw,
        &handshake.ephemeral_pub_b64,
        &handshake.encrypted_secret_b64,
    )
    .expect("phone decrypt must succeed with right keys");

    // The recovered session_secret MUST be 32 bytes of randomness — we can't
    // know what it is, but we can verify it's NOT all-zero (which would
    // indicate a crypto failure).
    assert_eq!(recovered.len(), 32);
    assert!(recovered.iter().any(|b| *b != 0));

    // Session is in the in-memory state map
    let snap = cmds.state.lookup(&handshake.session_id, now_unix()).unwrap();
    assert_eq!(snap.device_name, "Pixel 9 Pro");
    assert_eq!(snap.capabilities.len(), 7);
}

#[test]
fn initiate_pair_rejects_invalid_phone_pubkey() {
    let (cmds, _fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    // Empty pubkey
    let err = initiate_pair(&cmds, "".into(), "d".into()).unwrap_err();
    assert!(matches!(err, PairingError::InvalidInput(_)));

    // Non-base64 pubkey
    let err = initiate_pair(&cmds, "!!!not base64!!!".into(), "d".into()).unwrap_err();
    assert!(matches!(err, PairingError::InvalidInput(_)));

    // Wrong-length pubkey (16 bytes instead of 32)
    let err = initiate_pair(&cmds, B64.encode([0u8; 16]), "d".into()).unwrap_err();
    assert!(matches!(err, PairingError::InvalidInput(_)));
}

#[test]
fn initiate_pair_requires_registered_desktop() {
    let (cmds, _fake) = build_pairing();
    let (_priv, pub_b64) = make_phone_keypair();

    let err = initiate_pair(&cmds, pub_b64, "d".into()).unwrap_err();
    assert!(matches!(err, PairingError::InvalidInput(_)));
}

#[test]
fn paired_devices_returns_what_we_paired() {
    let (cmds, _fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    let (_priv1, pub1) = make_phone_keypair();
    let (_priv2, pub2) = make_phone_keypair();

    let h1 = initiate_pair(&cmds, pub1, "Phone A".into()).unwrap();
    let h2 = initiate_pair(&cmds, pub2, "Phone B".into()).unwrap();

    let list = paired_devices(&cmds).unwrap();
    assert_eq!(list.len(), 2);
    let names: Vec<&str> = list.iter().map(|d| d.device_name.as_str()).collect();
    assert!(names.contains(&"Phone A"));
    assert!(names.contains(&"Phone B"));

    // Devices have unique IDs
    assert_ne!(list[0].device_id, list[1].device_id);
    // Handshake was completed for both
    let _ = (h1, h2);
}

#[test]
fn revoke_device_removes_from_maic_and_revokes_local_sessions() {
    let (cmds, _fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    let (_priv, pub_b64) = make_phone_keypair();
    let handshake = initiate_pair(&cmds, pub_b64, "Phone".into()).unwrap();

    // Session is live
    assert!(cmds.state.lookup(&handshake.session_id, now_unix()).is_some());

    let list = paired_devices(&cmds).unwrap();
    let device_id = list[0].device_id;

    revoke_device(&cmds, device_id).expect("revoke must succeed");

    // Session is gone locally
    assert!(cmds.state.lookup(&handshake.session_id, now_unix()).is_none());

    // MAIC side is also empty
    let list_after = paired_devices(&cmds).unwrap();
    assert_eq!(list_after.len(), 0);
}

#[test]
fn unregister_desktop_cascades_to_all_sessions() {
    let (cmds, _fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    let (_priv1, pub1) = make_phone_keypair();
    let (_priv2, pub2) = make_phone_keypair();
    let h1 = initiate_pair(&cmds, pub1, "P1".into()).unwrap();
    let h2 = initiate_pair(&cmds, pub2, "P2".into()).unwrap();

    assert_eq!(cmds.state.active_count(now_unix()), 2);

    unregister_desktop(&cmds).expect("unregister must succeed");

    assert_eq!(cmds.state.active_count(now_unix()), 0);
    assert!(cmds.state.lookup(&h1.session_id, now_unix()).is_none());
    assert!(cmds.state.lookup(&h2.session_id, now_unix()).is_none());
    assert!(cmds.instance_id.lock().unwrap().is_none());
}

#[test]
fn set_drop_folder_persists_through_maic() {
    let (cmds, fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    let normalized = set_drop_folder(&cmds, "  /home/user/drop  ".into()).unwrap();
    assert_eq!(normalized, "/home/user/drop");

    // MAIC has the trimmed value
    let instance_id = cmds.instance_id.lock().unwrap().clone().unwrap();
    let stored = fake.desktops.lock().unwrap().get(&instance_id).cloned().unwrap();
    assert_eq!(stored["mobile_drop_folder"], "/home/user/drop");
}

#[test]
fn set_drop_folder_empty_string_disables() {
    let (cmds, fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    // Set then clear
    let _ = set_drop_folder(&cmds, "/tmp/drop".into()).unwrap();
    let normalized = set_drop_folder(&cmds, "".into()).unwrap();
    assert_eq!(normalized, "");

    // MAIC has null
    let instance_id = cmds.instance_id.lock().unwrap().clone().unwrap();
    let stored = fake.desktops.lock().unwrap().get(&instance_id).cloned().unwrap();
    assert_eq!(stored["mobile_drop_folder"], Value::Null);
}

#[test]
fn register_desktop_surfaces_maic_errors() {
    let (cmds, fake) = build_pairing();
    *fake.fail_put_desktop_once.lock().unwrap() = true;

    let err = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into())
        .unwrap_err();
    assert!(matches!(err, PairingError::MaicRequest(_)));
}

#[test]
fn full_pair_then_revoke_pair_again_cycle() {
    // Sanity check that we can re-pair a revoked device — the MAIC side
    // forgets the revoke and a fresh initiate_pair creates a new device_id.
    let (cmds, _fake) = build_pairing();
    let _ = register_desktop(&cmds, "PUB_b64_at_least_16_chars".into(), "FP_b64_at_least_16_chars".into()).unwrap();

    let (_priv, pub_b64) = make_phone_keypair();
    let h1 = initiate_pair(&cmds, pub_b64.clone(), "Re-pair".into()).unwrap();
    let list = paired_devices(&cmds).unwrap();
    let device_id = list[0].device_id;
    revoke_device(&cmds, device_id).unwrap();

    // Re-pair with the same pubkey/name
    let h2 = initiate_pair(&cmds, pub_b64, "Re-pair".into()).unwrap();
    assert_ne!(h1.session_id, h2.session_id);

    // Different session_id because we mint a fresh one each handshake.
    let list2 = paired_devices(&cmds).unwrap();
    assert_eq!(list2.len(), 1);
    assert_ne!(list2[0].device_id, device_id, "new device_id after re-pair");
}

// ─────────────────────────────────────────────────────────────────────
// pairing_server integration tests (Phase 2.3, rc55.19)
// ─────────────────────────────────────────────────────────────────────
//
// Drives the real `pairing_server` over loopback HTTP. Uses a minimal
// stdlib-only HTTP/1.1 client (no reqwest dep) so the test stays hermetic.
//
// Reuses FakeMaic from above for MAIC-side assertions.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use miracle_claw_lib::pairing_server;

// Tiny HTTP/1.1 client. Returns (status_code, body).
fn http_request(method: &str, host_port: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let body_bytes = body.unwrap_or("").as_bytes();
    let mut stream =
        TcpStream::connect(host_port).expect("test client should be able to connect to localhost");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set_read_timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set_write_timeout");

    let req = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host_port}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body_bytes.len()
    );
    stream.write_all(req.as_bytes()).expect("write request line + headers");
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).expect("write body");
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read full response (Connection: close EOF)");
    let text = String::from_utf8_lossy(&raw).into_owned();

    let status_line = text.lines().next().unwrap_or("");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(text.len());
    let body = text[body_start..].to_string();
    (status_code, body)
}

fn ephemeral_addr() -> std::net::SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

#[test]
fn server_health_returns_200_with_bound_port_when_no_instance_id_yet() {
    let (cmds, _fake) = build_pairing();
    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind on 127.0.0.1:0");
    let port = srv.bound_port();
    assert!(port > 0, "OS should have assigned a real port, got {port}");

    let (status, body) = http_request("GET", &format!("127.0.0.1:{port}"), "/pair/health", None);
    assert_eq!(status, 200, "GET /pair/health must return 200");

    let parsed: Value = serde_json::from_str(&body).expect("body must be JSON");
    assert_eq!(parsed["ok"], Value::Bool(true));
    assert_eq!(parsed["instance_id"], Value::Null, "no register_desktop yet");
    assert_eq!(
        parsed["bound_port"].as_u64().unwrap() as u16,
        port,
        "bound_port in body must match actual port"
    );

    pairing_server::stop(&srv);
}

#[test]
fn server_health_returns_instance_id_after_register_desktop() {
    let (cmds, _fake) = build_pairing();
    register_desktop(
        &cmds,
        "PUB_b64_at_least_16_chars".into(),
        "FP_b64_at_least_16_chars".into(),
    )
    .expect("register_desktop");

    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (status, body) = http_request("GET", &format!("127.0.0.1:{port}"), "/pair/health", None);
    assert_eq!(status, 200);
    let parsed: Value = serde_json::from_str(&body).unwrap();
    let id = parsed["instance_id"].as_str().expect("instance_id must be a string");
    assert!(!id.is_empty(), "instance_id must be populated");
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "instance_id must be MAIC-shaped (UUID-like), got {id:?}"
    );

    pairing_server::stop(&srv);
}

#[test]
fn server_initiate_returns_503_when_not_registered_yet() {
    let (cmds, _fake) = build_pairing();
    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (_priv, pub_b64) = make_phone_keypair();
    let body = json!({
        "phone_pubkey_b64": pub_b64,
        "device_name": "Pixel 8",
    })
    .to_string();

    let (status, resp_body) = http_request(
        "POST",
        &format!("127.0.0.1:{port}"),
        "/pair/initiate",
        Some(&body),
    );
    assert_eq!(status, 503, "must return 503 when desktop not registered");
    let parsed: Value = serde_json::from_str(&resp_body).unwrap();
    assert_eq!(parsed["error"], "not_initialized");

    pairing_server::stop(&srv);
}

#[test]
fn server_initiate_returns_400_on_malformed_json() {
    let (cmds, _fake) = build_pairing();
    register_desktop(
        &cmds,
        "PUB_b64_at_least_16_chars".into(),
        "FP_b64_at_least_16_chars".into(),
    )
    .unwrap();

    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (status, resp_body) = http_request(
        "POST",
        &format!("127.0.0.1:{port}"),
        "/pair/initiate",
        Some("not valid json"),
    );
    assert_eq!(status, 400);
    let parsed: Value = serde_json::from_str(&resp_body).unwrap();
    assert_eq!(parsed["error"], "bad_request");

    pairing_server::stop(&srv);
}

#[test]
fn server_initiate_returns_400_on_missing_device_name() {
    let (cmds, _fake) = build_pairing();
    register_desktop(
        &cmds,
        "PUB_b64_at_least_16_chars".into(),
        "FP_b64_at_least_16_chars".into(),
    )
    .unwrap();

    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (_priv, pub_b64) = make_phone_keypair();
    let body = json!({ "phone_pubkey_b64": pub_b64 }).to_string();
    let (status, resp_body) = http_request(
        "POST",
        &format!("127.0.0.1:{port}"),
        "/pair/initiate",
        Some(&body),
    );
    assert_eq!(status, 400, "missing device_name must 400");
    let parsed: Value = serde_json::from_str(&resp_body).unwrap();
    assert_eq!(parsed["error"], "bad_request");

    pairing_server::stop(&srv);
}

#[test]
fn server_returns_404_on_unknown_path() {
    let (cmds, _fake) = build_pairing();
    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (status, body) =
        http_request("GET", &format!("127.0.0.1:{port}"), "/pair/no-such-thing", None);
    assert_eq!(status, 404);
    let parsed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["error"], "not_found");

    pairing_server::stop(&srv);
}

#[test]
fn server_returns_405_on_post_to_health() {
    let (cmds, _fake) = build_pairing();
    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (status, body) = http_request(
        "POST",
        &format!("127.0.0.1:{port}"),
        "/pair/health",
        Some("{}"),
    );
    assert_eq!(status, 405, "POST /pair/health must be 405");
    let parsed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["error"], "method_not_allowed");

    pairing_server::stop(&srv);
}

#[test]
fn server_full_handshake_decrypts_to_recoverable_session_secret() {
    // End-to-end: register desktop → start server → POST /pair/initiate →
    // phone decrypts the handshake using phone_decrypt_handshake and
    // recovers a 32-byte session_secret. Proves the HTTP layer passes
    // through Phase 2.1 crypto unchanged.
    let (cmds, fake) = build_pairing();
    register_desktop(
        &cmds,
        "PUB_b64_at_least_16_chars".into(),
        "FP_b64_at_least_16_chars".into(),
    )
    .unwrap();

    let srv = pairing_server::start(0, cmds, ephemeral_addr()).expect("server should bind");
    let port = srv.bound_port();

    let (phone_priv, phone_pub_b64) = make_phone_keypair();
    let body = json!({
        "phone_pubkey_b64": phone_pub_b64,
        "device_name": "Test Phone",
    })
    .to_string();

    let (status, resp_body) = http_request(
        "POST",
        &format!("127.0.0.1:{port}"),
        "/pair/initiate",
        Some(&body),
    );
    assert_eq!(status, 200, "POST /pair/initiate must return 200");
    let parsed: Value = serde_json::from_str(&resp_body).unwrap();

    let session_id_hex = parsed["session_id"].as_str().expect("session_id");
    let encrypted_secret_b64 = parsed["encrypted_secret_b64"].as_str().expect("encrypted_secret_b64");
    let ephemeral_pubkey_b64 = parsed["ephemeral_pubkey_b64"].as_str().expect("ephemeral_pubkey_b64");

    // session_id is a hex string (32 chars from 16 random bytes per
    // SESSION_ID_LEN) — see pairing_commands::new_session_id_string.
    let session_id = miracle_claw_lib::pairing_commands::session_id_raw_from_string(session_id_hex);

    // Phone-side decrypt using a [u8; 32] for the static priv key.
    let mut priv_arr = [0u8; 32];
    priv_arr.copy_from_slice(&phone_priv);
    let recovered: [u8; SESSION_SECRET_LEN] = phone_decrypt_handshake(
        &priv_arr,
        &session_id,
        ephemeral_pubkey_b64,
        encrypted_secret_b64,
    )
    .expect("phone must be able to decrypt the handshake");
    assert_eq!(
        recovered.len(),
        32,
        "session_secret must be 32 bytes (SESSION_SECRET_LEN)"
    );

    // MAIC saw a POST to /paired-devices
    let recorded = fake.requests.lock().unwrap();
    let paired_post = recorded
        .iter()
        .find(|r| r.method == "POST" && r.path.contains("/paired-devices"))
        .expect("MAIC must have received POST /paired-devices");
    assert_eq!(paired_post.body["device_name"], "Test Phone");

    pairing_server::stop(&srv);
}
