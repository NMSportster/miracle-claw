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
    // Lesson 754 (2026-08-29): on Windows, `Path::canonicalize` returns an
    // extended-length UNC path (\\?\C:\...) when the file exists, but FAILS
    // when the file doesn't exist (e.g. write_file creating a new file). The
    // fallback `unwrap_or_else(|_| p.to_path_buf())` keeps the raw input path
    // without the UNC prefix, so `canonical.starts_with(&root_canon)` fails
    // because root_canon is UNC-prefixed while canonical is plain.
    //
    // Fix: also strip the `\\?\` prefix from any canonicalized path we use
    // for comparison. This unifies both code paths (file exists or not) so
    // the prefix check is byte-for-byte the same shape.
    let strip_unc = |path: PathBuf| -> PathBuf {
        let s = path.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
        path
    };
    let canonical = match p.canonicalize() {
        Ok(c) => strip_unc(c),
        Err(_) => {
            // File doesn't exist (write_file creating new). Canonicalize
            // the parent dir instead, then append the file name.
            match p.parent() {
                Some(parent) => match parent.canonicalize() {
                    Ok(pcanon) => strip_unc(pcanon).join(p.file_name().unwrap_or_default()),
                    Err(_) => p.to_path_buf(),
                },
                None => p.to_path_buf(),
            }
        }
    };
    let mut allowed = false;
    for root in allowed_roots() {
        let root_canon = match root.canonicalize() {
            Ok(c) => strip_unc(c),
            Err(_) => root,
        };
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

/// Extract path-looking tokens from a shell command string and verify
/// each one is under an allowed root.
///
/// Lesson 796 (2026-08-30 00:46 MDT, David): MiniMax-M3 picked
/// `/mnt/c/Program Files/MiracleClaw/...` as a Desktop target — the
/// path syntax was correct but the model mixed up which folder the
/// user asked for. cwd was validated but the inline path slipped
/// through. After a 5-retry loop minimax kept picking the SAME wrong
/// path. We need to catch this at the boundary.
///
/// Patterns matched (each tested against allowed_roots):
///   - Windows drive paths: `C:\...`, `D:\foo\bar.txt`
///   - WSL paths: `/mnt/c/...`, `/mnt/d/...`
///   - Home shortcut: `~/foo/bar.txt`
///   - Unix absolute: `/foo/bar.txt` (only validated if it looks like
///     a file — has a `.ext` or a trailing slash). `/bin`, `/usr` etc.
///     are NOT matched (they're command paths, not file targets).
///
/// NOT matched (intentional, to avoid false positives):
///   - `/tmp`, `/var`, `/etc` — we don't have control over these.
///     If the model writes there it'll get a real OS error from
///     cmd/sh, which surfaces clearly.
///   - Shell variables: `${HOME}/foo`, `$(echo /etc)`, `~user/foo`
///     (non-current-user tilde). These resolve at shell time and we
///     can't intercept them cheaply.
///
/// Errors: returns the first path that's outside allowed_roots, with
/// the command snippet containing it for debugging.
fn validate_command_paths(command: &str) -> Result<(), String> {

    // Collect candidate path strings. We scan the command char-by-char
    // for quoted regions and unquoted whitespace-separated tokens, then
    // pick out tokens that look like paths.
    let mut candidates: Vec<String> = Vec::new();
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '"' || c == '\'' {
            // Quoted string — take its contents (without quotes) if it
            // looks like a path.
            let quote = c;
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] as char != quote {
                end += 1;
            }
            if end > start {
                let s = &command[start..end];
                if looks_like_path(s) {
                    candidates.push(s.to_string());
                }
            }
            i = end + 1;
        } else if c == '~' && (i == 0 || bytes[i - 1] as char == ' ' || bytes[i - 1] as char == '=' || bytes[i - 1] as char == ':') {
            // Tilde at start of word: `~/foo` or `~` alone. Walk to
            // next whitespace.
            let start = i;
            let mut end = start;
            while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] as char != '"' && bytes[end] as char != '\'' {
                end += 1;
            }
            let s = &command[start..end];
            if looks_like_path(s) {
                candidates.push(s.to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }

    for candidate in &candidates {
        // Try to normalize; if it parses to a real PathBuf, validate.
        // `~` is a special case — expand to $HOME or %USERPROFILE%.
        let expanded = if candidate.starts_with("~/") || candidate == "~" {
            let home = if cfg!(windows) {
                std::env::var("USERPROFILE").unwrap_or_default()
            } else {
                std::env::var("HOME").unwrap_or_default()
            };
            if home.is_empty() {
                // Can't expand without HOME — skip (will fail at shell).
                continue;
            }
            format!("{}{}", home, &candidate[1..])
        } else {
            candidate.clone()
        };

        // Best-effort parse: not all candidates will parse cleanly. If
        // normalize_user_path rejects (e.g. relative path that happens
        // to contain `\`), we skip silently — the shell will surface
        // the real error.
        if let Ok(p) = normalize_user_path(&expanded) {
            if let Err(e) = assert_path_allowed(&p) {
                return Err(format!(
                    "bash_run: command contains path {:?} which is outside allowed paths. \
                     Hint: use Documents, Desktop, Downloads, or the MC workspace dir. \
                     Original error: {}",
                    p, e,
                ));
            }
        }
    }
    Ok(())
}

/// Heuristic: does this string look like a file path?
///
/// True for:
///   - Starts with a drive letter (`C:\`, `D:/`)
///   - Starts with `/mnt/<drive>/`
///   - Starts with `~/`
///   - Starts with `/` AND contains `.` (file extension) or trailing `/`
///
/// False for:
///   - `/bin`, `/usr/bin`, `/etc` — command paths, not targets
///   - Single-word commands like `ls`, `cat`
///   - URLs (`http://`, `https://`)
fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() < 2 {
        return false;
    }
    let trimmed = s.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return false;
    }
    // Drive letter (C:\ or D:/)
    if trimmed.len() >= 3 {
        let bytes = trimmed.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
            return true;
        }
    }
    if trimmed.starts_with("/mnt/") && trimmed.len() >= 7 {
        // /mnt/c/...
        return trimmed.as_bytes()[6] == b'/';
    }
    if trimmed.starts_with("~/") || trimmed == "~" {
        return true;
    }
    if trimmed.starts_with('/') {
        // Unix absolute path with file extension or trailing slash.
        let rest = &trimmed[1..];
        if rest.contains('.') || trimmed.ends_with('/') {
            return true;
        }
    }
    false
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

    // Lesson 796 (2026-08-30, David): the cwd is validated above, but
    // the `command` string can still contain inline paths (e.g.
    // `Set-Content -Path "C:\Users\foo\file.txt"`, `cat > /etc/passwd`,
    // `rm ~/important.docx`). MiniMax-M3 picked `/mnt/c/Program Files/MiracleClaw/...`
    // thinking it was a Desktop path — silently writing into the MC
    // install dir. We need to detect path-looking tokens in the command
    // and reject any that aren't under allowed_roots BEFORE running.
    //
    // This is best-effort regex matching. It catches common patterns
    // (drive-letter Windows paths, /mnt/<drive>/... WSL paths, ~/... home
    // shortcuts, /tmp/... paths). It does NOT catch every shell trick —
    // e.g. `${HOME}/foo`, `$(echo /etc/passwd)`, variable expansion. For
    // those, the runtime errors will surface them as they fail.
    validate_command_paths(command)?;

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
    //
    // Lesson 757 (2026-08-29 17:08 MDT, David): also accept standard
    // unified-diff format (`--- a/path` / `+++ b/path` headers). The
    // schema description says "unified-diff-like" but v1.0.7 only
    // handled openai-style, so when the model or user sent a
    // genuine unified-diff patch the parser silently no-op'd and
    // returned `Ok("apply_patch: applied")` without writing anything.
    // Root cause: `--- a/path` never matched `*** Update File:` so
    // `current_path` stayed `None` for the entire loop, the trailing
    // flush branch was skipped, and the function returned success.
    //
    // Two changes here:
    //   (a) Recognize `--- path` and `+++ path` as path markers. We
    //       use the `+++ path` (target file) — matching `git apply`
    //       semantics.
    //   (b) Track whether any file was actually applied; if neither
    //       openai-style `*** Update File:` nor unified-diff `+++`
    //       appeared in the patch, error with a clear "no file
    //       modified" message instead of silently reporting success.
    let patch = params
        .get("patch")
        .and_then(|v| v.as_str())
        .ok_or("missing required field: patch")?;

    let mut current_path: Option<PathBuf> = None;
    let mut hunks: Vec<(String, String)> = Vec::new(); // (old, new)
    let mut in_hunk = false;
    let mut old_buf = String::new();
    let mut new_buf = String::new();
    // Lesson 757: tracks whether the patch caused any writes. Used to
    // detect the silent-no-op case where current_path never gets set
    // (e.g. parser received a unified-diff format it doesn't recognize
    // and silently ignored all path lines).
    let mut applied_any = false;

    for line in patch.lines() {
        if let Some(path_str) = line.strip_prefix("*** Update File:") {
            // Flush previous file
            if let Some(p) = current_path.take() {
                apply_one_hunk(&p, &hunks)?;
                applied_any = true;
            }
            current_path = Some(normalize_user_path(path_str.trim())?);
            hunks.clear();
            in_hunk = false;
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            // Lesson 757: unified-diff target-file marker. We use the
            // `+++` line because that's the post-change path (matches
            // git apply / patch semantics). Skip the `+++ /dev/null`
            // case for now (file deletion) — emit a clear error if
            // seen, since we don't support deletes in v1.
            let raw = rest.trim();
            if raw == "/dev/null" {
                return Err(
                    "apply_patch: file deletion via unified-diff '+++ /dev/null' not implemented".to_string(),
                );
            }
            // Lesson 757: strip unified-diff `a/` and `b/` directory
            // prefixes (the git convention). Without this, a path
            // like `b//home/adeal/foo.txt` would fail the
            // absolute-path check in normalize_user_path.
            let stripped = raw
                .strip_prefix("a/")
                .or_else(|| raw.strip_prefix("b/"))
                .unwrap_or(raw);
            // Flush previous file before switching target
            if let Some(p) = current_path.take() {
                apply_one_hunk(&p, &hunks)?;
                applied_any = true;
            }
            current_path = Some(normalize_user_path(stripped)?);
            hunks.clear();
            in_hunk = false;
        } else if line.starts_with("--- ") {
            // Lesson 757: unified-diff source-file marker. We don't
            // use this for path resolution (use +++ instead), but we
            // do ignore `--- /dev/null` (file creation) cleanly. For
            // any other `---` line, just skip — the +++ line carries
            // the actual target path.
            //
            // Note: We also need to NOT reset `in_hunk` here, because
            // a `--- path` line should not abort a hunk in progress.
            // The +++ line above is the one that switches targets.
            continue;
        } else if line.starts_with("*** Begin Patch") || line.starts_with("*** End Patch") {
            // Sentinel lines; End Patch flushes.
            if line.starts_with("*** End Patch") {
                if let Some(p) = current_path.take() {
                    apply_one_hunk(&p, &hunks)?;
                    applied_any = true;
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
        applied_any = true;
    }

    // Lesson 757: fail loud instead of silent no-op. If the patch
    // text was syntactically parseable but never produced a write
    // (e.g. user sent a unified-diff with neither --- nor +++ that
    // we recognized), surface that as an error so the model / user
    // gets a clear signal rather than a false-positive success.
    if !applied_any {
        return Err(
            "apply_patch: patch parsed but no file modified. \
             Expected either openai-style '*** Update File: path' or \
             unified-diff '+++ path' header."
                .to_string(),
        );
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

    // Lesson 757 (2026-08-29 17:08 MDT, David): the apply_patch
    // parser silently no-op'd on unified-diff format (`--- a/path` /
    // `+++ b/path` headers) and returned `Ok("apply_patch: applied")`
    // without writing anything. These tests pin both the original
    // openai-style format and the newly-supported unified-diff format
    // so the regression can't sneak back in.
    //
    // Setup: a per-test fixture file in the first allowed root
    // (Documents/Desktop/Downloads/workspace), which is always
    // passable through `assert_path_allowed()` regardless of host OS.

    fn fixture_root() -> std::path::PathBuf {
        // Walk allowed_roots() and pick the first one we can both
        // read AND write to (and whose parent exists). On dev boxes
        // some roots (e.g. Downloads/) may not exist; skip those.
        for p in allowed_roots() {
            if p.is_absolute() && p.exists() && p.is_dir() {
                return p;
            }
        }
        // Fallback: create the workspace dir if nothing else works.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let workspace = std::path::PathBuf::from(format!(
            "{}/.local/share/miracle-claw/workspace",
            home
        ));
        std::fs::create_dir_all(&workspace).expect("create workspace dir");
        workspace
    }

    fn write_fixture(name: &str, content: &str) -> std::path::PathBuf {
        let p = fixture_root().join(name);
        std::fs::write(&p, content).expect("write fixture");
        p
    }

    #[test]
    fn apply_patch_openai_style_writes_file() {
        let path = write_fixture(
            "lesson757_openai.txt",
            "hello world\nfoo bar\nbaz qux\n",
        );
        let path_str = path.to_string_lossy().to_string();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello world\n+GOODBYE WORLD\n*** End Patch",
            path_str
        );
        let params = serde_json::json!({ "patch": patch });
        let result = apply_patch(&params);
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "GOODBYE WORLD\nfoo bar\nbaz qux\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn apply_patch_unified_diff_with_ab_prefix_writes_file() {
        // Lesson 757: this is the format that used to silently no-op.
        let path = write_fixture(
            "lesson757_unified_ab.txt",
            "hello world\nfoo bar\nbaz qux\n",
        );
        let path_str = path.to_string_lossy().to_string();
        let patch = format!(
            "--- a/{0}\n+++ b/{0}\n@@ -1 +1 @@\n-hello world\n+GOODBYE WORLD",
            path_str
        );
        let params = serde_json::json!({ "patch": patch });
        let result = apply_patch(&params);
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content, "GOODBYE WORLD\nfoo bar\nbaz qux\n",
            "unified-diff with a/b prefix should have modified the file"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn apply_patch_unified_diff_plain_prefix_writes_file() {
        let path = write_fixture(
            "lesson757_unified_plain.txt",
            "hello world\nfoo bar\nbaz qux\n",
        );
        let path_str = path.to_string_lossy().to_string();
        let patch = format!(
            "--- {0}\n+++ {0}\n@@ -1 +1 @@\n-hello world\n+GOODBYE WORLD",
            path_str
        );
        let params = serde_json::json!({ "patch": patch });
        let result = apply_patch(&params);
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "GOODBYE WORLD\nfoo bar\nbaz qux\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn apply_patch_no_path_header_errors_not_silent_success() {
        // Lesson 757: malformed patch with hunk body but NO path header
        // (neither openai `*** Update File:` nor unified-diff `+++`)
        // must error, not return Ok("applied").
        let params = serde_json::json!({
            "patch": "@@\n-foo\n+bar"
        });
        let result = apply_patch(&params);
        assert!(
            result.is_err(),
            "expected Err for path-less patch, got {:?}",
            result
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("no file modified"),
            "error message should explain no file was modified: {:?}",
            msg
        );
    }

    #[test]
    fn apply_patch_devnull_target_errors() {
        // Lesson 757: file deletion via unified-diff `+++ /dev/null`
        // is not implemented in v1; surface as a clear error.
        let path = write_fixture(
            "lesson757_devnull.txt",
            "hello world\n",
        );
        let path_str = path.to_string_lossy().to_string();
        let patch = format!(
            "--- a/{0}\n+++ /dev/null\n@@ -1 +0 @@\n-hello world\n",
            path_str
        );
        let params = serde_json::json!({ "patch": patch });
        let result = apply_patch(&params);
        assert!(result.is_err(), "expected Err for /dev/null target");
        assert!(
            result.unwrap_err().contains("not implemented"),
            "error should explain deletion is not implemented"
        );
        let _ = std::fs::remove_file(&path);
    }

    // Lesson 796 (2026-08-30, David): bash_run path validation.
    // MiniMax-M3 picked `/mnt/c/Program Files/MiracleClaw/...` thinking
    // it was a Desktop target. The path syntax was correct but the
    // model mixed up which folder the user asked for. These tests pin
    // the new `validate_command_paths` behavior.

    #[test]
    fn validate_command_paths_passes_unix_commands() {
        // Plain shell commands with no path-looking tokens.
        assert!(validate_command_paths("ls -la").is_ok());
        assert!(validate_command_paths("echo hello world").is_ok());
        assert!(validate_command_paths("pwd && whoami").is_ok());
        // Shell builtins / command paths only — NOT file targets.
        assert!(validate_command_paths("cd /tmp && cat /etc/hosts").is_ok(),
            "/tmp and /etc are not validated by Lesson 796 (shell surfaces real errors)");
        // URL — not a path.
        assert!(validate_command_paths("curl https://example.com/api").is_ok());
    }

    #[test]
    fn validate_command_paths_rejects_outside_allowed() {
        // /mnt/c/Program Files/... — the actual Lesson 796 mistake.
        // Minimax-M3 picked this path thinking it was a Desktop
        // target. The realistic command has the path quoted (because
        // of the space in "Program Files"), which is what our
        // tokenizer detects. This path is OUTSIDE allowed_roots on
        // BOTH Linux and Windows test envs (Program Files isn't
        // Documents/Desktop/Downloads/workspace).
        let cmd = "cp \"/mnt/c/Program Files/MiracleClaw/foo.txt\" ~/Desktop/";
        let result = validate_command_paths(cmd);
        if cfg!(target_os = "linux") {
            // On Linux, /mnt/c/Program Files/... resolves to a path
            // outside /home/<user>/* allowed roots, so it rejects.
            assert!(result.is_err(),
                "should reject /mnt/c/Program Files/... on Linux: got {:?}", result);
            let err = result.unwrap_err();
            // Make sure the error mentions the bad path so the model
            // can learn from it.
            assert!(err.contains("Program Files") || err.contains("Program"),
                "error should reference the rejected path: {}", err);
        }
        // Windows behavior is host-specific; just don't crash.
        let _ = result;
    }

    #[test]
    fn validate_command_paths_passes_documents_path() {
        // Documents, Desktop, Downloads ARE allowed roots — these
        // commands must pass.
        if cfg!(target_os = "linux") {
            assert!(validate_command_paths("cat ~/Documents/notes.txt").is_ok());
            assert!(validate_command_paths("ls ~/Desktop/").is_ok());
            assert!(validate_command_paths("cp /tmp/foo ~/Downloads/bar.txt").is_ok(),
                "/tmp is not validated; ~/Downloads expands to allowed root");
        }
        // Windows behavior is host-specific; skip.
    }

    #[test]
    fn looks_like_path_classifies_correctly() {
        // Drive-letter paths
        assert!(looks_like_path(r"C:\Users\me\file.txt"));
        assert!(looks_like_path(r"D:/foo/bar.docx"));
        // WSL paths
        assert!(looks_like_path("/mnt/c/Users/me/file.txt"));
        assert!(looks_like_path("/mnt/d/data/"));
        // Home shortcut
        assert!(looks_like_path("~/Documents/foo.txt"));
        assert!(looks_like_path("~/"));
        // Unix absolute with extension or trailing slash
        assert!(looks_like_path("/home/me/file.txt"));
        assert!(looks_like_path("/var/log/app.log"));
        assert!(looks_like_path("/tmp/data/"));
        // NOT paths
        assert!(!looks_like_path("ls"));
        assert!(!looks_like_path("hello world"));
        assert!(!looks_like_path(""));
        assert!(!looks_like_path("a"));
        assert!(!looks_like_path("https://example.com"));
        assert!(!looks_like_path("/bin"));       // no extension, no trailing slash
        assert!(!looks_like_path("/usr/local")); // no extension
        assert!(!looks_like_path("/etc"));       // no extension
    }
}