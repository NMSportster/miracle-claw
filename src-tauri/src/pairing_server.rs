// Phase 2.3 — tiny_http listener for phone-side pairing requests.
//
// The phone reaches this desktop over LAN (preferred) or via the MAIC
// relay. Either way, the contract is the same:
//
//   POST /pair/initiate
//     body: {"phone_pubkey_b64": "...", "device_name": "David's Pixel 8"}
//     200:  {"session_id": "...", "encrypted_secret_b64": "...",
//            "ephemeral_pubkey_b64": "...", "instance_id": "...",
//            "expires_at": 1234567890}
//     400:  {"error": "bad_request", "detail": "..."}      (malformed body)
//     503:  {"error": "not_initialized", "detail": "..."} (no instance_id)
//
//   GET /pair/health
//     200: {"ok": true, "instance_id": "...", "bound_port": 12345}
//
//   everything else → 404 {"error": "not_found"}
//
// Design notes
// ------------
// * tiny_http is sync (one thread per connection). For pairing traffic
//   (≤ 1 connection per minute per phone, max ~10 phones per desktop) this
//   is fine — tokio + hyper would be 50ms startup + 50MB RAM for no real
//   gain.
// * The bound port is exposed via `PairingServerState::bound_port()` so
//   the heartbeat thread can advertise `endpoint = "http://<ip>:<port>"`
//   to MAIC. The phone then learns the endpoint from
//   GET /v1/users/me/desktop.
// * `stop()` is idempotent (AtomicBool check before join), so the
//   `RunEvent::Exit` handler can call it without worrying about
//   double-shutdown.
// * No new error variants. Server failures (bind errors, join errors)
//   surface as `PairingError::Internal` so callers don't need to learn
//   a new error type — except `Internal` doesn't exist either, so we
//   use `PairingError::State` (closest existing variant: "session
//   storage error"). Semantically a stretch, but it round-trips through
//   serde::Display fine.
//
// Wire compat
// -----------
// The POST body is whatever the phone sends — currently Dart Phase 3.x
// sends {phone_pubkey_b64, device_name} (matching the existing
// `initiate_pair` signature). If we add fields later (e.g. phone_fingerprint
// for the v2 trust-on-first-use flow), they're additive — server ignores
// unknown fields, `initiate_pair` is the only consumer that needs to
// change.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, ListenAddr, Response, Server, StatusCode};

use crate::pairing_commands::{PairingCommands, PairingError, PairingResult};

// Public state managed by Tauri / pairing_init. The heartbeat thread
// reads `bound_port()` to advertise the endpoint to MAIC; tests inspect
// it to confirm the OS picked a real port.

pub struct PairingServerState {
    /// Port the server actually bound to. 0 means "not bound yet".
    bound_port: AtomicU16,
    /// Set true by `stop()` to ask the handler loop to exit.
    shutdown: AtomicBool,
    /// Handler thread. `Option` because we move it out on `stop()` for a
    /// clean join.
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl PairingServerState {
    pub fn bound_port(&self) -> u16 {
        self.bound_port.load(Ordering::Acquire)
    }

    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
    }

    /// Internal — set by `start()` once the server has bound.
    fn set_bound_port(&self, port: u16) {
        self.bound_port.store(port, Ordering::Release);
    }
}

// Wire shape for POST /pair/initiate. Matches `initiate_pair`'s signature.
// Extra fields are allowed and ignored (forward-compat for v2 trust-on-first-use).

#[derive(Debug, Deserialize)]
struct InitiatePairRequest {
    phone_pubkey_b64: String,
    device_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    extra: serde_json::Value,
}

// Bind + spawn handler. `preferred_port` is a hint — pass 0 to let the OS
// pick an ephemeral port (recommended; avoids firewall dialogs).
///
/// Returns `Arc<PairingServerState>` so the caller (Tauri setup) and the
/// heartbeat thread can both hold a reference to the same state. The
/// returned Arc is what Tauri `.manage()`s (or stores in `AppState`) so
/// the `RunEvent::Exit` handler can find it for `stop()`.
///
/// `Arc::try_unwrap` would fail if anything else cloned the state between
/// `start()` and the Tauri `.manage()` call — that's by design, because
/// keeping more than one strong reference to the server state outside
/// of Arc would defeat the point of using Arc. The Arc clone in setup()
/// (for the heartbeat thread) is fine — `Arc::try_unwrap` will fail there
/// and we return the `Arc` instead.
///
/// Spec change vs. earlier draft (rc55.18 WIP): returns Arc, not by-value.
/// Reason: the heartbeat thread needs to call `bound_port()` on the same
/// state the server thread mutates; Arc-with-AtomicU16 internally already
/// provides the right concurrency, but Arc-on-the-struct lets us share
/// `stop()`'s JoinHandle too.

pub fn start(
    preferred_port: u16,
    cmds: Arc<PairingCommands>,
    bind_addr: SocketAddr,
) -> PairingResult<Arc<PairingServerState>> {
    let server = Server::http(bind_addr).map_err(|e| {
        PairingError::State(format!(
            "failed to bind pairing HTTP server on {bind_addr}: {e}"
        ))
    })?;

    // tiny_http returns ListenAddr (IP or Unix). For our purposes we
    // only bind to IP sockets (bind_addr is always SocketAddr); if we
    // somehow ended up on a Unix socket, the port we advertise to MAIC
    // would be meaningless — fall back to 0.
    let actual_port = match server.server_addr() {
        ListenAddr::IP(s) => s.port(),
        #[cfg(unix)]
        ListenAddr::Unix(_) => 0,
    };

    let state_arc = Arc::new(PairingServerState {
        bound_port: AtomicU16::new(0),
        shutdown: AtomicBool::new(false),
        handle: Mutex::new(None),
    });
    state_arc.set_bound_port(actual_port);

    let state_for_thread = Arc::clone(&state_arc);
    let handle = thread::Builder::new()
        .name("miracle-claw-pairing-http".to_string())
        .spawn(move || {
            run_handler_loop(server, cmds, state_for_thread, preferred_port);
        })
        .map_err(|e| {
            PairingError::State(format!(
                "failed to spawn pairing HTTP handler thread: {e}"
            ))
        })?;

    *state_arc.handle.lock().unwrap() = Some(handle);

    Ok(state_arc)
}

// Blocking handler loop. Each request is handled inline (tiny_http is
// sync); we don't hand off to a thread pool because pairing traffic is
// rare. The loop exits when `shutdown` flips to true OR when
// `server.recv()` errors persistently (port unbound, etc.).

fn run_handler_loop(
    server: Server,
    cmds: Arc<PairingCommands>,
    state: Arc<PairingServerState>,
    preferred_port: u16,
) {
    eprintln!(
        "[miracle-claw] pairing HTTP server listening on {} (preferred={})",
        server.server_addr(),
        preferred_port
    );

    loop {
        if state.shutdown.load(Ordering::Acquire) {
            break;
        }

        // recv_timeout lets us check the shutdown flag without busy-looping.
        // 1 second granularity is fine for a clean shutdown.
        let mut request = match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(req)) => req,
            Ok(None) => continue, // timeout — loop back, check shutdown
            Err(e) => {
                eprintln!("[miracle-claw] pairing HTTP server recv error: {e}");
                // Don't tight-loop on persistent errors.
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        };

        let url = request.url().to_string();
        let method_str = request.method().as_str().to_string();

        let response = match (method_str.as_str(), url.as_str()) {
            ("GET", "/pair/health") => handle_health(&cmds, &state),
            ("POST", "/pair/health") => json_error(
                StatusCode(405),
                "method_not_allowed",
                "POST not supported on /pair/health (use GET)",
            ),
            ("POST", "/pair/initiate") => match read_json_body(&mut request) {
                Ok(body) => handle_initiate(&cmds, body),
                Err(resp) => resp,
            },
            ("GET", "/pair/initiate") => json_error(
                StatusCode(405),
                "method_not_allowed",
                "GET not supported on /pair/initiate (use POST)",
            ),
            _ => json_error(
                StatusCode(404),
                "not_found",
                &format!("no such endpoint: {method_str} {url}"),
            ),
        };

        if let Err(e) = request.respond(response) {
            eprintln!("[miracle-claw] pairing HTTP server respond error: {e}");
        }
    }

    eprintln!("[miracle-claw] pairing HTTP server shutting down");
}

// GET /pair/health — quick liveness probe for the phone's pre-handshake
// check. Returns the instance_id (or null if not yet registered) and the
// port we bound to.

fn handle_health(
    cmds: &PairingCommands,
    state: &PairingServerState,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let instance_id = cmds
        .instance_id
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();

    let body = json!({
        "ok": true,
        "instance_id": if instance_id.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(instance_id)
        },
        "bound_port": state.bound_port(),
    });

    json_response(StatusCode(200), body)
}

// POST /pair/initiate — the actual handshake. Hands off to the existing
// `initiate_pair` (Phase 2.2), which already does the crypto. We just
// adapt HTTP/JSON plumbing.

fn handle_initiate(
    cmds: &PairingCommands,
    body: serde_json::Value,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let req: InitiatePairRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return json_error(
                StatusCode(400),
                "bad_request",
                &format!("invalid /pair/initiate body: {e}"),
            );
        }
    };

    match crate::pairing_commands::initiate_pair(cmds, req.phone_pubkey_b64, req.device_name) {
        Ok(resp) => {
            let body = json!({
                "session_id": resp.session_id,
                "encrypted_secret_b64": resp.encrypted_secret_b64,
                "ephemeral_pubkey_b64": resp.ephemeral_pub_b64,
                "capabilities": resp.capabilities,
                "expires_at": resp.expires_at_unix,
            });
            json_response(StatusCode(200), body)
        }
        Err(PairingError::InvalidInput(detail)) => {
            // Distinguish "you haven't registered yet" (503) from "your
            // input was bad" (400). The existing `require_instance_id()`
            // surfaces as InvalidInput with this exact prefix.
            if detail.starts_with("desktop is not registered") {
                json_error(
                    StatusCode(503),
                    "not_initialized",
                    "desktop has not registered an instance yet — call mc_register_desktop first",
                )
            } else {
                json_error(StatusCode(400), "bad_request", &detail)
            }
        }
        Err(PairingError::Crypto(detail)) => {
            json_error(StatusCode(400), "crypto_error", &detail)
        }
        Err(PairingError::NotLoggedIn) => json_error(
            StatusCode(503),
            "not_initialized",
            "desktop is not logged in to MAIC — JWT missing",
        ),
        Err(e) => json_error(
            StatusCode(500),
            "internal_error",
            &format!("{e}"),
        ),
    }
}

// Body reading: tiny_http doesn't auto-decode JSON, so we slurp the body,
// check Content-Length-ish sanity, and hand the JSON to the handler.

fn read_json_body(
    request: &mut tiny_http::Request,
) -> Result<serde_json::Value, Response<std::io::Cursor<Vec<u8>>>> {
    use std::io::Read;

    // 64 KiB cap — pairing requests are tiny (pubkey + name + small JSON
    // overhead). Anything bigger is either a misconfiguration or a probe;
    // either way, 400.
    const MAX_BODY_BYTES: usize = 64 * 1024;

    let mut body = Vec::new();
    let mut limited = request.as_reader().take(MAX_BODY_BYTES as u64);
    if let Err(e) = limited.read_to_end(&mut body) {
        return Err(json_error(
            StatusCode(400),
            "bad_request",
            &format!("failed to read body: {e}"),
        ));
    }

    serde_json::from_slice(&body).map_err(|e| {
        json_error(
            StatusCode(400),
            "bad_request",
            &format!("body is not valid JSON: {e}"),
        )
    })
}

fn json_response(
    status: StatusCode,
    body: serde_json::Value,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => {
            // We couldn't serialize our own response — bail with a minimal
            // error body. This is a programmer bug, not a request bug.
            return Response::from_string(format!(
                "{{\"error\":\"internal_serialize_failed\",\"detail\":\"{e}\"}}"
            ))
            .with_status_code(StatusCode(500));
        }
    };
    Response::from_data(bytes)
        .with_status_code(status)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .unwrap(),
        )
}

fn json_error(
    status: StatusCode,
    code: &str,
    detail: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(status, json!({ "error": code, "detail": detail }))
}

// Stop the server. Idempotent. Joins the handler thread on first call.
// Safe to call from `RunEvent::Exit` even if `start()` failed earlier.

pub fn stop(state: &PairingServerState) {
    if state.shutdown.swap(true, Ordering::AcqRel) {
        return; // already stopping
    }
    let handle = state.handle.lock().unwrap().take();
    if let Some(h) = handle {
        // Tiny_http's Server has no explicit close — drop is the contract.
        // The handler loop is blocked in `recv_timeout(1s)`, so it will
        // see the shutdown flag within ~1s of `stop()` returning.
        // Cap the join at 5s to avoid hanging app shutdown if something
        // is wedged. We run the join on a side thread and sleep on the
        // main one; if the join thread is wedged past 5s, the OS will
        // reap it on process exit anyway.
        let join_thread_name = "miracle-claw-pairing-http-join".to_string();
        if let Ok(jh) = thread::Builder::new()
            .name(join_thread_name)
            .spawn(move || {
                let _ = h.join();
            })
        {
            thread::sleep(Duration::from_secs(5));
            let _ = jh.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing_commands::PairingCommands;
    use crate::pairing_state::PairingState;

    /// Bare-minimum PairingCommands for handler unit tests. We never call
    /// `initiate_pair` here — only exercise the 400/404 paths that don't
    /// touch the instance state. Real handshake tests live in
    /// `tests/pairing_server_test.rs` (use the FakeMaicHttp from
    /// pairing_commands_test).
    fn fake_pairing_commands() -> Arc<PairingCommands> {
        // PairingCommands::new requires MaicHttp — we use a dummy empty
        // impl. Unit tests here never invoke any method on it.
        Arc::new(PairingCommands::new(
            Arc::new(PairingState::new()),
            Arc::new(DummyMaic),
        ))
    }

    struct DummyMaic;
    impl crate::pairing_commands::MaicHttp for DummyMaic {
        fn put_desktop(
            &self,
            _instance_id: &str,
            _body: &serde_json::Value,
        ) -> PairingResult<serde_json::Value> {
            unimplemented!()
        }
        fn delete_desktop(&self, _instance_id: &str) -> PairingResult<()> {
            unimplemented!()
        }
        fn post_paired_device(
            &self,
            _instance_id: &str,
            _body: &serde_json::Value,
        ) -> PairingResult<serde_json::Value> {
            unimplemented!()
        }
        fn get_paired_devices(
            &self,
            _instance_id: &str,
        ) -> PairingResult<Vec<serde_json::Value>> {
            unimplemented!()
        }
        fn delete_paired_device(
            &self,
            _instance_id: &str,
            _device_id: i64,
        ) -> PairingResult<()> {
            unimplemented!()
        }
        fn post_drop_folder(
            &self,
            _instance_id: &str,
            _body: &serde_json::Value,
        ) -> PairingResult<serde_json::Value> {
            unimplemented!()
        }
        fn put_heartbeat(
            &self,
            _instance_id: &str,
            _body: &serde_json::Value,
        ) -> PairingResult<()> {
            unimplemented!()
        }
    }

    #[test]
    fn initiate_request_requires_two_fields() {
        let bad = json!({"phone_pubkey_b64": "abc"});
        let parsed: Result<InitiatePairRequest, _> = serde_json::from_value(bad);
        assert!(parsed.is_err(), "missing device_name must fail to parse");

        let good = json!({
            "phone_pubkey_b64": "abc",
            "device_name": "Test Phone",
        });
        let parsed: InitiatePairRequest = serde_json::from_value(good).unwrap();
        assert_eq!(parsed.phone_pubkey_b64, "abc");
        assert_eq!(parsed.device_name, "Test Phone");
    }

    #[test]
    fn initiate_request_tolerates_extra_fields() {
        let with_extra = json!({
            "phone_pubkey_b64": "abc",
            "device_name": "Test Phone",
            "phone_fingerprint": "deadbeef", // future v2 field
            "protocol_version": 2,
        });
        let parsed: InitiatePairRequest = serde_json::from_value(with_extra).unwrap();
        assert_eq!(parsed.phone_pubkey_b64, "abc");
    }

    #[test]
    fn health_response_shape() {
        let cmds = fake_pairing_commands();
        let state = PairingServerState {
            bound_port: AtomicU16::new(37241),
            shutdown: AtomicBool::new(false),
            handle: Mutex::new(None),
        };
        let resp = handle_health(&cmds, &state);
        assert_eq!(
            resp.status_code(),
            StatusCode(200),
            "GET /pair/health must be 200"
        );
    }

    #[test]
    fn unknown_route_returns_404() {
        let resp = json_error(
            StatusCode(404),
            "not_found",
            "no such endpoint: GET /pair/nope",
        );
        assert_eq!(resp.status_code(), StatusCode(404));
    }

    #[test]
    fn health_includes_bound_port() {
        let state = PairingServerState {
            bound_port: AtomicU16::new(51523),
            shutdown: AtomicBool::new(false),
            handle: Mutex::new(None),
        };
        assert_eq!(state.bound_port(), 51523);
    }

    #[test]
    fn stop_is_idempotent() {
        let state = PairingServerState {
            bound_port: AtomicU16::new(0),
            shutdown: AtomicBool::new(false),
            handle: Mutex::new(None),
        };
        // First call: should set shutdown, but no handle to join (None).
        stop(&state);
        assert!(state.shutdown.load(Ordering::Acquire));
        // Second call: must not panic.
        stop(&state);
    }
}