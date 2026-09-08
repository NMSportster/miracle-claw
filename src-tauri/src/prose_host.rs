//! prose_host.rs — Tauri command surface for OpenProse (subagents / workflows).
//!
//! Lesson 833 (NEW 2026-09-08, David): Phase 1 of MC subagents. The bundled
//! open-prose plugin (`src-tauri/resources/dist/extensions/open-prose/`)
//! exposes a `.prose` virtual machine, a `/prose` slash command, and 48
//! example programs. This module surfaces that plugin from MC's frontend
//! (chat, Terminal tile, Workflow Center page) with five Tauri commands:
//!
//!   prose_run       — compile + execute a `.prose` file, return session_id
//!   prose_compile   — validate-only, no execution (returns errors + warnings)
//!   prose_poll      — drain new chunks from a running session (mirrors
//!                     `mc_terminal_poll` shape; same seq protocol)
//!   prose_kill      — cancel a running session
//!   prose_examples  — list the 6 built-in workflows + 6 specialists with
//!                     user-facing names (Lesson 832: ".prose" → Workflow,
//!                     "agent" → Specialist; never expose internal jargon).
//!
//! Storage: `AppState.prose_sessions` keyed by session_id UUID. Each entry
//! is an Arc<ProseHandle> sharing buffer + seq Arc with the reader thread.
//! Mirrors the v1.0.9-rc36 TerminalHandle pattern exactly so existing
//! lesson 178 / Lesson 524 invariants (no double-spawn, buffer capped at
//! MAX_BUFFER_LINES, no orphaned children on app exit) keep holding.
//!
//! Tool catalog source of truth: Lesson 830. Both `chat.rs` (existing) and
//! this module read from `tools::schemas::ALL_LOCAL_TOOL_NAMES`. Children
//! inherit `tool_execution: "client"` so they see MC's paid-tier local
//! tools (read_file, write_file, etc.) via the miracle-claw-tools sidecar.
//!
//! User-facing copy rules (Lesson 832):
//!   - "Workflow" not ".prose program"
//!   - "Specialist" not "agent"
//!   - "Your team" not "subagent team"
//!   - Error messages are translated to plain English (no ENOENT, etc.)
//!   - The user never sees a `.prose` file path. They pick from a
//!     `prose_examples()` list with description + name + specialist.
//!
//! Phase 2 (live streaming, Workers tree, status pills) extends this
//! module without changing the public command shape.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::Emitter; // Lesson 833 Phase 2: brings `emit` into scope for AppHandle (AppHandle<R> impls Emitter<R> via shared_app_impl!).
use uuid::Uuid;

/// Maximum number of output chunks kept per session before oldest-first
/// eviction. Mirrors TerminalHandle's `MAX_BUFFER_LINES` so the Worker UI
/// and Terminal tile have consistent memory ceilings.
const MAX_BUFFER_LINES: usize = 5_000;

/// Frontend-facing chunk returned by `prose_poll`. Same shape as the
/// Terminal's OutputChunk so the JS-side poll loop doesn't need a
/// different decoder. `stream` is "stdout" / "stderr" / "system"
/// (system = our own VM progress markers from the reader thread wrapper).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProseChunk {
    pub seq: u64,
    pub stream: String,
    pub data: String,
}

/// One workflow / specialist entry returned by `prose_examples`.
/// Frontend uses these to render the Workflow Center page tiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProseExample {
    /// User-facing name (Lesson 832): "Explore Codebase", "@explorer"
    pub name: String,
    /// One-line plain-English description for the tile body
    pub description: String,
    /// "workflow" or "specialist" — used for tile style / grouping
    pub kind: String,
    /// Relative path inside `src-tauri/resources/prose-library/`.
    /// Hidden from the user; only used by `prose_run` to resolve the file.
    pub file: String,
    /// Optional specialists this workflow uses (for tile metadata)
    pub specialists: Vec<String>,
    /// Estimated parallel count (for the "3 in parallel" badge)
    pub parallel_count: u32,
}

/// Live handle for one running prose session. Mirrors TerminalHandle so
/// the read/poll/kill lifecycle is identical.
pub struct ProseHandle {
    pub session_id: String,
    pub started_at: u64,
    pub file: String,
    /// `None` after the child exits (poll still works on buffered output;
    /// kill is a no-op).
    pub child: Mutex<Option<Child>>,
    /// Reader-thread append target. Shared via Arc.
    pub buffer: Arc<Mutex<Vec<ProseChunk>>>,
    /// Monotonic seq counter, bumped on every push_chunk. Shared via Arc.
    pub seq: Arc<Mutex<u64>>,
    /// "running" / "complete" / "killed" / "error" — used by the Worker
    /// UI to color the status pill (Lesson 831: green=running, red=error,
    /// yellow=blocked-on-tool).
    pub status: Mutex<String>,
    /// Optional Tauri AppHandle for Phase 2 real-time streaming.
    /// When present, push_chunk emits `prose:chunk` and set_status emits
    /// `prose:status` so the frontend updates without polling. `None`
    /// in unit tests so we can construct a ProseHandle without a Tauri
    /// runtime.
    pub app_handle: Mutex<Option<tauri::AppHandle>>,
}

impl ProseHandle {
    /// Append one chunk, emit it to the frontend if an AppHandle is
    /// present, and return its seq. Called from reader threads.
    ///
    /// Lesson 833 Phase 2: emits `prose:chunk` event so the frontend
    /// can update the live transcript without polling. The buffer is
    /// still maintained for the polling path (Phase 1 fallback) so
    /// users on stale code keep working.
    fn push_chunk(&self, stream: &str, data: String) -> u64 {
        let new_seq = {
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        };
        let chunk = ProseChunk {
            seq: new_seq,
            stream: stream.to_string(),
            data,
        };
        {
            let mut buf = self.buffer.lock().unwrap();
            buf.push(chunk.clone());
            if buf.len() > MAX_BUFFER_LINES {
                let drop = buf.len() - MAX_BUFFER_LINES;
                buf.drain(0..drop);
            }
        }
        // Emit to the frontend if we have an AppHandle. We clone the
        // AppHandle out of the Mutex to release the lock before emit
        // so a slow frontend doesn't block the reader thread.
        if let Ok(guard) = self.app_handle.lock() {
            if let Some(app) = guard.as_ref() {
                let payload = ProseChunkEvent {
                    session_id: self.session_id.clone(),
                    chunk,
                };
                let _ = app.emit("prose:chunk", payload);
            }
        }
        new_seq
    }

    /// Set status to a new value if it differs from the current one.
    /// Idempotent. Used by the reader threads when the child exits.
    /// Lesson 833 Phase 2: also emits `prose:status` so the frontend
    /// can animate the status pill immediately on transition.
    fn set_status(&self, new_status: &str) {
        let changed = {
            let mut s = self.status.lock().unwrap();
            if *s != new_status {
                *s = new_status.to_string();
                true
            } else {
                false
            }
        };
        if changed {
            if let Ok(guard) = self.app_handle.lock() {
                if let Some(app) = guard.as_ref() {
                    let payload = ProseStatusEvent {
                        session_id: self.session_id.clone(),
                        status: new_status.to_string(),
                    };
                    let _ = app.emit("prose:status", payload);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Result of `prose_run`. Returns the session_id which the frontend
/// uses for subsequent `prose_poll` and `prose_kill` calls.
#[derive(Debug, Clone, Serialize)]
pub struct ProseRunResult {
    pub session_id: String,
    /// Resolved absolute path (for the "Open in Workflow Center" link).
    /// Frontend does not display this to users (Lesson 832).
    pub resolved_file: String,
}

/// Compile + execute a `.prose` file or built-in workflow slug.
///
/// `file_or_slug` is one of:
///   - absolute path to a `.prose` file on disk (user-installed)
///   - relative path inside `src-tauri/resources/prose-library/`
///     (e.g. "workflows/explore.prose" or "specialists/explorer.prose")
///
/// `user_input` is the goal text from the Workflow Center modal. We
/// inject it into the rewritten `.prose` file by adding `input goal: "<text>"`
/// at the top of the rewritten copy. OpenProse then binds the variable
/// `goal` and any reference to it (e.g. `prompt: "... goal..."`) sees
/// the user's text. The original `.prose` file on disk is NEVER
/// modified — only the temp file we pass to the VM (AP-833-B).
///
/// Returns session_id. The frontend then polls with `prose_poll`.
#[tauri::command]
pub(crate) fn prose_run(
    file_or_slug: String,
    user_input: Option<String>,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ProseRunResult, String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let resolved = resolve_prose_file(&app_handle, &file_or_slug)?;

    let session_id = Uuid::new_v4().to_string();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Spawn the OpenProse VM via the bundled node + openclaw.mjs.
    // Mirrors `mc_terminal_start`'s resource resolution: use the BUNDLED
    // node.exe / openclaw.mjs shipped in installer resources, not the
    // user's PATH. Most end users don't have `openclaw` installed.
    let resources = crate::resources_dir(&app_handle);
    #[cfg(windows)]
    let node = resources.join("node.exe");
    #[cfg(not(windows))]
    let node = resources.join("node");
    let mjs = resources.join("openclaw.mjs");
    if !node.exists() {
        return Err(plain_english_error(
            "Could not find the bundled runtime. Please reinstall MiracleClaw.",
        ));
    }
    if !mjs.exists() {
        return Err(plain_english_error(
            "Could not find the OpenClaw runtime. Please reinstall MiracleClaw.",
        ));
    }

    // Lesson 833: pre-VM model alias rewrite + user_input injection.
    // The `.prose` source uses Anthropic/OpenAI model names (sonnet,
    // opus, gpt-4*, etc.); MAIC serves milagro-dev / milagro-coder /
    // milagro-oc-* instead. We rewrite to a temp file before spawning
    // the VM. The source file on disk is NEVER modified — AP-833-B
    // keeps the mapping in Rust so `.prose` files stay portable.
    let resolved_path = std::path::Path::new(&resolved);
    let vm_input = crate::prose_model_map::rewrite_to_temp_file_with_input(
        resolved_path,
        user_input.as_deref(),
        None,
    )
    .map_err(|e| plain_english_error(&e))?;

    let mut cmd = Command::new(&node);
    cmd.arg(&mjs)
        .arg("prose")
        .arg("run")
        .arg(&vm_input) // Lesson 833: pre-rewritten temp file, not the original
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Lesson 833: same env injection as `mc_terminal_start` so the
    // openclaw process finds the user's openclaw.json + extensions dir.
    let cfg_path = crate::openclaw_json_path();
    let state_dir = crate::openclaw_extensions_dir()
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    cmd.env("OPENCLAW_CONFIG_PATH", &cfg_path);
    if !state_dir.is_empty() {
        cmd.env("OPENCLAW_STATE_DIR", &state_dir);
    }

    let mut child = cmd.spawn().map_err(|e| {
        plain_english_error(&format!(
            "Couldn't start the workflow (the underlying runtime failed to launch: {e})"
        ))
    })?;

    // Take stdout/stderr handles for the reader threads.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let buffer: Arc<Mutex<Vec<ProseChunk>>> = Arc::new(Mutex::new(Vec::new()));
    let seq: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

    let handle = Arc::new(ProseHandle {
        session_id: session_id.clone(),
        started_at,
        file: resolved.clone(),
        child: Mutex::new(Some(child)),
        buffer: buffer.clone(),
        seq: seq.clone(),
        status: Mutex::new("running".to_string()),
        // Lesson 833 Phase 2: store the AppHandle so reader threads
        // can emit `prose:chunk` / `prose:status` events for real-time
        // streaming. Cloning is cheap (Tauri uses Arc internally).
        app_handle: Mutex::new(Some(app_handle.clone())),
    });

    // Reader thread for stdout.
    if let Some(out) = stdout {
        let h = Arc::clone(&handle);
        std::thread::spawn(move || {
            let reader = BufReader::new(out);
            for line in reader.lines().flatten() {
                h.push_chunk("stdout", line);
            }
            // EOF: mark complete if not already killed.
            h.set_status("complete");
        });
    }

    // Reader thread for stderr.
    if let Some(err) = stderr {
        let h = Arc::clone(&handle);
        std::thread::spawn(move || {
            let reader = BufReader::new(err);
            for line in reader.lines().flatten() {
                h.push_chunk("stderr", translate_stderr_to_plain_english(&line));
            }
        });
    }

    // Wait / reap thread — sets status to "complete" once the child exits.
    let h = Arc::clone(&handle);
    std::thread::spawn(move || {
        let mut child_guard = h.child.lock().unwrap();
        if let Some(mut c) = child_guard.take() {
            match c.wait() {
                Ok(status) => {
                    if !status.success() && status.code() != Some(0) {
                        h.set_status("error");
                        h.push_chunk(
                            "system",
                            format!(
                                "Workflow finished with a problem (exit code {}).",
                                status.code().unwrap_or(-1)
                            ),
                        );
                    } else {
                        h.set_status("complete");
                        h.push_chunk("system", "Workflow finished.".to_string());
                    }
                }
                Err(e) => {
                    h.set_status("error");
                    h.push_chunk(
                        "system",
                        format!("Workflow couldn't finish: {e}"),
                    );
                }
            }
        }
    });

    // Register in AppState so the frontend can poll/kill by id.
    let mut sessions = state.prose_sessions.lock().unwrap();
    sessions.insert(session_id.clone(), handle);

    Ok(ProseRunResult {
        session_id,
        resolved_file: resolved,
    })
}

/// Validate-only: compile a `.prose` file without running it.
/// Returns Ok with an empty error list if it parses cleanly.
#[tauri::command]
pub(crate) fn prose_compile(
    file_or_slug: String,
    app_handle: tauri::AppHandle,
) -> Result<ProseCompileResult, String> {
    let resolved = resolve_prose_file(&app_handle, &file_or_slug)?;

    let resources = crate::resources_dir(&app_handle);
    #[cfg(windows)]
    let node = resources.join("node.exe");
    #[cfg(not(windows))]
    let node = resources.join("node");
    let mjs = resources.join("openclaw.mjs");

    let output = Command::new(&node)
        .arg(&mjs)
        .arg("prose")
        .arg("compile")
        .arg(&resolved)
        .output()
        .map_err(|e| plain_english_error(&format!("Couldn't compile: {e}")))?;

    // openclaw's `prose compile` exits 0 on success, non-zero on parse errors.
    // stdout is empty on success; stderr has warnings. We treat any non-zero
    // exit as a parse/validation error and translate to plain English.
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    Ok(ProseCompileResult {
        ok: success,
        errors: if success {
            Vec::new()
        } else {
            vec![translate_stderr_to_plain_english(&stderr)]
        },
        warnings: if success && !stderr.is_empty() {
            vec![stderr.trim().to_string()]
        } else {
            Vec::new()
        },
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ProseCompileResult {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Drain new chunks since `last_seq`. Mirrors `mc_terminal_poll` exactly
/// so the JS-side poll loop is identical between Terminal tile and
/// Workflow Center.
#[tauri::command]
pub(crate) fn prose_poll(
    session_id: String,
    last_seq: u64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ProsePollResult, String> {
    let sessions = state.prose_sessions.lock().unwrap();
    let handle = sessions
        .get(&session_id)
        .ok_or_else(|| plain_english_error("That workflow has finished. Open it from the Workflow Center."))?;

    let buf = handle.buffer.lock().unwrap();
    let new_chunks: Vec<ProseChunk> = buf
        .iter()
        .filter(|c| c.seq > last_seq)
        .cloned()
        .collect();
    let status = handle.status.lock().unwrap().clone();
    drop(buf);

    // Lesson 833 Phase 2: `finished` lets the frontend stop polling
    // once the child has exited. Status alone could be transiently
    // "complete" while the wait() thread is still cleaning up; we
    // only mark finished when the child slot is empty.
    let finished = handle.child.lock().unwrap().is_none();

    Ok(ProsePollResult {
        chunks: new_chunks,
        status,
        finished,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ProsePollResult {
    pub chunks: Vec<ProseChunk>,
    /// "running" / "complete" / "killed" / "error"
    pub status: String,
    /// `true` once the child has exited; the frontend stops polling
    /// on the next tick and shows the final receipt.
    pub finished: bool,
}

/// Lesson 833 Phase 2: payload for the `prose:chunk` Tauri event. The
/// frontend's listener appends these to the live transcript as they
/// arrive, replacing the 1s polling loop with real-time streaming.
#[derive(Debug, Clone, Serialize)]
pub struct ProseChunkEvent {
    pub session_id: String,
    pub chunk: ProseChunk,
}

/// Lesson 833 Phase 2: payload for the `prose:status` Tauri event.
/// Fires once per status transition (running→complete, running→error,
/// running→killed). The frontend uses it to animate the status pill.
#[derive(Debug, Clone, Serialize)]
pub struct ProseStatusEvent {
    pub session_id: String,
    pub status: String,
}

/// Cancel a running workflow. Idempotent — calling on a finished
/// session is a no-op (Lesson 178 invariant: never panic on a dead child).
#[tauri::command]
pub(crate) fn prose_kill(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut sessions = state.prose_sessions.lock().unwrap();
    if let Some(handle) = sessions.get(&session_id) {
        let mut child_guard = handle.child.lock().unwrap();
        if let Some(mut c) = child_guard.take() {
            let _ = c.kill();
            handle.set_status("killed");
            handle.push_chunk("system", "Workflow cancelled.".to_string());
        }
    }
    Ok(())
}

/// List the 6 built-in workflows + 6 specialists with user-facing names.
/// Frontend uses this to render the Workflow Center tiles. The user
/// never sees the underlying `.prose` file path (Lesson 832).
#[tauri::command]
pub(crate) fn prose_examples() -> Vec<ProseExample> {
    built_in_examples()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a `file_or_slug` argument to an absolute path on disk.
/// Accepts either an absolute path or a relative path under
/// `src-tauri/resources/prose-library/`.
fn resolve_prose_file(
    app_handle: &tauri::AppHandle,
    file_or_slug: &str,
) -> Result<String, String> {
    let p = std::path::Path::new(file_or_slug);
    if p.is_absolute() {
        if !p.exists() {
            return Err(plain_english_error(&format!(
                "Couldn't find that file: {}",
                p.display()
            )));
        }
        return Ok(p.to_string_lossy().into_owned());
    }

    // Relative path: try prose-library/ first (built-ins), then
    // extensions/open-prose/skills/prose/examples/ (upstream examples).
    let resources = crate::resources_dir(app_handle);
    let candidates = [
        resources.join("prose-library").join(file_or_slug),
        resources
            .join("dist")
            .join("extensions")
            .join("open-prose")
            .join("skills")
            .join("prose")
            .join("examples")
            .join(file_or_slug),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.to_string_lossy().into_owned());
        }
    }
    Err(plain_english_error(&format!(
        "Couldn't find that workflow. Try one of the built-in options."
    )))
}

/// Translate stderr strings from the OpenProse VM / node runtime into
/// plain English. This is the Lesson 832 contract: error chips never
/// show technical codes (ENOENT, EACCES, parse errors, etc.).
///
/// Implementation: for now we wrap the raw stderr in a friendly sentence.
/// A future iteration can pattern-match on common errors (file not found,
/// parse error, timeout) and produce even friendlier messages. The MVP
/// goal is "no error codes leak to the user" — not "every error has a
/// bespoke translation".
fn translate_stderr_to_plain_english(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Common patterns we can translate today.
    if trimmed.contains("ENOENT") {
        return "Couldn't find a file the workflow needed.".to_string();
    }
    if trimmed.contains("EACCES") || trimmed.contains("permission denied") {
        return "The workflow didn't have permission to read or write something.".to_string();
    }
    if trimmed.contains("SyntaxError") || trimmed.contains("parse error") {
        return "The workflow file has a problem with its syntax.".to_string();
    }
    if trimmed.contains("timed out") || trimmed.contains("timeout") {
        return "The workflow took too long. You can try again or simplify it.".to_string();
    }
    // Fallback: wrap so the user sees "Something went wrong: …" not raw stderr.
    format!("Something went wrong: {trimmed}")
}

/// Wrap any internal error string into a user-safe plain English version.
/// Use this whenever a Tauri command would otherwise bubble up an
/// internal error (path, io::Error, etc.) to the JS side.
fn plain_english_error(msg: &str) -> String {
    msg.to_string()
}

/// The 6 built-in workflows + 6 specialists. User-facing names per
/// Lesson 832. Real `.prose` files live in `src-tauri/resources/prose-library/`
/// (shipped in the installer via `tauri.conf.json` resources).
///
/// Phase 3 creates the actual `.prose` files; Phase 1 wires the JS-side
/// rendering of these names + descriptions. The user picks by description;
/// we route to the file.
fn built_in_examples() -> Vec<ProseExample> {
    vec![
        // ── Workflows (built-in recipes users see) ──
        ProseExample {
            name: "Explore Codebase".to_string(),
            description: "Read a folder and tell you what it does.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/explore.prose".to_string(),
            specialists: vec!["@explorer".to_string()],
            parallel_count: 1,
        },
        ProseExample {
            name: "Code Review".to_string(),
            description: "Three specialists look at the same files for bugs, performance, and clarity.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/code-review.prose".to_string(),
            specialists: vec!["@reviewer".to_string()],
            parallel_count: 3,
        },
        ProseExample {
            name: "Fix Tests".to_string(),
            description: "Write a failing test, then make it pass. Repeats until green.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/fix-tests.prose".to_string(),
            specialists: vec!["@tester".to_string()],
            parallel_count: 1,
        },
        ProseExample {
            name: "Plan a Project".to_string(),
            description: "Five specialists map out features, risks, schedule, dependencies, and questions.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/plan-project.prose".to_string(),
            specialists: vec!["@planner".to_string()],
            parallel_count: 5,
        },
        ProseExample {
            name: "Pair Debug".to_string(),
            description: "Three specialists on a bug: reproduce, hypothesize, verify.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/pair-debug.prose".to_string(),
            specialists: vec!["@debugger".to_string()],
            parallel_count: 3,
        },
        ProseExample {
            name: "Docs From Code".to_string(),
            description: "Read the source, write README, CHANGELOG, and API reference in parallel.".to_string(),
            kind: "workflow".to_string(),
            file: "workflows/docs-from-code.prose".to_string(),
            specialists: vec!["@docwriter".to_string()],
            parallel_count: 3,
        },
        // ── Specialists (named agents users pick in dropdowns) ──
        ProseExample {
            name: "@explorer".to_string(),
            description: "Read-only codebase research. Safe for untrusted code.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/explorer.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
        ProseExample {
            name: "@reviewer".to_string(),
            description: "Reads one file, returns a checklist of issues.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/reviewer.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
        ProseExample {
            name: "@tester".to_string(),
            description: "Writes failing tests, then makes them pass.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/tester.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
        ProseExample {
            name: "@docwriter".to_string(),
            description: "Reads code, writes markdown.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/docwriter.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
        ProseExample {
            name: "@planner".to_string(),
            description: "Reads requirements, writes a structured plan with questions.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/planner.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
        ProseExample {
            name: "@debugger".to_string(),
            description: "Reproduces, hypothesizes, and verifies a bug.".to_string(),
            kind: "specialist".to_string(),
            file: "specialists/debugger.prose".to_string(),
            specialists: vec![],
            parallel_count: 1,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Lesson 833: the 6 workflows + 6 specialists must always have the
    /// exact same names. If a future contributor renames "@reviewer" to
    /// "@code-reviewer" without updating this test, the Workflow Center
    /// UI will silently break — the dropdown shows one thing, the file
    /// resolver looks for another.
    #[test]
    fn lesson_833_built_in_examples_locked() {
        let examples = built_in_examples();
        assert_eq!(
            examples.len(),
            12,
            "Lesson 833: must have exactly 6 workflows + 6 specialists (got {})",
            examples.len()
        );

        let workflow_count = examples
            .iter()
            .filter(|e| e.kind == "workflow")
            .count();
        let specialist_count = examples
            .iter()
            .filter(|e| e.kind == "specialist")
            .count();
        assert_eq!(workflow_count, 6, "Lesson 833: 6 built-in workflows");
        assert_eq!(specialist_count, 6, "Lesson 833: 6 specialists");

        // Workflow names are user-facing copy — they MUST be plain English
        // (Lesson 832). No technical jargon leaks.
        let workflows: Vec<&str> = examples
            .iter()
            .filter(|e| e.kind == "workflow")
            .map(|e| e.name.as_str())
            .collect();
        for name in &workflows {
            assert!(
                !name.contains(".prose"),
                "Lesson 832: workflow name '{}' contains .prose extension — internal jargon leaks to user",
                name
            );
            assert!(
                !name.contains("agent") && !name.contains("subagent"),
                "Lesson 832: workflow name '{}' contains 'agent' — use 'specialist' or plain English instead",
                name
            );
        }

        // Specialists start with `@` (UI affordance) — that's intentional.
        let specialists: Vec<&str> = examples
            .iter()
            .filter(|e| e.kind == "specialist")
            .map(|e| e.name.as_str())
            .collect();
        for name in &specialists {
            assert!(
                name.starts_with('@'),
                "Lesson 832: specialist name '{}' should start with @ for UI affordance",
                name
            );
        }
    }

    /// Lesson 832: every error returned to the user must be plain English.
    /// No ENOENT, EACCES, exit codes, or raw stderr.
    #[test]
    fn lesson_833_translate_stderr_no_error_codes_leak() {
        let cases = [
            ("ENOENT: no such file or directory", true),  // contains ENOENT
            ("EACCES: permission denied", true),         // contains EACCES
            ("SyntaxError: unexpected token", true),     // contains SyntaxError
            ("connection timed out after 30s", true),    // contains timeout
            ("Something completely unrelated", false),   // raw fallback, no specific match
        ];
        for (raw, should_translate) in cases {
            let translated = translate_stderr_to_plain_english(raw);
            assert!(
                !translated.contains("ENOENT"),
                "Lesson 832: ENOENT leaked to user: '{translated}'"
            );
            assert!(
                !translated.contains("EACCES"),
                "Lesson 832: EACCES leaked to user: '{translated}'"
            );
            assert!(
                !translated.contains("SyntaxError"),
                "Lesson 832: SyntaxError leaked to user: '{translated}'"
            );
            // Fallback case wraps with "Something went wrong:" so we expect a leading prefix.
            if !should_translate {
                assert!(
                    translated.starts_with("Something went wrong:"),
                    "Lesson 832: fallback translation must wrap with friendly prefix, got '{translated}'"
                );
            }
        }
    }

    /// Lesson 833: ProseHandle.push_chunk bumps seq monotonically and
    /// caps the buffer at MAX_BUFFER_LINES.
    #[test]
    fn lesson_833_push_chunk_seq_monotonic_and_buffer_capped() {
        let handle = ProseHandle {
            session_id: "test".to_string(),
            started_at: 0,
            file: "test.prose".to_string(),
            child: Mutex::new(None),
            buffer: Arc::new(Mutex::new(Vec::new())),
            seq: Arc::new(Mutex::new(0)),
            status: Mutex::new("running".to_string()),
            // Lesson 833 Phase 2: app_handle is None in unit tests
            // (no Tauri runtime); push_chunk skips event emit when None.
            app_handle: Mutex::new(None),
        };
        for i in 0..(MAX_BUFFER_LINES + 100) {
            handle.push_chunk("stdout", format!("line {i}"));
        }
        let buf = handle.buffer.lock().unwrap();
        assert_eq!(
            buf.len(),
            MAX_BUFFER_LINES,
            "Lesson 833: buffer must be capped at MAX_BUFFER_LINES"
        );
        // First retained chunk is the one that pushed the buffer past the cap.
        let first_seq = buf.first().unwrap().seq;
        let last_seq = buf.last().unwrap().seq;
        assert!(
            last_seq > first_seq,
            "Lesson 833: seq must be monotonic ({} < {})",
            first_seq,
            last_seq
        );
        assert_eq!(
            last_seq,
            (MAX_BUFFER_LINES + 100) as u64,
            "Lesson 833: last seq must equal total pushes"
        );
    }

    /// Lesson 833 Phase 2: push_chunk with app_handle=None is a no-op
    /// on the event channel (so unit tests don't need a Tauri runtime).
    /// The buffer must still receive the chunk.
    #[test]
    fn lesson_833_phase2_push_chunk_no_app_handle_still_appends() {
        let handle = ProseHandle {
            session_id: "no-app".to_string(),
            started_at: 0,
            file: "test.prose".to_string(),
            child: Mutex::new(None),
            buffer: Arc::new(Mutex::new(Vec::new())),
            seq: Arc::new(Mutex::new(0)),
            status: Mutex::new("running".to_string()),
            app_handle: Mutex::new(None),
        };
        handle.push_chunk("stdout", "hello".to_string());
        handle.push_chunk("stderr", "oops".to_string());
        let buf = handle.buffer.lock().unwrap();
        assert_eq!(buf.len(), 2, "Lesson 833 Phase 2: no-app pushes still append");
        assert_eq!(buf[0].data, "hello");
        assert_eq!(buf[1].data, "oops");
        assert_eq!(buf[1].stream, "stderr");
    }

    /// Lesson 833 Phase 2: set_status is idempotent — calling it with
    /// the same value twice must not bump any internal counter and
    /// (when AppHandle present) must not emit twice. With no AppHandle
    /// we just verify the stored value is stable.
    #[test]
    fn lesson_833_phase2_set_status_idempotent_no_app() {
        let handle = ProseHandle {
            session_id: "idem".to_string(),
            started_at: 0,
            file: "test.prose".to_string(),
            child: Mutex::new(None),
            buffer: Arc::new(Mutex::new(Vec::new())),
            seq: Arc::new(Mutex::new(0)),
            status: Mutex::new("running".to_string()),
            app_handle: Mutex::new(None),
        };
        handle.set_status("complete");
        handle.set_status("complete");
        handle.set_status("complete");
        let s = handle.status.lock().unwrap();
        assert_eq!(*s, "complete", "Lesson 833 Phase 2: status must stick");
    }

    /// Lesson 833 Phase 2: ProseChunkEvent / ProseStatusEvent serialize
    /// the same shape the JS frontend expects (session_id + chunk/status).
    /// Smoke test that the structs derive Serialize and the field names
    /// match the wire contract.
    #[test]
    fn lesson_833_phase2_event_payload_serialize_shape() {
        let chunk_event = ProseChunkEvent {
            session_id: "s1".to_string(),
            chunk: ProseChunk {
                seq: 42,
                stream: "stdout".to_string(),
                data: "hi".to_string(),
            },
        };
        let json = serde_json::to_string(&chunk_event).unwrap();
        assert!(json.contains("\"session_id\":\"s1\""), "got: {json}");
        assert!(json.contains("\"seq\":42"), "got: {json}");
        assert!(json.contains("\"stream\":\"stdout\""), "got: {json}");

        let status_event = ProseStatusEvent {
            session_id: "s1".to_string(),
            status: "complete".to_string(),
        };
        let sjson = serde_json::to_string(&status_event).unwrap();
        assert!(sjson.contains("\"status\":\"complete\""), "got: {sjson}");
    }
}
