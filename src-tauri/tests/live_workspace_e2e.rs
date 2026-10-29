//! Live end-to-end test for the **shipped** desktop workspace sync module.
//!
//! 2026-10-29 (Office Bot): `live_workspace_claim.rs` validated the MAIC
//! *contract* — it re-implemented the claim → execute → finish sequence
//! with its own HTTP calls. That proves the server accepts the payloads,
//! but it does NOT prove the code that actually ships in the app
//! (`workspace_sync::WorkspaceClient`) performs that sequence correctly.
//!
//! This test drives the real module: it constructs a `WorkspaceClient`,
//! calls the public `start()`, waits for the background loop to pick up a
//! throwaway note, and asserts the note reached `status: done` with a
//! `result` subkey written by the module's own `execute_note()`.
//!
//! **The test owns its own note.** It creates a throwaway note with a
//! client-generated UUID and deletes it afterwards. It never touches a
//! real user note — see the corruption incident described in
//! `live_workspace_sync.rs`.
//!
//! MAIC has no POST for notes. Creation is a batch write:
//!   PUT /v1/users/me/workspace/notes/{note_id}
//!   body: {"subkeys": {"request": {...}}}
//! with a client-generated UUID note_id.
//!
//! Skipped automatically unless `MAIC_API_KEY` and `MAIC_BASE` are set.
//!
//! Run:
//!   source ~/.config/secrets/maic.env
//!   cargo test --test live_workspace_e2e -- --test-threads=1 --nocapture

use std::time::{Duration, Instant};

use miracle_claw_lib::workspace_sync::WorkspaceClient;
use serde_json::json;

fn maic_key() -> Option<String> {
    std::env::var("MAIC_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn maic_base() -> Option<String> {
    std::env::var("MAIC_BASE")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn auth_header() -> String {
    format!("Bearer {}", maic_key().expect("MAIC_API_KEY"))
}

/// Mirrors the desktop's `X-Miracle-Device-Id` header. MAIC's
/// `DEVICE_ID_PATTERN` is `^[a-zA-Z0-9_:.@-]{1,64}$`.
fn device_header() -> String {
    "desktop:live_workspace_e2e_test".to_string()
}

fn maic_get(url: &str) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    match agent.get(url).set("Authorization", &auth_header()).call() {
        Ok(resp) => {
            let code = resp.status();
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(e) => panic!("GET {url} failed: {e}"),
    }
}

fn maic_put(url: &str, body: &serde_json::Value) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let payload = body.to_string();
    match agent
        .put(url)
        .set("Authorization", &auth_header())
        .set("X-Miracle-Device-Id", &device_header())
        .set("Content-Type", "application/json")
        .send_string(&payload)
    {
        Ok(resp) => {
            let code = resp.status();
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(e) => panic!("PUT {url} failed: {e}"),
    }
}

fn maic_delete(url: &str) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    match agent
        .delete(url)
        .set("Authorization", &auth_header())
        .set("X-Miracle-Device-Id", &device_header())
        .call()
    {
        Ok(resp) => {
            let code = resp.status();
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            (code, body)
        }
        Err(e) => panic!("DELETE {url} failed: {e}"),
    }
}

/// Read the `status` subkey value for a note, or None if absent.
///
/// MAIC's `GET /notes/{id}/{subkey}` returns a WRAPPER:
///   {"subkey": "status", "version": N, "value": {...}}
/// The interesting payload is under `value`. Lesson: reading the wrapper
/// directly (as an earlier version of this test did) makes every check
/// silently see an empty state — which is how a real note got
/// overwritten while a test "passed" against it.
fn read_status(base: &str, note_id: &str) -> Option<serde_json::Value> {
    let (code, body) = maic_get(&format!("{base}/v1/users/me/workspace/notes/{note_id}/status"));
    if code != 200 {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    parsed.get("value").cloned()
}

fn read_result(base: &str, note_id: &str) -> Option<serde_json::Value> {
    let (code, body) = maic_get(&format!("{base}/v1/users/me/workspace/notes/{note_id}/result"));
    if code != 200 {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    parsed.get("value").cloned()
}

/// The real module drives the whole sequence. This asserts it.
#[test]
fn shipped_workspace_client_claims_executes_and_finishes_a_note() {
    let (Some(key), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    // ── 1. Start the REAL sync loop FIRST. ────────────────────────────
    // Order matters. The shipped client seeds its event cursor to the
    // current end of the event log on start(), so a fresh desktop only
    // picks up notes created AFTER it launched. That is the intended
    // "phone sends a task, desktop picks it up" flow, and it is also
    // what stops a fresh install from replaying (and re-executing) the
    // user's entire history.
    let client = WorkspaceClient::new(base.clone(), key.clone());
    client.start();

    assert!(
        client.status().running,
        "client did not report running after start()"
    );
    eprintln!(
        "cursor seeded to event {}",
        client.status().last_event_id
    );

    // ── 2. Create a throwaway note the way the phone does. ─────────────
    let note_id = uuid::Uuid::new_v4().to_string();
    let (code, body) = maic_put(
        &format!("{base}/v1/users/me/workspace/notes/{note_id}"),
        &json!({
            "subkeys": {
                "request": {
                    "title": "live_workspace_e2e probe",
                    "text": "driven by the shipped WorkspaceClient, not a re-implementation",
                    "kind": "standing_instruction",
                }
            }
        }),
    );
    assert!(
        code == 200 || code == 201,
        "note create failed: HTTP {code}: {body}"
    );
    eprintln!("created throwaway note {note_id}");

    // Always clean up, even on assertion failure.
    let cleanup = || {
        let (c, b) = maic_delete(&format!("{base}/v1/users/me/workspace/notes/{note_id}"));
        eprintln!("cleanup note {note_id}: HTTP {c} {b}");
    };

    // ── 3. Wait for the loop to finish OUR note. ──────────────────────
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut final_status: Option<serde_json::Value> = None;
    let mut final_result: Option<serde_json::Value> = None;

    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
        let st = read_status(&base, &note_id);
        if let Some(ref s) = st {
            let state = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
            eprintln!("[{note_id}] status.state = {state:?}");
            if state == "done" {
                final_status = st;
                final_result = read_result(&base, &note_id);
                break;
            }
            if state == "needs_approval" || state == "skipped" {
                panic!("unexpected terminal status {state:?} for {note_id}");
            }
        }
    }

    client.stop();
    cleanup();

    let status = final_status.expect(&format!(
        "shipped WorkspaceClient did not write status:done for {note_id} within 120s"
    ));
    let result = final_result.expect("status was done but no result subkey was written");

    assert_eq!(
        status.get("state").and_then(|v| v.as_str()),
        Some("done"),
        "final status was not done: {status}"
    );

    // The result must carry the fields the phone renders.
    let summary = result.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !summary.is_empty(),
        "result.summary was empty: {result}"
    );
    assert!(
        result.get("details").and_then(|v| v.as_str()).is_some(),
        "result.details was missing: {result}"
    );
    assert!(
        result.get("completed_at").and_then(|v| v.as_str()).is_some(),
        "result.completed_at was missing: {result}"
    );

    eprintln!("summary: {summary}");
    eprintln!("kind: {:?}", result.get("kind"));
    eprintln!(
        "tasks_processed by the loop: {}",
        client.status().tasks_processed
    );

    assert!(
        client.status().tasks_processed > 0,
        "loop reported zero tasks processed"
    );

    eprintln!("PASS: shipped WorkspaceClient claimed, executed, and finished {note_id}");
}

/// Regression guard for the 2026-10-29 data-corruption incident.
///
/// A note that ALREADY has a `result` subkey has already been executed.
/// The daily_summary cron writes `request` + `result` and NEVER writes a
/// `status` subkey — so the original status-only guard ("skip if status
/// is done/skipped") did not protect it. A fresh desktop replaying the
/// event history claimed the note and overwrote the real recap with the
/// stub payload.
///
/// This test creates a note that looks exactly like a cron-written one
/// (request + result, NO status), then runs the shipped client and
/// asserts the existing result is untouched.
#[test]
fn shipped_client_does_not_overwrite_an_existing_result() {
    let (Some(key), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    // Start the client first so its cursor is seeded to "now" and it
    // sees this note's events.
    let client = WorkspaceClient::new(base.clone(), key.clone());
    client.start();

    let note_id = uuid::Uuid::new_v4().to_string();
    let sentinel = "ORIGINAL RESULT — must not be overwritten";
    let (code, body) = maic_put(
        &format!("{base}/v1/users/me/workspace/notes/{note_id}"),
        &json!({
            "subkeys": {
                "request": {
                    "title": "guard probe",
                    "text": "pre-existing result",
                    "kind": "daily_summary",
                    "needs_approval_for": [],
                },
                "result": {
                    "kind": "daily_summary",
                    "summary": sentinel,
                    "details": sentinel,
                }
            }
        }),
    );
    assert!(
        code == 200 || code == 201,
        "note create failed: HTTP {code}: {body}"
    );
    eprintln!("created guard probe note {note_id}");

    let cleanup = || {
        let (c, b) = maic_delete(&format!("{base}/v1/users/me/workspace/notes/{note_id}"));
        eprintln!("cleanup note {note_id}: HTTP {c} {b}");
    };

    // Give the loop ample time to notice and (wrongly) reprocess it.
    std::thread::sleep(Duration::from_secs(25));

    let result = read_result(&base, &note_id);
    let status = read_status(&base, &note_id);
    client.stop();
    cleanup();

    let result = result.expect("result subkey vanished — the note was destroyed");
    let got = result.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        got, sentinel,
        "the shipped client OVERWROTE an existing result: {result}"
    );
    // And it must not have written a status it wasn't asked to write.
    if let Some(s) = status {
        let state = s.get("state").and_then(|v| v.as_str()).unwrap_or("");
        assert_ne!(
            state, "done",
            "client claimed a note that already had a result: {s}"
        );
    }

    eprintln!("PASS: existing result preserved on {note_id}");
}
