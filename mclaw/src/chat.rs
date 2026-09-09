//! `mclaw chat` — interactive REPL
//!
//! Multi-turn conversation with readline + persistent history.
//! History saved to `~/.config/mc/chat_history` (or platform equivalent).
//!
//! Slash commands:
//!   /exit, /quit   - leave the REPL
//!   /clear         - clear the conversation context
//!   /model <id>    - switch the model
//!   /no-tools      - disable tool calling (text-only)
//!   /tools         - enable tool calling (default)
//!   /help          - show help

use anyhow::{Context, Result};
use clap::Args;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::path::PathBuf;

use crate::client::{self, ChatResp};

#[derive(Args, Debug)]
pub struct ChatArgs {
    /// Initial model (default: user's default model from MAIC).
    #[arg(long, value_name = "ID")]
    model: Option<String>,

    /// Disable tool calling (text-only mode).
    #[arg(long)]
    no_tools: bool,
}

pub async fn chat(args: ChatArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let c = client::Client::new(&config)?;
    let mut model = args.model.unwrap_or_else(|| "milagro-dev".to_string());
    let mut tools_enabled = !args.no_tools;

    let history_path = history_path()?;
    let mut rl = DefaultEditor::new()?;
    if !rl.load_history(&history_path).is_err() {
        // missing history is fine, just won't have any saved lines
    }

    // Multi-turn message list. We append user/assistant turns and resend
    // the whole conversation on each turn. (For long sessions this would
    // need to be context-window-aware; out of scope for v0.1.)
    let mut messages: Vec<serde_json::Value> = Vec::new();

    println!("mclaw chat — type /help for commands, /exit to quit");
    println!();

    loop {
        let prompt = format!("mclaw [{}] {}> ", if tools_enabled { "tools" } else { "no-tools" }, model);
        let line = match rl.readline(&prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: cancel current line, stay in REPL
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D: leave REPL
                println!();
                break;
            }
            Err(e) => {
                eprintln!("readline error: {}", e);
                break;
            }
        };

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Save to history (skip slash commands so /exit doesn't pollute history)
        if !line.starts_with('/') {
            let _ = rl.add_history_entry(line);
        }

        // Slash commands
        if let Some(cmd) = line.strip_prefix('/') {
            match cmd.trim() {
                "exit" | "quit" => break,
                "clear" => {
                    messages.clear();
                    println!("(conversation cleared)");
                    continue;
                }
                "help" => {
                    println!();
                    println!("  /exit, /quit   leave the REPL");
                    println!("  /clear         clear the conversation context");
                    println!("  /model <id>    switch the model (current: {})", model);
                    println!("  /no-tools      disable tool calling");
                    println!("  /tools         enable tool calling (current: {})",
                             if tools_enabled { "yes" } else { "no" });
                    println!("  /help          show this help");
                    println!();
                    continue;
                }
                other if other.starts_with("model ") => {
                    let new_model = other.strip_prefix("model ").unwrap().trim();
                    if new_model.is_empty() {
                        eprintln!("usage: /model <id>");
                    } else {
                        model = new_model.to_string();
                        println!("(model: {})", model);
                    }
                    continue;
                }
                "no-tools" => {
                    tools_enabled = false;
                    println!("(tools disabled)");
                    continue;
                }
                "tools" => {
                    tools_enabled = true;
                    println!("(tools enabled)");
                    continue;
                }
                _ => {
                    eprintln!("unknown command: /{} — try /help", cmd);
                    continue;
                }
            }
        }

        // Append user message and send
        messages.push(serde_json::json!({"role": "user", "content": line}));
        let resp = match send(&c, &messages, &model, tools_enabled).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}", e);
                // Pop the user message so the conversation stays clean on retry
                messages.pop();
                continue;
            }
        };
        // Print the model's reply
        if !resp.content.is_empty() {
            println!("{}", resp.content);
        }
        for tc in &resp.tool_calls {
            if let Some(result) = &tc.result {
                eprintln!("  [{}: {}]", tc.name, result);
            }
        }
        // Append assistant turn so context is preserved across rounds
        messages.push(serde_json::json!({"role": "assistant", "content": resp.content}));
        println!();
    }

    let _ = rl.save_history(&history_path);
    Ok(0)
}

async fn send(
    c: &client::Client,
    messages: &[serde_json::Value],
    model: &str,
    tools_enabled: bool,
) -> Result<ChatResp> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "tool_execution": if tools_enabled { "server" } else { "client" },
        "tools": [],
    });
    let resp = c
        .post("/v1/chat/completions", &body)
        .await
        .map_err(|e| match e {
            client::Error::NoAuth => anyhow::anyhow!("not logged in: run `mclaw login` first"),
            client::Error::AuthFailed(msg) => anyhow::anyhow!("auth failed: {}", msg),
            other => anyhow::anyhow!("{}", other),
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("MAIC error ({}): {}", status, body));
    }
    let wire: client::OpenAIChatResp = resp.json().await?;
    Ok(wire.into())
}

fn history_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("config dir")?;
    #[cfg(windows)]
    let app_dir = dir.join("mclaw");
    #[cfg(not(windows))]
    let app_dir = dir.join("mc");
    std::fs::create_dir_all(&app_dir).context("creating history dir")?;
    Ok(app_dir.join("chat_history"))
}
