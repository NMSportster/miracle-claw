// ============================================================================
// miracle-claw-launcher — Tauri sidecar that boots the OpenClaw gateway.
//
// Why this exists:
//   Tauri's sidecar model wraps ONE binary per externalBin entry. We don't want
//   to expose Node.js directly to the permission allowlist (that's a shell
//   escape into the user's machine). So we ship this small Rust binary as the
//   sidecar; it owns the lookup of the bundled Node + openclaw bundle and
//   execs `node miracle-claw-gateway.mjs gateway ...`. Tauri's allowlist locks the
//   exact invocation pattern of THIS binary — see src-tauri/capabilities/main.json
//   for `cmd: "miracle-claw-launcher"` with the `--gateway-port` arg validator.
//
// Resource layout (after `tauri build` produces the bundle):
//
//   <resources>/
//     node.exe                      (Tauri resource, Windows)
//     node/                         (Tauri resource, *nix)
//     miracle-claw-gateway.mjs                  (Tauri resource — points at node_modules/ + dist/)
//     package.json
//     node_modules/
//     dist/
//
// In `cargo tauri dev` / `cargo run` mode, the resources directory falls back
// to `src-tauri/resources/` so we can iterate without rebuilding the bundle.
//
// Args (exact order, allowlist-validated upstream):
//   --gateway-port <N>           required, 1024-65535
//   --bind loopback|lan|tailnet   default loopback (also drives openclaw --bind)
//   --auth none|token|password    default none (per-user Tauri window, same box)
//   --log-level silent|info|debug  default info
//
// Lifecycle:
//   - Parent (Tauri main) spawns us with stdio piped (stdout→log, stderr→log).
//   - We exec node. If node exits, we exit with the same code. The parent
//     owns the child process handle and is responsible for kill on teardown.
//     We just propagate exit codes verbatim so the parent's logs are clean.
// ============================================================================

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

// Shared constants — same file as the main lib uses, gated by `#[path]` so
// each crate (lib bin + launcher bin) resolves it independently.
#[path = "launcher_info.rs"]
mod launcher_info;

use launcher_info::OPENCLAW_PORT as DEFAULT_GATEWAY_PORT;
use launcher_info::MAIC_PLUGIN_FILENAMES;
const MIN_PORT: u16 = 1024;
const MAX_PORT: u16 = 65535;

#[derive(Debug)]
struct Args {
    gateway_port: u16,
    bind: String,
    auth: String,
    log_level: String,
}

#[derive(Debug)]
enum LauncherError {
    /// Parsing --gateway-port failed (Tauri allowlist should already block
    /// bad input — but we re-validate here as defense-in-depth).
    InvalidPort,
    MissingPort,
    /// Bundle is broken: missing miracle-claw-gateway.mjs, missing node executable, etc.
    MissingBundleResource(&'static str),
    /// node openclaw exited with non-zero status. Carries the exit code.
    NodeNonZero(i32),
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LauncherError::InvalidPort => write!(
                f,
                "--gateway-port value is not a valid integer in the range {}-{}",
                MIN_PORT, MAX_PORT
            ),
            LauncherError::MissingPort => write!(f, "--gateway-port <N> is required"),
            LauncherError::MissingBundleResource(what) => {
                write!(f, "bundle is missing required resource: {}", what)
            }
            LauncherError::NodeNonZero(c) => write!(f, "node openclaw exited {}", c),
        }
    }
}

fn print_usage() {
    eprintln!(
        "miracle-claw-launcher — boots OpenClaw gateway from bundled resources.\n\
         \n\
         Usage:\n  \
           miracle-claw-launcher --gateway-port <N> [--bind <mode>] [--auth <mode>] [--log-level <lvl>]\n\
         \n\
         Flags:\n  \
           --gateway-port <N>          Port the gateway will listen on (1024-65535)\n  \
           --bind  loopback|lan|tailnet  default: loopback\n  \
           --auth  none|token|password   default: none\n  \
           --log-level silent|info|debug  default: info\n\
         \n\
         Exit codes:\n  \
           0  clean shutdown (gateway exited 0)\n  \
           1  launcher-side error (bad args, missing bundle)\n  \
           N  gateway exit code propagated"
    );
}

fn parse_args() -> Result<Args, LauncherError> {
    let mut iter = env::args().skip(1);

    let mut gateway_port: Option<u16> = None;
    let mut bind = String::from("loopback");
    let mut auth = String::from("none");
    let mut log_level = String::from("info");

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--gateway-port" => {
                let raw = iter.next().ok_or(LauncherError::MissingPort)?;
                let n: u64 = raw
                    .parse()
                    .map_err(|_| LauncherError::InvalidPort)?;
                let n_u16 = u16::try_from(n).map_err(|_| LauncherError::InvalidPort)?;
                if !(MIN_PORT..=MAX_PORT).contains(&n_u16) {
                    return Err(LauncherError::InvalidPort);
                }
                gateway_port = Some(n_u16);
            }
            "--bind" => {
                bind = iter.next().unwrap_or_else(|| "loopback".to_string());
            }
            "--auth" => {
                auth = iter.next().unwrap_or_else(|| "none".to_string());
            }
            "--log-level" => {
                log_level = iter.next().unwrap_or_else(|| "info".to_string());
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("[miracle-claw-launcher] unknown arg: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
    }

    Ok(Args {
        gateway_port: gateway_port.ok_or(LauncherError::MissingPort)?,
        bind,
        auth,
        log_level,
    })
}

/// Locate the resources directory. Three cases:
///
/// 1. **Production bundle** (`tauri build`): Tauri sets `TAURI_BUNDLE_RESOURCES_DIR`.
/// 2. **Standalone executable**: look in cwd, then next-to-exe (for tests).
/// 3. **Dev mode** (`cargo tauri dev`): walk up from the launcher's path until
///    we find `src-tauri/resources/`.
fn locate_resources() -> Result<PathBuf, LauncherError> {
    if let Ok(p) = env::var("TAURI_BUNDLE_RESOURCES_DIR") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Ok(path);
        }
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("miracle-claw-gateway.mjs").is_file() {
                return Ok(dir.to_path_buf());
            }
            // Common install layout: sidecar lives under bin/, resources next to it.
            if let Some(up) = dir.parent() {
                let res = up.join("resources");
                if res.is_dir() && res.join("miracle-claw-gateway.mjs").is_file() {
                    return Ok(res);
                }
            }
        }

        let mut cursor = exe.parent();
        while let Some(dir) = cursor {
            let candidate = dir.join("resources");
            if candidate.join("miracle-claw-gateway.mjs").is_file() {
                return Ok(candidate);
            }
            cursor = dir.parent();
        }
    }

    Err(LauncherError::MissingBundleResource(
        "could not locate resources directory (looked for miracle-claw-gateway.mjs)",
    ))
}

/// Locate the Node binary inside the resources directory.
///
/// Windows: `node.exe` at resources root.
/// *nix: `node/` file (dev) or `<node>/bin/node` (portable Node).
/// Last resort: $PATH lookup (for Linux dev when no bundled Node exists yet).
fn locate_node_executable(resources: &Path) -> Result<PathBuf, LauncherError> {
    #[cfg(windows)]
    {
        let node = resources.join("node.exe");
        if !node.is_file() {
            return Err(LauncherError::MissingBundleResource("node.exe"));
        }
        return Ok(node);
    }
    #[cfg(not(windows))]
    {
        let at_root = resources.join("node");
        if at_root.is_file() {
            return Ok(at_root);
        }
        let portable = resources.join("node").join("bin").join("node");
        if portable.is_file() {
            return Ok(portable);
        }
        if let Ok(p) = env::var("MC_NODE_BIN") {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Ok(path);
            }
        }
        if let Ok(Some(p)) = which_in_path("node") {
            return Ok(p);
        }
        Err(LauncherError::MissingBundleResource(
            "node binary (expected resources/node or resources/node/bin/node, or $PATH for dev)",
        ))
    }
}

fn which_in_path(name: &str) -> std::io::Result<Option<PathBuf>> {
    let path = match env::var_os("PATH") {
        Some(p) => p,
        None => return Ok(None),
    };
    for dir in env::split_paths(&path) {
        let candidate = if cfg!(windows) {
            dir.join(format!("{}.exe", name))
        } else {
            dir.join(name)
        };
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn build_node_command(args: &Args, node_bin: &Path, resources: &Path) -> Command {
    let miracle_claw_gateway = resources.join("miracle-claw-gateway.mjs");
    let node_path = resources.join("node_modules");

    let mut cmd = Command::new(node_bin);
    cmd.arg(&miracle_claw_gateway)
        .arg("gateway")
        .arg("--port")
        .arg(args.gateway_port.to_string())
        .arg("--bind")
        .arg(&args.bind)
        .arg("--auth")
        .arg(&args.auth)
        // First-run defense in depth: openclaw's gateway refuses to start
        // with exit code 78 ("Missing config") on a fresh install where no
        // openclaw.json exists. lib.rs setup() pre-writes a minimal valid
        // config before spawning us; this flag is the belt to that
        // suspenders in case the pre-write failed (file locked, antivirus,
        // disk full). The user's config still wins when it exists.
        .arg("--allow-unconfigured")
        .env("OPENCLAW_STATE_DIR", default_openclaw_state_dir())
        // Lesson 973 (2026-09-11): the bundled openclaw gateway reads its
        // config from `openclaw.json` by default. MC writes to `chat.json`
        // in the same state dir. Without this env var, the gateway ignores
        // the `agents.defaults.model` block we seeded in lib.rs setup()
        // and falls through to its hardcoded `openai/gpt-5.5` default,
        // which has no key in the embedded agent's auth store → 401
        // `missing-provider-auth` on every chat turn. Setting
        // `OPENCLAW_CONFIG_PATH` to the same `chat.json` lib.rs writes
        // makes the gateway pick up the seeded `maic/` provider chain.
        .env("OPENCLAW_CONFIG_PATH", default_openclaw_config_path())
        .env("NODE_PATH", &node_path)
        // AP-833-U (2026-09-08, David): bundled openclaw (v2026.7.1-2)
        // has a startup update check that hits registry.npmjs.org/openclaw
        // on every gateway boot and surfaces an "Update available: vX.Y.Z
        // (running v2026.7.1-2)" notification in the embedded UI. MC owns
        // its own release cadence via the installer; an out-of-band openclaw
        // upgrade would replace node_modules/ with a set we haven't tested
        // against and could break the MAIC plugin handshake / chat history
        // format. This env var disables openclaw's auto-install path. The
        // REAL fix is `update.checkOnStart: false` in openclaw.json (see
        // `disable_update_check_for_existing_installs` in lib.rs) — this
        // is belt to that suspenders in case the user later resets their
        // config or the openclaw.json is missing the block for some reason.
        .env("OPENCLAW_NO_AUTO_UPDATE", "1")
        // Run from resources dir so the bundled `node` symlink (Linux dev)
        // resolves and openclaw's relative imports (skills/, dist/) find their
        // siblings. Absolute paths we pass (miracle-claw-gateway.mjs) would still work
        // without this, but openclaw resolves some paths relative to cwd.
        .current_dir(resources)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if !args.log_level.is_empty() {
        cmd.env("OPENCLAW_LOG_LEVEL", &args.log_level);
    }

    cmd
}

#[cfg(windows)]
fn default_openclaw_state_dir() -> OsString {
    if let Ok(roam) = env::var("APPDATA") {
        return OsString::from(format!("{}\\MiracleClaw", roam));
    }
    OsString::from("MiracleClaw")
}

#[cfg(not(windows))]
fn default_openclaw_state_dir() -> OsString {
    if let Ok(home) = env::var("HOME") {
        // Isolated from system OpenClaw's ~/.openclaw/. MC carries its own
        // state, sessions, config, and extensions under ~/.miracle-claw/.
        // This prevents config races when both run on the same machine.
        return OsString::from(format!("{}/.miracle-claw", home));
    }
    OsString::from("miracle-claw")
}

/// Lesson 973 (2026-09-11 12:55 MDT, David): point the bundled openclaw
/// gateway at `chat.json` inside MC's state dir, NOT the default
/// `<stateDir>/openclaw.json`.
///
/// **Why this exists**: lib.rs's `openclaw_json_path()` returns
/// `<stateDir>/chat.json` and writes `agents.defaults.model` + provider
/// seed lists there. The bundled openclaw's `resolveConfigPath` (in
/// `dist/paths-*.js`) defaults to `<stateDir>/openclaw.json` whenever
/// `OPENCLAW_CONFIG_PATH` is unset. That mismatch meant the gateway was
/// booting with NO config (relied on `--allow-unconfigured` to survive),
/// which is why `chat.json`'s carefully-seeded `agents.defaults.model`
/// block was ignored and the gateway fell through to its hardcoded
/// `?? openai/gpt-5.5` default → `missing-provider-auth` 401 on every
/// chat turn.
///
/// The TUI launch path in lib.rs (mc_terminal_start) already sets
/// `OPENCLAW_CONFIG_PATH=<stateDir>/chat.json` for its own shell. The
/// GUI launch path (this file) was missing the same env var — the
/// launcher only set `OPENCLAW_STATE_DIR`, which is what made the
/// bundled gateway look one directory up the wrong file.
///
/// We mirror the TUI's path here so both shells hit the same config.
fn default_openclaw_config_path() -> OsString {
    let state_dir = default_openclaw_state_dir();
    let mut buf = OsString::with_capacity(state_dir.len() + 16);
    buf.push(state_dir);
    #[cfg(windows)]
    {
        buf.push("\\chat.json");
    }
    #[cfg(not(windows))]
    {
        buf.push("/chat.json");
    }
    buf
}

fn run(args: Args) -> Result<(), LauncherError> {
    let resources = locate_resources()?;
    let node_bin = locate_node_executable(&resources)?;

    if !resources.join("miracle-claw-gateway.mjs").is_file() {
        return Err(LauncherError::MissingBundleResource("miracle-claw-gateway.mjs"));
    }
    if !resources.join("package.json").is_file() {
        return Err(LauncherError::MissingBundleResource(
            "package.json (with openclaw deps)",
        ));
    }
    if !resources.join("node_modules").is_dir() {
        return Err(LauncherError::MissingBundleResource(
            "node_modules/ (openclaw dependencies)",
        ));
    }

    eprintln!(
        "[miracle-claw-launcher] booting gateway on port {} (bind={}, auth={})",
        args.gateway_port, args.bind, args.auth
    );
    eprintln!(
        "[miracle-claw-launcher] node={} resources={}",
        node_bin.display(),
        resources.display()
    );

    let mut cmd = build_node_command(&args, &node_bin, &resources);
    let mut child = cmd
        .spawn()
        .map_err(|e| {
            eprintln!("[miracle-claw-launcher] spawn failed: {}", e);
            e
        })
        .map_err(|_| LauncherError::MissingBundleResource("node openclaw spawn"))?;

    eprintln!(
        "[miracle-claw-launcher] node openclaw pid={}",
        child.id()
    );

    // Poll loop. Parent (lib.rs) holds its own copy of the child handle and
    // owns kill-on-teardown. We just propagate exit codes.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(1);
                if code != 0 {
                    return Err(LauncherError::NodeNonZero(code));
                }
                return Ok(());
            }
            Ok(None) => thread::sleep(Duration::from_millis(150)),
            Err(e) => {
                eprintln!("[miracle-claw-launcher] try_wait failed: {}", e);
                return Err(LauncherError::NodeNonZero(1));
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    // Lesson 528: identify THIS sidecar EXE in the side-car log so we know
    // exactly which build is running. The launcher's own banner prints first,
    // before any args are parsed or node is exec'd, so a stale/wrong install
    // is detectable from line 1 of the log.
    eprintln!(
        "[miracle-claw-launcher] miracle-claw-launcher v{} (build {} UTC)",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_TIMESTAMP"),
    );

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[miracle-claw-launcher] {}", e);
            print_usage();
            return std::process::ExitCode::from(1);
        }
    };

    if let Err(e) = run(args) {
        eprintln!("[miracle-claw-launcher] {}", e);
        return match e {
            LauncherError::NodeNonZero(c) => {
                std::process::ExitCode::from(u8::try_from(c).unwrap_or(1))
            }
            _ => std::process::ExitCode::from(1),
        };
    }

    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lesson 973 (2026-09-11 12:55 MDT, David): `default_openclaw_config_path`
    /// MUST return a path inside the same state dir as
    /// `default_openclaw_state_dir`, with the basename `chat.json` (NOT
    /// `openclaw.json`). The bundled openclaw gateway reads whichever file
    /// this env var points at; if it points at a non-existent file the
    /// gateway boots with no config and falls through to the hardcoded
    /// `openai/gpt-5.5` default → 401 `missing-provider-auth` on first chat.
    ///
    /// This test pins two invariants:
    /// 1. The directory portion of `default_openclaw_config_path()` matches
    ///    `default_openclaw_state_dir()` — both must agree on where MC's
    ///    state lives, otherwise we'd point the gateway at a dir the
    ///    `setup()` writer never touched.
    /// 2. The basename is `chat.json` — the file lib.rs's
    ///    `openclaw_json_path()` actually writes to. If we ever rename the
    ///    file, this test fails before the regression ships.
    #[test]
    fn lesson_973_config_path_lives_in_state_dir_and_ends_in_chat_json() {
        let state_dir = default_openclaw_state_dir();
        let config_path = default_openclaw_config_path();

        let state_str = state_dir.to_string_lossy();
        let config_str = config_path.to_string_lossy();

        // The config path MUST start with the state dir — otherwise the
        // gateway is looking somewhere other than where MC writes.
        assert!(
            config_str.starts_with(state_str.as_ref()),
            "config path {} must live inside state dir {}",
            config_str,
            state_str
        );

        // The basename MUST be `chat.json` so we agree with
        // `openclaw_json_path()` in lib.rs.
        #[cfg(windows)]
        let suffix = "\\chat.json";
        #[cfg(not(windows))]
        let suffix = "/chat.json";
        assert!(
            config_str.ends_with(suffix),
            "config path {} must end with {} (lib.rs writes to chat.json, NOT openclaw.json)",
            config_str,
            suffix
        );
    }
}
