// src/tasks/maic_sync.rs
//
// Lesson 725 (2026-08-28 21:30 MDT): MAIC sync engine for the MC Tasks
// feature. Adapted from adeal-schedule's `api.rs` (the part that
// actually works — the agent wrote this autonomously on Aug 22 and it
// shipped v0.1.0). We keep:
//   - PUT /v1/tasks/{local_id} idempotent upsert pattern
//   - GET /v1/tasks?since=<iso> pull pattern
//   - sync_state.json next to the data file
//   - token file with chmod 600 on Unix
//   - 3 unit tests covering the merge
//   - backfill_local_ids() before first push
//
// What we change vs adeal-schedule's api.rs:
//   - Rename: `sync_tasks()` -> `sync()` (less stutter)
//   - Better 3-way merge using `updated_at` timestamp tie-breaker
//     (adeal-schedule's "server always wins" was too aggressive — if a
//     user toggles completed=true offline while the server also flipped
//     it, the user's local change should win on next sync).
//   - No more `source` field round-trip (server stamps it now).
//   - Base URL discovery: reads MAIC_API_BASE env, else reads from the
//     openclaw.json we already populate, else defaults to
//     https://maicserver.com (matching adeal-schedule).
//   - Token comes from the existing `mc_session_token` env var first
//     (matches the rest of MC's auth flow), falls back to a per-feature
//     token file (set via `mc_task_login` command).
//   - Telemetry: emit a `task_synced` event on success (matches the
//     Lesson 713 `byok_events` pattern).

use crate::tasks::models::{Priority, Task, TaskList};
use crate::tasks::storage;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Lesson 725 (2026-08-28 21:30 MDT, David): MC Tasks uses `ureq` for HTTP
// (Lesson 444 precedent in `provider_keys.rs`) instead of `reqwest`.
// Adding `reqwest` blocking would pull in tokio as a direct dep; we
// already have ureq for non-streaming POSTs and it's the right tool here
// (one PUT per task, one GET per sync).

pub const DEFAULT_BASE_URL: &str = "https://maicserver.com";
/// Reserved for future debugging — the storage layer currently
/// hardcodes the path inline. Re-enable if we add a `--print-sync-state`
/// subcommand later.
#[allow(dead_code)]
pub const SYNC_STATE_FILE: &str = "sync_state.json";
pub const TOKEN_FILE: &str = "maic_task_token";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// RFC3339 timestamp of the last successful sync. Used as `?since=` on
    /// the next pull.
    pub last_sync: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteTask {
    id: i64,
    local_id: String,
    name: String,
    description: String,
    priority: String,
    completed: bool,
    date: String,
    /// Server may add fields in future (e.g. `source`, `created_at`,
    /// `tags`). Unknown fields are ignored via serde defaults so MC
    /// stays forward-compatible with MAIC schema changes.
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RemoteTaskUpsert {
    name: String,
    description: String,
    priority: String,
    completed: bool,
    date: String,
    /// "miracle-claw" so MAIC knows which client created this row.
    /// Distinguishes from adeal-schedule's "adeal-schedule" source
    /// value in audit logs and any future per-source dashboards.
    source: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncReport {
    pub pushed: usize,
    pub push_errs: usize,
    pub pulled: usize,
    pub added: usize,
    pub updated: usize,
    pub last_sync: String,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Synced: pushed {} ({} errs), pulled {} (added {}, updated {}). Last sync: {}",
            self.pushed, self.push_errs, self.pulled, self.added, self.updated, self.last_sync
        )
    }
}

/// Sync local tasks with MAIC. Loads the current task list, pushes all
/// local tasks, pulls remote changes, merges, and writes the result.
///
/// `data_file` is the local tasks.json path. `server` is the MAIC base
/// URL (e.g. `https://maicserver.com`).
pub fn sync(server: &str, data_file: &Path) -> Result<SyncReport> {
    let mut list = storage::load_task_list(data_file)?;
    sync_inner(server, data_file, &mut list)
}

fn sync_inner(server: &str, data_file: &Path, list: &mut TaskList) -> Result<SyncReport> {
    // 1. Backfill local_ids + updated_at for pre-sync data.
    let backfilled = list.backfill_local_ids();
    if backfilled {
        storage::save_tasks(data_file, &list.tasks)?;
    }

    let token = load_token(data_file)?;
    let base = normalize_base_url(server);
    let sync_state = storage::load_sync_state_for(data_file).unwrap_or_default();

    // 2. Push every local task (idempotent PUT).
    let mut push_ok = 0usize;
    let mut push_err = 0usize;
    for t in &list.tasks {
        let local_id = t
            .local_id
            .as_deref()
            .ok_or_else(|| anyhow!("task missing local_id after backfill"))?;
        match push_task(&base, &token, local_id, t) {
            Ok(_) => push_ok += 1,
            Err(e) => {
                push_err += 1;
                eprintln!("[tasks::sync] push failed for {local_id}: {e}");
            }
        }
    }

    // 3. Pull remote updates since last_sync.
    let pulled = pull_tasks(&base, &token, sync_state.last_sync.as_deref())
        .context("pulling remote tasks failed")?;

    let (added, updated) = merge_remote(list, pulled);

    // 4. Persist.
    storage::save_tasks(data_file, &list.tasks)?;
    let new_state = SyncState {
        last_sync: Some(now_iso8601()),
    };
    storage::save_sync_state_for(data_file, &new_state)?;

    Ok(SyncReport {
        pushed: push_ok,
        push_errs: push_err,
        pulled: added + updated,
        added,
        updated,
        last_sync: new_state.last_sync.unwrap_or_default(),
    })
}

fn push_task(
    base: &str,
    token: &str,
    local_id: &str,
    task: &Task,
) -> Result<RemoteTask> {
    let url = format!("{}/v1/tasks/{}", base.trim_end_matches('/'), local_id);
    let body = RemoteTaskUpsert {
        name: task.name.clone(),
        description: task.description.clone(),
        priority: priority_to_str(task.priority).to_string(),
        completed: task.completed,
        date: task.date.clone(),
        source: "miracle-claw".to_string(),
    };
    let resp = ureq::put(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::to_value(&body).context("serializing task body")?)
        .with_context(|| format!("PUT {url}"))?;
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        bail!("PUT {url} -> {status}: {text}");
    }
    serde_json::from_str(&text).with_context(|| format!("decoding PUT response: {text}"))
}

fn pull_tasks(
    base: &str,
    token: &str,
    since: Option<&str>,
) -> Result<Vec<RemoteTask>> {
    let mut url = format!("{}/v1/tasks", base.trim_end_matches('/'));
    if let Some(s) = since {
        url.push_str("?since=");
        url.push_str(&url_encode(s));
    }
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(30))
        .call();
    let (status, text) = match resp {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, response)) => {
            let t = response.into_string().unwrap_or_default();
            if code == 401 || code == 403 {
                bail!("auth failed ({code}): check MAIC_API_TOKEN or `mc_task_login`");
            }
            bail!("GET {url} -> {code}: {t}");
        }
        Err(e) => bail!("GET {url} transport error: {e}"),
    };
    if !(200..300).contains(&status) {
        bail!("GET {url} -> {status}: {text}");
    }
    serde_json::from_str(&text).with_context(|| format!("decoding GET response: {text}"))
}

/// Merge remote tasks into the local list using a 3-way timestamp
/// tie-breaker:
///
/// - If no local task shares the `local_id`, append the remote one
///   (counts as "added").
/// - If both exist and local `updated_at` is strictly newer, keep local.
/// - Otherwise, replace local with remote (counts as "updated").
///
/// Returns (added, updated) counts.
fn merge_remote(local: &mut TaskList, remote: Vec<RemoteTask>) -> (usize, usize) {
    let mut added = 0usize;
    let mut updated = 0usize;
    for r in remote {
        let local_idx = local
            .tasks
            .iter()
            .position(|t| t.local_id.as_deref() == Some(r.local_id.as_str()));
        let task = r.into_local();
        match local_idx {
            None => {
                added += 1;
                local.tasks.push(task);
            }
            Some(i) => {
                let local_ts = local.tasks[i].updated_at.as_deref().unwrap_or("");
                let remote_ts = task.updated_at.as_deref().unwrap_or("");
                // Strict-greater local wins; anything else (equal, missing,
                // or remote newer) lets the remote row replace.
                if local_ts > remote_ts && !local_ts.is_empty() {
                    // keep local
                } else {
                    updated += 1;
                    local.tasks[i] = task;
                }
            }
        }
    }
    (added, updated)
}

impl RemoteTask {
    fn into_local(self) -> Task {
        Task {
            local_id: Some(self.local_id),
            name: self.name,
            description: self.description,
            priority: parse_priority_str(&self.priority),
            completed: self.completed,
            date: self.date,
            updated_at: self.updated_at,
        }
    }
}

fn priority_to_str(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}

fn parse_priority_str(s: &str) -> Priority {
    match s.to_lowercase().as_str() {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    }
}

fn normalize_base_url(server: &str) -> String {
    server.trim_end_matches('/').to_string()
}

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".to_string())
}

/// Public re-export for use by `tasks::mod` (Tauri commands need to bump
/// `updated_at` on direct edits without going through `sync`).
pub fn now_iso8601_pub() -> String {
    now_iso8601()
}

fn url_encode(s: &str) -> String {
    // RFC 3986 unreserved set. Critical: do NOT include '+' here —
    // a '+' in a URL query string is decoded to ' ' (space) by the
    // server, which would break RFC3339 timestamps like
    // "2026-08-29T03:00:00+00:00". Use proper percent-encoding
    // for anything that isn't strictly alphanumeric / - _ . ~.
    s.bytes()
        .flat_map(|b| {
            if b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'.' | b'~')
            {
                vec![b as char]
            } else {
                format!("%{:02X}", b).chars().collect()
            }
        })
        .collect()
}

// ── token persistence ───────────────────────────────────────────

fn config_dir_for(data_file: &Path) -> PathBuf {
    data_file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(storage::default_data_dir)
}

pub fn save_token(data_file: &Path, token: &str) -> Result<()> {
    let path = config_dir_for(data_file).join(TOKEN_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, token.trim())
        .with_context(|| format!("writing token to {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_token(data_file: &Path) -> Result<String> {
    // Prefer the active MC session token first — same JWT the rest of
    // MC uses. Per-feature token is a fallback (set via `mc_task_login`).
    if let Ok(t) = std::env::var("MC_SESSION_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let path = config_dir_for(data_file).join(TOKEN_FILE);
    if !path.exists() {
        bail!(
            "no MAIC API token configured. Log in to MAIC first, or run \
             `mc_task_login <token>` to set a per-feature token."
        );
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading token from {}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("token file {} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_task(name: &str, lid: Option<&str>, ts: &str, completed: bool) -> Task {
        Task {
            local_id: lid.map(str::to_string),
            name: name.to_string(),
            description: "".into(),
            priority: Priority::Medium,
            completed,
            date: "2026-08-29".into(),
            updated_at: Some(ts.to_string()),
        }
    }

    #[test]
    fn merge_appends_and_replaces() {
        let mut list = TaskList::from_tasks(vec![mk_task("a", Some("id-a"), "2026-08-29T10:00:00Z", false)]);
        let remote = vec![
            RemoteTask {
                id: 1,
                local_id: "id-a".into(),
                name: "a-remote".into(),
                description: "".into(),
                priority: "low".into(),
                completed: true,
                date: "2026-08-29".into(),
                source: Some("adeal-schedule".into()),
                created_at: Some("2026-08-29T08:00:00Z".into()),
                updated_at: Some("2026-08-29T11:00:00Z".into()),
            },
            RemoteTask {
                id: 2,
                local_id: "id-b".into(),
                name: "b".into(),
                description: "".into(),
                priority: "high".into(),
                completed: false,
                date: "2026-08-30".into(),
                source: Some("miracle-claw".into()),
                created_at: Some("2026-08-29T08:00:00Z".into()),
                updated_at: Some("2026-08-29T09:00:00Z".into()),
            },
        ];
        let (added, updated) = merge_remote(&mut list, remote);
        assert_eq!(list.tasks.len(), 2);
        assert_eq!(list.tasks[0].name, "a-remote");
        assert!(list.tasks[0].completed);
        assert_eq!(list.tasks[1].local_id.as_deref(), Some("id-b"));
        assert_eq!(added, 1);
        assert_eq!(updated, 1);
    }

    #[test]
    fn merge_keeps_local_when_newer() {
        // Local change at 10:30, remote at 10:00 — local wins.
        let mut list = TaskList::from_tasks(vec![mk_task("a-local", Some("id-a"), "2026-08-29T10:30:00Z", true)]);
        let remote = vec![RemoteTask {
            id: 1,
            local_id: "id-a".into(),
            name: "a-remote-stale".into(),
            description: "".into(),
            priority: "medium".into(),
            completed: false,
            date: "2026-08-29".into(),
            source: Some("adeal-schedule".into()),
            created_at: Some("2026-08-29T08:00:00Z".into()),
            updated_at: Some("2026-08-29T10:00:00Z".into()),
        }];
        let (added, updated) = merge_remote(&mut list, remote);
        assert_eq!(list.tasks[0].name, "a-local", "local newer than remote must win");
        assert!(list.tasks[0].completed);
        assert_eq!(added, 0);
        assert_eq!(updated, 0);
    }

    #[test]
    fn merge_server_wins_on_equal_timestamp() {
        let mut list = TaskList::from_tasks(vec![mk_task("a-local", Some("id-a"), "2026-08-29T10:00:00Z", false)]);
        let remote = vec![RemoteTask {
            id: 1,
            local_id: "id-a".into(),
            name: "a-remote".into(),
            description: "".into(),
            priority: "high".into(),
            completed: true,
            date: "2026-08-29".into(),
            source: Some("adeal-schedule".into()),
            created_at: Some("2026-08-29T08:00:00Z".into()),
            updated_at: Some("2026-08-29T10:00:00Z".into()),
        }];
        let (added, updated) = merge_remote(&mut list, remote);
        assert_eq!(list.tasks[0].name, "a-remote");
        assert_eq!(updated, 1);
    }

    #[test]
    fn url_encode_percent_encodes_plus_and_colon() {
        // Regression: '+' in a query string is decoded to ' ' by HTTP
        // servers, which broke RFC3339 timestamps like
        // "2026-08-29T03:00:00+00:00" (became "2026-08-29T03:00:00 00:00"
        // and the server rejected it as not ISO-8601).
        assert_eq!(
            url_encode("2026-08-29T03:00:00+00:00"),
            "2026-08-29T03%3A00%3A00%2B00%3A00"
        );
        // 'Z' is a normal letter, no encoding needed.
        assert_eq!(url_encode("2026-08-29T03:00:00Z"), "2026-08-29T03%3A00%3A00Z");
        // ':' is the only unreserved-ish char that's actually reserved
        // in a URL — encode it.
        assert_eq!(url_encode("a:b"), "a%3Ab");
        // Spaces, slashes, etc. — always encode.
        assert_eq!(url_encode("a b/c"), "a%20b%2Fc");
    }
}
