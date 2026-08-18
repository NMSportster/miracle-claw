// ============================================================================
// Miracle Claw — Tauri lib
//
// First-run path (called once per process start, from setup()):
//   1. Locate the bundled resources (Tauri sets TAURI_BUNDLE_RESOURCES_DIR,
//      but in dev mode we look in src-tauri/resources/).
//   2. Ensure the MAIC provider plugin is present in the user's
//      openclaw extensions directory (idempotent — SHA-checked copy).
//      ~/.openclaw/extensions/maic/ on *nix,
//      %APPDATA%\MiracleClaw\extensions\maic\ on Windows.
//   3. Patch the user's openclaw.json (idempotent deep-merge) so the plugin
//      is in `pluginRoots` if not already.
//   4. Spawn miracle-claw-launcher with --gateway-port N as a sidecar.
//   5. Poll TCP connect to 127.0.0.1:28789 until it accepts (15s cap).
//      (openclaw accepts the connection immediately when the port is bound,
//      so we don't need an HTTP roundtrip.)
//   6. Register RunEvent::ExitRequested to kill the launcher cleanly.
//
// Architecture doc: README.md
// ============================================================================

use std::fs;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{Manager, RunEvent};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

mod launcher_info;
use launcher_info::{launcher_binary_name, MAIC_PLUGIN_FILENAMES, OPENCLAW_PORT};

// ----------------------------------------------------------------------------
// State we hold for the lifetime of the process
// ----------------------------------------------------------------------------

#[derive(Default)]
struct AppState {
    /// Handle to the launcher sidecar child process (if started).
    /// Mutex because RunEvent handlers + setup() cross thread boundaries.
    launcher_child:
        Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(dead_code)] // fields are populated but not all read by the frontend yet
struct FirstRunReport {
    maic_plugin_installed: bool,
    maic_plugin_already_present: bool,
    openclaw_json_patched: bool,
    openclaw_json_already_patched: bool,
    launcher_spawned: bool,
    gateway_ready: bool,
    gateway_error: Option<String>,
}

// ----------------------------------------------------------------------------
// Resource resolution
// ----------------------------------------------------------------------------

/// Returns the path to the bundled resources directory.
///
/// In production: Tauri sets `TAURI_BUNDLE_RESOURCES_DIR` before our process
/// starts. This is the directory we ship `node.exe` (Win), the openclaw bundle,
/// and the MAIC plugin source into via `bundle.resources` in tauri.conf.json.
///
/// In dev (`cargo tauri dev`): that env var is not set. We fall back to
/// `<src-tauri>/resources/` so iter-loop testing works without a full bundle.
fn resources_dir(_app: &tauri::AppHandle) -> PathBuf {
    if let Ok(p) = std::env::var("TAURI_BUNDLE_RESOURCES_DIR") {
        return PathBuf::from(p);
    }
    // Dev fallback: try walking up from current_exe to find src-tauri/resources.
    let mut cursor = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from));
    while let Some(dir) = cursor {
        let candidate = dir.join("resources");
        if candidate.join("openclaw.mjs").is_file() {
            return candidate;
        }
        cursor = dir.parent().map(PathBuf::from);
    }
    PathBuf::from("src-tauri/resources")
}

/// Path to the launcher sidecar binary. Tauri resolves this from
/// `bundle.externalBin` at runtime; this helper is just for log clarity.
#[allow(dead_code)]
fn launcher_exe_path(resources: &Path) -> PathBuf {
    #[cfg(windows)]
    let exe = format!("{}.exe", launcher_binary_name());
    #[cfg(not(windows))]
    let exe = launcher_binary_name().to_string();

    let candidates = [
        PathBuf::from(&exe),
        resources.join("..").join("bin").join(&exe),
        resources.join("bin").join(&exe),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(exe))
}

// ----------------------------------------------------------------------------
// MAIC plugin first-run bootstrap
// ----------------------------------------------------------------------------

/// Returns the user's openclaw extensions directory.
/// *nix:    $HOME/.openclaw/extensions/
/// Windows: %APPDATA%\MiracleClaw\extensions\
fn openclaw_extensions_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(roam) = std::env::var("APPDATA") {
            return PathBuf::from(roam).join("MiracleClaw").join("extensions");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".openclaw").join("extensions");
        }
    }
    PathBuf::from("./.openclaw/extensions")
}

/// Returns the path to the user's openclaw.json.
fn openclaw_json_path() -> PathBuf {
    openclaw_extensions_dir()
        .parent()
        .map(|p| p.join("openclaw.json"))
        .unwrap_or_else(|| PathBuf::from("./openclaw.json"))
}

/// Returns the path to the bundled MAIC plugin source (a directory containing
/// `openclaw.plugin.json`, `index.js`, `package.json`).
fn bundled_maic_plugin_dir(resources: &Path) -> PathBuf {
    resources.join("maic-plugin")
}

#[derive(PartialEq)]
enum CopyResult {
    AlreadyPresent,
    Installed,
    SourceMissing,
}

fn copy_maic_plugin_if_needed(
    resources: &Path,
) -> io::Result<(CopyResult, PathBuf)> {
    let src_dir = bundled_maic_plugin_dir(resources);
    if !src_dir.is_dir() {
        return Ok((CopyResult::SourceMissing, PathBuf::new()));
    }
    let dest_dir = openclaw_extensions_dir().join("maic");

    // Idempotency check: hash every source file, compare to a manifest
    // written next to the install. Skip copy if matches.
    let manifest = dest_dir.join(".miracle-claw-installed-sha256");
    let src_hashes = compute_hashes_manifest(&src_dir)?;
    let on_disk = fs::read_to_string(&manifest).unwrap_or_default();
    if dest_dir.is_dir() && on_disk == src_hashes {
        return Ok((CopyResult::AlreadyPresent, dest_dir));
    }

    fs::create_dir_all(&dest_dir)?;
    for name in MAIC_PLUGIN_FILENAMES {
        let sp = src_dir.join(name);
        if sp.is_file() {
            fs::copy(&sp, dest_dir.join(name))?;
        }
    }
    fs::write(manifest, src_hashes)?;

    Ok((CopyResult::Installed, dest_dir))
}

fn compute_hashes_manifest(dir: &Path) -> io::Result<String> {
    let mut out = String::new();
    for name in MAIC_PLUGIN_FILENAMES {
        let p = dir.join(name);
        if p.is_file() {
            let h = sha256_file(&p)?;
            out.push_str(&format!("{} {}\n", name, h));
        }
    }
    Ok(out)
}

fn sha256_file(p: &Path) -> io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut hasher = Sha256::new();
    hasher.update(&buf);
    Ok(format!("{:x}", hasher.finalize()))
}

// ----------------------------------------------------------------------------
// openclaw.json patch (idempotent deep merge)
// ----------------------------------------------------------------------------

fn ensure_maic_in_openclaw_json() -> io::Result<bool> {
    let path = openclaw_json_path();
    if !path.is_file() {
        let initial = json!({
            "plugins": {
                "roots": [path_to_json_string(&openclaw_extensions_dir())]
            },
            "pluginRoots": [path_to_json_string(&openclaw_extensions_dir())]
        });
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap())?;
        return Ok(true);
    }

    let raw = fs::read_to_string(&path)?;
    let mut config: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let extensions_path = openclaw_extensions_dir();
    let extensions_str = path_to_json_string(&extensions_path);

    let mut patched = false;

    // Key path 1: plugins.roots (older openclaw).
    if config.get("plugins").is_none() {
        config["plugins"] = json!({});
        patched = true;
    }
    let roots = config["plugins"]
        .as_object_mut()
        .unwrap()
        .entry("roots".to_string())
        .or_insert_with(|| json!([]));
    if let Some(arr) = roots.as_array_mut() {
        if !arr.iter().any(|v| v.as_str() == Some(&extensions_str)) {
            arr.push(json!(extensions_str.clone()));
            patched = true;
        }
    } else {
        *roots = json!([extensions_str.clone()]);
        patched = true;
    }

    // Key path 2: pluginRoots (newer openclaw).
    let plugin_roots = config
        .as_object_mut()
        .unwrap()
        .entry("pluginRoots".to_string())
        .or_insert_with(|| json!([]));
    if let Some(arr) = plugin_roots.as_array_mut() {
        if !arr.iter().any(|v| v.as_str() == Some(&extensions_str)) {
            arr.push(json!(extensions_str));
            patched = true;
        }
    } else {
        *plugin_roots = json!([extensions_str]);
        patched = true;
    }

    if patched {
        fs::write(&path, serde_json::to_string_pretty(&config).unwrap())?;
    }
    Ok(patched)
}

fn path_to_json_string(p: &Path) -> String {
    // JSON doesn't allow backslashes; expand to forward slashes so config
    // files are portable between Win and *nix reading (and the `json!` macro
    // never chokes on a Windows path).
    p.to_string_lossy().replace('\\', "/")
}

// ----------------------------------------------------------------------------
// Health poll: TCP connect to 127.0.0.1:28789 until the port is bound.
// (openclaw accepts the connection the moment the port is bound; we don't
// need HTTP semantics — the webview's first GET will validate auth/MAIC.)
// ----------------------------------------------------------------------------

fn wait_for_gateway_ready(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_millis(1000);
    let addrs = format!("127.0.0.1:{}", port);

    while Instant::now() < deadline {
        if TcpStream::connect(addrs.as_str()).is_ok() {
            return Ok(());
        }
        thread::sleep(backoff);
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
    Err(format!(
        "gateway did not bind port {} within {:?}",
        port, timeout
    ))
}

// ----------------------------------------------------------------------------
// Tauri command: lets the frontend ask us what happened
// ----------------------------------------------------------------------------

#[tauri::command]
fn first_run_report(state: tauri::State<'_, AppState>) -> FirstRunReport {
    let launcher_spawned = state.launcher_child.lock().unwrap().is_some();
    let gateway_ready =
        TcpStream::connect(format!("127.0.0.1:{}", OPENCLAW_PORT)).is_ok();
    FirstRunReport {
        maic_plugin_installed: false,
        maic_plugin_already_present: false,
        openclaw_json_patched: false,
        openclaw_json_already_patched: false,
        launcher_spawned,
        gateway_ready,
        gateway_error: None,
    }
}

// ----------------------------------------------------------------------------
// Setup hook
// ----------------------------------------------------------------------------

fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_handle = app.handle().clone();
    let resources = resources_dir(&app_handle);

    eprintln!(
        "[miracle-claw] setup: resources = {}",
        resources.display()
    );

    // 1. Install MAIC plugin if needed.
    match copy_maic_plugin_if_needed(&resources) {
        Ok((CopyResult::AlreadyPresent, _)) => eprintln!("[miracle-claw] maic plugin: present (no change)"),
        Ok((CopyResult::Installed, _)) => eprintln!("[miracle-claw] maic plugin: installed fresh"),
        Ok((CopyResult::SourceMissing, _)) => eprintln!("[miracle-claw] maic plugin: source missing in bundle (skipping)"),
        Err(e) => eprintln!("[miracle-claw] maic plugin install error: {}", e),
    }

    // 2. Patch openclaw.json if needed.
    match ensure_maic_in_openclaw_json() {
        Ok(true) => eprintln!("[miracle-claw] openclaw.json: patched"),
        Ok(false) => eprintln!("[miracle-claw] openclaw.json: already patched"),
        Err(e) => eprintln!("[miracle-claw] openclaw.json patch error: {}", e),
    }

    // 3. Spawn launcher sidecar.
    let shell = app_handle.shell();
    let port_str = OPENCLAW_PORT.to_string();
    let launcher_name = launcher_binary_name().to_string();
    eprintln!(
        "[miracle-claw] spawning sidecar: {} --gateway-port {}",
        launcher_name, port_str
    );
    let spawned = shell.sidecar(launcher_name.clone()).and_then(|cmd| {
        cmd.args(["--gateway-port", &port_str]).spawn()
    });

    let (mut rx, child) = match spawned {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[miracle-claw] sidecar spawn failed: {}", e);
            return Err(Box::new(e));
        }
    };

    // Background log capture: pipe sidecar stdout/stderr to our stderr with a
    // tag so it's visible in Tauri's dev console.
    std::thread::spawn(move || {
        while let Some(event) = rx.blocking_recv() {
            match event {
                CommandEvent::Stdout(bytes) => {
                    eprint!("[launcher.stdout] {}", String::from_utf8_lossy(&bytes));
                    let _ = std::io::stderr().flush();
                }
                CommandEvent::Stderr(bytes) => {
                    eprint!("[launcher.stderr] {}", String::from_utf8_lossy(&bytes));
                    let _ = std::io::stderr().flush();
                }
                CommandEvent::Error(e) => eprintln!("[launcher.error] {}", e),
                CommandEvent::Terminated(payload) => eprintln!(
                    "[launcher.terminated] code={:?} signal={:?}",
                    payload.code, payload.signal
                ),
                _ => {}
            }
        }
    });

    app.state::<AppState>()
        .launcher_child
        .lock()
        .unwrap()
        .replace(child);

    // 4. Wait for gateway to be ready (poll TCP connect, 15s cap).
    match wait_for_gateway_ready(OPENCLAW_PORT, Duration::from_secs(15)) {
        Ok(()) => eprintln!("[miracle-claw] gateway READY on port {}", OPENCLAW_PORT),
        Err(e) => eprintln!("[miracle-claw] gateway NOT ready: {}", e),
    }

    // 5. Webview URL pre-configured in tauri.conf.json → http://localhost:28789/.
    //    The webview loads as soon as the renderer fires; nothing to do here.

    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![first_run_report])
        .setup(|app| {
            setup(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                eprintln!("[miracle-claw] exit — killing launcher child");
                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Some(child) = state.launcher_child.lock().unwrap().take() {
                        // tauri-plugin-shell v2's CommandChild::kill takes &self.
                        let _ = child.kill();
                    }
                }
            }
            _ => {}
        });
}
