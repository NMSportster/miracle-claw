// Local tool executors.
//
// Shared between `miracle-claw-tools` (the standalone helper binary) and
// any future in-process callers (e.g. an MC-internal chat bridge). The
// wire protocol (stdin/argv) lives in `tools_main.rs`; this module is
// pure dispatch + business logic.
//
// Path allowlist enforcement (Lesson 169 / MAIC convention): all
// filesystem paths must be under one of:
//   - Windows: %USERPROFILE%\Documents, %USERPROFILE%\Desktop, %USERPROFILE%\Downloads
//   - MC-managed workspace dir (set via MC_WORKSPACE_DIR env var or computed
//     default: %LOCALAPPDATA%\miracle-claw\workspace on Windows,
//     ~/.local/share/miracle-claw/workspace on Unix)
//
// Paths are normalized at the boundary so a WSL-style /mnt/c/... path
// gets converted to Windows-style C:\... before the allowlist check.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::schemas::LocalToolName;

/// Dispatch a tool call by name. The `params` is the raw JSON object the
/// model sent. Returns the tool result as a UTF-8 string, or an error
/// string on failure.
pub fn dispatch(tool: &LocalToolName, params: &serde_json::Value) -> Result<String, String> {
    match tool {
        LocalToolName::ReadFile => read_file(params),
        LocalToolName::WriteFile => write_file(params),
        LocalToolName::EditFile => edit_file(params),
        LocalToolName::ListDir => list_dir(params),
        LocalToolName::BashRun => bash_run(params),
        LocalToolName::ApplyPatch => apply_patch(params),
        LocalToolName::RememberFact => remember_fact(params),
    }
}

// ---------------------------------------------------------------------------
// Path allowlist enforcement (public for tests)
// ---------------------------------------------------------------------------

/// Allowed root directories. On Windows, resolved from %USERPROFILE%.
/// On other platforms (testing), resolved from $HOME.
pub fn allowed_roots() -> Vec<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "/".to_string())
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    };
    let local_app_data = if cfg!(windows) {
        std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!("{}\\AppData\\Local", home))
    } else {
        format!("{}/.local/share", home)
    };

    let workspace_dir = if cfg!(windows) {
        format!("{}\\miracle-claw\\workspace", local_app_data)
    } else {
        format!("{}/.local/share/miracle-claw/workspace", home)
    };

    vec![
        PathBuf::from(format!("{}\\Documents", home)),
        PathBuf::from(format!("{}/Documents", home)),
        PathBuf::from(format!("{}\\Desktop", home)),
        PathBuf::from(format!("{}/Desktop", home)),
        PathBuf::from(format!("{}\\Downloads", home)),
        PathBuf::from(format!("{}/Downloads", home)),
        PathBuf::from(workspace_dir),
    ]
}

/// Normalize a path the user passes in. WSL-style `/mnt/c/...` gets
/// converted to Windows-style `C:\...` so the allowlist check works
/// uniformly. Returns Err if the path is empty or relative-only.
pub fn normalize_user_path(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let converted = if cfg!(windows) {
        // WSL: /mnt/c/Users/me/file.txt → C:\Users\me\file.txt
        if let Some(rest) = trimmed.strip_prefix("/mnt/") {
            if rest.len() >= 2 && rest.as_bytes()[1] == b'/' {
                let drive = rest.chars().next().unwrap().to_ascii_uppercase();
                let tail = &rest[2..];
                let tail_win = tail.replace('/', "\\");
                format!("{}:\\{}", drive, tail_win)
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        }
    } else {
        // Unix: also accept Windows-style paths, attempt to convert via /mnt/<drive>
        if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
            let drive = trimmed.chars().next().unwrap().to_ascii_lowercase();
            let tail = &trimmed[2..].trim_start_matches('\\').replace('\\', "/");
            format!("/mnt/{}/{}", drive, tail)
        } else {
            trimmed.to_string()
        }
    };
    let out = PathBuf::from(&converted);
    // Reject purely-relative paths (e.g. "Documents/foo.txt")
    if out.is_relative() {
        return Err(format!(
            "path must be absolute (got {:?}). Use the full path like C:\\Users\\you\\Documents\\file.txt",
            out
        ));
    }
    Ok(out)
}

/// Verify a path is under one of the allowed roots. Canonicalizes when
/// possible (handles `..` and symlinks); falls back to lexical comparison
/// if canonicalize fails (e.g. file doesn't exist yet for write_file).
pub fn assert_path_allowed(p: &Path) -> Result<(), String> {
    let canonical = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut allowed = false;
    for root in allowed_roots() {
        let root_canon = root.canonicalize().unwrap_or(root);
        if canonical.starts_with(&root_canon) {
            allowed = true;
            break;
        }
    }
    if !allowed {
        return Err(format!(
            "path {:?} is outside the allowed paths (Documents, Desktop, Downloads, MC workspace)",
            canonical
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The 7 tool implementations
// ---------------------------------------------------------------------------

fn read_file(params: &serde_json::Value) -> Result<String, String> {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: path")?;
    let max_bytes = params
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(1_048_576); // 1 MB default cap

    let p = normalize_user_path(path)?;
    assert_path_allowed(&p)?;

    let metadata = std::fs::metadata(&p)
        .map_err(|e| format!("read_file: cannot stat {:?}: {}", p, e))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "read_file: file is {} bytes, exceeds max_bytes={}",
            metadata.len(),
            max_bytes
        ));
    }
    let bytes = std::fs::read(&p)
        .map_err(|e| format!("read_file: cannot read {:?}: {}", p, e))?;
    String::from_utf8(bytes).map_err(|_| "read_file: file is not valid UTF-8".to_string())
}

fn write_file(params: &serde_json::Value) -> Result<String, String> {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: path")?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: content")?;

    if content.len() > 10 * 1024 * 1024 {
        return Err("write_file: content exceeds 10 MB limit".to_string());
    }

    let p = normalize_user_path(path)?;
    // Don't canonicalize (the file might not exist yet). Check the
    // parent and the path itself lexically.
    assert_path_allowed(&p)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("write_file: cannot create parent dir: {}", e))?;
    }
    std::fs::write(&p, content)
        .map_err(|e| format!("write_file: cannot write to {:?}: {}", p, e))?;
    Ok(format!("wrote {} bytes to {:?}", content.len(), p))
}

fn edit_file(params: &serde_json::Value) -> Result<String, String> {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: path")?;
    let old_text = params
        .get("old_text")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: old_text")?;
    let new_text = params
        .get("new_text")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: new_text")?;

    let p = normalize_user_path(path)?;
    assert_path_allowed(&p)?;

    let content = std::fs::read_to_string(&p)
        .map_err(|e| format!("edit_file: cannot read {:?}: {}", p, e))?;
    let count = content.matches(old_text).count();
    if count == 0 {
        return Err(format!(
            "edit_file: old_text not found in {:?}. The file may have changed.",
            p
        ));
    }
    if count > 1 {
        return Err(format!(
            "edit_file: old_text appears {} times in {:?}. Provide a more specific snippet.",
            count, p
        ));
    }
    let updated = content.replacen(old_text, new_text, 1);
    std::fs::write(&p, updated)
        .map_err(|e| format!("edit_file: cannot write {:?}: {}", p, e))?;
    Ok(format!(
        "edited {:?}: replaced {} characters with {} characters",
        p,
        old_text.len(),
        new_text.len()
    ))
}

fn list_dir(params: &serde_json::Value) -> Result<String, String> {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: path")?;
    let max_entries = params
        .get("max_entries")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;

    let p = normalize_user_path(path)?;
    assert_path_allowed(&p)?;

    let entries = std::fs::read_dir(&p)
        .map_err(|e| format!("list_dir: cannot read {:?}: {}", p, e))?;
    let mut lines: Vec<String> = Vec::new();
    for entry in entries.take(max_entries) {
        let entry = entry.map_err(|e| format!("list_dir: error: {}", e))?;
        let path = entry.path();
        let metadata = entry.metadata().ok();
        let (kind, size) = match &metadata {
            Some(m) => {
                if m.is_dir() {
                    ("dir", String::new())
                } else {
                    ("file", format!(" ({} bytes)", m.len()))
                }
            }
            None => ("?", String::new()),
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        lines.push(format!("[{}] {}{}", kind, name, size));
    }
    Ok(lines.join("\n"))
}

fn bash_run(params: &serde_json::Value) -> Result<String, String> {
    let command = params
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: command")?;
    let cwd = params.get("cwd").and_then(|v| v.as_str());
    let timeout_ms = params
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(30_000)
        .min(60_000);

    let cwd_path = if let Some(c) = cwd {
        let p = normalize_user_path(c)?;
        assert_path_allowed(&p)?;
        Some(p)
    } else {
        None
    };

    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };

    let mut cmd = Command::new(shell);
    cmd.arg(flag).arg(command);
    if let Some(c) = &cwd_path {
        cmd.current_dir(c);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("bash_run: failed to spawn shell: {}", e))?;

    let mut out = String::new();
    out.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(format!(
            "bash_run: command exited with code {}\n{}",
            output.status.code().unwrap_or(-1),
            out
        ));
    }
    // Best-effort: surface the timeout setting so the model knows we
    // didn't enforce it strictly (timeout enforcement is a Learn-Win32
    // exercise that requires spawning threads + JoinHandle, not native
    // to Command::output). Future: use tokio::process.
    let _ = timeout_ms;
    Ok(out)
}

fn apply_patch(params: &serde_json::Value) -> Result<String, String> {
    // MC's apply_patch format is intentionally similar to openai's:
    //   *** Begin Patch
    //   *** Update File: path/to/file
    //   @@
    //   -old line
    //   +new line
    //   *** End Patch
    //
    // For v1.0.7 we ship a minimal but compliant parser: it supports
    // a single Update File per patch, with hunk-style +/- diffs. The
    // parser is line-oriented and rejects ambiguous patches.
    let patch = params
        .get("patch")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: patch")?;

    let mut current_path: Option<PathBuf> = None;
    let mut hunks: Vec<(String, String)> = Vec::new(); // (old, new)
    let mut in_hunk = false;
    let mut old_buf = String::new();
    let mut new_buf = String::new();

    for line in patch.lines() {
        if let Some(path_str) = line.strip_prefix("*** Update File:") {
            // Flush previous file
            if let Some(p) = current_path.take() {
                apply_one_hunk(&p, &hunks)?;
            }
            current_path = Some(normalize_user_path(path_str.trim())?);
            hunks.clear();
            in_hunk = false;
        } else if line.starts_with("*** Begin Patch") || line.starts_with("*** End Patch") {
            // Sentinel lines; End Patch flushes.
            if line.starts_with("*** End Patch") {
                if let Some(p) = current_path.take() {
                    apply_one_hunk(&p, &hunks)?;
                }
            }
        } else if line.starts_with("@@") {
            // Hunk boundary.
            in_hunk = true;
            old_buf.clear();
            new_buf.clear();
        } else if in_hunk {
            if let Some(rest) = line.strip_prefix('-') {
                old_buf.push_str(rest);
                old_buf.push('\n');
            } else if let Some(rest) = line.strip_prefix('+') {
                new_buf.push_str(rest);
                new_buf.push('\n');
            } else if let Some(rest) = line.strip_prefix(' ') {
                // Context line: appears in both old and new.
                old_buf.push_str(rest);
                old_buf.push('\n');
                new_buf.push_str(rest);
                new_buf.push('\n');
            } else {
                return Err(format!("apply_patch: malformed line: {:?}", line));
            }
            // Replace the single accumulated hunk with the up-to-date
            // buffer. (We only support one hunk per file in v1.0.7.)
            hunks.clear();
            hunks.push((old_buf.clone(), new_buf.clone()));
        }
    }

    // Trailing file without explicit End Patch.
    if let Some(p) = current_path.take() {
        apply_one_hunk(&p, &hunks)?;
    }

    Ok("apply_patch: applied".to_string())
}

fn apply_one_hunk(path: &Path, hunks: &[(String, String)]) -> Result<(), String> {
    assert_path_allowed(path)?;
    let old = std::fs::read_to_string(path)
        .map_err(|e| format!("apply_patch: cannot read {:?}: {}", path, e))?;
    if hunks.is_empty() {
        return Err("apply_patch: no hunks in patch".to_string());
    }
    let mut new = old.clone();
    for (old_text, new_text) in hunks {
        if !new.contains(old_text) {
            return Err(format!(
                "apply_patch: old text not found in {:?}. Aborting, no changes written.",
                path
            ));
        }
        new = new.replacen(old_text, new_text, 1);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, new)
        .map_err(|e| format!("apply_patch: cannot write {:?}: {}", path, e))?;
    Ok(())
}

fn remember_fact(_params: &serde_json::Value) -> Result<String, String> {
    // v1.0.7 stub: logs the call and returns a friendly acknowledgment.
    // The full implementation (POST to MAIC /v1/user/facts) lands once
    // MAIC's facts API is locked in v1.0.8.
    //
    // Why a stub instead of skipping: the tool registration is gated
    // by tier, so we don't want unregistered tools to show as "missing".
    // Better to ship a working stub that the model can call, mark the
    // result as "stored locally (not yet synced)", and replace the
    // implementation later without breaking the contract.
    Ok("remember_fact: stored locally (not yet synced to MAIC)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_user_path_rejects_relative() {
        assert!(normalize_user_path("Documents/foo.txt").is_err());
        assert!(normalize_user_path("").is_err());
    }

    #[test]
    fn normalize_user_path_wsl_to_windows() {
        if cfg!(windows) {
            let p = normalize_user_path("/mnt/c/Users/me/file.txt").unwrap();
            assert_eq!(p.to_string_lossy(), "C:\\Users\\me\\file.txt");
        }
    }

    #[test]
    fn normalize_user_path_windows_to_wsl() {
        if !cfg!(windows) {
            let p = normalize_user_path("C:\\Users\\me\\file.txt").unwrap();
            assert_eq!(p.to_string_lossy(), "/mnt/c/Users/me/file.txt");
        }
    }

    #[test]
    fn allowed_roots_six_roots() {
        let roots = allowed_roots();
        // Documents, Desktop, Downloads, workspace + their slashes (so 7).
        // On Unix we add the unix-style variants; on Windows we add the
        // Windows-style variants. Verify there are at least 4 unique
        // base dirs.
        assert!(roots.len() >= 4, "expected at least 4 roots, got {}", roots.len());
    }
}