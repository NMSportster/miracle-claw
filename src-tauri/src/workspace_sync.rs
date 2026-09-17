//! workspace_sync.rs — Background loop that consumes the MAIC workspace
//! feed and executes notes locally.
//!
//! Spec: memory/projects/workspace_architecture.md
//! MAIC backend: api/routes/workspace.py
//!
//! What this module does:
//!   1. Polls `GET /v1/users/me/workspace/poll?since_event_id=N` in a
//!      loop (long-poll, 30s server-side timeout).
//!   2. For each new event on a note we haven't processed yet, fetches
//!      the full note via `GET /v1/users/me/workspace/notes/{id}`.
//!   3. If the note's `request` subkey has `needs_approval_for: [...]`
//!      and the matching `approval` subkey isn't present with
//!      `approved: true`, we write a `needs_approval` status and skip
//!      execution.
//!   4. Otherwise we claim the note by writing `status: in_progress`,
//!      then run the "execute" step (stubbed in v1 — see
//!      `execute_note()`), then write `result: {summary, details, ...}`
//!      and `status: done`.
//!   5. Multiple-desktop safety: the executor is idempotent and writes
//!      are last-writer-wins, so two desktops racing on the same note
//!      both write result/done. The user sees the latest. (Phase 5 will
//!      add proper atomic claim via a dedicated subkey if needed.)
//!
//! Concurrency: the sync loop runs on a dedicated daemon thread spawned
//! from lib.rs::setup. The HTTP layer uses blocking ureq — fine because
//! the loop is single-threaded and most time is spent waiting on poll.
//!
//! Errors: transient MAIC errors log + back off + retry. We never panic
//! the loop. The thread survives MAIC outages indefinitely.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ─────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum WorkspaceError {
    NotLoggedIn,
    MaicRequest(String),
    MaicJson(String),
    Other(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoggedIn => write!(f, "not logged in"),
            Self::MaicRequest(s) => write!(f, "MAIC request failed: {}", s),
            Self::MaicJson(s) => write!(f, "MAIC JSON parse: {}", s),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for WorkspaceError {}

type WorkspaceResult<T> = Result<T, WorkspaceError>;

// ─────────────────────────────────────────────────────────────────────
// Shared state (Arc-wrapped so the bg thread + status() can both read)
// ─────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Shared {
    running: AtomicBool,
    last_event_id: AtomicI64,
    tasks_processed: AtomicI64,
    jwt: Mutex<String>,
}

// ─────────────────────────────────────────────────────────────────────
// WorkspaceClient
// ─────────────────────────────────────────────────────────────────────

/// Thin wrapper around ureq that knows how to hit the workspace
/// endpoints. Mirrors `LiveMaicHttp` in pairing_commands.rs.
pub struct WorkspaceClient {
    endpoint: String,
    shared: Arc<Shared>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl WorkspaceClient {
    pub fn new(endpoint: String, jwt: String) -> Self {
        let shared = Arc::new(Shared {
            running: AtomicBool::new(false),
            last_event_id: AtomicI64::new(0),
            tasks_processed: AtomicI64::new(0),
            jwt: Mutex::new(jwt),
        });
        Self {
            endpoint,
            shared,
            join: Mutex::new(None),
        }
    }

    /// Update the JWT (called after silent_relogin).
    pub fn set_jwt(&self, jwt: String) {
        if let Ok(mut guard) = self.shared.jwt.lock() {
            *guard = jwt;
        }
    }

    /// Snapshot of the current state, for the Tauri status command.
    pub fn status(&self) -> WorkspaceStatus {
        WorkspaceStatus {
            running: self.shared.running.load(Ordering::SeqCst),
            last_event_id: self.shared.last_event_id.load(Ordering::SeqCst),
            tasks_processed: self.shared.tasks_processed.load(Ordering::SeqCst),
        }
    }

    /// Spawn the sync loop on a background thread. Idempotent — calling
    /// twice is a no-op. Stores the JoinHandle so we can wait on
    /// shutdown.
    pub fn start(&self) {
        if self.shared.running.swap(true, Ordering::SeqCst) {
            // Already running.
            return;
        }
        let endpoint = self.endpoint.clone();
        let shared = Arc::clone(&self.shared);

        let handle = thread::Builder::new()
            .name("workspace-sync".into())
            .spawn(move || {
                sync_loop(endpoint, shared);
            })
            .expect("failed to spawn workspace-sync thread");

        *self.join.lock().unwrap() = Some(handle);
    }

    /// Signal the sync loop to stop and join the thread.
    pub fn stop(&self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.join.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStatus {
    pub running: bool,
    pub last_event_id: i64,
    pub tasks_processed: i64,
}

// ─────────────────────────────────────────────────────────────────────
// Sync loop (runs on its own thread)
// ─────────────────────────────────────────────────────────────────────

fn sync_loop(endpoint: String, shared: Arc<Shared>) {
    let mut backoff_ms = 500u64;

    while shared.running.load(Ordering::SeqCst) {
        let jwt_snapshot = match shared.jwt.lock() {
            Ok(g) => g.clone(),
            Err(_) => {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        if jwt_snapshot.is_empty() {
            // Not logged in. Sleep and retry.
            std::thread::sleep(Duration::from_secs(10));
            continue;
        }

        match poll_once(&endpoint, &jwt_snapshot, shared.last_event_id.load(Ordering::SeqCst)) {
            Ok(events) => {
                backoff_ms = 500; // reset backoff on success
                for ev in events {
                    shared.last_event_id.store(ev.event_id, Ordering::SeqCst);
                    if let Some(note_id) = handle_event(&endpoint, &jwt_snapshot, &ev) {
                        shared.tasks_processed.fetch_add(1, Ordering::SeqCst);
                        eprintln!("[workspace-sync] processed note {}", note_id);
                    }
                }
            }
            Err(WorkspaceError::NotLoggedIn) => {
                // JWT rejected — wait for silent_relogin to refresh it.
                std::thread::sleep(Duration::from_secs(30));
            }
            Err(e) => {
                eprintln!("[workspace-sync] poll error: {} (backing off {}ms)", e, backoff_ms);
                std::thread::sleep(Duration::from_millis(backoff_ms));
                backoff_ms = (backoff_ms * 2).min(60_000);
            }
        }
    }

    eprintln!("[workspace-sync] loop exiting");
}

/// Fetch events newer than `since_event_id`. Returns an empty Vec on
/// 204 (long-poll timeout).
fn poll_once(endpoint: &str, jwt: &str, since: i64) -> WorkspaceResult<Vec<WorkspaceEvent>> {
    let url = format!(
        "{}/v1/users/me/workspace/poll?since_event_id={}&timeout_seconds=30",
        endpoint.trim_end_matches('/'),
        since,
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(45))
        .build();
    let resp = match agent.get(&url).set("Authorization", &format!("Bearer {}", jwt)).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(204, _)) => return Ok(Vec::new()),
        Err(ureq::Error::Status(401 | 403, _)) => return Err(WorkspaceError::NotLoggedIn),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return Err(WorkspaceError::MaicRequest(format!("HTTP {}: {}", code, body)));
        }
        Err(e) => return Err(WorkspaceError::MaicRequest(e.to_string())),
    };

    let body = resp
        .into_string()
        .map_err(|e| WorkspaceError::Other(format!("read body: {}", e)))?;
    serde_json::from_str::<Vec<WorkspaceEvent>>(&body)
        .map_err(|e| WorkspaceError::MaicJson(e.to_string()))
}

/// Dispatch an event: if it's a created/updated on a subkey we care
/// about, fetch the note and try to claim + execute. Returns the note_id
/// if we processed it, None otherwise.
fn handle_event(endpoint: &str, jwt: &str, ev: &WorkspaceEvent) -> Option<String> {
    // Only react to changes on request/status/approval. Updates to
    // `result` from another source (e.g. a different desktop) don't
    // need a re-execute.
    if ev.subkey != "request" && ev.subkey != "status" && ev.subkey != "approval" {
        return None;
    }

    let note = match fetch_note(endpoint, jwt, &ev.note_id) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("[workspace-sync] fetch_note({}) failed: {}", ev.note_id, e);
            return None;
        }
    };

    // Already done? Don't re-execute. BUT if the event is on the
    // `approval` subkey and we're sitting in `needs_approval` state,
    // we need to re-evaluate — the user just approved (or denied).
    if let Some(status) = note.subkeys.get("status") {
        let state = status.value.get("state").and_then(|s| s.as_str()).unwrap_or("");
        if matches!(state, "done" | "skipped") {
            return None;
        }
        if state == "in_progress" {
            // Someone (us?) is already working on it. Skip — re-poll
            // will catch the eventual done/skipped status update.
            return None;
        }
        if state == "needs_approval" && ev.subkey != "approval" {
            // Still waiting for approval; the event isn't on the
            // approval subkey so re-evaluating won't help.
            return None;
        }
    }

    // Approval check: if request.needs_approval_for is non-empty and
    // there is no approved=true in approval, write needs_approval and bail.
    let request = note.subkeys.get("request");
    let needs_approval = request
        .and_then(|r| r.value.get("needs_approval_for"))
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    if needs_approval {
        let approval_granted = note
            .subkeys
            .get("approval")
            .and_then(|a| a.value.get("approved"))
            .and_then(|a| a.as_bool())
            .unwrap_or(false);

        if !approval_granted {
            let _ = write_subkey(
                endpoint,
                jwt,
                &ev.note_id,
                "status",
                &json!({
                    "state": "needs_approval",
                    "message": "Waiting for phone approval",
                    "required_for": request
                        .and_then(|r| r.value.get("needs_approval_for"))
                        .cloned()
                        .unwrap_or(json!([])),
                    "updated_at": now_iso8601(),
                }),
            );
            return None;
        }
    }

    // Claim: PUT status: in_progress. Last-writer-wins — concurrent
    // desktops may both claim, but execute_note() is idempotent so
    // duplicate result writes just produce identical output.
    let claim = write_subkey(
        endpoint,
        jwt,
        &ev.note_id,
        "status",
        &json!({
            "state": "in_progress",
            "claimed_by": desktop_device_id(),
            "claimed_at": now_iso8601(),
        }),
    );
    if let Err(e) = claim {
        eprintln!("[workspace-sync] claim failed for {}: {}", ev.note_id, e);
        return None;
    }

    // Execute (stubbed in v1 — real work comes in Phase 5).
    let result_value = execute_note(&note);

    // Write result + done status.
    let _ = write_subkey(endpoint, jwt, &ev.note_id, "result", &result_value);
    let _ = write_subkey(
        endpoint,
        jwt,
        &ev.note_id,
        "status",
        &json!({
            "state": "done",
            "completed_at": now_iso8601(),
        }),
    );

    Some(ev.note_id.clone())
}

/// Fetch a note (all subkeys) from MAIC.
fn fetch_note(endpoint: &str, jwt: &str, note_id: &str) -> WorkspaceResult<NoteApiResponse> {
    let url = format!(
        "{}/v1/users/me/workspace/notes/{}",
        endpoint.trim_end_matches('/'),
        note_id,
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    let resp = match agent.get(&url).set("Authorization", &format!("Bearer {}", jwt)).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(WorkspaceError::NotLoggedIn),
        Err(ureq::Error::Status(404, _)) => {
            return Err(WorkspaceError::Other("note not found".into()));
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return Err(WorkspaceError::MaicRequest(format!("HTTP {}: {}", code, body)));
        }
        Err(e) => return Err(WorkspaceError::MaicRequest(e.to_string())),
    };
    let body = resp
        .into_string()
        .map_err(|e| WorkspaceError::Other(format!("read body: {}", e)))?;
    serde_json::from_str::<NoteApiResponse>(&body)
        .map_err(|e| WorkspaceError::MaicJson(e.to_string()))
}

/// PUT one subkey.
fn write_subkey(endpoint: &str, jwt: &str, note_id: &str, subkey: &str, value: &Value) -> WorkspaceResult<()> {
    let url = format!(
        "{}/v1/users/me/workspace/notes/{}/{}",
        endpoint.trim_end_matches('/'),
        note_id,
        subkey,
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    let body = serde_json::to_string(value)
        .map_err(|e| WorkspaceError::Other(format!("serialize: {}", e)))?;
    let device_id = desktop_device_id();
    match agent
        .put(&url)
        .set("Authorization", &format!("Bearer {}", jwt))
        .set("X-Miracle-Device-Id", &format!("desktop:{}", device_id))
        .set("Content-Type", "application/json")
        .send_string(&body)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401 | 403, _)) => Err(WorkspaceError::NotLoggedIn),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Err(WorkspaceError::MaicRequest(format!("HTTP {}: {}", code, body)))
        }
        Err(e) => Err(WorkspaceError::MaicRequest(e.to_string())),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Execution stub
// ─────────────────────────────────────────────────────────────────────

/// v1 stub: pretend to "execute" the note and return a result. Real
/// execution hooks (search, file ops, MAIC tool calls, etc.) land in
/// Phase 5 once the wire is proven end-to-end.
fn execute_note(note: &NoteApiResponse) -> Value {
    let req = note.subkeys.get("request");
    let title = req
        .and_then(|r| r.value.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("(untitled)");
    let text = req
        .and_then(|r| r.value.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let kind = req
        .and_then(|r| r.value.get("kind"))
        .and_then(|t| t.as_str())
        .unwrap_or("message");

    json!({
        "summary": format!("Acknowledged: {} ({})", title, kind),
        "details": format!(
            "Desktop received the task at {}.\n\nMessage:\n{}\n\n(execution is stubbed in v1; Phase 5 wires real handlers)",
            now_iso8601(), text,
        ),
        "kind": kind,
        "completed_at": now_iso8601(),
    })
}

// ─────────────────────────────────────────────────────────────────────
// Wire types
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WorkspaceEvent {
    pub event_id: i64,
    pub note_id: String,
    pub subkey: String,
    pub event_kind: String,
}

#[derive(Debug, Deserialize)]
pub struct NoteApiResponse {
    pub note_id: String,
    pub subkeys: std::collections::HashMap<String, SubkeyApiEntry>,
}

#[derive(Debug, Deserialize)]
pub struct SubkeyApiEntry {
    pub value: Value,
    pub version: i64,
}

// ─────────────────────────────────────────────────────────────────────
// Small helpers
// ─────────────────────────────────────────────────────────────────────

fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = epoch_to_ymdhms(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

/// Civil-from-days algorithm by Howard Hinnant (public domain).
fn epoch_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let hour = (secs_of_day / 3600) as u32;
    let minute = ((secs_of_day % 3600) / 60) as u32;
    let second = (secs_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m, d, hour, minute, second)
}

/// Per-install stable device id. Reads from MAIC_DESKTOP_DEVICE_ID env
/// (set on first launch by the installer) or falls back to a hostname
/// hash. Doesn't need to be a UUID — just stable for this desktop.
fn desktop_device_id() -> String {
    if let Ok(id) = std::env::var("MAIC_DESKTOP_DEVICE_ID") {
        if !id.trim().is_empty() {
            return id;
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "miracle-claw-desktop".to_string())
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_1970_01_01() {
        let (y, m, d, h, mi, s) = epoch_to_ymdhms(0);
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_one_day() {
        let (y, m, d, h, _mi, _s) = epoch_to_ymdhms(86_400);
        assert_eq!((y, m, d), (1970, 1, 2));
    }

    #[test]
    fn epoch_now_is_recent() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (y, _m, _d, _h, _mi, _s) = epoch_to_ymdhms(secs);
        assert!(y >= 2026 && y <= 2030, "got year {}", y);
    }

    #[test]
    fn iso_format_is_utc() {
        let iso = now_iso8601();
        assert!(iso.ends_with('Z'));
        assert_eq!(iso.len(), 20);
    }
}
