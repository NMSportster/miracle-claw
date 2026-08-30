// `miracle-claw-tools` — local tool executor for the MAIC plugin.
//
// Spawned by `depot/maic-plugin/index.js` when the model invokes one of
// the 7 paid-tier tools (read_file, write_file, edit_file, list_dir,
// bash_run, apply_patch, remember_fact). This binary is shipped alongside
// `miracle-claw.exe` in the same Resources directory and is found by the
// plugin via a path relative to its own location.
//
// Wire protocol:
//   - argv[1] = tool name (e.g. "read_file")
//   - argv[2] (optional) = JSON-encoded params object (matches the OpenAI tool schema)
//                      OR the literal "-" sentinel, which means "read params from stdin"
//   - stdin = JSON-encoded params object (used when argv[2] is "-", or when no argv[2])
//   - stdout = the tool result as a UTF-8 string (the model sees this)
//   - stderr = error message on failure
//   - exit 0 = success, exit 1 = error, exit 2 = usage error
//
// Lesson 837 (2026-08-30): The plugin uses `useStdin = (process.platform === "win32"
// || paramsJson.length >= 2048)` and sends `[toolName, "-"]` when stdin is needed
// (always on Windows). The binary must recognize "-" as a sentinel and read params
// from stdin. Lesson 804's plugin-side fix shipped in rc55.13, but this binary side
// was missed — every rc55.13+ Windows tool call has been failing with
// `argv[2] is not valid JSON: EOF while parsing a value at line 1 column 1`.
//
// All business logic lives in `tools::exec::dispatch`. This file is
// intentionally thin — just argv/stdin parsing and exit code mapping.

use std::io::{self, Read, Write};

// Re-declare the same modules the lib uses. We don't share the lib
// crate (`miracle_claw_lib`) because:
//   1. The lib pulls in tauri, which we don't want to link into the
//      tool executor (smaller binary, faster startup).
//   2. The tool executor is supposed to be a separate, self-contained
//      process that talks to the OS only — no UI, no IPC.
//
// The duplicated modules are `tools` (for the schema enum + dispatch).
// Pure data + pure functions, no tauri deps.
//
// We DO NOT include `auth` here: the tool executor doesn't need
// tier-gating logic (the plugin only invokes its argv name when the
// user is paid, so the gate is at the plugin level). If we ever need
// `Tier` in the executor, we can add it here the same way.
#[path = "tools/mod.rs"]
mod tools;

use tools::schemas::LocalToolName;
use tools::schemas::ALL_LOCAL_TOOL_NAMES;

fn main() {
    // Lesson 528: identify THIS tools EXE in the side-car log. The tools
    // binary is spawned by miracle-claw.exe to run local file/bash tools,
    // so its banner makes it easy to confirm version alignment between
    // the main EXE and the tools helper.
    eprintln!(
        "[miracle-claw-tools] miracle-claw-tools v{} (build {} UTC)",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_TIMESTAMP"),
    );

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: miracle-claw-tools <tool_name> [params_json_or_-]");
        eprintln!("  pass params as argv[2] OR pass \"-\" to read JSON params from stdin");
        eprintln!("  valid tool names: {:?}", ALL_LOCAL_TOOL_NAMES);
        std::process::exit(2);
    }

    let tool_name_str = &args[1];
    let tool_name = LocalToolName::from_str(tool_name_str);
    // Validate: from_str has a "safe default" fallback. Guard against it.
    if tool_name.as_str() != tool_name_str {
        eprintln!(
            "unknown tool: {} (valid: {:?})",
            tool_name_str, ALL_LOCAL_TOOL_NAMES
        );
        std::process::exit(1);
    }

    // Read params: prefer argv[2], fall back to stdin.
    // Lesson 837: argv[2] = "-" is the stdin sentinel used by the plugin on
    // Windows (and when params JSON > 2 KiB).
    let params_from_argv: Option<String> = if args.len() >= 3 && args[2] != "-" {
        Some(args[2].clone())
    } else {
        None
    };
    let params = if let Some(json_str) = params_from_argv {
        match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("argv[2] is not valid JSON: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        let mut s = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut s) {
            eprintln!("could not read stdin: {}", e);
            std::process::exit(1);
        }
        match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("stdin is not valid JSON: {}", e);
                std::process::exit(1);
            }
        }
    };

    match tools::exec::dispatch(&tool_name, &params) {
        Ok(out) => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            // Best-effort: ignore broken-pipe errors at the OS boundary.
            let _ = handle.write_all(out.as_bytes());
            let _ = handle.flush();
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}