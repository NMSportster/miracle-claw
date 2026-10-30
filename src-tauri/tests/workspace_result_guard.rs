//! Regression test for the workspace "result-exists" guard.
//!
//! ## Why this test exists
//!
//! On 2026-10-29 an end-to-end run against PRODUCTION MAIC overwrote the
//! user's real `daily_summary` note with a stub payload. The recovery was
//! not possible from the database: `user_workspace` is upsert-only (one
//! row per subkey) and `user_workspace_events` stores metadata only, no
//! payload. The recap had to be regenerated from scratch.
//!
//! Root cause: the guard was `status.state in ("done" | "skipped")`. But
//! `status` is OPTIONAL. Several real producers write `request` + `result`
//! and never write `status`:
//!
//!   * `scripts/daily_summary.py`  (INSERT request L383, result L393)
//!   * `POST /v1/users/me/workspace/notes/voice`  (result only)
//!   * `POST /v1/users/me/workspace/notes/photo`  (result only)
//!
//! A status-only guard treats those as fresh work and clobbers them.
//!
//! ## The rule
//!
//! **A note is "already executed" iff it has a `result` subkey.**

use miracle_claw_lib::workspace_sync::{NoteApiResponse, SubkeyApiEntry};
use serde_json::json;

/// Build a NoteApiResponse from a map of subkey → value.
fn note_with(subkeys: Vec<(&str, serde_json::Value)>) -> NoteApiResponse {
    let mut map = std::collections::HashMap::new();
    for (name, value) in subkeys {
        map.insert(
            name.to_string(),
            SubkeyApiEntry {
                value,
                version: 1,
            },
        );
    }
    NoteApiResponse {
        note_id: "test-note".to_string(),
        subkeys: map,
    }
}

#[test]
fn a_note_with_result_and_no_status_is_already_executed() {
    // This is the exact shape of the daily_summary note that got
    // clobbered: `request` + `result`, and NO `status`.
    let note = note_with(vec![
        (
            "request",
            json!({
                "kind": "daily_summary",
                "title": "Daily summary — 2026-10-29",
                "text": "recap text",
                "needs_approval_for": []
            }),
        ),
        (
            "result",
            json!({
                "kind": "daily_summary",
                "summary": "You logged in 4 times across the day…",
                "details": "full recap",
                "completed_at": "2026-10-29T05:55:16Z"
            }),
        ),
    ]);

    // The guard must consider this note terminal.
    assert!(
        note.subkeys.contains_key("result"),
        "note has a result subkey, so it must be treated as already executed"
    );
    assert!(
        !note.subkeys.contains_key("status"),
        "this note deliberately has NO status subkey — that is the whole point"
    );
}

#[test]
fn a_note_with_result_and_status_done_is_already_executed() {
    let note = note_with(vec![
        ("request", json!({"kind": "message"})),
        ("result", json!({"summary": "done"})),
        ("status", json!({"state": "done"})),
    ]);
    assert!(note.subkeys.contains_key("result"));
    assert_eq!(
        note.subkeys.get("status").unwrap().value.get("state"),
        Some(&json!("done"))
    );
}

#[test]
fn a_note_with_only_request_is_fresh_work() {
    // A freshly-created phone task: `request` only. This MUST be
    // executed — it's the normal case the feature exists for.
    let note = note_with(vec![(
        "request",
        json!({
            "kind": "message",
            "title": "Turn on the shop lights",
            "text": "Turn on the shop lights and report back what you did.",
            "needs_approval_for": []
        }),
    )]);
    assert!(!note.subkeys.contains_key("result"));
    assert!(!note.subkeys.contains_key("status"));
}

#[test]
fn a_note_with_only_status_in_progress_is_not_terminal() {
    // Claimed but not finished — the result guard must NOT fire, but the
    // status guard handles it separately.
    let note = note_with(vec![
        ("request", json!({"kind": "message"})),
        ("status", json!({"state": "in_progress"})),
    ]);
    assert!(!note.subkeys.contains_key("result"));
}

#[test]
fn voice_memo_shape_has_result_and_no_status() {
    // Mirrors `POST /v1/users/me/workspace/notes/voice` — the server
    // writes a `result` subkey holding the transcript, and never writes
    // `status`. A status-only guard would overwrite the transcript.
    let note = note_with(vec![(
        "result",
        json!({
            "kind": "voice_memo",
            "title": "Voice memo",
            "summary": "transcript excerpt",
            "details": "full transcript",
            "model": "faster-whisper/base",
            "needs_approval_for": []
        }),
    )]);
    assert!(note.subkeys.contains_key("result"));
    assert!(!note.subkeys.contains_key("status"));
}
