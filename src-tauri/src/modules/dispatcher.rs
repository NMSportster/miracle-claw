//! Module command dispatcher — the bridge between Tauri commands and
//! installed module sidecars.
//!
//! ## How it works
//!
//! When JS calls `invoke('mc_voice_transcribe', { seconds: 5 })`:
//!
//! 1. The Tauri command `mc_voice_transcribe` (stub in `lib.rs`) is invoked.
//! 2. Stub calls `dispatcher::dispatch("mc_voice_transcribe", payload)`.
//! 3. Dispatcher looks up `mc_voice_transcribe` in the registry:
//!    - If found: spawn the module's sidecar binary, pass `action="transcribe"`
//!      + the payload via stdin JSON, return the response.
//!    - If not found: return `ModuleError::NotInstalled("voice")`.
//!
//! ## Why a dispatcher?
//!
//! - **One Tauri command per stub** keeps the ACL simple (`allow-mc-voice-transcribe`
//!   etc. — 4-file pattern from Lesson 219).
//! - **Stubs in base MC** mean the JS side doesn't need to know which modules
//!   are installed. The same `invoke('mc_voice_transcribe', ...)` call works
//!   whether voice is installed or not — it just gets a friendly error.
//! - **Module authors** just write `installer.json` and a sidecar binary.
//!   They don't touch base MC.
//!
//! ## Sidecar IPC protocol
//!
//! ```jsonc
//! // stdin (one JSON object, newline-terminated):
//! {
//!   "action": "transcribe",
//!   "params": { "seconds": 5 }
//! }
//!
//! // stdout (one JSON object, newline-terminated):
//! {
//!   "ok": true,
//!   "result": "hello world"
//! }
//! // OR
//! {
//!   "ok": false,
//!   "error": "no microphone found"
//! }
//! ```
//!
//! This mirrors MAIC's `tool_execution: "client"` pattern (Lesson 169).

use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use super::registry::Registry;
use super::ModuleError;

/// What we send to the sidecar on stdin.
#[derive(Debug, Clone, Serialize)]
struct SidecarRequest<'a> {
    action: &'a str,
    params: serde_json::Value,
}

/// What the sidecar sends back on stdout.
#[derive(Debug, Clone, Deserialize)]
struct SidecarResponse {
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Dispatch a Tauri command invocation to its module's sidecar.
///
/// `tauri_cmd` is the Tauri command name (e.g. `"mc_voice_transcribe"`).
/// `params` is the raw JSON params object from the JS invocation.
///
/// Returns the sidecar's `result` field as `serde_json::Value`, or
/// `ModuleError::NotInstalled` if no module owns this command.
pub fn dispatch(
    registry: &Registry,
    tauri_cmd: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ModuleError> {
    let (module, action) = match registry.resolve_command(tauri_cmd) {
        Some(pair) => pair,
        None => return Err(ModuleError::NotInstalled(tauri_cmd.to_string())),
    };

    let binary_path = sidecar_binary_path(&module.install_dir, &module.manifest.binary.name);
    if !binary_path.exists() {
        return Err(ModuleError::SpawnFailed(format!(
            "module '{}' sidecar not found at {}",
            module.manifest.id,
            binary_path.display()
        )));
    }

    run_sidecar(&binary_path, &action, params)
}

/// Compute the sidecar binary's absolute path.
///
/// On Windows, appends `.exe` if the manifest didn't.
fn sidecar_binary_path(module_dir: &std::path::Path, binary_name: &str) -> PathBuf {
    let mut p = module_dir.join("bin").join(binary_name);
    if cfg!(target_os = "windows") && p.extension().is_none() {
        p.set_extension("exe");
    }
    p
}

/// Spawn the sidecar, write the request to stdin, read the response from stdout.
///
/// Synchronous (std::process) on purpose — the caller wraps it in
/// `tokio::task::spawn_blocking` if needed. Tauri commands are themselves
/// async, so wrapping is the caller's responsibility.
fn run_sidecar(
    binary: &std::path::Path,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ModuleError> {
    let request = SidecarRequest { action, params };

    let mut child = std::process::Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ModuleError::SpawnFailed(format!("spawn failed: {e}")))?;

    // Write request to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let request_json = serde_json::to_string(&request)
            .map_err(|e| ModuleError::SpawnFailed(format!("request serialize: {e}")))?;
        stdin
            .write_all(request_json.as_bytes())
            .map_err(|e| ModuleError::SpawnFailed(format!("stdin write: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| ModuleError::SpawnFailed(format!("wait failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ModuleError::SpawnFailed(format!(
            "sidecar exited with status {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: SidecarResponse = serde_json::from_str(stdout.trim()).map_err(|e| {
        ModuleError::SpawnFailed(format!(
            "sidecar returned invalid JSON: {e}. Raw: {}",
            stdout.trim()
        ))
    })?;

    if response.ok {
        Ok(response
            .result
            .unwrap_or(serde_json::Value::Null))
    } else {
        Err(ModuleError::SpawnFailed(format!(
            "sidecar returned error: {}",
            response.error.unwrap_or_default()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test the request/response serde round-trip.
    #[test]
    fn sidecar_request_serializes() {
        let r = SidecarRequest {
            action: "transcribe",
            params: serde_json::json!({"seconds": 5}),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"action\":\"transcribe\""));
        assert!(s.contains("\"params\":{\"seconds\":5}"));
    }

    #[test]
    fn sidecar_response_ok_with_result() {
        let r: SidecarResponse = serde_json::from_str(
            r#"{"ok": true, "result": {"text": "hello"}}"#,
        )
        .unwrap();
        assert!(r.ok);
        assert_eq!(r.result.unwrap(), serde_json::json!({"text": "hello"}));
    }

    #[test]
    fn sidecar_response_err_with_message() {
        let r: SidecarResponse = serde_json::from_str(
            r#"{"ok": false, "error": "no microphone"}"#,
        )
        .unwrap();
        assert!(!r.ok);
        assert_eq!(r.error.unwrap(), "no microphone");
    }

    #[test]
    fn sidecar_response_minimal_ok() {
        let r: SidecarResponse = serde_json::from_str(r#"{"ok": true}"#).unwrap();
        assert!(r.ok);
        assert!(r.result.is_none());
    }

    #[test]
    fn sidecar_path_appends_exe_on_windows() {
        let p = sidecar_binary_path(std::path::Path::new("/tmp/mod"), "voice");
        if cfg!(target_os = "windows") {
            assert_eq!(p.extension().and_then(|s| s.to_str()), Some("exe"));
        } else {
            assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("voice"));
        }
    }

    #[test]
    fn dispatch_returns_not_installed_for_unknown_cmd() {
        let reg = Registry::new();
        let res = dispatch(&reg, "mc_voice_transcribe", serde_json::json!({}));
        assert!(matches!(res, Err(ModuleError::NotInstalled(_))));
    }
}
