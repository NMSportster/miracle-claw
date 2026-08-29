// src/tasks/storage.rs
//
// Lesson 725 (2026-08-28 21:30 MDT): on-disk persistence for MC tasks.
// Adapted from adeal-schedule's storage.rs (which had a known schema
// migration bug — old JSON files from before Aug 22 failed to load with
// `missing field 'name' at line 9 column 3`). This version fixes that.
//
// Storage path:
//   - Default: `<data_dir>/com.milagro.miracle-claw/tasks/` (per-OS app data)
//   - Override: `mc.tasks.data_dir` setting (future)
//   - File: `<dir>/tasks.json` (pretty-printed JSON, one array of Task)
//
// Schema migration (the fix adeal-schedule was missing):
//   - adeal-schedule v0.1 stored tasks as `{id, time, description, completed, source}`
//     and broke when the v0.1 schema switched to `{name, description, priority, ...}`.
//   - This module accepts **either** shape. Old rows are converted in place
//     and saved back as the new shape on the next write. If we see unknown
//     fields, we ignore them (forward compat for future Task fields).
//
// Threat model:
//   - The tasks file lives in the per-user app data dir. OS-level permissions
//     protect it from other users on the same machine. We don't encrypt it
//     (tasks are not secrets), but we do put the MAIC token (separate file)
//     behind a chmod 600 on Unix.

use crate::tasks::models::{Priority, Task, TaskList};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Default data directory for MC tasks. Per-OS app data, namespaced by
/// the app identifier so it doesn't collide with adeal-schedule's
/// `%APPDATA%\adeal-schedule\` (which we leave alone — David's test bed).
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("com.milagro.miracle-claw").join("tasks"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".miracle-claw").join("tasks"))
                .unwrap_or_else(|| PathBuf::from("mc-tasks"))
        })
}

/// The on-disk tasks file path.
pub fn default_tasks_file() -> PathBuf {
    default_data_dir().join("tasks.json")
}

/// Load tasks from a JSON file. Returns an empty list if the file is
/// missing or empty. Performs schema migration on the fly:
///   - Old `{id, time, description, completed, source}` rows are converted
///     to the new `{local_id, name, description, priority, completed, date}`
///     shape.
///   - Old rows that lack `name` get their description moved into `name`
///     (typical case for adeal-schedule v0.1).
///   - Old rows that lack `date` get today's date (Local).
///   - Unknown fields are ignored (forward compat).
pub fn load_tasks(path: &Path) -> Result<Vec<Task>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // Empty file = no tasks (don't error, just empty list).
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    // First try strict deserialization (current schema). If that fails,
    // fall back to the JSON-Value-tolerant migration path.
    match serde_json::from_str::<Vec<Task>>(&content) {
        Ok(tasks) => Ok(tasks),
        Err(strict_err) => {
            // Migration path: parse as Value, convert each row.
            let raw: Vec<Value> = serde_json::from_str(&content).map_err(|_| {
                anyhow::anyhow!(
                    "Failed to parse {}: {} (and migration also failed)",
                    path.display(),
                    strict_err
                )
            })?;
            let tasks = raw
                .into_iter()
                .map(migrate_legacy_task)
                .collect::<Vec<_>>();
            // Persist the migrated form immediately so we don't keep
            // doing this on every load. Caller may re-save; we save here
            // too as a safety net (idempotent).
            save_tasks(path, &tasks)?;
            Ok(tasks)
        }
    }
}

/// Convert one legacy `{id, time, description, completed, source}` row
/// into the current schema. Best-effort: missing fields become defaults.
/// Returns a Task ready to be re-serialized.
fn migrate_legacy_task(raw: Value) -> Task {
    let obj = raw.as_object();

    // Old fields:
    let old_id = obj.and_then(|o| o.get("id")).cloned();
    let old_time: Option<String> = obj.and_then(|o| o.get("time")).and_then(|v| v.as_str()).map(String::from);
    let old_description = obj
        .and_then(|o| o.get("description"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default();
    let old_completed = obj
        .and_then(|o| o.get("completed"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // For `name`: prefer explicit `name` if present (current schema), else
    // use the first line of `description`, else fall back to a generic label.
    let name = obj
        .and_then(|o| o.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            let trimmed = old_description.lines().next().unwrap_or("").trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| {
            // Last resort: use the old `time` field, or a generic label.
            // Use Option::clone so the borrow checker can prove we don't
            // move `old_time` (it's still needed below for `date`).
            old_time.clone().unwrap_or_else(|| "Untitled task".to_string())
        });

    // For `date`: prefer explicit current-schema `date` (YYYY-MM-DD),
    // else try to parse `time` as YYYY-MM-DD, else today.
    let date = obj
        .and_then(|o| o.get("date"))
        .and_then(|v| v.as_str())
        .filter(|s| TaskList::validate_date(s))
        .map(String::from)
        .or_else(|| {
            old_time
                .as_deref()
                .and_then(|s| s.get(..10))
                .filter(|s| TaskList::validate_date(s))
                .map(String::from)
        })
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());

    // For `priority`: prefer current field, else default to Medium.
    let priority = obj
        .and_then(|o| o.get("priority"))
        .and_then(|v| v.as_str())
        .map(parse_priority_str)
        .unwrap_or(Priority::Medium);

    // For `local_id`: prefer current, else the legacy `id` (UUIDs and
    // numeric ids both work), else generate a fresh UUID.
    let local_id = obj
        .and_then(|o| o.get("local_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            old_id.and_then(|v| match v {
                Value::String(s) => Some(s),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
        })
        .or_else(|| Some(uuid::Uuid::new_v4().to_string()));

    // Description gets the OLD description verbatim (so any multi-line
    // content survives), unless the current schema had a different one.
    let description = obj
        .and_then(|o| o.get("description"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or(old_description);

    Task {
        local_id: Some(local_id.unwrap()),
        name,
        description,
        priority,
        completed: old_completed,
        date,
        updated_at: Some(now_iso8601()),
    }
}

fn parse_priority_str(s: &str) -> Priority {
    match s.to_lowercase().as_str() {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".to_string())
}

/// Save tasks to a JSON file, creating parent directories as needed.
pub fn save_tasks(path: &Path, tasks: &[Task]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(tasks)
        .context("serializing tasks to JSON")?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Convenience: load a `TaskList` from a JSON file.
pub fn load_task_list(path: &Path) -> Result<TaskList> {
    let tasks = load_tasks(path)?;
    Ok(TaskList::from_tasks(tasks))
}

// ── sync state persistence (used by maic_sync) ───────────────────

/// Where MAIC sync state lives, given the tasks.json path. Convention:
/// `<dir>/sync_state.json` (sibling to tasks.json).
fn sync_state_path_for(data_file: &Path) -> PathBuf {
    data_file
        .parent()
        .map(|p| p.join("sync_state.json"))
        .unwrap_or_else(|| default_data_dir().join("sync_state.json"))
}

/// Load MAIC sync state from `<dir>/sync_state.json`. Returns a default
/// `SyncState` if the file is missing or empty (not an error — first-run
/// path). Falls back to default if the JSON is malformed.
pub fn load_sync_state_for(data_file: &Path) -> Result<crate::tasks::maic_sync::SyncState> {
    let path = sync_state_path_for(data_file);
    if !path.exists() {
        return Ok(crate::tasks::maic_sync::SyncState::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading sync state from {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(crate::tasks::maic_sync::SyncState::default());
    }
    serde_json::from_str(&raw).context("decoding sync_state.json")
}

/// Save MAIC sync state to `<dir>/sync_state.json`.
pub fn save_sync_state_for(
    data_file: &Path,
    state: &crate::tasks::maic_sync::SyncState,
) -> Result<()> {
    let path = sync_state_path_for(data_file);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json)
        .with_context(|| format!("writing sync state to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_legacy_adeal_schedule_row() {
        // What adeal-schedule v0.1 wrote, before the schema change.
        let legacy = serde_json::json!({
            "id": 17,
            "time": "2026-08-22 09:00",
            "description": "Call customer about brakes",
            "completed": false,
            "source": "adeal-schedule"
        });
        let t = migrate_legacy_task(legacy);
        assert_eq!(t.name, "Call customer about brakes");
        assert_eq!(t.date, "2026-08-22");
        assert_eq!(t.priority, Priority::Medium);
        assert!(!t.completed);
        assert_eq!(t.local_id.as_deref(), Some("17"));
    }

    #[test]
    fn migrate_legacy_row_with_multiline_description() {
        let legacy = serde_json::json!({
            "id": 42,
            "time": "2026-08-22",
            "description": "Top priority\nCheck front pads\nRear drums too",
            "completed": true,
            "source": "adeal-schedule"
        });
        let t = migrate_legacy_task(legacy);
        assert_eq!(t.name, "Top priority");
        assert!(t.description.contains("Rear drums too"));
        assert!(t.completed);
    }

    #[test]
    fn migrate_legacy_row_with_no_date_defaults_to_today() {
        let legacy = serde_json::json!({
            "id": 1,
            "description": "stranded row",
            "completed": false
        });
        let t = migrate_legacy_task(legacy);
        assert!(TaskList::validate_date(&t.date), "date must be YYYY-MM-DD, got {}", t.date);
    }

    #[test]
    fn current_schema_loads_without_migration() {
        let path = std::env::temp_dir().join("mc-tasks-test-current.json");
        let tasks = vec![
            Task::new("a".into(), "desc-a".into(), Priority::High, "2026-08-29".into()),
            Task::new("b".into(), "desc-b".into(), Priority::Low, "2026-08-29".into()),
        ];
        save_tasks(&path, &tasks).unwrap();
        let loaded = load_tasks(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "a");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn legacy_schema_loads_via_migration() {
        let path = std::env::temp_dir().join("mc-tasks-test-legacy.json");
        let legacy = r#"[
            {"id": 1, "time": "2026-08-22", "description": "first", "completed": false, "source": "adeal-schedule"},
            {"id": 2, "time": "2026-08-22", "description": "second", "completed": true,  "source": "adeal-schedule"}
        ]"#;
        fs::write(&path, legacy).unwrap();
        let loaded = load_tasks(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "first");
        assert_eq!(loaded[1].name, "second");
        assert!(loaded[1].completed);
        // Migration should have re-saved in current schema form.
        let raw = fs::read_to_string(&path).unwrap();
        let v: Vec<Value> = serde_json::from_str(&raw).unwrap();
        assert!(v[0].get("local_id").is_some(), "migrated file must use new schema");
        assert!(v[0].get("priority").is_some());
        fs::remove_file(&path).ok();
    }
}
