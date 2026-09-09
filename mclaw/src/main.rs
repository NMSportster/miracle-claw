//! mclaw — Miracle Claw CLI
//!
//! Ask MAIC from any shell, cron, or CI. See `docs/mc-headless-companion-2026-09.md`
//! for the design doc.
//!
//! Exit codes:
//!   0 = success
//!   1 = generic MAIC/network error
//!   2 = auth error (run `mclaw login` or set MAIC_JWT)
//!   3 = rate limited
//!   4 = tool execution error
//!   64 = bad CLI args

use anyhow::Result;
use clap::{Parser, Subcommand};

use mclaw::VERSION;
use mclaw::{ask, chat, info, login};

#[derive(Parser, Debug)]
#[command(name = "mclaw", version = VERSION, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// One-shot ask: send a prompt, print the response, exit.
    Ask(ask::AskArgs),
    /// Interactive REPL (readline + history + multi-turn).
    Chat(chat::ChatArgs),
    /// Log in to MAIC and store the JWT in the OS keychain.
    Login(login::LoginArgs),
    /// Log out and wipe the keychain.
    Logout(login::LogoutArgs),
    /// Print the current user, tier, and plan.
    Whoami(info::WhoAmIArgs),
    /// Print quota and current usage. (Coming in rc55.27.)
    Usage(info::UsageArgs),
    /// List available models.
    Models(info::ModelsArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Init logger: stderr, info level by default; override with RUST_LOG
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    let code = match cli.command {
        Commands::Ask(a) => ask::run(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Chat(a) => chat::chat(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Login(a) => login::login(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Logout(a) => login::logout(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Whoami(a) => info::whoami(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Usage(a) => info::usage(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Models(a) => info::models(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
    };

    std::process::exit(code);
}
