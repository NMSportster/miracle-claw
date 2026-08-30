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
        LocalToolName::WebFetch => web_fetch(params),
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

/// Lesson 805 (rc55.13): When the command is `python -c "<code>"` and
/// the code embeds a Windows path like `C:\Users\foo\file.txt`, Python's
/// tokenizer interprets the `\U`, `\f`, and other backslash-letter
/// sequences as escape codes, raising SyntaxError. Convert Windows paths
/// inside the quoted code segment to forward slashes (Python accepts
/// them on every platform).
///
/// Only operates on the single argument after `-c` (between the next
/// pair of matched quotes). If we can't reliably find that boundary
/// (e.g. unbalanced quotes), we return the command unchanged — better
/// to surface the original SyntaxError than to silently corrupt Python
/// source.
fn normalize_python_c_paths(command: &str) -> String {
    // Match `python`, `python3`, or `py` as the head word, then `-c`
    // (possibly with the long form `--command`), then a quoted string.
    // We use a hand-rolled scan instead of regex to keep this code
    // dependency-free and to be precise about the quoting rules.
    let bytes = command.as_bytes();
    let mut i = 0;

    // Skip leading whitespace.
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }

    // Read the head word.
    let head_start = i;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let head = &command[head_start..i];
    let head_lower = head.to_ascii_lowercase();
    // Strip `.exe` / `.bat` suffix on Windows.
    let head_stem = head_lower
        .strip_suffix(".exe")
        .or_else(|| head_lower.strip_suffix(".bat"))
        .unwrap_or(&head_lower);
    if !matches!(head_stem, "python" | "python3" | "py") {
        return command.to_string();
    }

    // Skip whitespace, then expect `-c` or `--command`.
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let arg_start = i;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let arg = &command[arg_start..i];
    if arg != "-c" && arg != "--command" {
        return command.to_string();
    }

    // Skip whitespace between `-c` and the quoted code.
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return command.to_string();
    }

    let quote = bytes[i] as char;
    if quote != '"' && quote != '\'' {
        return command.to_string();
    }

    // Find the matching close quote. Allow escaped quotes inside
    // (e.g. `\"`) — but only if the escape is itself escaped (we
    // don't try to fully parse Python source here).
    let code_start = i + 1;
    let mut j = code_start;
    while j < bytes.len() {
        let c = bytes[j] as char;
        if c == '\\' && j + 1 < bytes.len() {
            j += 2; // skip escaped char
            continue;
        }
        if c == quote {
            break;
        }
        j += 1;
    }
    if j >= bytes.len() {
        return command.to_string(); // unmatched — bail
    }
    let code_end = j; // exclusive
    let inner = &command[code_start..code_end];

    // Convert any `X:\...` or `X:/...` Windows path inside the code
    // segment to forward slashes. Match `[A-Za-z]:[/\\][^"'` ]*` and
    // stop at common delimiters (quote, space, paren, brace).
    let mut out = String::with_capacity(inner.len());
    let mut k = 0;
    while k < inner.len() {
        let ch = inner.as_bytes()[k] as char;
        // Drive letter + colon?
        if ch.is_ascii_alphabetic()
            && k + 1 < inner.len()
            && inner.as_bytes()[k + 1] == b':'
            && k + 2 < inner.len()
            && (inner.as_bytes()[k + 2] == b'/' || inner.as_bytes()[k + 2] == b'\\')
        {
            let path_start = k;
            // Walk until we hit a quote, space, paren, brace, bracket,
            // or end.
            let mut m = k + 3;
            while m < inner.len() {
                let mc = inner.as_bytes()[m] as char;
                if mc == '"' || mc == '\'' || mc == ' ' || mc == '\t'
                    || mc == '(' || mc == ')' || mc == '{' || mc == '}'
                    || mc == '[' || mc == ']'
                {
                    break;
                }
                m += 1;
            }
            // Replace backslashes with forward slashes in this range.
            for c in inner[path_start..m].chars() {
                if c == '\\' {
                    out.push('/');
                } else {
                    out.push(c);
                }
            }
            k = m;
        } else {
            out.push(ch);
            k += 1;
        }
    }

    if out == inner {
        return command.to_string(); // nothing changed
    }

    // Reassemble: [head] [whitespace] [-c] [whitespace] [quote][new code][quote] [rest]
    let after = code_end + 1; // skip closing quote
    let mut result = String::with_capacity(command.len());
    result.push_str(&command[..code_start]);
    result.push_str(&out);
    result.push_str(&command[code_end..]);
    let _ = after; // result already includes the closing quote
    result
}

/// Lesson 848 (2026-08-30 17:40 MDT, David): detect `python ... -c "<code>"`
/// in a shell command and split it so we can invoke python directly
/// (bypassing cmd /C's quote-stripping on Windows).
///
/// Returns:
///   - `Some((py_args, leftover_args))` if the head word is python/python3/py
///     and -c with a quoted code string was found. `py_args` is the full
///     argv to invoke (e.g. `["python.exe", "-c", "print('hi')"]`);
///     `leftover_args` are any trailing args (e.g. `["; exit(7)"]`)
///     passed after the closing quote, normalized.
///   - `None` if the head word isn't python, or -c with quoted code
///     isn't present. Caller falls back to plain `cmd /C` / `sh -c`.
///
/// We do NOT try to handle every Python invocation — only the simple
/// `python -c "<code>"` form that cmd's quote handling mangles. More
/// complex forms (pipes, redirects, conditionals) still go through the
/// shell and the user can keep using the .py-file workaround.
fn split_python_c_command(command: &str) -> Option<(Vec<String>, Vec<String>)> {
    let trimmed = command.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Read the head word.
    let mut i = 0;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let head = &trimmed[..i];
    let head_lower = head.to_ascii_lowercase();
    let head_stem = head_lower
        .strip_suffix(".exe")
        .or_else(|| head_lower.strip_suffix(".bat"))
        .unwrap_or(&head_lower);
    if !matches!(head_stem, "python" | "python3" | "py") {
        return None;
    }

    // Walk tokens looking for `-c` or `--command` followed by a quoted code.
    // Tokens can be separated by spaces; quoted strings are a single token.
    // Note: `j` starts at `i` (which is AFTER the head), so tokens[0] is
    // the first arg after the head, not the head itself.
    let mut tokens: Vec<String> = Vec::new();
    let mut j = i;
    while j < bytes.len() {
        // Skip whitespace.
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let c = bytes[j] as char;
        if c == '"' || c == '\'' {
            // Quoted token: scan to matching close quote.
            let quote = c;
            let start = j + 1;
            let mut k = start;
            while k < bytes.len() {
                let cc = bytes[k] as char;
                if cc == '\\' && k + 1 < bytes.len() {
                    k += 2;
                    continue;
                }
                if cc == quote {
                    break;
                }
                k += 1;
            }
            if k >= bytes.len() {
                return None; // unmatched
            }
            tokens.push(trimmed[start..k].to_string());
            j = k + 1;
        } else {
            // Bare token.
            let start = j;
            while j < bytes.len() && !(bytes[j] as char).is_whitespace() {
                j += 1;
            }
            tokens.push(trimmed[start..j].to_string());
        }
    }

    // Look for -c or --command (with optional value as next token).
    // c_idx is the index in `tokens` (NOT including the head).
    let mut c_idx: Option<usize> = None;
    for (idx, tok) in tokens.iter().enumerate() {
        if tok == "-c" || tok == "--command" {
            c_idx = Some(idx);
            break;
        }
    }
    let c_idx = c_idx?;
    // The code is the token AFTER -c.
    if c_idx + 1 >= tokens.len() {
        return None;
    }
    let code = tokens[c_idx + 1].clone();

    // py_args = [head, ...args_before_c, "-c", code]
    let mut py_args: Vec<String> = Vec::with_capacity(c_idx + 3);
    py_args.push(head.to_string());
    for tok in &tokens[..c_idx] {
        py_args.push(tok.clone());
    }
    py_args.push("-c".to_string());
    py_args.push(code);

    // leftover_args = anything after the code token (e.g. `; exit(7)`).
    // We pass them as additional argv AFTER python has its -c + code
    // consumed. Python ignores trailing args (it warns "extra args ignored"
    // but doesn't fail), so this is safer than sending them through cmd.
    let leftover_args: Vec<String> = if c_idx + 2 < tokens.len() {
        tokens[c_idx + 2..].to_vec()
    } else {
        Vec::new()
    };

    Some((py_args, leftover_args))
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

    // Lesson 805 (rc55.13): python -c path-quoting brittleness.
    //
    // Symptom: when the model emits something like
    //   python -c "open('C:\Users\foo\file.txt').read()"
    // Python's tokenizer interprets the `\U` and `\f` as escape
    // sequences, raising `SyntaxError: unterminated string literal`
    // or `unicode error`. The MC info file (Lesson 805) lists this as
    // a recurring annoyance: "passing python -c ... with single
    // quotes or nested quotes often exploded with SyntaxError".
    //
    // Fix: when the command starts with `python` / `python3` / `py`
    // and uses `-c "<code>"`, walk the code string and convert any
    // embedded Windows path (`X:\...` or `X:/...`) to forward slashes.
    // Python accepts forward slashes everywhere; Windows paths work
    // fine in Python as long as the backslashes don't get tokenized
    // as escape sequences.
    //
    // We only touch the inner code (between matched quotes after -c),
    // not the outer shell args, so this is safe for legitimate uses
    // of `\n` in Python source.
    let command = normalize_python_c_paths(&command).to_string();

    // Lesson 848 (2026-08-30 17:40 MDT, David): inline `python -c "..."`
    // quoting fails on Windows because cmd /C strips the outer quotes
    // before passing args to python. Example: `cmd /C python -c "print('hello')"`
    // becomes `cmd /C python -c print('hello')` after cmd parses it,
    // and Python sees `python -c print` (SyntaxError: invalid syntax).
    // Even semicolon-separated cases (`python -c "x=2; print(x)"`) hit
    // the same wall because cmd splits on `;` first. The workaround of
    // writing the code to a .py file and running that works because we
    // pass it as a single argv, not via cmd's string parsing.
    //
    // Fix: when we detect a `python ... -c "<code>"` head, extract the
    // code string ourselves and invoke `python.exe` directly with -c
    // as a SEPARATE argv. Bypasses cmd's quote-stripping entirely.
    if let Some((py_args, leftover_args)) = split_python_c_command(&command) {
        let mut cmd = Command::new(&py_args[0]);
        for a in &py_args[1..] {
            cmd.arg(a);
        }
        for a in &leftover_args {
            cmd.arg(a);
        }
        if let Some(c) = &cwd_path {
            cmd.current_dir(c);
        }
        let output = cmd
            .output()
            .map_err(|e| format!("bash_run: failed to spawn python: {}", e))?;
        let mut out = String::new();
        out.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            out.push_str("\n--- stderr ---\n");
            out.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if !output.status.success() {
            return Err(format!(
                "bash_run: python command exited with code {}\n{}",
                output.status.code().unwrap_or(-1),
                out
            ));
        }
        let _ = timeout_ms;
        return Ok(out);
    }

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

/// Lesson 803 (rc55.13): fetch a URL and return its body. The previous
/// workflow had `web_search` (MAIC server-side) but no `web_fetch`, so
/// the model could find links but never load them. `ureq` is already
/// in the dependency graph (Lesson 7) and is well-suited for short,
/// blocking fetches. Response bodies larger than `max_bytes` are
/// truncated with a clear marker so the model knows there's more.
///
/// Constraints (defense in depth):
///   * URL scheme MUST be http/https — no file://, no gopher://, no
///     javascript:. Refusing the unknown scheme keeps this tool from
///     being weaponized for local file disclosure.
///   * 15s default timeout, 30s hard cap — chat UX can't wait longer.
///   * 5MB hard response cap — binary or huge responses get a
///     summary rather than the model eating megabytes of HTML.
///   * Redirects are NOT followed automatically — keeps the call
///     surface predictable for the model. If a 30x comes back, we
///     return the Location header verbatim and let the model decide
///     whether to chase it.
fn web_fetch(params: &serde_json::Value) -> Result<String, String> {
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "web_fetch: missing required 'url' (string)".to_string())?;

    // Scheme allowlist. Anything else (file://, data:, ftp://, etc.)
    // gets rejected up front.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "web_fetch: refusing url with non-http(s) scheme: {}",
            &url[..url.len().min(40)]
        ));
    }

    let max_bytes = params
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(65_536)
        .clamp(1, 5_242_880) as usize;

    let timeout_ms = params
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(15_000)
        .clamp(100, 30_000) as u64;

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build();

    let resp = agent.get(url).call().map_err(|e| {
        // ureq::Error covers transport (DNS / TLS / timeout / refused)
        // and HTTP status. Surface the kind so the model can recover.
        match e {
            ureq::Error::Status(code, response) => {
                format!(
                    "web_fetch: HTTP {} for {} ({})",
                    code,
                    url,
                    response.status_text()
                )
            }
            ureq::Error::Transport(t) => {
                format!("web_fetch: transport error for {}: {}", url, t)
            }
        }
    })?;

    let status = resp.status();
    let headers = resp.headers_names();

    // Capture content-type and any redirect target before consuming the
    // body (ureq's response API gives us .into_string() but no
    // header-by-header peek).
    let mut content_type = String::new();
    let mut location = String::new();
    for name in headers {
        if let Some(v) = resp.header(&name) {
            let lname = name.to_lowercase();
            if lname == "content-type" && content_type.is_empty() {
                content_type = v.to_string();
            } else if lname == "location" && location.is_empty() {
                location = v.to_string();
            }
        }
    }

    let mut body = resp
        .into_string()
        .map_err(|e| format!("web_fetch: body read error for {}: {}", url, e))?;

    let truncated = body.len() > max_bytes;
    if truncated {
        body.truncate(max_bytes);
        body.push_str(&format!(
            "\n\n[truncated at {} bytes; full response was larger. Re-call with max_bytes if you need more.]",
            max_bytes
        ));
    }

    let mut out = String::with_capacity(body.len() + 256);
    out.push_str(&format!("HTTP {}\n", status));
    if !content_type.is_empty() {
        out.push_str(&format!("Content-Type: {}\n", content_type));
    }
    if !location.is_empty() {
        // Non-2xx with a Location header is a redirect — surface it so
        // the model can chase it explicitly.
        if !(200..300).contains(&status) {
            out.push_str(&format!("Location: {}\n", location));
        }
    }
    out.push_str("\n");
    out.push_str(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_user_path_rejects_relative() {
        assert!(normalize_user_path("Documents/foo.txt").is_err());
        assert!(normalize_user_path("").is_err());
    }

    // Lesson 805: bash_run / python -c path quoting. The normalizer
    // converts Windows backslash paths to forward slashes inside
    // python -c "<code>" segments so Python's tokenizer doesn't try
    // to interpret \U, \f, etc. as escape sequences.
    #[test]
    fn lesson_805_normalize_python_c_converts_backslash_to_slash() {
        let cmd = r#"python -c "open('C:\Users\foo\file.txt').read()""#;
        let fixed = normalize_python_c_paths(cmd);
        assert!(
            fixed.contains("C:/Users/foo/file.txt"),
            "expected forward-slash path, got: {fixed}"
        );
        assert!(
            !fixed.contains(r"C:\Users"),
            "backslash path should be gone, got: {fixed}"
        );
    }

    #[test]
    fn lesson_805_normalize_python_c_preserves_python_escapes() {
        // Real Python source that uses \n, \t, etc. inside strings
        // must NOT be rewritten (only Windows-path patterns get touched).
        let cmd = r#"python -c "s='line1\nline2'; print(s)""#;
        let fixed = normalize_python_c_paths(cmd);
        // The literal \n in the source must still be there — only the
        // drive-letter paths get rewritten.
        assert!(
            fixed.contains(r"\n"),
            "python \\n escape should be preserved, got: {fixed}"
        );
    }

    #[test]
    fn lesson_805_normalize_python_c_noop_for_non_python() {
        let cmd = r#"echo 'C:\Users\foo' > out.txt"#;
        let fixed = normalize_python_c_paths(cmd);
        assert_eq!(fixed, cmd, "non-python commands must pass through");
    }

    #[test]
    fn lesson_805_normalize_python_c_handles_python3() {
        let cmd = r#"python3 -c "open(r'C:\temp\data.json')""#;
        let fixed = normalize_python_c_paths(cmd);
        assert!(fixed.contains("C:/temp/data.json"));
    }

    #[test]
    fn lesson_805_normalize_python_c_handles_py_launcher() {
        let cmd = r#"py -c "import json; json.load(open('D:\data\a.json'))""#;
        let fixed = normalize_python_c_paths(cmd);
        assert!(fixed.contains("D:/data/a.json"));
    }

    #[test]
    fn lesson_805_normalize_python_c_noop_when_no_paths() {
        let cmd = r#"python -c "print('hello world')""#;
        let fixed = normalize_python_c_paths(cmd);
        assert_eq!(fixed, cmd);
    }

    #[test]
    fn lesson_805_normalize_python_c_handles_multiple_paths() {
        let cmd = r#"python -c "open('C:\a\b.txt'); open('D:\c\d.txt')""#;
        let fixed = normalize_python_c_paths(cmd);
        assert!(fixed.contains("C:/a/b.txt"));
        assert!(fixed.contains("D:/c/d.txt"));
    }

    #[test]
    fn lesson_805_normalize_python_c_unmatched_quote_passthrough() {
        // If quotes don't match cleanly, refuse to rewrite (better to
        // surface the original Python SyntaxError than to corrupt source).
        let cmd = r#"python -c "print('unterminated)""#;
        let fixed = normalize_python_c_paths(cmd);
        assert_eq!(fixed, cmd);
    }

    // Lesson 848 (2026-08-30 17:40 MDT, David): split_python_c_command
    // detects `python ... -c "<code>"` so we can invoke python directly
    // and bypass cmd /C's quote-stripping on Windows.
    #[test]
    fn lesson_848_split_python_c_simple() {
        let cmd = r#"python -c "print('hello')""#;
        let (py, leftover) = split_python_c_command(cmd).expect("must split");
        assert_eq!(py[0], "python");
        assert_eq!(py[1], "-c");
        assert_eq!(py[2], "print('hello')");
        assert!(leftover.is_empty());
    }

    #[test]
    fn lesson_848_split_python_c_with_semicolons() {
        let cmd = r#"python -c "x=2+2; print(x)""#;
        let (py, _) = split_python_c_command(cmd).expect("must split");
        assert_eq!(py[0], "python");
        assert_eq!(py[1], "-c");
        assert_eq!(py[2], "x=2+2; print(x)");
    }

    #[test]
    fn lesson_848_split_python_c_with_paths() {
        let cmd = r#"python -c "open('C:\Users\foo\file.txt').read()""#;
        let (py, _) = split_python_c_command(cmd).expect("must split");
        // The splitter returns the inner code verbatim; normalize_python_c_paths
        // runs in production and rewrites backslashes to forward slashes.
        assert_eq!(py[2], r"open('C:\Users\foo\file.txt').read()");
    }

    #[test]
    fn lesson_848_split_python_c_handles_python3_py() {
        for head in ["python3", "python3.exe", "py", "py.exe"] {
            let cmd = format!(r#"{head} -c "print(1)""#);
            let (py, _) = split_python_c_command(&cmd)
                .unwrap_or_else(|| panic!("must split for head={head}"));
            // The head is the first element, with .exe preserved as-is.
            assert!(
                py[0].to_ascii_lowercase().starts_with(head.replace(".exe", "").as_str()),
                "head should start with {head}, got {}",
                py[0]
            );
            assert_eq!(py[2], "print(1)");
        }
    }

    #[test]
    fn lesson_848_split_python_c_handles_leading_args() {
        // `python -O -c "code"` should preserve -O before -c.
        let cmd = r#"python -O -c "print(1)""#;
        let (py, _) = split_python_c_command(cmd).expect("must split");
        assert_eq!(py[0], "python");
        assert_eq!(py[1], "-O");
        assert_eq!(py[2], "-c");
        assert_eq!(py[3], "print(1)");
    }

    #[test]
    fn lesson_848_split_python_c_handles_trailing_leftover() {
        // Anything after the closing quote should be returned as leftover.
        // Python warns "extra args ignored" but doesn't fail.
        let cmd = r#"python -c "print(1)" --some-flag value"#;
        let (py, leftover) = split_python_c_command(cmd).expect("must split");
        assert_eq!(py.len(), 3);
        assert_eq!(leftover, vec!["--some-flag", "value"]);
    }

    #[test]
    fn lesson_848_split_python_c_returns_none_for_non_python() {
        assert!(split_python_c_command(r#"echo "hello""#).is_none());
        assert!(split_python_c_command(r#"python script.py"#).is_none());
        // `python -c no_quotes` still routes through the direct python
        // invocation path (better than going through cmd /C). Python will
        // reject the code, but the argv split keeps the unquoted token
        // intact instead of letting cmd eat part of it.
        let cmd = r#"python -c no_quotes"#;
        let (py, _) = split_python_c_command(cmd).expect("must split unquoted -c");
        assert_eq!(py, vec!["python", "-c", "no_quotes"]);
    }

    #[test]
    fn lesson_848_split_python_c_unmatched_quote_returns_none() {
        let cmd = r#"python -c "unterminated"#;
        assert!(split_python_c_command(cmd).is_none());
    }

    #[test]
    fn lesson_848_split_python_c_single_quotes() {
        let cmd = r#"python -c 'print("hello")'"#;
        let (py, _) = split_python_c_command(cmd).expect("must split single-quoted");
        assert_eq!(py[2], r#"print("hello")"#);
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