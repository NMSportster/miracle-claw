// Lesson 1136 (NEW 2026-10-09 ~07:30 MDT, David): regression tests
// for the model catalog capability-field backfill. The bug pattern
// we're guarding against:
//
//   Fresh install seeds chat.json with bare `{id, name}` entries
//   for every model. MC's request builder reads `maxTokens` from
//   each model entry and falls back to 8192 when missing. Reasoning
//   models like `chat-nemotron-super` (native cap 65K) get
//   truncated at ~2K output tokens → `stopReason=length` →
//   "Agent couldn't generate a response".
//
// `backfill_model_capability_fields` self-heals these entries on
// the next MC launch. Tests below cover the three behaviors:
//
//   1. bare entry gets all three fields populated
//   2. partially-populated entry only gets the missing fields
//   3. user-added custom model is left alone (no MODEL_SPEC match)
//
// The function is `pub` on `miracle_claw_lib` so it can be called
// from this integration test.

use miracle_claw_lib::backfill_model_capability_fields;
use serde_json::{json, Value};

#[test]
fn backfill_patches_bare_entry() {
    let mut cfg = json!({
        "models": {
            "providers": {
                "maic": {
                    "models": [
                        {"id": "chat-nemotron-super", "name": "Chat Nemotron Super"},
                    ]
                }
            }
        }
    });
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(changed, "should report a change for bare entry");
    let entry = &cfg["models"]["providers"]["maic"]["models"][0];
    assert_eq!(entry["maxTokens"], json!(65536));
    assert_eq!(entry["contextWindow"], json!(262144));
    assert_eq!(entry["reasoning"], json!(true));
}

#[test]
fn backfill_only_fills_missing_fields() {
    let mut cfg = json!({
        "models": {
            "providers": {
                "maic": {
                    "models": [
                        // User explicitly overrode maxTokens to 4096 —
                        // backfill must NOT clobber it.
                        {"id": "chat-nemotron-super", "name": "x", "maxTokens": 4096},
                    ]
                }
            }
        }
    });
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(changed, "should still report a change for the missing fields");
    let entry = &cfg["models"]["providers"]["maic"]["models"][0];
    assert_eq!(entry["maxTokens"], json!(4096), "user value preserved");
    assert_eq!(entry["contextWindow"], json!(262144), "missing field filled");
    assert_eq!(entry["reasoning"], json!(true), "missing field filled");
}

#[test]
fn backfill_skips_user_added_custom_models() {
    let mut cfg = json!({
        "models": {
            "providers": {
                "maic": {
                    "models": [
                        // "my-custom-llm" is not in MODEL_SPEC, so it
                        // must be left completely untouched.
                        {"id": "my-custom-llm", "name": "My Custom LLM"},
                    ]
                }
            }
        }
    });
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(!changed, "custom model should not be reported as changed");
    let entry = &cfg["models"]["providers"]["maic"]["models"][0];
    assert_eq!(entry, &json!({"id": "my-custom-llm", "name": "My Custom LLM"}));
}

#[test]
fn backfill_handles_all_known_model_ids() {
    // Every id in MODEL_SPEC should be patchable. Catch typos in the
    // spec table (e.g. "milagro-dev" vs "milagro-devloper") that would
    // make a real model silently fall through to defaults.
    let known_ids = [
        "milagro-m1-t1", "milagro-m1-t2", "milagro-m1-t3",
        "milagro-dev", "milagro-dev-coder",
        "milagro-m1", "milagro-chat", "milagro-coder", "milagro-stock",
        "milagro-oc-minimax", "milagro-oc-glm", "milagro-oc-qwen",
        "milagro-oc-deepseek", "milagro-oc-kimi",
        "chat-glm", "chat-qwen", "chat-deepseek",
        "chat-nemotron-nano", "chat-nemotron-super", "chat-nemotron-ultra",
    ];
    for id in known_ids {
        let mut cfg = json!({
            "models": {
                "providers": {
                    "maic": {"models": [{"id": id, "name": id}]}
                }
            }
        });
        let changed = backfill_model_capability_fields(&mut cfg);
        assert!(changed, "expected patch for known model id: {id}");
        let entry = &cfg["models"]["providers"]["maic"]["models"][0];
        assert!(entry.get("maxTokens").is_some_and(|v| v.as_u64().unwrap() > 0), "{id}: maxTokens missing or zero");
        assert!(entry.get("contextWindow").is_some_and(|v| v.as_u64().unwrap() > 0), "{id}: contextWindow missing or zero");
        assert!(entry.get("reasoning").is_some(), "{id}: reasoning missing");
    }
}

#[test]
fn backfill_returns_false_when_no_changes_needed() {
    let mut cfg = json!({
        "models": {
            "providers": {
                "maic": {
                    "models": [
                        {"id": "chat-nemotron-super", "name": "x",
                         "maxTokens": 65536, "contextWindow": 262144, "reasoning": true},
                    ]
                }
            }
        }
    });
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(!changed, "should report no change when entry is already complete");
}

#[test]
fn backfill_handles_missing_providers_block() {
    // Defensive: a partially-constructed cfg (no `models.providers`)
    // must not panic.
    let mut cfg = json!({});
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(!changed);
}

#[test]
fn backfill_handles_models_not_array() {
    // Defensive: if `models` is a string or other non-array, don't
    // panic — just skip.
    let mut cfg = json!({
        "models": {
            "providers": {
                "maic": {"models": "not-an-array"}
            }
        }
    });
    let changed = backfill_model_capability_fields(&mut cfg);
    assert!(!changed);
}
