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
use mclaw::{ask, banner, chat, info, login, modules};

#[derive(Parser, Debug)]
#[command(
    name = "mclaw",
    version = VERSION,
    about = banner::TAGLINE,
    long_about = format!("{}\n\n\
mclaw is the headless companion to Miracle Claw — ask MAIC, run tools, and manage modules from any shell, cron, or CI.\n\n\
Get started:\n  \
mclaw login                    # log in and stash the JWT in the OS keychain\n  \
mclaw whoami                   # confirm the current user + tier\n  \
mclaw ask \"summarize...\"       # one-shot prompt\n  \
mclaw chat                     # interactive REPL\n  \
mclaw tools                    # list installed modules\n  \
mclaw marketplace list         # browse the marketplace\n  \
mclaw marketplace install firecrawl\n  \
mclaw run firecrawl.fetch_url --input url=https://example.com\n\n\
Exit codes:\n  \
0   success\n  \
1   generic MAIC / network error\n  \
2   auth error (run `mclaw login`)\n  \
3   rate limited\n  \
4   tool execution error\n  \
64  bad CLI args", banner::TAGLINE),
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// One-shot ask: send a prompt, print the response, exit.
    #[command(about = "Send a prompt to a model and print the response", long_about = "Send a prompt to a model and print the response, then exit.\n\nEXAMPLES:\n  mclaw ask \"What's the weather in Albuquerque?\"\n  mclaw ask \"summarize this URL\" --input url=https://example.com\n  mclaw ask --model milagro-dev \"write a haiku about Rust\"\n  mclaw ask --no-tools \"explain quicksort\"")]
    Ask(ask::AskArgs),
    /// Interactive REPL (readline + history + multi-turn).
    #[command(about = "Start an interactive chat session (readline + history)", long_about = "Start an interactive chat session with readline editing, persistent history, and multi-turn context.\n\nEXAMPLES:\n  mclaw chat\n  mclaw chat --model milagro-dev\n  mclaw chat --no-tools\n\nKEYBINDINGS:\n  Ctrl-D      exit\n  Ctrl-C      cancel current turn (or exit if idle)\n  Up/Down     navigate history\n  Tab         completion (models, /commands)\n  /help       in-REPL command list")]
    Chat(chat::ChatArgs),
    /// Log in to MAIC and store the JWT in the OS keychain.
    #[command(about = "Log in to MAIC and store the JWT in the OS keychain", long_about = "Log in to MAIC and store the JWT in the OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager).\n\nThe JWT is reused for every subsequent `mclaw` invocation; you only need to log in again if it expires (7 days) or is revoked server-side.\n\nEXAMPLES:\n  mclaw login\n  mclaw login --username david\n  mclaw login --maic-url https://staging.maicserver.com")]
    Login(login::LoginArgs),
    /// Log out and wipe the keychain.
    #[command(about = "Log out and wipe the keychain entry", long_about = "Log out by removing the stored JWT from the OS keychain. After this, the next `mclaw` invocation will return \"not logged in\" and prompt you to run `mclaw login`.")]
    Logout(login::LogoutArgs),
    /// Print the current user, tier, and plan.
    #[command(about = "Print the current MAIC user, tier, and plan", long_about = "Print the current MAIC user, tier (free / pro / pro_plus / team), and plan_code.\n\nEXAMPLES:\n  mclaw whoami\n  mclaw whoami --json | jq .tier")]
    Whoami(info::WhoAmIArgs),
    /// Print quota and current usage. (Coming in rc55.27.)
    #[command(about = "Print quota and current usage (placeholder for rc55.27)", long_about = "Print MAIC quota and current usage. PLACEHOLDER: not yet implemented; coming in rc55.27. Exits 1 for now.")]
    Usage(info::UsageArgs),
    /// List available models.
    #[command(about = "List available models (MAIC /v1/models)", long_about = "List the models available to the current user via MAIC's /v1/models endpoint.\n\nEXAMPLES:\n  mclaw models\n  mclaw models --json | jq '.[].id'")]
    Models(info::ModelsArgs),
    /// Run a tool on an installed module. `mclaw run <module>.<tool> [--input k=v ...]`
    #[command(about = "Run a tool on an installed module", long_about = "Run a tool on an installed first-party module. The module must be installed first via `mclaw marketplace install <id>`.\n\nThe `module.tool` shorthand is preferred; the explicit `--tool` form is for shells where `.` is awkward.\n\nEXAMPLES:\n  mclaw run firecrawl.fetch_url --input url=https://example.com\n  mclaw run firecrawl.fetch_url --input url=https://example.com --json\n  mclaw run data-analyzer.summarize --input csv_url=https://example.com/data.csv --input focus=revenue\n  mclaw run brand-asset-kit.bio --input platform=linkedin --input 'keywords=auto repair, Albuquerque'")]
    Run(modules::RunArgs),
    /// List installed modules grouped by category.
    #[command(about = "List installed modules grouped by category", long_about = "List installed modules grouped by category.\n\nEXAMPLES:\n  mclaw tools\n  mclaw tools --json\n  mclaw tools --category data")]
    Tools(modules::ToolsArgs),
    /// Browse the module marketplace. Sub-commands: list | install | uninstall.
    #[command(about = "Browse the module marketplace (list / install / uninstall)", long_about = "Browse the module marketplace.\n\nEXAMPLES:\n  mclaw marketplace list\n  mclaw marketplace list data\n  mclaw marketplace install firecrawl\n  mclaw marketplace install data-analyzer --json\n  mclaw marketplace uninstall firecrawl")]
    Marketplace(modules::MarketplaceArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Init logger: stderr, info level by default; override with RUST_LOG
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    // Brand text on login (also works for any interactive entry point).
    if matches!(cli.command, Commands::Login(_)) {
        banner::print_banner();
    }

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
        Commands::Run(a) => modules::run(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Tools(a) => modules::tools(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
        Commands::Marketplace(a) => modules::marketplace(a).await.unwrap_or_else(|e| {
            eprintln!("error: {:#}", e);
            1
        }),
    };

    std::process::exit(code);
}
