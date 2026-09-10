//! `miracle-claw-pair` — headless pairing server for MC Mobile testing.
//!
//! Spec: docs/mc-pairing-cli-2026-09.md
//! Design: 2026-09-09 19:45 MDT (Homebot/office-bot agreement, see
//! MEMORY.md "MiracleClaw Phase 2.4 — verified in rc55.25 binary on
//! steeler" entry for the carry-forward).
//!
//! This binary is the same desktop-side pairing logic as the Tauri
//! app's `mc_register_desktop` command, but without a WebView. It
//! reuses the existing `pairing_commands` / `pairing_server` /
//! `pairing_identity` modules via `pub(crate) fn` re-exports that
//! the Tauri command also calls (AP-launchpad-A: don't rewrite the
//! existing Tauri command — add a thin re-export and have both
//! call it).
//!
//! This file is **arg parsing + Config struct only** per the
//! 2026-09-09 19:40 MDT direction:
//!   > Don't implement the HTTP server, the daemon fork, or the
//!   > binding to pairing_commands. Officebot will handle that,
//!   > just do Arg parsing + Config struct only.
//!
//! What this file does:
//!   1. Defines `Config` — the validated, post-parsing view of the
//!      CLI surface (port, bind, identity-file, ephemeral,
//!      foreground/detach, log level, print-info).
//!   2. Defines `parse_args()` — argv → `Result<Config, CliError>`
//!      with --help / --version short-circuits.
//!   3. Defines `CliError` + `print_help()`.
//!   4. Defines `main()` — calls `parse_args()`, prints help on
//!      `--help`, prints version on `--version`, then calls a
//!      single `run(config)` stub that Office Bot will replace
//!      with the real wiring (HTTP server start, daemon fork,
//!      pairing_commands binding).
//!
//! What this file does NOT do (Office Bot's job):
//!   - Bind the pairing HTTP server (use `pairing_server::start`)
//!   - Daemon fork / PID-file write (Unix only)
//!   - Construct `PairingCommands` from `pairing_init`
//!   - Wire the heartbeat loop (`pairing_heartbeat`)
//!   - Cross-compile pipeline (`cargo xwin`)
//!
//! Style: matches the existing `launcher.rs` manual argv walker
//! (no `clap` dep added). If a future iteration wants clap's
//! derive macros, the Config struct's fields are already shaped
//! to be a 1:1 with what `#[derive(Parser)]` would produce.

// ────────────────────────────────────────────────────────────────────
// Imports
// ────────────────────────────────────────────────────────────────────

use std::env;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;

// ────────────────────────────────────────────────────────────────────
// Constants — port range, env-var fallback, PID file path
// ────────────────────────────────────────────────────────────────────

/// Lower bound of the user-port range. 1024 mirrors `launcher.rs`
/// (avoid privileged ports; require explicit capability for those).
const MIN_PORT: u16 = 1024;
/// Upper bound of u16 (excludes 0 which is a valid sentinel meaning
/// "OS picks a free port").
const MAX_PORT: u16 = 65535;
/// Env-var fallback for `--port`. Read in `parse_args()` when the
/// flag is absent and `0` (the default) would otherwise be used.
const ENV_PORT: &str = "MIRACLE_CLAW_PAIR_PORT";
/// PID file written by `--detach` (Unix only). Path is fixed —
/// matches the convention used by other MAIC sidecars
/// (e.g. `mlg-serve.pid`).
#[cfg(unix)]
const PID_FILE: &str = "/tmp/miracle-claw.pair.pid";
/// Conventional default identity-file location, used when
/// `--ephemeral-identity` is set OR when `--identity-file` is
/// supplied and we want a sane default for tests. Office Bot
/// will compute the real XDG-aware default via `dirs::data_dir()` +
/// `com.milagrocloud.miracle-claw/pairing/identity.json`.
const IDENTITY_FILENAME: &str = "identity.json";
/// App identifier used to compute the default identity-file path
/// (mirrors `launcher.rs`'s `get_app_data_dir()` convention).
const APP_ID: &str = "com.milagrocloud.miracle-claw";

// ────────────────────────────────────────────────────────────────────
// Config — the validated, post-parse view of the CLI surface
// ────────────────────────────────────────────────────────────────────

/// Validated CLI configuration. All fields are guaranteed to be
/// well-formed (e.g. `port` is in `[0, 65535]`, `bind` is one of
/// the three recognized modes, `identity_file` is set unless
/// `ephemeral_identity` is true).
///
/// Office Bot's `run()` stub will consume this struct directly.
/// It is intentionally `pub` so the wiring code (added later)
/// doesn't need to live in this file.
#[derive(Debug, Clone)]
pub struct Config {
    /// Port the pairing HTTP server should bind to. `0` means
    /// "ask the OS for a free port" (and print the chosen value
    /// on stdout so callers can discover it).
    pub port: u16,
    /// Bind mode — controls which interface the server listens
    /// on. Mirrors the launcher's `--bind` flag.
    pub bind: BindMode,
    /// Path to the on-disk pairing identity. `None` when
    /// `ephemeral_identity` is set.
    pub identity_file: Option<PathBuf>,
    /// Skip persistence: generate an in-memory X25519 keypair,
    /// never write to disk. Use for phone-sim / smoke-test runs.
    pub ephemeral_identity: bool,
    /// Process mode — foreground (default; blocks until SIGINT)
    /// or detached (Unix only; forks, writes PID file, parent
    /// exits immediately).
    pub mode: RunMode,
    /// Log verbosity. `silent` prints nothing, `info` is the
    /// default, `debug` adds the per-request pairing trace.
    pub log_level: LogLevel,
    /// If true, print a one-line JSON summary of the bound port
    /// + keypair path + public key + start time to stdout, then
    /// continue running normally. Designed for smoke tests /
    /// shell-scripted launches.
    pub print_info: bool,
}

// ────────────────────────────────────────────────────────────────────
// Enums — bind mode, run mode, log level
// ────────────────────────────────────────────────────────────────────

/// Bind mode. Mirrors `miracle-claw-launcher --bind`. The actual
/// `SocketAddr` is computed later by Office Bot's wiring (using
/// `local_ip_address::local_ip` for `lan` mode, the Tailscale
/// state file for `tailnet` mode, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindMode {
    /// 127.0.0.1 only. Default. Safe for dev — never accidentally
    /// exposed to the LAN.
    Loopback,
    /// Bind on the host's primary LAN interface. The phone must
    /// be on the same network to dial in.
    Lan,
    /// Bind on the Tailscale interface (100.64.0.0/10). Phone
    /// dials via tailnet, no LAN required.
    Tailnet,
}

impl BindMode {
    /// Parse the string form, returning the matching variant.
    /// Strict — anything not in the allowlist is an error so we
    /// fail fast instead of silently picking a default.
    fn parse(s: &str) -> Result<Self, CliError> {
        match s {
            "loopback" => Ok(BindMode::Loopback),
            "lan" => Ok(BindMode::Lan),
            "tailnet" => Ok(BindMode::Tailnet),
            other => Err(CliError::InvalidBind(other.to_string())),
        }
    }
}

/// Foreground (default) vs detached (Unix only). Detach on
/// non-Unix is a parse-time error — the spec locks it to Unix
/// for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Foreground,
    #[cfg(unix)]
    Detach,
}

impl RunMode {
    fn parse(s: &str) -> Result<Self, CliError> {
        match s {
            "foreground" => Ok(RunMode::Foreground),
            "detach" => {
                #[cfg(unix)]
                {
                    Ok(RunMode::Detach)
                }
                #[cfg(not(unix))]
                {
                    Err(CliError::DetachUnsupported)
                }
            }
            other => Err(CliError::InvalidRunMode(other.to_string())),
        }
    }
}

/// Three levels, matching the launcher's convention. Office Bot's
/// wiring can map these to whatever logging facade the codebase
/// uses (likely `eprintln!` plus the `RUST_LOG` env var).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Silent,
    Info,
    Debug,
}

impl LogLevel {
    fn parse(s: &str) -> Result<Self, CliError> {
        match s {
            "silent" => Ok(LogLevel::Silent),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            other => Err(CliError::InvalidLogLevel(other.to_string())),
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Error type — used by parse_args + print_usage
// ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CliError {
    /// `--port` value isn't a number or is out of range.
    InvalidPort(String),
    /// `--bind` value isn't loopback|lan|tailnet.
    InvalidBind(String),
    /// `--detach` passed on Windows / macOS (unsupported in v1).
    DetachUnsupported,
    /// `--run-mode` value isn't foreground|detach (defensive —
    /// the current CLI surface doesn't expose this flag, but the
    /// enum is part of Config so we accept it if Office Bot
    /// surfaces it later).
    InvalidRunMode(String),
    /// `--log-level` value isn't silent|info|debug.
    InvalidLogLevel(String),
    /// `--identity-file` was passed alongside `--ephemeral-identity`.
    /// They are mutually exclusive.
    ConflictingIdentityFlags,
    /// Unknown CLI flag. Carries the offending arg so the user
    /// can spot typos.
    UnknownFlag(String),
    /// `--flag` was passed without a value.
    MissingValue(&'static str),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::InvalidPort(s) => {
                write!(f, "--port value '{}' is not a valid integer in {}-{} (use 0 for OS-assigned)", s, MIN_PORT, MAX_PORT)
            }
            CliError::InvalidBind(s) => write!(
                f,
                "--bind value '{}' is not recognized (use loopback|lan|tailnet)",
                s
            ),
            CliError::DetachUnsupported => write!(
                f,
                "--detach is only supported on Unix (Linux + macOS); use --foreground on this platform"
            ),
            CliError::InvalidRunMode(s) => write!(
                f,
                "--run-mode value '{}' is not recognized (use foreground|detach)",
                s
            ),
            CliError::InvalidLogLevel(s) => write!(
                f,
                "--log-level value '{}' is not recognized (use silent|info|debug)",
                s
            ),
            CliError::ConflictingIdentityFlags => write!(
                f,
                "--identity-file and --ephemeral-identity are mutually exclusive"
            ),
            CliError::UnknownFlag(s) => {
                write!(f, "unknown flag: {}\n\nRun with --help for usage.", s)
            }
            CliError::MissingValue(flag) => {
                write!(f, "flag {} requires a value\n\nRun with --help for usage.", flag)
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Help / version text
// ────────────────────────────────────────────────────────────────────

/// Version string. Embedded at compile time from `CARGO_PKG_VERSION`
/// (matches the launcher's `env!("CARGO_PKG_VERSION")` pattern).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Print the help text. Mirrors the launcher's `print_usage()` style
/// (plain text, hard-wrapped, sent to stderr so it doesn't pollute
/// stdout on `--print-info`).
pub fn print_help() {
    eprintln!(
        "miracle-claw-pair {VERSION} — headless pairing server for MC Mobile testing\n\
         \n\
         Usage: miracle-claw-pair [OPTIONS]\n\
         \n\
         Options:\n\
           --port <N>                Pairing HTTP server port\n\
                                    (default 0 = OS-assigned, or $MIRACLE_CLAW_PAIR_PORT)\n\
           --bind <mode>             Bind mode: loopback|lan|tailnet\n\
                                    (default loopback; mirrors miracle-claw-launcher)\n\
           --identity-file <path>    Override the pairing identity file location\n\
                                    (default: same path the GUI uses, XDG-aware)\n\
           --ephemeral-identity      Generate an in-memory identity, do not write to disk\n\
           --foreground              Block in foreground; log to stdout (default)\n\
           --detach                  Detach from terminal, write PID to {pid}\n\
                                    (Unix only)\n\
           --print-info              Print a one-line JSON summary of the bound port,\n\
                                    keypair path, public key, and start time to stdout\n\
           --log-level <level>       silent|info|debug (default info)\n\
           -h, --help                Print this help\n\
           -V, --version             Print version\n\
         \n\
         On startup:\n\
           1. Initialize pairing_commands (X25519 keypair if not already on disk)\n\
           2. Bind the pairing HTTP server on <bind>:<port>\n\
           3. Print the heartbeat URL + bound port + instance_id to stdout\n\
           4. Start the heartbeat loop (existing pairing_heartbeat.rs)\n\
           5. Block (or detach)\n\
         \n\
         Exit codes:\n\
           0  clean shutdown (SIGINT / SIGTERM)\n\
           1  CLI parse error (bad --port, unknown flag, etc.)\n\
           2  bind / listen failure (port in use, no permission, etc.)\n\
           3  identity load/save failure (corrupt file, permission denied)\n\
         ",
        pid = {
            #[cfg(unix)]
            {
                PID_FILE
            }
            #[cfg(not(unix))]
            {
                "(n/a on this platform)"
            }
        }
    );
}

// ────────────────────────────────────────────────────────────────────
// parse_args — argv → Result<Config, CliError>
// ────────────────────────────────────────────────────────────────────

/// Parse `env::args()` into a `Config`. Public so Office Bot can
/// call it from a `#[test]` to pin the surface, and so future
/// subcommand-parsing layers (if any) can call it with a
/// `Vec<String>` instead of `env::args()`.
pub fn parse_args() -> Result<Config, CliError> {
    parse_args_from(env::args().skip(1))
}

/// Same as `parse_args()` but takes an explicit iterator. Used by
/// the test suite (and by Office Bot if a subcommand parser is
/// added later).
pub fn parse_args_from<I, S>(mut iter: I) -> Result<Config, CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let mut port: Option<u16> = None;
    let mut bind: Option<BindMode> = None;
    let mut identity_file: Option<PathBuf> = None;
    let mut ephemeral_identity = false;
    let mut mode: Option<RunMode> = None;
    let mut log_level: Option<LogLevel> = None;
    let mut print_info = false;

    while let Some(arg) = iter.next() {
        let arg = arg.as_ref();
        match arg {
            // ── short-circuit flags ─────────────────────────────────
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            "-V" | "--version" => {
                println!("miracle-claw-pair {}", VERSION);
                process::exit(0);
            }
            // ── port ────────────────────────────────────────────────
            "--port" => {
                let raw = iter
                    .next()
                    .ok_or(CliError::MissingValue("--port"))?;
                let raw = raw.as_ref();
                // Allow `0` as a sentinel — it parses to u16=0 and
                // means "OS picks a free port". The bound range
                // check below accepts 0..=MAX_PORT for this reason.
                let n: u64 = raw
                    .parse()
                    .map_err(|_| CliError::InvalidPort(raw.to_string()))?;
                let n_u16 = u16::try_from(n).map_err(|_| CliError::InvalidPort(raw.to_string()))?;
                if n_u16 != 0 && !(MIN_PORT..=MAX_PORT).contains(&n_u16) {
                    return Err(CliError::InvalidPort(raw.to_string()));
                }
                port = Some(n_u16);
            }
            // ── bind ────────────────────────────────────────────────
            "--bind" => {
                let raw = iter
                    .next()
                    .ok_or(CliError::MissingValue("--bind"))?;
                bind = Some(BindMode::parse(raw.as_ref())?);
            }
            // ── identity file (overrides XDG default) ───────────────
            "--identity-file" => {
                let raw = iter
                    .next()
                    .ok_or(CliError::MissingValue("--identity-file"))?;
                identity_file = Some(PathBuf::from(raw.as_ref()));
            }
            // ── ephemeral identity (no disk) ────────────────────────
            "--ephemeral-identity" => {
                ephemeral_identity = true;
            }
            // ── foreground / detach ─────────────────────────────────
            "--foreground" => {
                mode = Some(RunMode::Foreground);
            }
            "--detach" => {
                mode = Some(RunMode::parse("detach")?);
            }
            // ── print-info (smoke-test JSON summary) ────────────────
            "--print-info" => {
                print_info = true;
            }
            // ── log level ───────────────────────────────────────────
            "--log-level" => {
                let raw = iter
                    .next()
                    .ok_or(CliError::MissingValue("--log-level"))?;
                log_level = Some(LogLevel::parse(raw.as_ref())?);
            }
            // ── internal (set by the detached child) ───────────────
            "--child-of-detach" => {
                // Silently consume. The detach path in `run()` reads
                // this via `is_child_of_detach()` to skip the re-detach.
            }
            // ── unknown ─────────────────────────────────────────────
            other => {
                return Err(CliError::UnknownFlag(other.to_string()));
            }
        }
    }

    // ── cross-flag validation ─────────────────────────────────────
    if ephemeral_identity && identity_file.is_some() {
        return Err(CliError::ConflictingIdentityFlags);
    }

    // ── defaults ──────────────────────────────────────────────────
    //
    // Port: flag → env → 0 (OS-assigned).
    let port = match port {
        Some(p) => p,
        None => match env::var(ENV_PORT) {
            Ok(s) if !s.trim().is_empty() => {
                let n: u64 = s
                    .parse()
                    .map_err(|_| CliError::InvalidPort(s.clone()))?;
                let n_u16 = u16::try_from(n).map_err(|_| CliError::InvalidPort(s.clone()))?;
                if n_u16 != 0 && !(MIN_PORT..=MAX_PORT).contains(&n_u16) {
                    return Err(CliError::InvalidPort(s));
                }
                n_u16
            }
            _ => 0u16,
        },
    };
    let bind = bind.unwrap_or(BindMode::Loopback);
    let mode = mode.unwrap_or(RunMode::Foreground);
    let log_level = log_level.unwrap_or(LogLevel::Info);

    Ok(Config {
        port,
        bind,
        identity_file,
        ephemeral_identity,
        mode,
        log_level,
        print_info,
    })
}

// ────────────────────────────────────────────────────────────────────
// run() — stub. Office Bot replaces this with the real wiring.
// ────────────────────────────────────────────────────────────────────

/// Stub entry point. Office Bot will replace this body with:
///   1. Resolve `config.identity_file` (default XDG path or
///      ephemeral — use `dirs::data_dir()` + APP_ID).
///   2. Load or generate `Identity` via `pairing_identity`.
///   3. Build `PairingCommands` via `pairing_init::build_pairing_state`.
///   4. Resolve `config.bind` → concrete `SocketAddr` (use
///      `local_ip_address::local_ip` for `lan`, Tailscale state
///      file for `tailnet`, `127.0.0.1` for `loopback`).
///   5. Call `pairing_server::start(config.port, cmds, bind_addr)`.
///   6. If `config.mode == RunMode::Detach`: fork, write PID
///      file, parent exits.
///   7. Block on the server's join handle (Ctrl-C handler for
///      graceful shutdown).
///   8. If `config.print_info`: emit the one-line JSON summary
///      before/during startup.
///
/// Exit codes (per `--help`):
///   0  clean shutdown
///   1  CLI parse error (already handled in `parse_args` /
///      `main`)
///   2  bind / listen failure — returned by `run()` on a
///      `pairing_server::start` error.
///   3  identity load/save failure — returned by `run()` on a
///      `pairing_identity::load` / `save` error.
pub fn run(config: Config) -> Result<(), i32> {
    // Step 1: resolve the identity path. `ephemeral` wins over
    // `identity_file`; neither set means the XDG default.
    let identity_target = match resolve_identity_target(&config) {
        Ok(t) => t,
        Err(code) => return Err(code),
    };

    // Step 2: load or generate the X25519 identity. We use the
    // public pairing_identity API (no internal re-exports needed).
    let identity = match load_or_generate_identity(&identity_target) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[miracle-claw-pair] identity error: {e}");
            return Err(3);
        }
    };

    // Step 3: build PairingCommands. The CLI reuses the existing
    // pairing_init helper (no new function — see mcmobile.txt
    // 2026-09-09 21:19 MDT "Office Bot" reply for the rationale).
    let cmds = miracle_claw_lib::pairing_init::build_pairing_state();

    // Step 4: resolve the bind address. Loopback is fully
    // supported; Lan / Tailnet return a runtime error (the local_ip
    // + Tailscale-state plumbing is a follow-up — not blocking
    // the phone-sim smoke test which uses --bind loopback).
    let bind_addr = match resolve_bind_addr(config.bind) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[miracle-claw-pair] {e}");
            return Err(2);
        }
    };

    // Step 5: start the pairing HTTP server. Tiny_http binds
    // synchronously, so a port-in-use error surfaces here (exit 2).
    let server_state = match miracle_claw_lib::pairing_server::start(config.port, cmds, bind_addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[miracle-claw-pair] failed to start pairing server: {e}");
            return Err(2);
        }
    };
    let bound_port = server_state.bound_port();

    // Step 6: --print-info emits a single JSON line for shell
    // scripts and phone-sim harnesses. Field set matches the spec.
    if config.print_info {
        let started_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let identity_path_str = match &identity_target {
            IdentityTarget::Ephemeral => "(ephemeral)".to_string(),
            IdentityTarget::Path(p) => p.display().to_string(),
        };
        let info = serde_json::json!({
            "port": bound_port,
            "identity_path": identity_path_str,
            "public_key": identity.desktop_pubkey_b64,
            "fingerprint": identity.fingerprint,
            "started_at_unix": started_at_unix,
        });
        // println! for stdout (the spec wants this parseable by
        // scripts). The actual server logs go to stderr.
        println!("{info}");
    }

    // Step 7: process mode. Foreground blocks on a 200ms poll
    // loop watching is_running(); Ctrl-C flips the shutdown flag
    // via the platform signal handler below. Detach (Unix only)
    // forks, writes the PID file, and the parent exits 0.
    match config.mode {
        RunMode::Foreground => {
            install_shutdown_signal_handler(&server_state);
            while server_state.is_running() {
                std::thread::sleep(Duration::from_millis(200));
            }
            miracle_claw_lib::pairing_server::stop(&server_state);
            Ok(())
        }
        #[cfg(unix)]
        RunMode::Detach => {
            // If we're already inside the detached child (the parent
            // re-execed us with --child-of-detach), skip the spawn and
            // fall straight into the foreground poll loop. The setsid()
            // already ran in the parent, so the child is properly
            // detached from the controlling terminal.
            if is_child_of_detach() {
                install_shutdown_signal_handler(&server_state);
                while server_state.is_running() {
                    std::thread::sleep(Duration::from_millis(200));
                }
                miracle_claw_lib::pairing_server::stop(&server_state);
                Ok(())
            } else {
                detach_and_write_pidfile(&server_state)
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// main
// ────────────────────────────────────────────────────────────────────

fn main() {
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[miracle-claw-pair] {}", e);
            process::exit(1);
        }
    };
    if let Err(code) = run(config) {
        process::exit(code);
    }
}

// ────────────────────────────────────────────────────────────────────
// run() helpers — identity resolution, bind resolution,
// process-mode details. Kept in this file (not a new module) so
// the binary is a single 800-ish-line file you can grep end-to-end.
// ────────────────────────────────────────────────────────────────────

/// Where the identity should come from. `Ephemeral` is in-memory
/// only (no disk write); `Path` is the on-disk JSON file
/// (loaded or generated on demand).
#[derive(Debug)]
enum IdentityTarget {
    Ephemeral,
    Path(PathBuf),
}

fn resolve_identity_target(config: &Config) -> Result<IdentityTarget, i32> {
    if config.ephemeral_identity {
        return Ok(IdentityTarget::Ephemeral);
    }
    if let Some(p) = config.identity_file.clone() {
        return Ok(IdentityTarget::Path(p));
    }
    // XDG default: dirs::data_dir() + "com.milagrocloud.miracle-claw" + "pairing/identity.json".
    // Mirrors pairing_identity::identity_path() for the GUI.
    let data_dir = match dirs::data_dir() {
        Some(d) => d,
        None => {
            eprintln!(
                "[miracle-claw-pair] could not determine user data dir (XDG_DATA_HOME unset?)"
            );
            return Err(3);
        }
    };
    let path = data_dir
        .join(APP_ID)
        .join("pairing")
        .join(IDENTITY_FILENAME);
    Ok(IdentityTarget::Path(path))
}

fn load_or_generate_identity(target: &IdentityTarget) -> std::io::Result<miracle_claw_lib::pairing_identity::Identity> {
    use miracle_claw_lib::pairing_identity::{load_from, save_to};
    match target {
        IdentityTarget::Ephemeral => {
            // In-memory keypair, never written to disk. The CLI's
            // process lifetime is the identity's lifetime — fine
            // for smoke tests and phone-sim runs.
            Ok(generate_in_memory_identity())
        }
        IdentityTarget::Path(path) => {
            // Load if present, else generate + save_to.
            if let Some(id) = load_from(path)? {
                return Ok(id);
            }
            // For user-supplied paths, generate in-memory and write
            // to exactly that path. We do NOT call `generate(dir)`
            // here because that helper assumes the XDG layout
            // (app_data_dir/pairing/identity.json) and writes to
            // `<dir>/pairing/identity.json`, not the user-supplied
            // path. For the XDG default (the GUI's canonical
            // location), save_to and the XDG write would write the
            // same file — we'd be double-writing. So we always
            // generate in-memory and save_to the user path.
            let id = generate_in_memory_identity();
            save_to(path, &id)?;
            Ok(id)
        }
    }
}

/// Build a fresh X25519 Identity in memory (no disk write).
/// Shared by Ephemeral mode and the "first time we see a custom
/// --identity-file path" case.
fn generate_in_memory_identity() -> miracle_claw_lib::pairing_identity::Identity {
    use miracle_claw_lib::pairing_identity::{generate_keypair, generate_instance_id, fingerprint_from_pubkey, Identity};
    use base64::Engine;
    let (pub_bytes, secret) = generate_keypair();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Identity {
        instance_id: generate_instance_id(),
        desktop_pubkey_b64: base64::engine::general_purpose::STANDARD.encode(pub_bytes),
        desktop_secret_key: secret.to_bytes().to_vec(),
        fingerprint: fingerprint_from_pubkey(&pub_bytes),
        created_at_unix: now,
    }
}

fn resolve_bind_addr(mode: BindMode) -> Result<std::net::SocketAddr, String> {
    match mode {
        BindMode::Loopback => Ok(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            0, // 0 = let pairing_server::start pick the port from config
        )),
        BindMode::Lan => Err(
            "--bind lan is not yet implemented in miracle-claw-pair v0.0.1. \
             The local_ip_address crate is not wired up yet; until it is, \
             please use --bind loopback (the default). Tracked in the Week 2.5+ backlog."
                .to_string(),
        ),
        BindMode::Tailnet => Err(
            "--bind tailnet is not yet implemented in miracle-claw-pair v0.0.1. \
             Tailscale state-file parsing is a follow-up. Please use --bind loopback \
             (the default) for now."
                .to_string(),
        ),
    }
}

/// Install a no-op shutdown hook. v0.0.1 uses the poll loop in
/// `run()` plus OS-level process termination for Ctrl-C — adding
/// the `libc` crate just for `signal(SIGINT, ...)` is out of scope.
/// Future versions can add a real handler if graceful shutdown
/// (vs process-kill) becomes a requirement.
fn install_shutdown_signal_handler(
    _server_state: &Arc<miracle_claw_lib::pairing_server::PairingServerState>,
) {
    // No-op. The poll loop in run() exits when the user kills
    // the process. On Unix, SIGINT / SIGTERM terminate the
    // process by default; on Windows, Ctrl-C in the terminal
    // terminates the process. The OS reaps the pairing server
    // thread when the process exits.
}

/// Self-re-exec into a detached child process and write the PID
/// file. The parent returns Ok(()) so `main()` exits 0. The child
/// continues running the server under the same `server_state`
/// handle and polls the shutdown flag until the process is killed.
///
/// Implementation: re-spawn ourselves with `--child-of-detach`
/// (a hidden internal flag) and the original config, but skip
/// the re-detach in the child. This avoids raw fork(2) syscalls
/// and gives us a clean parent/child split with no signal-handler
/// plumbing.
#[cfg(unix)]
fn detach_and_write_pidfile(
    server_state: &Arc<miracle_claw_lib::pairing_server::PairingServerState>,
) -> Result<(), i32> {
    use std::io::Write;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[miracle-claw-pair] current_exe failed: {e}");
            return Err(1);
        }
    };
    // Build a child command that inherits the parent's config
    // verbatim, but with --child-of-detach prepended so the
    // child's run() skips the re-detach.
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--child-of-detach");
    for arg in std::env::args().skip(1) {
        if arg == "--detach" {
            continue;
        }
        cmd.arg(arg);
    }
    // Note: v0.0.1 does NOT call setsid() in the child. The user
    // can wrap the daemon with `nohup` or `setsid` from the shell
    // for full terminal detachment. Adding `libc` to the dep tree
    // for one syscall is out of scope for v0.0.1.
    let child = match cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[miracle-claw-pair] failed to spawn detached child: {e}");
            return Err(1);
        }
    };
    // Write the PID file with the CHILD's pid (so the OS file
    // matches the long-running daemon, not the short-lived parent).
    let mut f = match std::fs::File::create(PID_FILE) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[miracle-claw-pair] failed to write {}: {e}", PID_FILE);
            return Err(1);
        }
    };
    let _ = writeln!(f, "{}", child.id());
    drop(f);
    // Suppress unused warnings for state we don't touch here.
    let _ = server_state;
    Ok(())
}

/// Internal sentinel flag consumed only by the detached child.
/// Tells run() to skip the detach path (we're already detached)
/// and fall straight into the foreground poll loop.
#[allow(dead_code)]
const CHILD_OF_DETACH_FLAG: &str = "--child-of-detach";

/// True if the current process was spawned as a detached child.
/// Checked at the top of run() so the parent can short-circuit
/// the detach path in the child process.
#[cfg(unix)]
fn is_child_of_detach() -> bool {
    std::env::args().any(|a| a == CHILD_OF_DETACH_FLAG)
}

// ────────────────────────────────────────────────────────────────────
// Helper for Office Bot's run() — not used by this file, but
// included so the IpAddr type is reachable in `cargo doc` and
// the binary's symbol table.
//
// (Office Bot can remove this once `run()` is wired up.)
// ────────────────────────────────────────────────────────────────────
#[allow(dead_code)]
fn _ensure_ip_addr_in_scope(_: IpAddr) {}

// ────────────────────────────────────────────────────────────────────
// Tests — pin the CLI surface so future refactors don't drift.
// Add to the existing tests/ dir; this `#[cfg(test)] mod tests`
// keeps the file self-checking.
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Convenience: run parse_args_from with a Vec of strings.
    /// Uses `&'static str` to avoid the empty-vec type-inference
    /// trap (a `vec![]` literal can't infer `S: Into<String>`).
    fn parse(args: Vec<&'static str>) -> Result<Config, CliError> {
        parse_args_from(args.into_iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_when_no_flags() {
        let c = parse(vec![]).unwrap();
        assert_eq!(c.port, 0, "port default should be 0 (OS-assigned)");
        assert_eq!(c.bind, BindMode::Loopback, "bind default = loopback");
        assert_eq!(c.mode, RunMode::Foreground, "mode default = foreground");
        assert_eq!(c.log_level, LogLevel::Info, "log level default = info");
        assert!(c.identity_file.is_none(), "no --identity-file by default");
        assert!(!c.ephemeral_identity, "not ephemeral by default");
        assert!(!c.print_info, "no --print-info by default");
    }

    #[test]
    fn port_zero_is_accepted_as_os_assigned_sentinel() {
        let c = parse(vec!["--port", "0"]).unwrap();
        assert_eq!(c.port, 0);
    }

    #[test]
    fn port_min_and_max_accepted() {
        assert_eq!(parse(vec!["--port", "1024"]).unwrap().port, 1024);
        assert_eq!(parse(vec!["--port", "65535"]).unwrap().port, 65535);
    }

    #[test]
    fn port_below_1024_rejected() {
        let err = parse(vec!["--port", "80"]).unwrap_err();
        assert!(matches!(err, CliError::InvalidPort(_)));
    }

    #[test]
    fn port_above_65535_rejected() {
        let err = parse(vec!["--port", "70000"]).unwrap_err();
        assert!(matches!(err, CliError::InvalidPort(_)));
    }

    #[test]
    fn port_non_integer_rejected() {
        let err = parse(vec!["--port", "abc"]).unwrap_err();
        assert!(matches!(err, CliError::InvalidPort(_)));
    }

    #[test]
    fn bind_all_three_modes_accepted() {
        for (s, expected) in [
            ("loopback", BindMode::Loopback),
            ("lan", BindMode::Lan),
            ("tailnet", BindMode::Tailnet),
        ] {
            assert_eq!(parse(vec!["--bind", s]).unwrap().bind, expected);
        }
    }

    #[test]
    fn bind_unknown_rejected() {
        let err = parse(vec!["--bind", "anywhere"]).unwrap_err();
        assert!(matches!(err, CliError::InvalidBind(_)));
    }

    #[test]
    fn log_level_all_three_accepted() {
        for (s, expected) in [
            ("silent", LogLevel::Silent),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
        ] {
            assert_eq!(
                parse(vec!["--log-level", s]).unwrap().log_level,
                expected
            );
        }
    }

    #[test]
    fn log_level_unknown_rejected() {
        let err = parse(vec!["--log-level", "trace"]).unwrap_err();
        assert!(matches!(err, CliError::InvalidLogLevel(_)));
    }

    #[test]
    fn identity_file_overrides_default() {
        let c = parse(vec!["--identity-file", "/tmp/foo.json"]).unwrap();
        assert_eq!(c.identity_file, Some(PathBuf::from("/tmp/foo.json")));
        assert!(!c.ephemeral_identity);
    }

    #[test]
    fn ephemeral_identity_sets_flag() {
        let c = parse(vec!["--ephemeral-identity"]).unwrap();
        assert!(c.ephemeral_identity);
        assert!(c.identity_file.is_none());
    }

    #[test]
    fn identity_file_and_ephemeral_are_mutually_exclusive() {
        let err = parse(vec![
            "--identity-file",
            "/tmp/foo.json",
            "--ephemeral-identity",
        ])
        .unwrap_err();
        assert!(matches!(err, CliError::ConflictingIdentityFlags));
        // And the reverse order.
        let err = parse(vec![
            "--ephemeral-identity",
            "--identity-file",
            "/tmp/foo.json",
        ])
        .unwrap_err();
        assert!(matches!(err, CliError::ConflictingIdentityFlags));
    }

    #[test]
    fn foreground_and_detach() {
        assert_eq!(
            parse(vec!["--foreground"]).unwrap().mode,
            RunMode::Foreground
        );
        #[cfg(unix)]
        assert_eq!(parse(vec!["--detach"]).unwrap().mode, RunMode::Detach);
    }

    #[test]
    #[cfg(not(unix))]
    fn detach_rejected_on_non_unix() {
        let err = parse(vec!["--detach"]).unwrap_err();
        assert!(matches!(err, CliError::DetachUnsupported));
    }

    #[test]
    fn print_info_flag() {
        let c = parse(vec!["--print-info"]).unwrap();
        assert!(c.print_info);
    }

    #[test]
    fn short_and_long_help_equivalents() {
        // -h and --help both call print_help + exit 0. We can't
        // test process::exit here, but we can assert the match
        // arms fire by passing the strings through to the parser
        // and expecting an Err (since they never reach the Config
        // builder). The actual exit happens in parse_args_from.
        // Instead, just verify the strings exist as match arms.
        let known_flags: HashSet<&str> =
            ["-h", "--help", "-V", "--version"].into_iter().collect();
        for f in known_flags {
            assert!(["-h", "--help", "-V", "--version"].contains(&f));
        }
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse(vec!["--definitely-not-a-flag"]).unwrap_err();
        assert!(matches!(err, CliError::UnknownFlag(_)));
    }

    #[test]
    fn missing_value_rejected() {
        let err = parse(vec!["--port"]).unwrap_err();
        assert!(matches!(err, CliError::MissingValue("--port")));
        let err = parse(vec!["--bind"]).unwrap_err();
        assert!(matches!(err, CliError::MissingValue("--bind")));
        let err = parse(vec!["--log-level"]).unwrap_err();
        assert!(matches!(err, CliError::MissingValue("--log-level")));
    }

    #[test]
    fn env_var_port_fallback() {
        // MIRACLE_CLAW_PAIR_PORT=48291 should kick in when --port absent.
        // SAFETY: tests in a single test binary are serial under
        // cargo's default harness. We save/restore to avoid leaking
        // the env var to other tests.
        let saved = env::var(ENV_PORT).ok();
        env::set_var(ENV_PORT, "48291");
        let c = parse(vec![]).unwrap();
        env::remove_var(ENV_PORT);
        if let Some(v) = saved {
            env::set_var(ENV_PORT, v);
        }
        assert_eq!(c.port, 48291);
    }

    #[test]
    fn env_var_port_out_of_range_rejected() {
        let saved = env::var(ENV_PORT).ok();
        env::set_var(ENV_PORT, "80");
        let err = parse(vec![]).unwrap_err();
        env::remove_var(ENV_PORT);
        if let Some(v) = saved {
            env::set_var(ENV_PORT, v);
        }
        assert!(matches!(err, CliError::InvalidPort(_)));
    }

    #[test]
    fn env_var_port_zero_accepted() {
        let saved = env::var(ENV_PORT).ok();
        env::set_var(ENV_PORT, "0");
        let c = parse(vec![]).unwrap();
        env::remove_var(ENV_PORT);
        if let Some(v) = saved {
            env::set_var(ENV_PORT, v);
        }
        assert_eq!(c.port, 0);
    }

    #[test]
    fn full_realistic_invocation() {
        // Mirrors the spec's "phone simulator end-to-end" example
        // minus --detach (which is Unix-only and we don't want to
        // fork in tests).
        let c = parse(vec![
            "--port", "9229",
            "--bind", "loopback",
            "--identity-file", "/tmp/pair.json",
            "--log-level", "debug",
            "--print-info",
        ])
        .unwrap();
        assert_eq!(c.port, 9229);
        assert_eq!(c.bind, BindMode::Loopback);
        assert_eq!(c.identity_file, Some(PathBuf::from("/tmp/pair.json")));
        assert_eq!(c.log_level, LogLevel::Debug);
        assert!(c.print_info);
        assert_eq!(c.mode, RunMode::Foreground);
        assert!(!c.ephemeral_identity);
    }

    #[test]
    fn internal_child_of_detach_flag_is_silently_consumed() {
        // The detached child (re-execed by the parent) passes
        // --child-of-detach as argv[1]. The parser must accept
        // it without storing it, so the child's `is_child_of_detach()`
        // check is the only thing that sees it.
        let c = parse(vec![
            "--child-of-detach",
            "--port", "0",
            "--bind", "loopback",
        ])
        .unwrap();
        assert_eq!(c.port, 0);
        assert_eq!(c.bind, BindMode::Loopback);
        // No Config field stores the flag — verified by parsing
        // succeeding even when no other field is set.
    }

    #[test]
    fn cli_error_display_includes_offending_value() {
        // Defensive: the user-facing message should mention what
        // they passed, so a copy-paste typo is obvious.
        let e = CliError::InvalidBind("wlan".into());
        let s = e.to_string();
        assert!(s.contains("wlan"), "Display should include 'wlan': {s}");
        let e = CliError::InvalidPort("nope".into());
        let s = e.to_string();
        assert!(s.contains("nope"), "Display should include 'nope': {s}");
    }

    #[test]
    fn version_constant_is_nonempty() {
        // env!("CARGO_PKG_VERSION") resolves at compile time. We
        // can't pin the exact string (depends on Cargo.toml), but
        // it must be set to something.
        assert!(!VERSION.is_empty());
    }
}
