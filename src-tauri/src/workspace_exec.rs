//! workspace_exec.rs — Real execution engine for workspace notes.
//!
//! This replaces the v1 `execute_note()` stub, which returned a fake
//! "Acknowledged: {title}" receipt without doing any work.
//!
//! ## How a note actually gets executed
//!
//! 1. We ask MAIC's OpenAI-compatible `/v1/chat/completions` route to
//!    fulfil the request, passing our LOCAL tool schemas with
//!    `tool_execution: "client"`. MAIC runs its own server-side tools
//!    (web_search, get_weather, …) and hands any *client* tool calls
//!    back to us in the response.
//! 2. We execute those client tool calls LOCALLY via
//!    `crate::tools::exec::dispatch` — reading files, running bash,
//!    fetching URLs — on the user's own machine.
//! 3. We feed the tool results back to MAIC and let the model produce
//!    the final answer.
//! 4. That answer becomes the note's `result` subkey, so the phone can
//!    render it.
//!
//! This is the same "the customer owns the compute" story the rest of
//! Miracle Claw is built on: the model lives in the cloud, the tools and
//! the data stay on the desktop.
//!
//! ## Safety
//!
//! Tool execution is bounded:
//!   - `MAX_TOOL_ITERATIONS` caps the round-trip loop so a confused
//!     model cannot spin forever.
//!   - The path allowlist in `crate::tools::exec` still applies to every
//!     filesystem tool.
//!   - `bash_run` runs through the same executor the chat UI uses, so
//!     it inherits the same policy.

use std::time::Duration;

use serde_json::{json, Value};

use crate::tools::schemas::{local_tool_to_openai, LocalToolName};

/// Maximum number of tool-call round trips for a single note.
const MAX_TOOL_ITERATIONS: usize = 8;

/// How long to wait for the model on each chat completion.
const MODEL_TIMEOUT: Duration = Duration::from_secs(120);

/// The result of executing a note.
pub struct ExecutionOutcome {
    /// Short summary shown in the phone's task list.
    pub summary: String,
    /// Full text, shown on the note detail screen.
    pub details: String,
    /// Which tools were actually invoked (for the "what did it do" line).
    pub tools_used: Vec<String>,
}

/// Execute a note's request against MAIC's model + local tools.
///
/// `endpoint` is the MAIC base URL (no `/v1` suffix), `jwt` the bearer
/// token, and `request` the note's `request` subkey value.
///
/// Returns `Err` with a human-readable message if the note could not be
/// executed — the caller writes that as a `failed` status so the phone
/// can show the user *why* it didn't work, rather than leaving a task
/// stuck in `in_progress` forever.
pub fn execute_request(
    endpoint: &str,
    jwt: &str,
    request: &Value,
) -> Result<ExecutionOutcome, String> {
    let title = request
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("(untitled task)");
    let text = request
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if text.trim().is_empty() {
        return Err("Task has no text to act on.".into());
    }

    // The system prompt tells the model what environment it is acting
    // in and that it actually has tools. Without the tool schemas
    // reaching the model it will just describe what it *would* do,
    // which is the same fake-receipt problem the stub had.
    let system = "You are Miracle Claw, an AI assistant running on the user's own desktop \
computer. You have been asked to carry out a task that the user sent from their phone.\n\n\
You have real tools that execute ON THE USER'S MACHINE. Use them whenever the task \
requires it. If you can actually do something with a tool, do it rather than \
describing how you would do it.\n\n\
When you have finished, reply with a short one-line summary of what you actually did, \
followed by a blank line and then the details of what happened. Be honest about \
failures — if a tool errored, say so.\n\n\
The task title is: "
        .to_string()
        + title;

    let mut messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": text}),
    ];

    // Our local tools, advertised to the model as client-side.
    let tools: Vec<Value> = crate::tools::schemas::all_local_tools()
        .iter()
        .map(local_tool_to_openai)
        .collect();

    let mut tools_used: Vec<String> = Vec::new();

    for _iteration in 0..MAX_TOOL_ITERATIONS {
        let response = chat_completion(endpoint, jwt, &messages, &tools)?;

        // If the model asked for tools, run them locally and feed the
        // results back.
        let tool_calls = response
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("tool_calls"))
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        if tool_calls.is_empty() {
            // Final answer.
            let content = response
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            if content.is_empty() {
                return Err("The model returned an empty response.".into());
            }

            let (summary, details) = split_summary(&content);
            return Ok(ExecutionOutcome {
                summary,
                details,
                tools_used,
            });
        }

        // Record the assistant message that requested the tools so the
        // conversation stays valid for the next round trip.
        let assistant_msg = response
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .unwrap_or_else(|| json!({"role": "assistant"}));
        messages.push(assistant_msg);

        for tc in &tool_calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args: Value = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));

            if name.is_empty() {
                messages.push(json!({
                    "role": "tool", "tool_call_id": id,
                    "content": "Error: missing tool name."
                }));
                continue;
            }

            tools_used.push(name.to_string());

            // Execute locally. Unknown tool names become a clean error
            // message back to the model rather than a panic — MAIC's
            // protocol treats unknown tools as client-side.
            let outcome = match LocalToolName::from_str(name) {
                LocalToolName::ReadFile
                | LocalToolName::WriteFile
                | LocalToolName::EditFile
                | LocalToolName::ListDir
                | LocalToolName::BashRun
                | LocalToolName::ApplyPatch
                | LocalToolName::RememberFact
                | LocalToolName::WebFetch => {
                    crate::tools::exec::dispatch(&LocalToolName::from_str(name), &args)
                }
            };

            let content = match outcome {
                Ok(s) => s,
                Err(e) => format!("Error: {}", e),
            };

            messages.push(json!({
                "role": "tool", "tool_call_id": id, "content": content
            }));
        }
    }

    Err(format!(
        "Task did not complete after {} tool rounds.",
        MAX_TOOL_ITERATIONS
    ))
}

/// Split the model's final answer into a short summary line and the
/// full details.
///
/// The model is asked for "summary, blank line, details", but models
/// don't always comply — so fall back to the first paragraph / first
/// line if there's no blank-line separator.
fn split_summary(content: &str) -> (String, String) {
    let normalized = content.replace("\r\n", "\n");
    if let Some(idx) = normalized.find("\n\n") {
        let summary = normalized[..idx].trim().to_string();
        let details = normalized[idx..].trim_start_matches('\n').to_string();
        if !summary.is_empty() {
            return (summary, details);
        }
    }
    // No blank line. Use the first sentence-ish chunk as the summary.
    let first_line = normalized.lines().next().unwrap_or("").trim().to_string();
    if first_line.is_empty() {
        let trimmed = normalized.trim().to_string();
        return (trimmed.clone(), trimmed);
    }
    (first_line, normalized.trim().to_string())
}

/// POST to MAIC's `/v1/chat/completions` and return the parsed response.
fn chat_completion(
    endpoint: &str,
    jwt: &str,
    messages: &[Value],
    tools: &[Value],
) -> Result<Value, String> {
    let base = endpoint.trim_end_matches('/');
    // The endpoint may or may not already carry `/v1`.
    let base = if base.ends_with("/v1") {
        base.to_string()
    } else {
        format!("{}/v1", base)
    };
    let url = format!("{}/chat/completions", base);

    let payload = json!({
        "model": "milagro-oc-minimax",
        "messages": messages,
        "tools": tools,
        "tool_execution": "client",
        "temperature": 0.2,
    });

    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    let agent = ureq::AgentBuilder::new()
        .timeout(MODEL_TIMEOUT)
        .build();

    let resp = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {}", jwt))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(|e| match e {
            ureq::Error::Status(401 | 403, _) => "Model request rejected — please sign in again.".to_string(),
            ureq::Error::Status(code, r) => {
                let b = r.into_string().unwrap_or_default();
                format!("Model request failed (HTTP {}): {}", code, b)
            }
            other => format!("Model request failed: {}", other),
        })?;

    let text = resp.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str::<Value>(&text).map_err(|e| format!("Bad model response: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_summary_splits_on_blank_line() {
        let (s, d) = split_summary("Turned on the shop lights.\n\nUsed the relay panel.");
        assert_eq!(s, "Turned on the shop lights.");
        assert_eq!(d, "Used the relay panel.");
    }

    #[test]
    fn split_summary_falls_back_to_first_line() {
        let (s, d) = split_summary("All done. Nothing else to report.");
        assert_eq!(s, "All done. Nothing else to report.");
        assert_eq!(d, "All done. Nothing else to report.");
    }

    #[test]
    fn split_summary_handles_empty() {
        let (s, d) = split_summary("");
        assert!(s.is_empty());
        assert!(d.is_empty());
    }

    #[test]
    fn split_summary_tolerates_crlf() {
        let (s, d) = split_summary("Line one.\r\n\r\nLine two.");
        assert_eq!(s, "Line one.");
        assert_eq!(d, "Line two.");
    }
}
