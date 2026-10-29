//! Live integration check for the desktop `workspace_sync` module against
//! the real MAIC backend.
//!
//! 2026-10-28: the desktop crate had never been run against the real
//! backend — it didn't even compile. Now that it does, this test
//! exercises the exact URLs and payload shapes the sync loop uses, so
//! that a contract drift surfaces in CI instead of as a silent no-op in
//! the app.
//!
//! **The test owns its own note.** An earlier revision of this file
//! picked the first note out of the user's workspace and wrote `status` /
//! `result` onto it. That corrupted a real `daily_summary` note: the
//! restore logic sent the `{value: ..., subkey: ...}` *envelope* back
//! instead of the inner value, because MAIC's `PUT .../{subkey}` stores
//! the raw request body as the value. Do not repeat that — create a
//! throwaway note and delete it when done.
//!
//! MAIC restricts note subkeys to a fixed allow-list:
//!   VALID_SUBKEYS = {"request", "status", "result", "approval", "history"}
//!
//! Skipped automatically unless `MAIC_API_KEY` and `MAIC_BASE` are set.
//!
//! Run:
//!   source ~/.config/secrets/maic.env
//!   cargo test --test live_workspace_sync -- --test-threads=1 --nocapture

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
    "desktop:live_workspace_sync_test".to_string()
}

/// GET a MAIC URL, returning (status, body).
fn maic_get(url: &str) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
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

/// PUT a MAIC URL with a JSON body, returning (status, body).
/// Mirrors `workspace_sync::write_subkey` exactly, including the device
/// header. MAIC stores the **raw request body** as the subkey value —
/// so send the value itself, never an envelope.
fn maic_put(url: &str, body: &serde_json::Value) -> (u16, String) {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    match agent
        .put(url)
        .set("Authorization", &auth_header())
        .set("X-Miracle-Device-Id", &device_header())
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
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
        .timeout(std::time::Duration::from_secs(30))
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

/// The desktop's poll URL, exactly as `workspace_sync.rs` builds it.
fn poll_url(base: &str, since_event_id: i64, timeout_seconds: u32) -> String {
    format!(
        "{base}/v1/users/me/workspace/poll?since_event_id={since_event_id}&timeout_seconds={timeout_seconds}"
    )
}

/// The desktop's note-fetch URL, exactly as `workspace_sync.rs` builds it.
fn note_url(base: &str, note_id: &str) -> String {
    format!("{base}/v1/users/me/workspace/notes/{note_id}")
}

/// The desktop's subkey-write URL, exactly as `workspace_sync.rs` builds it.
fn subkey_url(base: &str, note_id: &str, subkey: &str) -> String {
    format!("{base}/v1/users/me/workspace/notes/{note_id}/{subkey}")
}

#[test]
fn live_poll_endpoint_matches_desktop_contract() {
    let (Some(_), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    // The desktop's poll starts at since_event_id = 0 with a 30s timeout.
    // Use timeout_seconds=1 so the test doesn't block for 30s.
    let url = poll_url(&base, 0, 1);
    let (code, body) = maic_get(&url);

    assert!(
        code == 200 || code == 204,
        "poll endpoint returned HTTP {code}: {body}"
    );

    if code == 200 {
        let parsed: serde_json::Value =
            serde_json::from_str(&body).unwrap_or_else(|e| panic!("poll body not JSON: {e}"));
        let arr = parsed.as_array().expect("poll body should be a JSON array");
        eprintln!("poll returned {} event(s)", arr.len());

        // Every event must carry the fields the desktop's `handle_event`
        // reads: event_id, note_id, subkey, event_kind.
        for ev in arr.iter() {
            for field in ["event_id", "note_id", "subkey", "event_kind"] {
                assert!(
                    ev.get(field).is_some(),
                    "poll event missing `{field}`: {ev}"
                );
            }
            // The desktop's `handle_event` only acts on `request`, `status`
            // and `approval` (src/workspace_sync.rs). `result`/`history`
            // events are emitted by MAIC but ignored by the desktop.
            //
            // All five must still be in MAIC's VALID_SUBKEYS allow-list.
            let subkey = ev["subkey"].as_str().unwrap_or_default();
            assert!(
                ["request", "status", "approval", "result", "history"].contains(&subkey),
                "unexpected subkey `{subkey}` — MAIC's VALID_SUBKEYS allow-list is \
                 request/status/result/approval/history"
            );
        }
    }
}

#[test]
fn live_note_fetch_and_subkey_write_roundtrip() {
    let (Some(_), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    // ── 1. Create our OWN throwaway note. Never mutate a real one. ──────
    // MAIC validates note_id as a UUID and has no POST on
    // `/v1/users/me/workspace`; notes are created with a batch write to
    // `PUT /v1/users/me/workspace/notes/{note_id}`.
    let note_id = uuid::Uuid::new_v4().to_string();
    let (code, body) = maic_put(
        &note_url(&base, &note_id),
        &json!({
            "subkeys": {
                "request": {
                    "title": "live_workspace_sync probe",
                    "text": "roundtrip probe",
                    "kind": "probe",
                }
            }
        }),
    );
    assert!(
        code == 200 || code == 201,
        "note create failed: HTTP {code}: {body}"
    );
    eprintln!("created throwaway note {note_id}");

    // ── 2. Fetch the full note (the desktop does this before executing). ─
    let (code, body) = maic_get(&note_url(&base, &note_id));
    assert_eq!(code, 200, "note fetch failed: HTTP {code}: {body}");
    let note: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("note body not JSON: {e}"));

    // The desktop reads `note["subkeys"]` as a map of name -> {value: ...}.
    assert!(
        note.get("subkeys").is_some(),
        "note has no `subkeys` map: {note}"
    );
    let subkeys = note["subkeys"].as_object().expect("subkeys should be an object");
    eprintln!("note has subkeys: {:?}", subkeys.keys().collect::<Vec<_>>());

    // ── 3. Read the `request` subkey back via the subkey endpoint. ──────
    let (code, body) = maic_get(&subkey_url(&base, &note_id, "request"));
    assert_eq!(code, 200, "subkey GET request failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("subkey body not JSON: {e}"));
    assert!(
        sub.get("value").is_some(),
        "subkey response has no `value`: {sub}"
    );
    assert_eq!(
        sub["value"]["title"].as_str(),
        Some("live_workspace_sync probe"),
        "request value did not round-trip: {sub}"
    );

    // ── 4. Write `status` the way the desktop does — same subkey, same
    //    payload shape ({state, ...}). MAIC stores the raw body as the
    //    value, so send the value itself. ────────────────────────────────
    let (code, body) = maic_put(
        &subkey_url(&base, &note_id, "status"),
        &json!({
            "state": "in_progress",
            "claimed_by": "desktop:live_workspace_sync_test",
            "claimed_at": "2026-10-28T22:00:00Z",
        }),
    );
    assert!(
        code == 200 || code == 201,
        "status PUT failed: HTTP {code}: {body}"
    );

    let (code, body) = maic_get(&subkey_url(&base, &note_id, "status"));
    assert_eq!(code, 200, "status read-back failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("status body not JSON: {e}"));
    assert_eq!(
        sub["value"]["state"].as_str(),
        Some("in_progress"),
        "status did not round-trip: {sub}"
    );
    // The value must be the bare object we sent — NOT wrapped in an
    // envelope. This is the assertion that would have caught the bug
    // that corrupted a real daily_summary note.
    assert!(
        !sub["value"].get("subkey").is_some(),
        "status value was double-wrapped: {sub}"
    );

    // ── 5. Write `result` the way the desktop does. ─────────────────────
    let (code, body) = maic_put(
        &subkey_url(&base, &note_id, "result"),
        &json!({
            "summary": "live_workspace_sync roundtrip probe",
            "details": "written by the desktop live integration test",
        }),
    );
    assert!(
        code == 200 || code == 201,
        "result PUT failed: HTTP {code}: {body}"
    );

    let (code, body) = maic_get(&subkey_url(&base, &note_id, "result"));
    assert_eq!(code, 200, "result read-back failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("result body not JSON: {e}"));
    assert_eq!(
        sub["value"]["summary"].as_str(),
        Some("live_workspace_sync roundtrip probe"),
        "result did not round-trip: {sub}"
    );
    assert!(
        !sub["value"].get("subkey").is_some(),
        "result value was double-wrapped: {sub}"
    );

    // ── 6. Clean up. The note is ours, so deleting it removes the need
    //    for any restore logic (which is what went wrong before). ───────
    let (code, body) = maic_delete(&note_url(&base, &note_id));
    assert!(
        code == 204 || code == 200 || code == 404,
        "cleanup delete failed: HTTP {code}: {body}"
    );
    eprintln!("throwaway note {note_id} cleaned up");
}
