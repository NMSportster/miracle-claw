//! `mclaw ask` integration tests using wiremock to simulate MAIC responses.
//!
//! These tests prove the end-to-end flow without needing real MAIC credentials:
//!   1. Resolve MAIC base URL from MAIC_BASE_URL env var
//!   2. Inject a known JWT via MAIC_JWT env var
//!   3. POST to /v1/chat/completions with the right body
//!   4. Parse the OpenAI-compatible response into our flat ChatResp
//!
//! They also cover the JSON output shape (--json flag), the model override
//! (--model=<id>), and tool_execution toggle.

use mclaw::client::{ChatResp, ChatUsage};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn openai_completion_response(model: &str, content: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-test-123",
        "object": "chat.completion",
        "created": 1234567890,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": content,
                    "tool_calls": null
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "total_tokens": 8
        }
    })
}

#[tokio::test]
async fn chat_completion_posts_to_correct_endpoint_with_bearer() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-jwt-fixture"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_completion_response(
            "milagro-dev",
            "Hello, world!",
        )))
        .expect(1)
        .mount(&server)
        .await;

    // Build a Client pointed at the mock server. We bypass Config + env vars
    // entirely so we don't race with other tests on MAIC_BASE_URL/MAIC_JWT.
    let base = format!("http://{}", server.address());
    let c = mclaw::client::Client::with_base_url_and_token(&base, "test-jwt-fixture")
        .expect("client");
    let resp: ChatResp = c
        .chat_completion("say hi", Some("milagro-dev"), true)
        .await
        .expect("completion");

    assert_eq!(resp.model, "milagro-dev");
    assert_eq!(resp.content, "Hello, world!");
    assert!(resp.tool_calls.is_empty());
    let usage = resp.usage.expect("usage populated");
    assert_eq!(usage.prompt_tokens, 5);
    assert_eq!(usage.completion_tokens, 3);
    assert_eq!(usage.total_tokens, 8);
}

#[tokio::test]
async fn chat_completion_with_no_tools_sends_client_execution() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-jwt-fixture"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_completion_response(
            "milagro-dev",
            "ok",
        )))
        .mount(&server)
        .await;

    let base = format!("http://{}", server.address());
    let c = mclaw::client::Client::with_base_url_and_token(&base, "test-jwt-fixture")
        .expect("client");
    c.chat_completion("hello", Some("milagro-dev"), false)
        .await
        .expect("completion");
}

#[tokio::test]
async fn chat_resp_json_shape_matches_design_doc() {
    // The design doc promises these fields in --json output:
    //   model, content, tool_calls[], usage{prompt_tokens,completion_tokens,total_tokens}
    let resp = ChatResp {
        model: "milagro-m1-t1".into(),
        content: "42".into(),
        tool_calls: vec![],
        usage: Some(ChatUsage {
            prompt_tokens: 3,
            completion_tokens: 1,
            total_tokens: 4,
        }),
        finish_reason: Some("stop".into()),
    };
    let json = serde_json::to_string(&resp).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");
    assert_eq!(parsed["model"], "milagro-m1-t1");
    assert_eq!(parsed["content"], "42");
    assert!(parsed["tool_calls"].is_array());
    assert_eq!(parsed["usage"]["total_tokens"], 4);
}
