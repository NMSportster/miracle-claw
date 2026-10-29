//! Live end-to-end check of the desktop's **claim + execute** sequence
//! against the real MAIC backend.
//!
//! 2026-10-28: `tests/live_workspace_sync.rs` verified the individual
//! endpoints. This test verifies the *sequence* the sync loop performs —
//! the thing that actually has to work for Phase 5:
//!
//!   1. claim:   PUT status = {state: "in_progress", claimed_by, claimed_at}
//!   2. execute: PUT result  = {summary, details, kind, completed_at}
//!   3. finish:  PUT status = {state: "done", completed_at}
//!
//! It creates its own note so it never touches real user data, and
//! cleans up after itself.
//!
//! Skipped automatically unless `MAIC_API_KEY` and `MAIC_BASE` are set.
//!
//! Run:
//!   source ~/.config/secrets/maic.env
//!   cargo test --test live_workspace_claim -- --test-threads=1 --nocapture

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

/// Mirrors `workspace_sync::write_subkey`'s device header.
fn device_header() -> String {
    "desktop:live_workspace_claim_test".to_string()
}

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

fn subkey_url(base: &str, note_id: &str, subkey: &str) -> String {
    format!("{base}/v1/users/me/workspace/notes/{note_id}/{subkey}")
}

fn now_iso8601() -> String {
    // Matches `workspace_sync::now_iso8601`'s shape closely enough for MAIC.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 2026-10-28T22:00:00Z style, computed from the epoch.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[test]
fn live_claim_execute_finish_sequence() {
    let (Some(_), Some(base)) = (maic_key(), maic_base()) else {
        eprintln!("skip: MAIC_API_KEY / MAIC_BASE not set");
        return;
    };

    // ── 1. Create a note the way the phone does. ────────────────────────
    // MAIC has NO POST /v1/users/me/workspace. Notes are created with a
    // batch write to `PUT /v1/users/me/workspace/notes/{note_id}` whose
    // body is `{"subkeys": {"request": {...}, ...}}`. The note_id is
    // client-generated.
    // MAIC validates note_id as a UUID ("note_id must be a valid UUID").
    let note_id = uuid::Uuid::new_v4().to_string();
    let (code, body) = maic_put(
        &format!("{base}/v1/users/me/workspace/notes/{note_id}"),
        &json!({
            "subkeys": {
                "request": {
                    "title": "live_workspace_claim probe",
                    "text": "end-to-end claim/execute/finish probe",
                    "kind": "standing_instruction",
                }
            }
        }),
    );
    assert!(
        code == 200 || code == 201,
        "note create failed: HTTP {code}: {body}"
    );
    eprintln!("created note {note_id}");

    // ── 2. Claim: PUT status = in_progress. ──────────────────────────────
    let (code, body) = maic_put(
        &subkey_url(&base, &note_id, "status"),
        &json!({
            "state": "in_progress",
            "claimed_by": "desktop:live_workspace_claim_test",
            "claimed_at": now_iso8601(),
        }),
    );
    assert!(
        code == 200 || code == 201,
        "claim failed: HTTP {code}: {body}"
    );

    // Read the claim back — this is what proves the claim landed.
    let (code, body) = maic_get(&subkey_url(&base, &note_id, "status"));
    assert_eq!(code, 200, "claim read-back failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("status body not JSON: {e}"));
    assert_eq!(
        sub["value"]["state"].as_str(),
        Some("in_progress"),
        "claim did not land: {sub}"
    );
    assert!(
        !sub["value"]["claimed_by"].is_null(),
        "claimed_by missing from status: {sub}"
    );

    // ── 3. Execute: PUT result. ──────────────────────────────────────────
    // Mirror `execute_note()`'s exact result shape.
    let (code, body) = maic_put(
        &subkey_url(&base, &note_id, "result"),
        &json!({
            "summary": "Acknowledged: live_workspace_claim probe (standing_instruction)",
            "details": "Desktop received the task.\n\nMessage:\nend-to-end claim/execute/finish probe",
            "kind": "standing_instruction",
            "completed_at": now_iso8601(),
        }),
    );
    assert!(
        code == 200 || code == 201,
        "result write failed: HTTP {code}: {body}"
    );

    let (code, body) = maic_get(&subkey_url(&base, &note_id, "result"));
    assert_eq!(code, 200, "result read-back failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("result body not JSON: {e}"));
    let summary = sub["value"]["summary"].as_str().unwrap_or_default();
    assert!(
        summary.starts_with("Acknowledged:"),
        "result summary wrong: {sub}"
    );

    // ── 4. Finish: PUT status = done. ───────────────────────────────────
    let (code, body) = maic_put(
        &subkey_url(&base, &note_id, "status"),
        &json!({
            "state": "done",
            "completed_at": now_iso8601(),
        }),
    );
    assert!(
        code == 200 || code == 201,
        "finish failed: HTTP {code}: {body}"
    );

    let (code, body) = maic_get(&subkey_url(&base, &note_id, "status"));
    assert_eq!(code, 200, "finish read-back failed: HTTP {code}: {body}");
    let sub: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("status body not JSON: {e}"));
    assert_eq!(
        sub["value"]["state"].as_str(),
        Some("done"),
        "final status not 'done': {sub}"
    );

    // ── 5. The full note must now show the whole shape the phone reads. ─
    let (code, body) = maic_get(&format!("{base}/v1/users/me/workspace/notes/{note_id}"));
    assert_eq!(code, 200, "note fetch failed: HTTP {code}: {body}");
    let note: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("note body not JSON: {e}"));
    let subkeys = note["subkeys"].as_object().expect("note has subkeys object");
    for expected in ["request", "status", "result"] {
        assert!(
            subkeys.contains_key(expected),
            "note is missing `{expected}` subkey: {:?}",
            subkeys.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        note["subkeys"]["status"]["value"]["state"].as_str(),
        Some("done")
    );

    eprintln!(
        "claim -> execute -> finish sequence verified end-to-end on note {note_id}"
    );

    // ── 6. Clean up so the probe doesn't linger in the user's workspace. ─
    let (code, body) = maic_delete(&format!("{base}/v1/users/me/workspace/notes/{note_id}"));
    assert!(
        code == 204 || code == 200 || code == 404,
        "cleanup delete failed: HTTP {code}: {body}"
    );
    eprintln!("probe note {note_id} cleaned up");
}
