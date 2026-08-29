// src/tasks/models.rs
//
// Lesson 725 (2026-08-28 21:30 MDT, David): Tasks feature for MC v1.1.0+
// (Miracle Bot persistent memory). Models copy/adapted from the
// adeal-schedule v0.1.0 test program (`C:\Users\Adeal\Documents\adeal-schedule\`)
// shipped autonomously by the MC-openclaw agent on 2026-08-22.
//
// What's kept:
//   - `Task` struct with `local_id: Option<String>` (UUID, used as the
//     MAIC primary key for idempotent upserts).
//   - `Priority` enum (low/medium/high).
//   - `backfill_local_ids()` for migrating old JSON files that predate
//     the MAIC sync.
//   - `ensure_local_id()` for in-place UUID assignment.
//
// What's added vs adeal-schedule:
//   - `source` field (was String in adeal-schedule's RemoteTask, dropped
//     here — we don't need to round-trip it; server stamps it).
//   - `description` is `String` not `String` with Vec variants — single
//     canonical type now.
//   - `updated_at` field for the new "last-modified" tie-breaker in the
//     merge logic (adeal-schedule's server-wins is too aggressive).

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Task priority. Renders as `low` / `medium` / `high` everywhere.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Medium
    }
}

/// One task. The struct is the wire format — JSON-serialized to
/// `<data_dir>/tasks.json` and PUT/POSTed to MAIC's `/v1/tasks/{local_id}`.
///
/// `local_id` is the **stable** key across devices. We assign a UUID v4
/// when the task is first created locally, and the server uses it as the
/// idempotency key (PUT /v1/tasks/{local_id} is upsert-by-local_id, not
/// by server id). This is the same pattern adeal-schedule shipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// UUID v4, stable across sync. Server-side row uses this as the
    /// primary key. `Option` in serialized form so we can read pre-sync
    /// JSON files; if missing on load we backfill before saving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_id: Option<String>,

    /// Short, human label. Shown in the list and as the chat title.
    pub name: String,

    /// Long-form details. Optional in the UI, defaults to "".
    #[serde(default)]
    pub description: String,

    pub priority: Priority,

    pub completed: bool,

    /// ISO date `YYYY-MM-DD`. This is the bucket index — `list` filters
    /// by date, the dashboard groups by date.
    pub date: String,

    /// RFC3339 timestamp of the last write. Used as the tie-breaker in
    /// the 3-way merge during sync (server wins on equal timestamps,
    /// local wins if newer, server wins if newer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl Task {
    pub fn new(name: String, description: String, priority: Priority, date: String) -> Self {
        Self {
            local_id: Some(uuid::Uuid::new_v4().to_string()),
            name,
            description,
            priority,
            completed: false,
            date,
            updated_at: Some(now_iso8601()),
        }
    }

    pub fn ensure_local_id(&mut self) -> &str {
        if self.local_id.is_none() {
            self.local_id = Some(uuid::Uuid::new_v4().to_string());
        }
        self.local_id.as_deref().unwrap()
    }

    /// Update fields. `None` means "leave unchanged". Always bumps
    /// `updated_at` so the local change wins over a stale server row.
    pub fn update(
        &mut self,
        name: Option<String>,
        description: Option<String>,
        priority: Option<Priority>,
        date: Option<String>,
        completed: Option<bool>,
    ) {
        if let Some(name) = name { self.name = name; }
        if let Some(description) = description { self.description = description; }
        if let Some(priority) = priority { self.priority = priority; }
        if let Some(date) = date { self.date = date; }
        if let Some(completed) = completed { self.completed = completed; }
        self.updated_at = Some(now_iso8601());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskList {
    pub tasks: Vec<Task>,
}

impl TaskList {
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self { tasks }
    }

    /// All tasks for a specific date, with their global indices preserved
    /// (so callers can map "task index 3 for 2026-08-29" back to the row
    /// in `tasks`).
    /// Reserved helper for future use (e.g. paginated day view). Not
    /// currently called — the React page does its own grouping.
    #[allow(dead_code)]
    pub fn tasks_for_date(&self, date: &str) -> Vec<(usize, &Task)> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.date == date)
            .collect()
    }

    pub fn validate_date(date: &str) -> bool {
        NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
    }

    /// Backfill `local_id` (UUID) and `updated_at` for tasks that lack
    /// them. Used during schema migration when loading old JSON files
    /// that predate the MAIC sync.
    pub fn backfill_local_ids(&mut self) -> bool {
        let mut changed = false;
        for t in &mut self.tasks {
            if t.local_id.is_none() {
                t.ensure_local_id();
                changed = true;
            }
            if t.updated_at.is_none() {
                t.updated_at = Some(now_iso8601());
                changed = true;
            }
        }
        changed
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfill_assigns_local_ids() {
        let mut list = TaskList::from_tasks(vec![
            Task::new("a".into(), "".into(), Priority::Medium, "2026-08-29".into()),
            Task::new("b".into(), "".into(), Priority::Medium, "2026-08-29".into()),
        ]);
        list.tasks[0].local_id = None;
        list.tasks[1].local_id = None;
        assert!(list.backfill_local_ids());
        let lids: Vec<_> = list.tasks.iter().map(|t| t.local_id.clone().unwrap()).collect();
        assert_ne!(lids[0], lids[1]);
    }

    #[test]
    fn backfill_is_noop_when_present() {
        let mut list = TaskList::from_tasks(vec![
            Task::new("a".into(), "".into(), Priority::Medium, "2026-08-29".into()),
        ]);
        let original = list.tasks[0].local_id.clone();
        assert!(!list.backfill_local_ids());
        assert_eq!(list.tasks[0].local_id, original);
    }

    #[test]
    fn update_bumps_timestamp() {
        let mut t = Task::new("a".into(), "".into(), Priority::Medium, "2026-08-29".into());
        let before = t.updated_at.clone();
        // now_iso8601 is second-precision (RFC3339 without nanos). Sleep
        // >1s to guarantee the second rolls over before the second
        // timestamp is generated.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        t.update(Some("a-renamed".into()), None, None, None, None);
        assert_eq!(t.name, "a-renamed");
        assert!(t.updated_at > before, "update must bump updated_at");
    }
}
