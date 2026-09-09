//! `mclaw ask "<prompt>"` — one-shot chat call
//!
//! Streams nothing (we use MAIC's non-streaming endpoint) so the output is
//! well-defined for piping. Plain text by default, `--json` for machine-readable.
//!
//! Exit codes:
//!   0 = success
//!   1 = generic MAIC/network error
//!   2 = auth error (run `mclaw login`)
//!   3 = rate limited
//!   4 = tool execution error
//!   64 = bad CLI args

use anyhow::Result;
use clap::Args;

use crate::client::{self, AskJsonOut, ChatResp};

#[derive(Args, Debug)]
pub struct AskArgs {
    /// The prompt to send to the model.
    prompt: String,

    /// Output machine-readable JSON instead of plain text.
    #[arg(long)]
    json: bool,

    /// Model ID (default: user's default model from MAIC).
    #[arg(long, value_name = "ID")]
    model: Option<String>,

    /// Disable tool calling (text-only response).
    #[arg(long)]
    no_tools: bool,
}

pub async fn run(args: AskArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let client = client::Client::new(&config)?;

    let (resp, elapsed) =
        client::timed_chat(&client, &args.prompt, args.model.as_deref(), !args.no_tools)
            .await
            .map_err(|e| match e {
                client::Error::NoAuth => {
                    anyhow::anyhow!("not logged in: run `mclaw login` first")
                }
                client::Error::AuthFailed(msg) => {
                    anyhow::anyhow!("authentication failed: {}\nrun `mclaw login` to re-authenticate", msg)
                }
                client::Error::RateLimited(d) => {
                    anyhow::anyhow!("rate limited; try again in {:?}", d)
                }
                other => anyhow::anyhow!("{}", other),
            })?;

    if args.json {
        let out = AskJsonOut {
            model: resp.model,
            content: resp.content,
            tool_calls: resp.tool_calls,
            usage: resp.usage,
            duration_ms: elapsed.as_millis(),
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_plain(&resp);
    }
    Ok(0)
}

fn print_plain(resp: &ChatResp) {
    if !resp.content.is_empty() {
        println!("{}", resp.content);
    }
    for tc in &resp.tool_calls {
        if let Some(result) = &tc.result {
            eprintln!("  [{}: {}]", tc.name, result);
        } else {
            eprintln!("  [{}: {}]", tc.name, tc.args);
        }
    }
}
