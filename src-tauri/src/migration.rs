//! rc56.0 Bucket 2 migration: rename `openclaw.*` → `miracle-claw-gateway.*` and
//! `openclaw.json` → `chat.json`.
//!
//! Per `docs/mc-module-driven-surfaces-2026-09.md` §6.2 + §7 Week 6, the
//! four renames must move together. We do this on first launch of rc56.0
//! via `migrate_user_config()` which is called from `setup()` BEFORE
//! `ensure_openclaw_json_minimal()`. Rollback safety: if any step fails
//! the old file is left in place and the new path falls back to it, so
//! the user can still launch rc56.0 (and the migration will retry on
//! the next launch).
//!
//! ## What this migrates
//!
//! - User config: `~/.openclaw/openclaw.json` (or `%APPDATA%\MiracleClaw\openclaw.json`)
//!   → `~/.openclaw/chat.json` (or `%APPDATA%\MiracleClaw\chat.json`).
//!   The content is preserved verbatim — only the filename changes.
//!
//! ## What this does NOT migrate
//!
//! - The bundled resource `openclaw.mjs` → `miracle-claw-gateway.mjs`
//!   is renamed at build time (`src-tauri/tauri.conf.json` `resources`
//!   list + every `resources.join("openclaw.mjs")` call site in Rust).
//!   That's compiled in; no runtime migration is needed.
//! - The capability file `src-tauri/capabilities/openclaw.json` →
//!   `chat.json` is renamed at build time. No runtime effect because
//!   the chat window no longer exists (per Lesson 491 — the chat UI
//!   is the main webview navigated to localhost:28789).
//! - The webview window label `"openclaw"` → `"miracle-claw-chat"`
//!   is renamed at build time. The only call site is the defensive
//!   `get_webview_window("openclaw")` lookup in `maic_logout`
//!   (lib.rs:3631), which is currently a no-op because the chat
//!   window no longer exists in post-rc13 builds.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Result of a user-config migration attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Migration ran: the file was successfully moved and verified.
    Migrated { from: PathBuf, to: PathBuf },
    /// Nothing to do: the new file already exists, or the old file is absent.
    NoOp {
        /// What's already at the new path, or what's currently the active config.
        active: PathBuf,
    },
    /// Migration failed; we fell back to the old file. Will retry on next launch.
    RolledBack { reason: String, from: PathBuf },
}

/// One-shot user-config migration. Safe to call on every launch — it
/// short-circuits when the new file exists or the old file is absent.
///
/// Algorithm (Lesson 897, derived from the AP-launchpad-A "build the
/// wedge" principle and the Lesson 195 README-of-COPY-in-Dockerfile
/// pattern: read first, write second, verify third, delete last):
///
/// 1. Resolve `old = legacy_openclaw_json_path()` and `new = openclaw_json_path()`.
/// 2. If `new` exists, return `NoOp { active: new }` (already migrated or
///    a fresh user with no legacy file).
/// 3. If `old` doesn't exist, return `NoOp { active: new }` (no migration needed).
/// 4. Read `old` as bytes.
/// 5. Parse as JSON to verify it's structurally valid. If parse fails,
///    return `RolledBack` (don't touch the broken file; let the user
///    fix it manually or run the gateway's own repair).
/// 6. Ensure `new`'s parent dir exists (create if missing).
/// 7. Write `old`'s bytes to `new`.
/// 8. Re-read `new` and re-parse; if the round-trip changed anything,
///    delete `new` and return `RolledBack` (the bytes weren't valid JSON
///    even though the parse in step 5 said they were — unlikely but
///    defensive).
/// 9. Delete `old`.
/// 10. Return `Migrated { from: old, to: new }`.
///
/// On any unexpected `io::Error`, return `RolledBack` with the error
/// message; never panic, never leave the new file in a broken state.
pub fn migrate_user_config() -> MigrationOutcome {
    let old = legacy_openclaw_json_path();
    let new = crate::openclaw_json_path();

    if new.is_file() {
        return MigrationOutcome::NoOp { active: new };
    }
    if !old.is_file() {
        return MigrationOutcome::NoOp { active: new };
    }

    // Read the legacy file.
    let bytes = match fs::read(&old) {
        Ok(b) => b,
        Err(e) => {
            return MigrationOutcome::RolledBack {
                reason: format!("read {}: {}", old.display(), e),
                from: old,
            };
        }
    };

    // Verify it's valid JSON before we write a copy.
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(_) => {}
        Err(e) => {
            return MigrationOutcome::RolledBack {
                reason: format!(
                    "legacy {} is not valid JSON ({}); leaving it untouched",
                    old.display(),
                    e
                ),
                from: old,
            };
        }
    }

    // Ensure the parent dir exists (it should, since old is there, but
    // be defensive in case the user has a weird setup).
    if let Some(parent) = new.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return MigrationOutcome::RolledBack {
                reason: format!("create_dir_all({}): {}", parent.display(), e),
                from: old,
            };
        }
    }

    // Write the new file.
    if let Err(e) = fs::write(&new, &bytes) {
        return MigrationOutcome::RolledBack {
            reason: format!("write {}: {}", new.display(), e),
            from: old,
        };
    }

    // Round-trip verify: re-read new, re-parse. If anything's off, undo.
    let verify = match fs::read(&new) {
        Ok(b) => b,
        Err(e) => {
            // Can't even read what we just wrote — clean up and bail.
            let _ = fs::remove_file(&new);
            return MigrationOutcome::RolledBack {
                reason: format!("post-write read of {}: {}", new.display(), e),
                from: old,
            };
        }
    };
    if verify != bytes {
        let _ = fs::remove_file(&new);
        return MigrationOutcome::RolledBack {
            reason: format!(
                "round-trip changed {} bytes; not deleting the legacy file",
                new.display()
            ),
            from: old,
        };
    }
    if serde_json::from_slice::<serde_json::Value>(&verify).is_err() {
        let _ = fs::remove_file(&new);
        return MigrationOutcome::RolledBack {
            reason: format!("post-write parse of {} failed; not deleting legacy", new.display()),
            from: old,
        };
    }

    // All clear: delete the legacy file.
    if let Err(e) = fs::remove_file(&old) {
        // We successfully wrote the new file but couldn't delete the
        // old. Not a rollback — the new file is valid. Just log.
        eprintln!(
            "[miracle-claw] migration wrote {} but could not remove legacy {}: {}",
            new.display(),
            old.display(),
            e
        );
    }

    MigrationOutcome::Migrated { from: old, to: new }
}

/// Legacy path: where `openclaw.json` lived before the rc56.0 rename.
/// Kept as a separate fn so tests can target it without depending on
/// `openclaw_json_path()` (which returns the NEW path).
pub fn legacy_openclaw_json_path() -> PathBuf {
    crate::openclaw_extensions_dir()
        .parent()
        .map(|p| p.join("openclaw.json"))
        .unwrap_or_else(|| PathBuf::from("./openclaw.json"))
}

/// Public helper for tests + the GUI: returns whichever config file
/// currently exists and should be used, preferring the new name.
///
/// If the new file exists → use it.
/// Else if the legacy file exists → use it (migration will retry).
/// Else → return the new path (so `ensure_openclaw_json_minimal`
/// pre-writes defaults at the right place).
pub fn active_config_path() -> PathBuf {
    let new = crate::openclaw_json_path();
    if new.is_file() {
        return new;
    }
    let old = legacy_openclaw_json_path();
    if old.is_file() {
        return old;
    }
    new
}

// Re-export the function for the main `setup()` call site.
pub use migrate_user_config as run_user_config_migration;

// ─────────────────────────────────────────────────────────────────────
// Unit tests (Lesson 197: roll-back must be visible, not silent)
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Serialize HOME/APPDATA env var mutations across tests so they
    // don't trample each other (cargo test runs in parallel by default).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set up a temp dir as the user's state dir and return both old/new
    /// config paths. Writes nothing.
    fn arrange_state_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::TempDir::new().expect("create tempdir");
        #[cfg(windows)]
        {
            env::set_var("APPDATA", tmp.path());
        }
        #[cfg(not(windows))]
        {
            env::set_var("HOME", tmp.path());
        }
        let old = crate::openclaw_extensions_dir()
            .parent()
            .map(|p| p.join("openclaw.json"))
            .unwrap();
        let new = crate::openclaw_json_path();
        // Construct the parent dir so the migration has somewhere to write.
        if let Some(parent) = new.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        (tmp, old, new)
    }

    #[test]
    fn noop_when_new_already_exists() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, _old, new) = arrange_state_dir();
        std::fs::write(&new, b"{\"hello\":\"world\"}").unwrap();

        let outcome = migrate_user_config();
        assert!(matches!(outcome, MigrationOutcome::NoOp { .. }));
    }

    #[test]
    fn noop_when_legacy_absent_and_new_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, _old, _new) = arrange_state_dir();
        // No files written.
        let outcome = migrate_user_config();
        assert!(matches!(outcome, MigrationOutcome::NoOp { .. }));
    }

    #[test]
    fn migrates_legacy_to_new() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, old, new) = arrange_state_dir();
        let body = br#"{"gateway":{"mode":"local","bind":"loopback","auth":{"mode":"none"}}}"#;
        std::fs::write(&old, body).unwrap();

        let outcome = migrate_user_config();
        match outcome {
            MigrationOutcome::Migrated { from, to } => {
                assert_eq!(from, old);
                assert_eq!(to, new);
            }
            other => panic!("expected Migrated, got {:?}", other),
        }

        assert!(!old.is_file(), "legacy should be deleted");
        assert!(new.is_file(), "new should exist");
        assert_eq!(std::fs::read(&new).unwrap(), body, "content must be preserved");
    }

    #[test]
    fn rolls_back_on_invalid_json_legacy() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, old, new) = arrange_state_dir();
        // Not JSON at all.
        std::fs::write(&old, b"this is not json{").unwrap();

        let outcome = migrate_user_config();
        assert!(matches!(outcome, MigrationOutcome::RolledBack { .. }));
        assert!(old.is_file(), "legacy must be preserved on rollback");
        assert!(!new.is_file(), "new must NOT be created on rollback");
    }

    #[test]
    fn active_config_path_prefers_new() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, old, new) = arrange_state_dir();
        std::fs::write(&old, br#"{"legacy":true}"#).unwrap();
        std::fs::write(&new, br#"{"new":true}"#).unwrap();
        assert_eq!(active_config_path(), new);
    }

    #[test]
    fn active_config_path_falls_back_to_legacy() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, old, new) = arrange_state_dir();
        std::fs::write(&old, br#"{"legacy":true}"#).unwrap();
        assert_eq!(active_config_path(), old);
        assert!(!new.is_file(), "fallback case: new doesn't exist");
    }

    #[test]
    fn active_config_path_returns_new_when_neither_exists() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, _old, new) = arrange_state_dir();
        assert_eq!(active_config_path(), new);
    }
}
