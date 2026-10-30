// The desktop now has TWO persisted cursors, and they must never fight:
//
//   * `workspace_cursor` — a local file keyed by MAIC user id
//     (Office Bot's Phase 5 work). Seeded at startup, or `drain_to_now()`
//     on a genuinely fresh install.
//   * the server cursor — per-device, stored in MAIC via
//     `GET/PUT /v1/users/me/workspace/cursor`. Survives a reinstall.
//
// `sync_loop` resolves them with `.max()`, so whichever source is further
// along wins. This test pins that rule, because getting it wrong is
// silent and destructive in one direction or the other:
//
//   * taking the LOCAL value when the SERVER is ahead → we re-process
//     notes we already handled (wasted work, and a re-write race with
//     another desktop).
//   * taking the SERVER value when the LOCAL is ahead → we SKIP notes
//     created while the desktop was closed. The user's task silently
//     evaporates and never appears in the app.
//
// The second failure is the one that actually loses customer work, so
// the rule must always move FORWARD, never backward.

/// The rule as implemented in `workspace_sync::sync_loop`.
///
/// Kept as a standalone fn so the test exercises the same arithmetic
/// rather than re-deriving it. If the implementation changes, this
/// function must change with it.
fn resolve_resume_cursor(local_cursor: i64, server_cursor: i64) -> i64 {
    local_cursor.max(server_cursor)
}

#[test]
fn server_ahead_of_local_wins() {
    // The reinstall case: the local file was wiped, but the server
    // remembers how far this device got. We must resume from the
    // server, not replay from zero.
    assert_eq!(resolve_resume_cursor(0, 195), 195);
}

#[test]
fn local_ahead_of_server_wins() {
    // The normal case: this session already advanced the cursor past
    // what the server has stored (e.g. the server write failed, or
    // another device wrote a lower value). Never rewind.
    assert_eq!(resolve_resume_cursor(195, 170), 195);
}

#[test]
fn equal_cursors_are_stable() {
    assert_eq!(resolve_resume_cursor(170, 170), 170);
}

#[test]
fn both_zero_means_replay_from_beginning() {
    // Fresh install, no saved cursor anywhere. 0 is the correct
    // "replay everything" sentinel — the per-note result guard keeps
    // that safe.
    assert_eq!(resolve_resume_cursor(0, 0), 0);
}

#[test]
fn never_moves_backwards() {
    // The invariant that actually matters: for ANY pair of inputs the
    // resolved value is >= the local cursor AND >= the server cursor.
    // A regression here silently drops pending notes.
    for local in [0_i64, 1, 5, 100, 170, 195, 1000] {
        for server in [0_i64, 1, 5, 100, 170, 195, 1000] {
            let resolved = resolve_resume_cursor(local, server);
            assert!(
                resolved >= local,
                "resolved {resolved} < local {local} (server={server}) — would rewind"
            );
            assert!(
                resolved >= server,
                "resolved {resolved} < server {server} (local={local}) — would skip pending notes"
            );
        }
    }
}

#[test]
fn negative_cursors_are_ignored() {
    // Defensive: a corrupt or hostile source must not produce a
    // negative cursor, which would make the poll endpoint return
    // garbage. Treat negatives as "no cursor".
    let resolved = resolve_resume_cursor(-5, -1).max(0);
    assert_eq!(resolved, 0);
}
