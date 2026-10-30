//! Integration test for the real workspace execution engine.
//!
//! `workspace_exec::execute_request` is the code that replaces the v1 stub
//! that returned a fake "Acknowledged: {title}" receipt. This test drives
//! the REAL function against a mock MAIC that speaks the same wire
//! protocol as production:
//!
//!   POST /v1/chat/completions  (tool_execution: "client")
//!     → { finish_reason: "tool_calls", tool_calls: [...] }
//!     → client executes them locally
//!     → POST again with the tool results appended
//!     → { finish_reason: "stop", message: { content: "..." } }
//!
//! It uses a tiny hand-rolled HTTP server (no new dev-dependencies) so it
//! runs in CI with no network and no model backend.
//!
//! What this actually protects against: the stub was "fine" because it
//! never did anything. These assertions fail if the tool loop stops
//! executing tools locally, if the tool result isn't fed back, or if the
//! final answer isn't parsed back out.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

/// Spin up a mock MAIC chat endpoint.
///
/// `responses` is consumed one per POST. The handler records every
/// request body it received so the test can assert on what was sent.
fn spawn_mock_maic(responses: Vec<Value>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let received: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_c = Arc::clone(&received);
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_c = Arc::clone(&counter);

    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream: TcpStream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let idx = counter_c.fetch_add(1, Ordering::SeqCst);
            let body = read_http_body(&mut stream);
            received_c.lock().unwrap().push(body);

            let payload = responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| json!({"choices": [{"message": {"content": "done"}}]}));
            let resp = payload.to_string();
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp.len(),
                resp
            );
            let _ = stream.write_all(http.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://{}", addr), received)
}

/// Read the HTTP request body (Content-Length framed) from a socket.
fn read_http_body(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return String::new();
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body);
    }
    String::from_utf8_lossy(&body).to_string()
}

#[test]
fn executes_a_client_tool_locally_and_returns_the_final_answer() {
    // Response 1: the model asks for read_file.
    // Response 2: the model has the tool result and answers.
    let responses = vec![
        json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/etc/hostname\"}"
                        }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "The hostname is testhost.\n\nI read /etc/hostname locally and found testhost."
                }
            }]
        }),
    ];

    let (endpoint, received) = spawn_mock_maic(responses);

    let request = json!({
        "kind": "message",
        "title": "What is the hostname?",
        "text": "Read /etc/hostname and tell me the hostname.",
        "needs_approval_for": []
    });

    let outcome = miracle_claw_lib::workspace_exec::execute_request(&endpoint, "test-jwt", &request)
        .expect("execution should succeed");

    // The final answer must be split into a summary + details.
    assert_eq!(outcome.summary, "The hostname is testhost.");
    assert!(
        outcome.details.contains("I read /etc/hostname locally"),
        "details should carry the full answer, got: {}",
        outcome.details
    );

    // The tool must be recorded as used — that's what tells the user
    // the desktop actually did something.
    assert_eq!(
        outcome.tools_used,
        vec!["read_file".to_string()],
        "the local tool call should be recorded"
    );

    // Two round trips: one to request the tool, one to deliver the answer.
    let bodies = received.lock().unwrap();
    assert_eq!(bodies.len(), 2, "expected 2 chat completions");

    // The FIRST request must advertise our local tools and ask for
    // client-side execution. Without tool_execution=client, MAIC would
    // try to execute read_file server-side (and fail).
    let first: Value = serde_json::from_str(&bodies[0]).expect("first body is json");
    assert_eq!(
        first.get("tool_execution").and_then(|v| v.as_str()),
        Some("client"),
        "must request client-side tool execution"
    );
    let tools = first.get("tools").and_then(|v| v.as_array()).expect("tools array");
    assert!(!tools.is_empty(), "must advertise local tools to the model");
    let names: Vec<String> = tools
        .iter()
        .filter_map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        names.contains(&"read_file".to_string()),
        "read_file must be advertised, got: {:?}",
        names
    );

    // The SECOND request must carry the assistant's tool-call message
    // AND the tool result. If the tool result were missing, the model
    // would have no way to know what the file contained.
    let second: Value = serde_json::from_str(&bodies[1]).expect("second body is json");
    let messages = second
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages array");
    let tool_msg = messages
        .iter()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .expect("second request must contain a tool result message");
    assert_eq!(
        tool_msg.get("tool_call_id").and_then(|v| v.as_str()),
        Some("call_1"),
        "tool result must reference the tool call id"
    );
    let tool_content = tool_msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !tool_content.is_empty(),
        "tool result content must not be empty"
    );
}

#[test]
fn a_note_with_no_text_fails_cleanly() {
    // An empty task should produce a clear error rather than a confusing
    // model failure. The caller turns this into status: "failed" with the
    // reason, which the phone shows to the user.
    let request = json!({
        "kind": "message",
        "title": "Empty task",
        "text": "   ",
        "needs_approval_for": []
    });

    let err = miracle_claw_lib::workspace_exec::execute_request(
        "http://127.0.0.1:1", // never contacted — we fail before that
        "test-jwt",
        &request,
    )
    .expect_err("empty text should be an error");

    assert!(
        err.contains("no text"),
        "error should explain the problem, got: {}",
        err
    );
}

#[test]
fn a_model_error_is_reported_not_swallowed() {
    // If MAIC is unreachable the user must be told, not left with a
    // silent failure.
    let request = json!({
        "kind": "message",
        "title": "Do a thing",
        "text": "Please do the thing.",
        "needs_approval_for": []
    });

    let err = miracle_claw_lib::workspace_exec::execute_request(
        "http://127.0.0.1:1",
        "test-jwt",
        &request,
    )
    .expect_err("unreachable endpoint should be an error");

    assert!(
        !err.is_empty(),
        "error message must not be empty"
    );
}
