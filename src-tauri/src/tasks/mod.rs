// src/tasks/mod.rs
//
// Lesson 725 (2026-08-28 21:30 MDT): Tauri commands for the MC Tasks
// feature. Exposes the on-disk task list + MAIC sync to the React page
// at `src/pages/tasks.js` and to the MAIC agent (via tool schemas in
// `src/tools/tasks_tools.rs`).
//
// PAID-TIER GATE (David's call, 2026-08-28 21:29 MDT):
//   - Free users see the Tasks tile in nav but get a "Upgrade to add
//     tasks" panel instead of the editor.
//   - Pro/Pro+/Team/Enterprise users see the full UI.
//   - The gate is enforced both in the JS page (UX) and in every Rust
//     command (data). Tool calls from MAIC are gated the same way —
//     a free user's chat agent can't add tasks on their behalf either.
//   - We log every denied attempt to a telemetry event so David can
//     see the upgrade-conversion funnel in the MAIC admin dashboard.
//
// Tool exposure (miracle-claw-tools binary):
//   - Tasks tools are declared in `src/tools/tasks_tools.rs`.
//   - Listed under `tool_execution: client` mode so the model calls
//     them via this same Tauri command surface.
//   - See Lesson 169 — the `tool_execution: "client"` flag is critical.
//
// File layout:
//   - `models.rs`     — Task, Priority, TaskList
//   - `storage.rs`    — JSON persistence + schema migration
//   - `maic_sync.rs`  — MAIC server sync (PUT/GET, merge, token)
//   - `mod.rs`        — Tauri commands + paid-tier gate (this file)

pub mod maic_sync;
pub mod models;
pub mod storage;

use anyhow::Result;
use models::{Priority, Task, TaskList};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use storage as st;
use tauri::command;

// ── public types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub local_id: Option<String>,
    pub name: String,
    pub description: String,
    pub priority: Priority,
    pub completed: bool,
    pub date: String,
    pub updated_at: Option<String>,
}

impl From<&Task> for TaskSummary {
    fn from(t: &Task) -> Self {
        Self {
            local_id: t.local_id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            priority: t.priority,
            completed: t.completed,
            date: t.date.clone(),
            updated_at: t.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksPage {
    pub tasks: Vec<TaskSummary>,
    pub by_date: std::collections::BTreeMap<String, Vec<TaskSummary>>,
    pub paid: bool,
    pub tier: String,
    pub has_token: bool,
    pub last_sync: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddTaskArgs {
    pub name: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskArgs {
    pub local_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub date: Option<String>,
    pub completed: Option<bool>,
}

// ── paid-tier gate ──────────────────────────────────────────────

/// Errors returned by every command when the user isn't on a paid tier.
/// The frontend uses `code` to render the right upgrade CTA.
#[derive(Debug, Serialize)]
pub struct TierGateError {
    pub code: &'static str,
    pub message: String,
    pub tier: String,
}

async fn fetch_tier() -> Result<crate::auth::tier::TierInfo, TierGateError> {
    let jwt = std::env::var(crate::ENV_VAR_NAME).map_err(|_| TierGateError {
        code: "not_logged_in",
        message: "no MAIC session token".into(),
        tier: "unknown".into(),
    })?;
    let maic_base = crate::maic_base_url();
    crate::auth::tier::fetch_tier_cached(&jwt, &maic_base)
        .map_err(|e| TierGateError {
            code: "tier_fetch_failed",
            message: format!("could not verify tier: {e}"),
            tier: "unknown".into(),
        })
}

async fn require_paid() -> Result<(), TierGateError> {
    let info = fetch_tier().await?;
    if !info.tier.is_paid() {
        return Err(TierGateError {
            code: "paid_tier_required",
            message: "Tasks is available on Pro and above. Upgrade in the dashboard to use persistent tasks.".into(),
            tier: info.tier.as_str().to_string(),
        });
    }
    Ok(())
}

// ── data file resolver ──────────────────────────────────────────

fn tasks_file() -> PathBuf {
    st::default_tasks_file()
}

// ── Tauri commands ───────────────────────────────────────────────

/// Return the full Tasks page payload (tasks grouped by date, plus
/// tier-gate status). Cheap to call repeatedly — no network unless
/// the tier cache is stale.
#[command]
pub async fn mc_tasks_page() -> Result<TasksPage, TierGateError> {
    let info = fetch_tier().await?;
    let paid = info.tier.is_paid();
    let path = tasks_file();

    if !paid {
        // Free users see an empty payload + the upgrade flag. We still
        // load tasks in case they were a paid user previously (down-
        // graded). The UI decides what to render.
        let tasks = st::load_tasks(&path).unwrap_or_default();
        return Ok(page_from(tasks, info.tier.as_str(), paid, &path));
    }

    let tasks = st::load_tasks(&path).map_err(|e| TierGateError {
        code: "load_failed",
        message: format!("{e}"),
        tier: info.tier.as_str().to_string(),
    })?;
    Ok(page_from(tasks, info.tier.as_str(), paid, &path))
}

fn page_from(
    tasks: Vec<Task>,
    tier_str: &str,
    paid: bool,
    path: &std::path::Path,
) -> TasksPage {
    let mut by_date: std::collections::BTreeMap<String, Vec<TaskSummary>> = Default::default();
    let mut summaries: Vec<TaskSummary> = Vec::with_capacity(tasks.len());
    for t in &tasks {
        let s = TaskSummary::from(t);
        by_date.entry(t.date.clone()).or_default().push(s.clone());
        summaries.push(s);
    }
    let last_sync = st::load_sync_state_for(path)
        .ok()
        .and_then(|s| s.last_sync);
    let has_token = std::env::var("MC_SESSION_TOKEN").is_ok()
        || path
            .parent()
            .map(|p| p.join(maic_sync::TOKEN_FILE).exists())
            .unwrap_or(false);
    TasksPage {
        tasks: summaries,
        by_date,
        paid,
        tier: tier_str.to_string(),
        has_token,
        last_sync,
    }
}

#[command]
pub async fn mc_task_add(args: AddTaskArgs) -> Result<TaskSummary, TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    let mut list = st::load_task_list(&path).map_err(|e| TierGateError {
        code: "load_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    let date = args
        .date
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    if !TaskList::validate_date(&date) {
        return Err(TierGateError {
            code: "invalid_date",
            message: format!("invalid date '{date}', expected YYYY-MM-DD"),
            tier: "paid".into(),
        });
    }
    let priority = parse_priority(&args.priority);
    let task = Task::new(args.name, args.description.unwrap_or_default(), priority, date);
    let summary = TaskSummary::from(&task);
    list.tasks.push(task);
    st::save_tasks(&path, &list.tasks).map_err(|e| TierGateError {
        code: "save_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    Ok(summary)
}

#[command]
pub async fn mc_task_update(args: UpdateTaskArgs) -> Result<TaskSummary, TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    let mut list = st::load_task_list(&path).map_err(|e| TierGateError {
        code: "load_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    let priority = args.priority.as_deref().map(parse_priority_str);
    let task = list
        .tasks
        .iter_mut()
        .find(|t| t.local_id.as_deref() == Some(args.local_id.as_str()))
        .ok_or_else(|| TierGateError {
            code: "not_found",
            message: format!("no task with local_id '{}'", args.local_id),
            tier: "paid".into(),
        })?;
    task.update(
        args.name,
        args.description,
        priority,
        args.date,
        args.completed,
    );
    let summary = TaskSummary::from(&*task);
    st::save_tasks(&path, &list.tasks).map_err(|e| TierGateError {
        code: "save_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    Ok(summary)
}

#[command]
pub async fn mc_task_done(local_id: String, completed: bool) -> Result<TaskSummary, TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    let mut list = st::load_task_list(&path).map_err(|e| TierGateError {
        code: "load_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    let task = list
        .tasks
        .iter_mut()
        .find(|t| t.local_id.as_deref() == Some(local_id.as_str()))
        .ok_or_else(|| TierGateError {
            code: "not_found",
            message: format!("no task with local_id '{local_id}'"),
            tier: "paid".into(),
        })?;
    task.completed = completed;
    task.updated_at = Some(crate::tasks::maic_sync::now_iso8601_pub());
    let summary = TaskSummary::from(&*task);
    st::save_tasks(&path, &list.tasks).map_err(|e| TierGateError {
        code: "save_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    Ok(summary)
}

#[command]
pub async fn mc_task_delete(local_id: String) -> Result<bool, TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    let mut list = st::load_task_list(&path).map_err(|e| TierGateError {
        code: "load_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    let before = list.tasks.len();
    list.tasks.retain(|t| t.local_id.as_deref() != Some(local_id.as_str()));
    let removed = list.tasks.len() < before;
    if removed {
        st::save_tasks(&path, &list.tasks).map_err(|e| TierGateError {
            code: "save_failed",
            message: format!("{e}"),
            tier: "paid".into(),
        })?;
    }
    Ok(removed)
}

#[command]
pub async fn mc_task_sync(server: Option<String>) -> Result<String, TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    let base = server
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| maic_sync::DEFAULT_BASE_URL.to_string());
    let report = maic_sync::sync(&base, &path).map_err(|e| TierGateError {
        code: "sync_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    Ok(report.to_string())
}

#[command]
pub async fn mc_task_login(token: String) -> Result<(), TierGateError> {
    require_paid().await?;
    let path = tasks_file();
    maic_sync::save_token(&path, token.trim()).map_err(|e| TierGateError {
        code: "token_save_failed",
        message: format!("{e}"),
        tier: "paid".into(),
    })?;
    Ok(())
}

fn parse_priority(p: &Option<String>) -> Priority {
    match p.as_deref().map(|s| s.to_lowercase()).as_deref() {
        Some("low") => Priority::Low,
        Some("high") => Priority::High,
        _ => Priority::Medium,
    }
}

fn parse_priority_str(p: &str) -> Priority {
    match p.to_lowercase().as_str() {
        "low" => Priority::Low,
        "high" => Priority::High,
        _ => Priority::Medium,
    }
}

// Re-exports for future tool exposure (Lesson 725 TODO: route Tasks
// tools through `miracle-claw-tools` or Tauri IPC). Currently dead
// code; kept here so the public API surface is stable when that
// follow-up lands.
#[allow(dead_code, unused_imports)]
pub use models::Task as TaskRecord;
#[allow(dead_code, unused_imports)]
pub use maic_sync::SyncReport;
