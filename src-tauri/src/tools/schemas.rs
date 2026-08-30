//! The 7 local tool schemas MC advertises to the model via the MAIC plugin.
//!
//! Wire format: OpenAI-compatible JSON Schema for function-calling tools.
//! The plugin (`depot/maic-plugin/index.js`) re-encodes these Rust structs
//! into the JSON the OpenAI SDK expects. **Keep these in sync with the
//! plugin's tool implementations** — when a parameter changes here, the
//! plugin's executor must change too.
//!
//! Free users: not advertised (MAIC will return `tool_calls` only for tools
//! in the request body; missing = not callable).
//!
//! Paid users: all 7 advertised.

#![allow(dead_code, unused_imports)]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// The factory functions (read_file, write_file, etc.) are used by the
// `miracle-claw` lib to build the Tool list it returns from
// `mc_list_tools`. The `miracle-claw-tools` binary doesn't call them
// (it uses `exec::dispatch` directly), so each binary gets a different
// "used" set. The schema types (LocalTool, LocalToolName, ALL_LOCAL_TOOL_NAMES)
// are shared between both. Silence the noise at the module level — these
// are public API surface that the lib *could* call even if today's paths
// don't.

/// Stable identifiers. Use these constants instead of string literals so
/// renames are caught at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LocalToolName {
    ReadFile,
    WriteFile,
    EditFile,
    ListDir,
    BashRun,
    ApplyPatch,
    RememberFact,
    // Lesson 803 (rc55.13): web_fetch — fetch a URL and return the
    // raw response (or rendered markdown when the response is HTML).
    // MAIC also exposes a server-side `web_search` tool (Brave search),
    // but the model couldn't load arbitrary URLs. Adding it here makes
    // the link-following workflow whole: search → fetch. Paid-tier
    // only — the plugin only registers paid tools, so it never reaches
    // a Free user's tool list.
    WebFetch,
}

impl LocalToolName {
    pub fn as_str(self) -> &'static str {
        match self {
            LocalToolName::ReadFile => "read_file",
            LocalToolName::WriteFile => "write_file",
            LocalToolName::EditFile => "edit_file",
            LocalToolName::ListDir => "list_dir",
            LocalToolName::BashRun => "bash_run",
            LocalToolName::ApplyPatch => "apply_patch",
            LocalToolName::RememberFact => "remember_fact",
            LocalToolName::WebFetch => "web_fetch",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "read_file" => LocalToolName::ReadFile,
            "write_file" => LocalToolName::WriteFile,
            "edit_file" => LocalToolName::EditFile,
            "list_dir" => LocalToolName::ListDir,
            "bash_run" => LocalToolName::BashRun,
            "apply_patch" => LocalToolName::ApplyPatch,
            "remember_fact" => LocalToolName::RememberFact,
            "web_fetch" => LocalToolName::WebFetch,
            _ => LocalToolName::ReadFile, // safe default
        }
    }
}

pub const ALL_LOCAL_TOOL_NAMES: &[LocalToolName] = &[
    LocalToolName::ReadFile,
    LocalToolName::WriteFile,
    LocalToolName::EditFile,
    LocalToolName::ListDir,
    LocalToolName::BashRun,
    LocalToolName::ApplyPatch,
    LocalToolName::RememberFact,
    LocalToolName::WebFetch,
];

/// One tool, with its OpenAI-compatible JSON schema.
#[derive(Debug, Clone, Serialize)]
pub struct LocalTool {
    pub name: LocalToolName,
    pub description: &'static str,
    pub parameters: Value,
}

pub fn read_file() -> LocalTool {
    LocalTool {
        name: LocalToolName::ReadFile,
        description: "Read the contents of a file. Path must be under Documents/, Desktop/, Downloads/, or the workspace root.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file (Windows or WSL style)."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to return (default 1048576 = 1 MiB, hard cap 10 MiB). Refuse if file is larger.",
                    "minimum": 1,
                    "maximum": 10485760
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    }
}

pub fn write_file() -> LocalTool {
    LocalTool {
        name: LocalToolName::WriteFile,
        description: "Create or overwrite a file. Path must be under Documents/, Desktop/, Downloads/, or the workspace root. Writes are blocked for files larger than 10MB.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file (Windows or WSL style)."
                },
                "content": {
                    "type": "string",
                    "description": "The full file contents to write."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
    }
}

pub fn edit_file() -> LocalTool {
    LocalTool {
        name: LocalToolName::EditFile,
        description: "Edit a file by replacing an exact string. Prefer apply_patch for multi-line changes; use edit_file for single-shot find-and-replace.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file (Windows or WSL style)."
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact substring to replace. Must appear exactly once in the file."
                },
                "new_text": {
                    "type": "string",
                    "description": "The replacement string."
                }
            },
            "required": ["path", "old_text", "new_text"],
            "additionalProperties": false
        }),
    }
}

pub fn list_dir() -> LocalTool {
    LocalTool {
        name: LocalToolName::ListDir,
        description: "List files and subdirectories in a directory. Path must be under Documents/, Desktop/, Downloads/, or the workspace root.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the directory (Windows or WSL style)."
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum entries to return (default 500).",
                    "minimum": 1,
                    "maximum": 5000
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    }
}

pub fn bash_run() -> LocalTool {
    LocalTool {
        name: LocalToolName::BashRun,
        description: "Execute a shell command. The CWD is the workspace root by default; commands are sandboxed to Documents/, Desktop/, Downloads/, and the workspace root. Commands exceeding 30s wall time or producing >5MB of output are terminated. Available on Pro and above.",
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute. Prefer single-line, short-running commands."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (default: workspace root). Must be under allowed paths."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Maximum wall time in ms (default 30000, max 60000).",
                    "minimum": 100,
                    "maximum": 60000
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    }
}

pub fn apply_patch() -> LocalTool {
    LocalTool {
        name: LocalToolName::ApplyPatch,
        description: "Apply a structured patch to one or more files. Use this instead of edit_file when making multi-hunk changes or changes spanning multiple files. The patch format is unified-diff-like and the path must be under allowed directories.",
        parameters: json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The patch text in MC's apply_patch format. See docs for the exact grammar."
                }
            },
            "required": ["patch"],
            "additionalProperties": false
        }),
    }
}

pub fn remember_fact() -> LocalTool {
    LocalTool {
        name: LocalToolName::RememberFact,
        description: "Store a fact about the user (preferences, context, recurring work) in MC's persistent memory. Available on Pro and above. Free tier has no persistent memory.",
        parameters: json!({
            "type": "object",
            "properties": {
                "fact": {
                    "type": "string",
                    "description": "The fact to remember. Be concise and factual."
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for retrieval (e.g. ['car', 'oil-change', 'recurring'])."
                }
            },
            "required": ["fact"],
            "additionalProperties": false
        }),
    }
}

pub fn web_fetch() -> LocalTool {
    LocalTool {
        name: LocalToolName::WebFetch,
        description: "Fetch the contents of a URL and return it as text. HTTP/HTTPS only. Response bodies larger than 5MB are truncated. Binary content (non-text MIME types) is summarized as the first 1KB of bytes — use read_file on local files instead. Pair with web_search to follow links from a search result.",
        parameters: json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute URL to fetch (must start with http:// or https://)."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to return (default 65536, hard cap 5242880 = 5MB).",
                    "minimum": 1,
                    "maximum": 5242880
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Wall-clock timeout in ms (default 15000, max 30000).",
                    "minimum": 100,
                    "maximum": 30000
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
    }
}

/// All 8 tools, in the order they should be advertised to the model
/// (most useful first so the model prefers them when picking the first
/// tool). Built at runtime so we can use `serde_json::json!()`.
pub fn all_local_tools() -> Vec<LocalTool> {
    vec![
        read_file(),
        list_dir(),
        write_file(),
        edit_file(),
        apply_patch(),
        bash_run(),
        remember_fact(),
        web_fetch(),
    ]
}

/// Lazy-static reference to the all-tools list, useful for places that
/// need a `&[LocalTool]` (e.g., const contexts).
pub fn all_local_tools_slice() -> &'static [LocalTool] {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<LocalTool>> = OnceLock::new();
    CACHE.get_or_init(all_local_tools)
}

/// Serialize a single `LocalTool` into the OpenAI function-calling JSON
/// shape that the chat-completions `tools: [...]` array expects.
///
/// Wire format (Lesson 513 / Lesson 169 family):
/// ```json
/// {
///   "type": "function",
///   "function": {
///     "name": "read_file",
///     "description": "...",
///     "parameters": { "type": "object", "properties": {...}, ... }
///   }
/// }
/// ```
pub fn local_tool_to_openai(tool: &LocalTool) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name.as_str(),
            "description": tool.description,
            "parameters": tool.parameters,
        }
    })
}

/// Build the full `tools: [...]` array for the OpenAI chat-completions
/// request body. Tier-gated by the caller (pass only the tools the
/// user's tier is allowed to call). MAIC will return `tool_calls`
/// only for tools listed here; missing = not callable from the model.
pub fn local_tools_to_openai_array(tools: &[LocalTool]) -> Value {
    Value::Array(tools.iter().map(local_tool_to_openai).collect())
}

/// Tools that are flagged as "potentially destructive" so the UI can
/// require a per-tool confirmation prompt (not just a global tool
/// permission). These write/modify the filesystem or run commands.
pub fn is_sensitive_tool_name(name: &str) -> bool {
    matches!(
        name,
        "write_file" | "edit_file" | "apply_patch" | "bash_run" | "remember_fact"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tools_have_unique_names() {
        let mut seen = std::collections::HashSet::new();
        for t in all_local_tools() {
            assert!(
                seen.insert(t.name),
                "duplicate tool name in all_local_tools: {:?}",
                t.name
            );
        }
        // Lesson 803 (rc55.13): 8 tools (added web_fetch). Update this
        // assertion whenever `all_local_tools()` gains or loses a tool.
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn all_tools_have_object_parameters() {
        for t in all_local_tools() {
            assert_eq!(
                t.parameters["type"], "object",
                "tool {:?} parameters.type must be 'object'",
                t.name
            );
            assert!(
                t.parameters.get("properties").is_some(),
                "tool {:?} missing parameters.properties",
                t.name
            );
        }
    }

    #[test]
    fn bash_run_lists_path_allowlist_in_description() {
        // Sensitive tools need to surface the path allowlist in the
        // description so the model self-limits what it tries.
        let desc = bash_run().description;
        assert!(desc.contains("Documents"));
        assert!(desc.contains("Desktop"));
        assert!(desc.contains("Downloads"));
    }

    #[test]
    fn sensitive_tools_classified_correctly() {
        assert!(is_sensitive_tool_name("write_file"));
        assert!(is_sensitive_tool_name("edit_file"));
        assert!(is_sensitive_tool_name("apply_patch"));
        assert!(is_sensitive_tool_name("bash_run"));
        assert!(is_sensitive_tool_name("remember_fact"));
        assert!(!is_sensitive_tool_name("read_file"));
        assert!(!is_sensitive_tool_name("list_dir"));
        assert!(!is_sensitive_tool_name("unknown_tool"));
    }

    #[test]
    fn tool_name_round_trips() {
        for n in ALL_LOCAL_TOOL_NAMES {
            assert_eq!(LocalToolName::from_str(n.as_str()), *n);
        }
    }

    #[test]
    fn wire_label_matches_maic_conventions() {
        // Sanity check that the strings we publish to env / openclaw.json
        // match what MAIC's tool-call decoder expects.
        assert_eq!(LocalToolName::ReadFile.as_str(), "read_file");
        assert_eq!(LocalToolName::WriteFile.as_str(), "write_file");
        assert_eq!(LocalToolName::BashRun.as_str(), "bash_run");
        assert_eq!(LocalToolName::ApplyPatch.as_str(), "apply_patch");
        assert_eq!(LocalToolName::RememberFact.as_str(), "remember_fact");
    }

    #[test]
    fn each_tool_serializes_to_openai_format() {
        for t in all_local_tools() {
            let v = serde_json::to_value(&t).unwrap();
            assert!(v["name"].is_string(), "{:?} name not a string", t.name);
            assert!(v["description"].is_string(), "{:?} description not a string", t.name);
            assert!(v["parameters"].is_object(), "{:?} parameters not object", t.name);
            // OpenAI format uses `type: "function"` wrappers externally;
            // we publish the raw schema here and let the plugin wrap.
        }
    }
}