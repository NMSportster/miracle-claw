//! workspace_cursor.rs — Persisted MAIC workspace event cursor.
//!
//! Lesson (Office Bot, 2026-10-29): a fresh desktop must not replay the
//! user's whole event history. MAIC's poll endpoint returns
//! `id > since_event_id` ORDER BY id ASC with a LIMIT of 50, so starting
//! at 0 makes the client chew through every event ever recorded.
//! `drain_to_now()` fast-forwards the cursor to "now" on first start.
//!
//! BUT that created a second bug: if the desktop is restarted while a
//! note is pending (created by the phone, not yet claimed), the fresh
//! process seeds its cursor to "now" and will **never see that note**.
//! The user's task silently evaporates.
//!
//! The fix is to persist the cursor across restarts. On start:
//!   - If a saved cursor exists for this user → RESUME from it. Pending
//!     notes created before the shutdown are still picked up.
//!   - If no saved cursor exists → this is a genuine fresh install, so
//!     drain to "now" and start clean (skip history).
//!
//! Storage: `app_data_dir/workspace/cursor.json`, keyed by MAIC user id
//! (so switching accounts on the same machine doesn't cross-contaminate
//! cursors). Mirrors `pairing_identity.rs` — same location convention,
//! same "best-effort" persistence philosophy: a failed write must never
//! break the sync loop, it just degrades to the old behaviour.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CURSOR_FILENAME: &str = "cursor.json";

/// On-disk shape of the cursor file.
///
/// ```json
/// {
///   "version": 1,
///   "cursors": { "3002": 178 }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorFile {
    /// Schema version, so a future format change can migrate.
    #[serde(default = "default_version")]
    version: u32,
    /// Map of MAIC user id → last event id this desktop processed.
    #[serde(default)]
    cursors: BTreeMap<String, i64>,
}

fn default_version() -> u32 {
    1
}

impl Default for CursorFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            cursors: BTreeMap::new(),
        }
    }
}

/// Path to the cursor file for a given app data dir.
pub fn cursor_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("workspace").join(CURSOR_FILENAME)
}

/// Load the saved cursor for `user_id`.
///
/// Returns `Ok(None)` if:
///   - there is no cursor file yet (fresh install), or
///   - the file is unreadable/corrupt (we treat it as "no cursor" rather
///     than failing the sync loop).
///
/// A corrupt file is also removed so the next write starts clean.
pub fn load_cursor(app_data_dir: &Path, user_id: &str) -> Option<i64> {
    let path = cursor_path(app_data_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return None,
    };
    let file: CursorFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[workspace-cursor] cursor file unreadable ({e}); starting fresh");
            let _ = std::fs::remove_file(&path);
            return None;
        }
    };
    file.cursors.get(user_id).copied()
}

/// Persist the cursor for `user_id`. Best-effort — errors are logged but
/// never propagated, because losing a cursor write must not break the
/// sync loop (it just degrades to re-draining on next start).
pub fn save_cursor(app_data_dir: &Path, user_id: &str, event_id: i64) {
    if user_id.is_empty() {
        return;
    }
    let path = cursor_path(app_data_dir);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[workspace-cursor] could not create {}: {e}", parent.display());
            return;
        }
    }

    // Read-merge-write so we don't clobber another user's cursor.
    let mut file: CursorFile = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    // Never move the cursor backwards — a stale writer could otherwise
    // make the desktop re-process events it already handled.
    let existing = file.cursors.get(user_id).copied().unwrap_or(0);
    if event_id <= existing {
        return;
    }

    file.cursors.insert(user_id.to_string(), event_id);
    file.version = default_version();

    let json = match serde_json::to_string_pretty(&file) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[workspace-cursor] serialize failed: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("[workspace-cursor] write failed: {e}");
    }
}

/// Resolve the MAIC user id for the current JWT, so the cursor is keyed
/// per-account. Returns `""` if we can't determine it (the caller then
/// skips persistence rather than writing to a shared bucket).
///
/// The id is embedded in the JWT payload as `sub` (a numeric string in
/// MAIC's tokens). We decode the payload locally — no network call — so
/// this is cheap and safe to call on the hot path.
pub fn user_id_from_jwt(jwt: &str) -> String {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return String::new();
    }
    let payload = match base64url_decode(parts[1]) {
        Some(p) => p,
        None => return String::new(),
    };
    let val: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    for key in ["sub", "user_id", "uid"] {
        if let Some(s) = val.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
        if let Some(n) = val.get(key).and_then(|v| v.as_i64()) {
            return n.to_string();
        }
    }
    String::new()
}

/// Decode standard base64url (JWT uses `-`/`_` instead of `+`/`/`, and
/// drops padding).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let v = lookup[b as usize];
        if v == 255 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "mc-workspace-cursor-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn roundtrips_a_cursor() {
        let dir = tmpdir("roundtrip");
        assert_eq!(load_cursor(&dir, "3002"), None);
        save_cursor(&dir, "3002", 178);
        assert_eq!(load_cursor(&dir, "3002"), Some(178));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_cursors_for_different_users_separate() {
        let dir = tmpdir("multiuser");
        save_cursor(&dir, "3002", 178);
        save_cursor(&dir, "3003", 42);
        assert_eq!(load_cursor(&dir, "3002"), Some(178));
        assert_eq!(load_cursor(&dir, "3003"), Some(42));
        assert_eq!(load_cursor(&dir, "3004"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_moves_the_cursor_backwards() {
        let dir = tmpdir("backwards");
        save_cursor(&dir, "3002", 178);
        save_cursor(&dir, "3002", 100);
        assert_eq!(load_cursor(&dir, "3002"), Some(178));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_treated_as_no_cursor() {
        let dir = tmpdir("corrupt");
        let path = cursor_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json at all").unwrap();
        assert_eq!(load_cursor(&dir, "3002"), None);
        // …and the corrupt file is removed so the next save is clean.
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_user_id_is_not_persisted() {
        let dir = tmpdir("nouser");
        save_cursor(&dir, "", 178);
        assert_eq!(load_cursor(&dir, ""), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64url_decodes() {
        // {"sub":"3002"} -> eyJzdWIiOiIzMDAyIn0
        let decoded = base64url_decode("eyJzdWIiOiIzMDAyIn0").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["sub"], "3002");
    }

    #[test]
    fn extracts_user_id_from_jwt() {
        let payload = base64url_encode(br#"{"sub":"3002","exp":123}"#);
        let jwt = format!("header.{payload}.signature");
        assert_eq!(user_id_from_jwt(&jwt), "3002");
    }

    #[test]
    fn extracts_numeric_user_id_from_jwt() {
        let payload = base64url_encode(br#"{"sub":3002}"#);
        let jwt = format!("header.{payload}.signature");
        assert_eq!(user_id_from_jwt(&jwt), "3002");
    }

    #[test]
    fn malformed_jwt_yields_empty_user_id() {
        assert_eq!(user_id_from_jwt(""), "");
        assert_eq!(user_id_from_jwt("not.a.jwt"), "");
        assert_eq!(user_id_from_jwt("onlyonepart"), "");
        assert_eq!(user_id_from_jwt("header.!!!!.sig"), "");
    }

    fn base64url_encode(input: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((n >> 18) & 63) as usize] as char);
            out.push(TABLE[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((n >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(n & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }
}
