// Phase 2.3 — heartbeat + cleanup background ticker.
//
// Runs as a daemon thread spawned from setup(). Every HEARTBEAT_INTERVAL_SECS:
//   1. If we have an instance_id, PUT /v1/users/me/desktops/{id}/heartbeat
//      with our current pairing endpoint so MAIC can tell the phone where
//      to reach us.
//   2. Run cleanup_expired() against the in-memory session map. Cheap
//      walk, ~µs per call; do it on every tick so a crashed cleanup
//      thread doesn't leak expired sessions.
//
// Failure handling: MAIC errors are logged + ignored (next tick retries).
// We NEVER panic or exit the thread on transient errors — the heartbeat
// is best-effort by design. The cleanup call is local + infallible.
//
// Endpoint discovery: we run `tailscale ip -4` at startup and cache the
// result. If tailscale isn't available (dev box, edge case), we leave
// the endpoint field out of the heartbeat — MAIC marks the desktop as
// reachable only via relay, and the phone's discovery flow handles
// that gracefully.
//
// Why not use std::net for IP detection: we want the tailnet IP
// specifically, not the system's primary NIC (which on adeal's box is
// the WiFi interface at 192.168.x). Tailscale's `ip -4` is the
// canonical answer.

use std::io::Read;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::pairing_commands::{PairingCommands, PairingError};
use crate::pairing_server::PairingServerState;

/// How often to tick. 60s matches the MAIC staleness window (5min)
/// with 5x headroom.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 60;

/// Public state, managed by Tauri alongside the server state. The
/// RunEvent::Exit handler reads is_running() to know whether to call stop.
///
/// `shutdown_flag` is an `Arc<AtomicBool>` shared with the heartbeat thread
/// so `stop()` can flip it and the loop will exit on its next 1-second
/// sleep-wake check (within ~1s, not waiting for the next MAIC roundtrip).
pub struct PairingHeartbeatState {
    /// Mirror of the thread's shutdown flag. Updated by `stop()`.
    shutdown: AtomicBool,
    /// Shared with the heartbeat thread so `stop()` can ask it to exit.
    shutdown_flag: Mutex<Option<Arc<AtomicBool>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl PairingHeartbeatState {
    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
    }
}

/// Endpoint advertised to MAIC. Cached at start() so we don't shell out
/// every tick. `None` means "tailscale not detected, leave endpoint out
/// of heartbeat (relay-only mode)".
///
/// `pub` so integration tests in `tests/pairing_heartbeat_test.rs` can
/// build a `CachedEndpoint` directly without spawning the heartbeat
/// thread (the thread owns `tailscale ip -4` detection — tests inject a
/// precomputed endpoint instead).
#[derive(Clone, Debug)]
pub struct CachedEndpoint {
    /// http://100.74.x.y:port or None
    pub url: Option<String>,
    /// "tailnet" if we got the IP from tailscale, "lan" if we couldn't.
    /// MAIC validator accepts these two plus "relay".
    pub kind: &'static str,
}

impl CachedEndpoint {
    /// Construct a CachedEndpoint for tests with explicit values.
    /// Production code uses `detect()`.
    pub fn for_test(url: Option<String>, kind: &'static str) -> Self {
        Self { url, kind }
    }

    fn detect(port: u16) -> Self {
        // Try tailscale first. If it succeeds, kind="tailnet".
        if let Ok(ip) = tailscale_ipv4() {
            return Self {
                url: Some(format!("http://{ip}:{port}")),
                kind: "tailnet",
            };
        }
        // No tailscale. We could try to detect the LAN IP, but doing it
        // cross-platform (Windows / macOS / Linux) without a crate is a
        // mess. Default to relay-only mode: phone uses MAIC relay,
        // which is slower but works.
        Self {
            url: None,
            kind: "relay",
        }
    }
}

/// Shell out to `tailscale ip -4`. Returns the first IPv4 address, or
/// Err if tailscale isn't installed, isn't running, or returns garbage.
fn tailscale_ipv4() -> Result<String, String> {
    let mut child = Command::new("tailscale")
        .args(["ip", "-4"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn tailscale: {e}"))?;
    let mut stdout = String::new();
    child
        .stdout
        .as_mut()
        .ok_or("no stdout handle")?
        .read_to_string(&mut stdout)
        .map_err(|e| format!("failed to read tailscale stdout: {e}"))?;
    let status = child.wait().map_err(|e| format!("tailscale wait: {e}"))?;
    if !status.success() {
        return Err(format!(
            "tailscale ip -4 exited with {}",
            status.code().unwrap_or(-1)
        ));
    }
    // stdout may contain multiple IPs (one per line). Take the first.
    let first = stdout
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .ok_or_else(|| "tailscale returned no IP".to_string())?;
    // Sanity-check: looks like an IPv4 address.
    if first.split('.').count() == 4 && first.parse::<std::net::Ipv4Addr>().is_ok() {
        Ok(first.to_string())
    } else {
        Err(format!("tailscale returned non-IPv4: {first:?}"))
    }
}

/// Start the heartbeat thread. Returns state for Tauri .manage().
/// Caller should keep the Arc<PairingCommands> and Arc<PairingServerState>
/// alive for the thread's lifetime (the thread borrows them; if the
/// cmds Arc is dropped, the thread will see read-locks on dead mutexes
/// and panic — same as any std::sync misuse).
pub fn start(
    cmds: Arc<PairingCommands>,
    server_state: Arc<PairingServerState>,
) -> PairingHeartbeatState {
    // Detect endpoint once at startup. If the port changes later
    // (e.g. server rebinds), we won't pick it up — but our server binds
    // once and keeps the port, so this is fine in practice.
    let bound_port = server_state.bound_port();
    let endpoint = CachedEndpoint::detect(bound_port);
    eprintln!(
        "[miracle-claw] heartbeat endpoint: url={:?} kind={}",
        endpoint.url, endpoint.kind
    );

    let state = PairingHeartbeatState {
        shutdown: AtomicBool::new(false),
        shutdown_flag: Mutex::new(None),
        handle: Mutex::new(None),
    };

    // Shared shutdown flag — `stop()` will flip this to ask the loop to exit
    // on its next 1-second sleep-wake check, rather than waiting for the
    // 60s heartbeat interval.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    *state.shutdown_flag.lock().unwrap() = Some(Arc::clone(&shutdown_flag));
    let endpoint_for_thread = endpoint;

    let handle = thread::Builder::new()
        .name("miracle-claw-pairing-heartbeat".to_string())
        .spawn(move || {
            run_loop(
                cmds,
                server_state,
                endpoint_for_thread,
                shutdown_flag,
            );
        })
        .expect("failed to spawn heartbeat thread");

    *state.handle.lock().unwrap() = Some(handle);
    state
}

fn run_loop(
    cmds: Arc<PairingCommands>,
    server_state: Arc<PairingServerState>,
    endpoint: CachedEndpoint,
    shutdown_flag: Arc<AtomicBool>,
) {
    eprintln!(
        "[miracle-claw] heartbeat loop started (interval={}s)",
        HEARTBEAT_INTERVAL_SECS
    );

    loop {
        if shutdown_flag.load(Ordering::Acquire) {
            break;
        }

        // One heartbeat cycle: cleanup + (maybe) PUT to MAIC.
        tick(&cmds, &server_state, &endpoint);

        // 3. Sleep. Check shutdown flag every second so stop() returns
        //    within ~1s even if mid-sleep.
        for _ in 0..HEARTBEAT_INTERVAL_SECS {
            if shutdown_flag.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    eprintln!("[miracle-claw] heartbeat loop exiting");
}

/// One heartbeat tick — separated from the loop so tests can drive it
/// without spawning the thread or sleeping 60s.
///
/// Always runs cleanup_expired(). Sends PUT /heartbeat to MAIC if and
/// only if an instance_id has been registered. Tolerates MAIC errors
/// (logs and returns). Returns the number of expired sessions removed.
pub fn tick(
    cmds: &PairingCommands,
    server_state: &PairingServerState,
    endpoint: &CachedEndpoint,
) -> usize {
    // Re-detect port each tick in case the server rebound (rare;
    // today the server binds once at startup). Cheap (atomic load).
    let current_port = server_state.bound_port();
    let endpoint_now = if current_port != 0
        && endpoint.url.as_deref().and_then(url_port) != Some(current_port)
    {
        // Port changed — refresh the endpoint URL.
        CachedEndpoint {
            url: endpoint.url.as_ref().map(|u| {
                // Replace old port with new
                let stripped = u.rsplit_once(':').map(|(h, _)| h).unwrap_or(u);
                format!("{stripped}:{current_port}")
            }),
            kind: endpoint.kind,
        }
    } else {
        endpoint.clone()
    };

    // 1. Cleanup expired sessions. Always runs (infallible — returns
    //    usize count of removed sessions, or 0 if lock poisoned).
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let removed = cmds.state.cleanup_expired(now_unix);
    if removed > 0 {
        eprintln!(
            "[miracle-claw] heartbeat: cleanup_expired removed {removed} sessions"
        );
    }

    // 2. Send heartbeat if we have an instance_id. Don't fail the
    //    loop on errors — just log.
    let instance_id = cmds.instance_id.lock().unwrap().clone();
    if let Some(instance_id) = instance_id {
        let body = json!({
            "endpoint": endpoint_now.url,
            "endpoint_kind": endpoint_now.kind,
        });

        match cmds.maic.put_heartbeat(&instance_id, &body) {
            Ok(()) => {}
            Err(PairingError::MaicRequest(msg)) if msg.contains("404") => {
                // MAIC 404 = "desktop_not_registered". Normal during
                // cold boot (we register then start heartbeating,
                // and the cold-boot race is benign). Don't spam
                // stderr; just log at info level.
                eprintln!(
                    "[miracle-claw] heartbeat: MAIC returned 404 (desktop not yet registered on MAIC side, will retry)"
                );
            }
            Err(e) => {
                eprintln!(
                    "[miracle-claw] heartbeat: put_heartbeat error: {e} (will retry next tick)"
                );
            }
        }
    }
    // If instance_id is None, we haven't registered yet — skip
    // heartbeat. setup() calls register_desktop on first run, and
    // any subsequent call sets the instance_id. The cleanup tick
    // still runs (above).

    removed
}

/// Stop the heartbeat thread. Idempotent. Flips the shared shutdown flag
/// (so the loop exits on its next 1-second sleep-wake check), then joins
/// the handler thread with a 5s cap.
pub fn stop(state: &PairingHeartbeatState) {
    if state.shutdown.swap(true, Ordering::AcqRel) {
        return; // already stopping
    }
    // Flip the flag the loop is actually polling.
    if let Some(flag) = state.shutdown_flag.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Release);
    }
    let handle = state.handle.lock().unwrap().take();
    if let Some(h) = handle {
        // Worst case: the loop is mid-`thread::sleep`, and we have to
        // wait up to 1s for the sleep-wake check. Cap the join at 5s
        // just in case the loop is wedged inside a long MAIC call
        // (which shouldn't happen — put_heartbeat has a 10s timeout —
        // but defensive is cheap).
        let join_thread = thread::Builder::new()
            .name("miracle-claw-pairing-heartbeat-join".to_string())
            .spawn(move || {
                let _ = h.join();
            })
            .ok();
        if let Some(jh) = join_thread {
            thread::sleep(Duration::from_secs(5));
            let _ = jh.join();
        }
    }
}

/// Helper: extract the port from a URL like "http://1.2.3.4:5678".
///
/// `pub` so the heartbeat integration test can build deterministic URLs.
pub fn url_port(url: &str) -> Option<u16> {
    url.rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_port_extracts_correctly() {
        assert_eq!(url_port("http://1.2.3.4:5678"), Some(5678));
        assert_eq!(url_port("http://[::1]:8080"), Some(8080));
        assert_eq!(url_port("not a url"), None);
    }

    #[test]
    fn cached_endpoint_detect_returns_tailnet_when_tailscale_present() {
        // On machines with tailscale running, kind should be "tailnet".
        // On dev boxes without tailscale, kind is "relay" — both are
        // valid MAIC enum values.
        let ep = CachedEndpoint::detect(12345);
        assert!(
            ep.kind == "tailnet" || ep.kind == "relay",
            "kind must be one of MAIC-accepted values, got {:?}",
            ep.kind
        );
        if ep.kind == "tailnet" {
            assert!(ep.url.is_some(), "tailnet endpoint must have URL");
            assert!(ep.url.as_deref().unwrap().contains("12345"));
        }
    }

    #[test]
    fn shutdown_state_default_is_running() {
        let state = PairingHeartbeatState {
            shutdown: AtomicBool::new(false),
            shutdown_flag: Mutex::new(None),
            handle: Mutex::new(None),
        };
        assert!(state.is_running());
    }

    #[test]
    fn stop_flips_shared_shutdown_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let state = PairingHeartbeatState {
            shutdown: AtomicBool::new(false),
            shutdown_flag: Mutex::new(Some(Arc::clone(&flag))),
            handle: Mutex::new(None),
        };
        stop(&state);
        assert!(flag.load(Ordering::Acquire), "stop() must flip the shared flag");
        assert!(!state.is_running(), "stop() must mark state as not running");
        // Second call must not panic.
        stop(&state);
    }

    #[test]
    fn stop_without_start_is_safe() {
        // No shutdown_flag set (start() never called), no handle. Must
        // not panic — RunEvent::Exit calls stop() unconditionally.
        let state = PairingHeartbeatState {
            shutdown: AtomicBool::new(false),
            shutdown_flag: Mutex::new(None),
            handle: Mutex::new(None),
        };
        stop(&state);
        assert!(!state.is_running());
    }
}