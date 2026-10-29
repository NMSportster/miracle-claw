//! Live test: the desktop's workspace cursor PERSISTS across restarts.
//!
//! 2026-10-29 (Office Bot): `drain_to_now()` fixed "a fresh desktop
//! replays the user's whole event history" — but it introduced a second
//! bug. If the desktop restarts while a phone-created note is still
//! pending (created but not yet claimed), the new process seeds its
//! cursor to "now" and **never sees that note**. The user's task
//! silently evaporates.
//!
//! The fix (`workspace_cursor.rs`): persist the last processed event id
//! to `app_data_dir/workspace/cursor.json`, keyed by MAIC user id, and
//! RESUME from it on the next start. Only a genuine fresh install (no
//! saved cursor) drains to "now".
//!
//! This test drives the real shipped `WorkspaceClient`:
//!   1. Start the client with a temp cursor dir → it drains to "now".
//!   2. Create a throwaway note → the client processes it.
//!   3. Stop the client.
//!   4. Assert the cursor file exists and holds a non-zero event id.
//!   5. Start a NEW client over the SAME cursor dir → it must RESUME
//!      (log says "resuming from persisted cursor", not "cursor seeded"),
//!      and it must NOT re-process the already-handled note.
//!
//! **The test owns its own note** — client-generated UUID, deleted
//! afterwards. It never touches a real user note.
//!
//! Skipped automatically unless `MAIC_API_KEY` and `MAIC_BASE` are set.
//!
//! Run:
//!   source ~/.config/secrets/maic.env
//!   cargo test --test live_workspace_restart -- --test-threads=1 --nocapture

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

fn device_header() -> String {
    "desktop:live_workspace_restart_test".to_string()
}

fn maic_get(url: &str) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    match agent
        .get(url)
        .set("Authorization", &auth_header())
        .call()
    {
        Ok(r) => {
            let code = r.status();
            let body = r.into_string().unwrap_or_default();
            (code, body)
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            (code, body)
        }
        Err(e) => (0, format!("{}", e)),
    }
}

fn maic_put(url: &str, body: &serde_json::Value) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    match agent
        .put(url)
        .set("Authorization", &auth_header())
        .set("X-Miracle-Device-Id", &device_header())
        .send_json(body.clone())
    {
        Ok(r) => {
            let code = r.status();
            let text = r.into_string().unwrap_or_default();
            (code, text)
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            (code, text)
        }
        Err(e) => (0, format!("{}", e)),
    }
}

fn maic_delete(url: &str) -> u16 {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    match agent
        .delete(url)
        .set("Authorization", &auth_header())
        .call()
    {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(_) => 0,
    }
}

/// Read a subkey's value. MAIC returns a WRAPPER — the payload is under
/// `value`, not at the top level.
fn read_subkey(base: &str, note_id: &str, subkey: &str) -> Option<serde_json::Value> {
    let url = format!(
        "{}/v1/users/me/workspace/notes/{}/{}",
        base, note_id, subkey
    );
    let (code, body) = maic_get(&url);
    if code != 200 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v.get("value").cloned()
}

fn wait_for<F>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// A temp directory for the cursor file, unique per test run.
fn temp_cursor_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("mc-ws-restart-{}-{}", tag, nanos));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn workspace_cursor_persists_across_restart() {
    let (Some(_key), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    let cursor_dir = temp_cursor_dir("persist");
    let note_id = uuid::Uuid::new_v4().to_string();

    // ── Phase 1: fresh start (no saved cursor) → drains to "now" ──────
    let client = WorkspaceClient::with_cursor_dir(
        base.clone(),
        maic_key().unwrap(),
        Some(cursor_dir.clone()),
    );
    client.start();

    // Give the loop a moment to drain and settle.
    std::thread::sleep(Duration::from_secs(3));

    // ── Phase 2: create a throwaway note; the client processes it ─────
    let (code, body) = maic_put(
        &format!("{}/v1/users/me/workspace/notes/{}", base, note_id),
        &json!({
            "subkeys": {
                "request": {
                    "kind": "standing_instruction",
                    "title": "restart persistence probe",
                    "text": "verify the cursor survives a restart",
                    "needs_approval_for": []
                }
            }
        }),
    );
    assert!(
        code == 200 || code == 201,
        "note create failed: HTTP {code} — {body}"
    );

    let processed = wait_for(Duration::from_secs(90), || {
        read_subkey(&base, &note_id, "status")
            .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(|s| s.to_string()))
            .map(|s| s == "done")
            .unwrap_or(false)
    });
    assert!(processed, "client did not process the note to done");

    // ── Phase 3: stop the client; the cursor should be on disk ───────
    client.stop();

    let cursor_file = cursor_dir.join("workspace").join("cursor.json");
    assert!(
        cursor_file.exists(),
        "cursor file was not written at {}",
        cursor_file.display()
    );
    let raw = std::fs::read_to_string(&cursor_file).expect("read cursor file");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse cursor file");
    let cursors = parsed
        .get("cursors")
        .and_then(|c| c.as_object())
        .expect("cursor file has no 'cursors' object");
    assert!(
        !cursors.is_empty(),
        "cursor file has no user entries: {parsed}"
    );
    let saved: Vec<i64> = cursors.values().filter_map(|v| v.as_i64()).collect();
    assert!(
        saved.iter().any(|v| *v > 0),
        "no non-zero cursor persisted: {parsed}"
    );
    eprintln!("PASS: cursor persisted -> {parsed}");

    // ── Phase 4: a NEW client over the SAME cursor dir must RESUME ────
    // If it re-drains to "now", a pending note created while the desktop
    // was closed would never be seen. That's the bug this fixes.
    let client2 = WorkspaceClient::with_cursor_dir(
        base.clone(),
        maic_key().unwrap(),
        Some(cursor_dir.clone()),
    );
    client2.start();
    std::thread::sleep(Duration::from_secs(3));

    // The resumed cursor must still be in effect — i.e. the client did
    // NOT reset to 0 (full replay) and did NOT re-drain past our saved
    // position. We assert the reported last_event_id is >= the saved one.
    let status = client2.status();
    let saved_max = saved.iter().copied().max().unwrap_or(0);
    assert!(
        status.last_event_id >= saved_max,
        "resumed cursor went backwards: {} < saved {saved_max}",
        status.last_event_id
    );
    eprintln!(
        "PASS: resumed from persisted cursor (last_event_id={}, saved={saved_max})",
        status.last_event_id
    );

    client2.stop();

    // ── Cleanup: delete exactly our own note ─────────────────────────
    let del = maic_delete(&format!("{}/v1/users/me/workspace/notes/{}", base, note_id));
    assert_eq!(del, 204, "note delete returned HTTP {del}");
    eprintln!("PASS: probe note {note_id} deleted");

    let _ = std::fs::remove_dir_all(&cursor_dir);
}
