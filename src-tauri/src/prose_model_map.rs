//! prose_model_map.rs — Rewrite `.prose` model names before VM execution
//!
//! Lesson 833 (NEW 2026-09-08, David): OpenProse's example programs
//! use Anthropic/OpenAI model names (`sonnet`, `opus`, `haiku`,
//! `gpt-4*`, `gpt-5`). MC routes all chat through MAIC, which serves
//! a different family of model ids (`milagro-dev`, `milagro-coder`,
//! `milagro-oc-qwen`, etc.). Without rewriting, the OpenProse VM
//! tries to dispatch `sonnet` to MAIC and gets an unknown-model
//! error.
//!
//! The fix is a pre-VM rewrite: take the user's `.prose` file, do a
//! textual `model: <name>` rewrite to MAIC-native ids, write to a
//! temp file, and pass the temp path to `node openclaw.mjs prose run`.
//!
//! AP-833-B (Lesson 830): NEVER hard-code `milagro-dev` in the
//! `.prose` files themselves — that would lock the workflow to one
//! model forever. By rewriting at the Rust layer, we keep the source
//! `.prose` files portable (they work on a real Claude or OpenAI
//! install) AND we can swap the MAIC mapping in one place if the
//! default model changes.
//!
//! Why rewrite (not CLI flag) instead of HTTP intercept: OpenProse
//! runs in its own process and the model id is baked into the
//! program source. There's no place to hook a translation layer
//! without forking. A pre-execution text rewrite is the lowest-friction
//! place to translate.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Default MAIC model for `.prose` specialists and workflows.
/// `milagro-dev` is the 14B model — David's smartest local option
/// (MEMORY.md: "Miracle Claw default model: `milagro-dev`").
const DEFAULT_MAIC_MODEL: &str = "milagro-dev";

/// Build the static map from OpenProse-known model aliases to MAIC ids.
/// Any alias not in the map falls through to DEFAULT_MAIC_MODEL (so a
/// `.prose` file that mentions a model we've never heard of still works).
pub fn build_default_map() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    // Anthropic aliases
    m.insert("sonnet", DEFAULT_MAIC_MODEL);
    m.insert("claude-sonnet", DEFAULT_MAIC_MODEL);
    m.insert("claude-3-5-sonnet", DEFAULT_MAIC_MODEL);
    m.insert("opus", DEFAULT_MAIC_MODEL);
    m.insert("claude-opus", DEFAULT_MAIC_MODEL);
    m.insert("claude-3-opus", DEFAULT_MAIC_MODEL);
    m.insert("haiku", DEFAULT_MAIC_MODEL);
    m.insert("claude-haiku", DEFAULT_MAIC_MODEL);
    m.insert("claude-3-haiku", DEFAULT_MAIC_MODEL);
    // OpenAI aliases
    m.insert("gpt-4", DEFAULT_MAIC_MODEL);
    m.insert("gpt-4-turbo", DEFAULT_MAIC_MODEL);
    m.insert("gpt-4o", DEFAULT_MAIC_MODEL);
    m.insert("gpt-4o-mini", DEFAULT_MAIC_MODEL);
    m.insert("gpt-5", DEFAULT_MAIC_MODEL);
    m.insert("gpt-5-mini", DEFAULT_MAIC_MODEL);
    // Already-native MAIC ids — pass through unchanged
    m.insert("milagro-dev", "milagro-dev");
    m.insert("milagro-coder", "milagro-coder");
    m.insert("milagro-oc-qwen", "milagro-oc-qwen");
    m.insert("milagro-oc-glm", "milagro-oc-glm");
    m.insert("milagro-oc-deepseek", "milagro-oc-deepseek");
    m.insert("milagro-oc-kimi", "milagro-oc-kimi");
    m.insert("milagro-oc-minimax", "milagro-oc-minimax");
    m
}

/// Rewrite `model: <name>` lines in a `.prose` file to use MAIC ids.
/// Returns the rewritten text. If `map` is None, uses `build_default_map()`.
///
/// This is a textual rewrite — it intentionally ignores comments (lines
/// starting with `#`) so authors can leave notes like
/// `# orchestration role - sonnet excels here` (28-gas-town.prose
/// has these) without breaking anything. Only `agent <name>:` blocks
/// that contain a `model:` line get rewritten.
pub fn rewrite_prose_source(source: &str, map: Option<&HashMap<&'static str, &'static str>>) -> String {
    let map = map.cloned().unwrap_or_else(build_default_map);
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        // Skip comment lines entirely — preserve author notes verbatim.
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Look for `model: <name>` with any leading whitespace.
        // We preserve the original indentation exactly.
        if let Some(after) = trimmed.strip_prefix("model:") {
            let alias = after.trim();
            let mapped = map
                .get(alias)
                .copied()
                .unwrap_or(DEFAULT_MAIC_MODEL);
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];
            out.push_str(&format!("{}model: {}", indent, mapped));
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Read a `.prose` file from disk, rewrite model aliases, write to a
/// temp file, return the temp path. Caller is responsible for cleanup
/// (or letting the OS clean tmpdir on reboot).
///
/// Returns an error if the source file can't be read. Temp file creation
/// errors propagate as strings because this is called from `tauri::command`
/// which needs `Result<_, String>`.
pub fn rewrite_to_temp_file(
    source_path: &std::path::Path,
    map: Option<&HashMap<&'static str, &'static str>>,
) -> Result<PathBuf, String> {
    let source_text = fs::read_to_string(source_path).map_err(|e| {
        format!("Couldn't read the workflow file ({}): {e}", source_path.display())
    })?;
    let rewritten = rewrite_prose_source(&source_text, map);
    write_rewritten_to_temp(source_path, &rewritten)
}

/// Read a `.prose` file, rewrite model aliases AND inject a user-provided
/// `input goal` declaration at the top. The original file on disk is
/// NEVER modified — only the temp file the VM sees.
///
/// Lesson 833: this is the bridge between the Workflow Center modal
/// (one text field "What should this workflow look at?") and OpenProse's
/// `input <name>:` top-level binding. We add `input goal: "<text>"` as
/// the very first non-comment line in the rewritten copy so `goal` is
/// bound before any session/parallel block references it.
///
/// If the source file already has `input goal:` (e.g. a future author
/// hardcoded one), we REPLACE it rather than duplicating — so the
/// user's modal text always wins.
pub fn rewrite_to_temp_file_with_input(
    source_path: &std::path::Path,
    user_input: Option<&str>,
    map: Option<&HashMap<&'static str, &'static str>>,
) -> Result<PathBuf, String> {
    let source_text = fs::read_to_string(source_path).map_err(|e| {
        format!("Couldn't read the workflow file ({}): {e}", source_path.display())
    })?;
    let mut rewritten = rewrite_prose_source(&source_text, map);
    rewritten = inject_user_input(&rewritten, user_input);
    write_rewritten_to_temp(source_path, &rewritten)
}

/// Inject `input goal: "<text>"` as the first non-comment line of the
/// rewritten source. If `text` is None or empty, returns the source
/// unchanged. If the source already has an `input goal:` declaration,
/// the user's text REPLACES it (so the modal always wins).
fn inject_user_input(source: &str, user_input: Option<&str>) -> String {
    let text = match user_input {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return source.to_string(),
    };
    // Escape any double quotes in the user's text — OpenProse's input
    // declaration uses a quoted string literal.
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let new_decl = format!("input goal: \"{escaped}\"");

    // Remove any existing input goal: line (replace, don't duplicate).
    let lines: Vec<&str> = source.lines().collect();
    let mut filtered: Vec<&str> = Vec::with_capacity(lines.len() + 2);
    let mut removed = false;
    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("input goal:") {
            if !removed {
                removed = true;
                continue;
            }
        }
        filtered.push(line);
    }

    // Find insertion point: after the last comment/blank line at the top.
    let mut insert_idx = 0;
    for (i, line) in filtered.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            insert_idx = i + 1;
        } else {
            break;
        }
    }

    let mut out = String::with_capacity(source.len() + 64);
    for (i, line) in filtered.iter().enumerate() {
        if i == insert_idx {
            out.push_str(&new_decl);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    // Edge case: file was entirely comments/blanks — append at end.
    if insert_idx >= filtered.len() {
        out.push_str(&new_decl);
        out.push('\n');
    }
    out
}

fn write_rewritten_to_temp(
    source_path: &std::path::Path,
    rewritten: &str,
) -> Result<PathBuf, String> {
    // Temp file name: same basename + `.rewritten.prose` suffix in the
    // OS tmpdir. We use the OS tmpdir so we don't pollute the user's
    // project with auto-generated files.
    let basename = source_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workflow.prose".to_string());
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("mc-prose-{}", basename));
    let mut f = fs::File::create(&tmp).map_err(|e| {
        format!("Couldn't write the temp workflow file: {e}")
    })?;
    f.write_all(rewritten.as_bytes())
        .map_err(|e| format!("Couldn't write the temp workflow file: {e}"))?;
    f.sync_all().ok();
    Ok(tmp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Lesson 833: a `.prose` file with `model: sonnet` must be rewritten
    /// to `model: milagro-dev` so the OpenProse VM dispatches to MAIC.
    #[test]
    fn lesson_833_rewrite_sonnet_to_milagro_dev() {
        let input = "agent reviewer:\n  model: sonnet\n  prompt: \"...\"\n";
        let out = rewrite_prose_source(input, None);
        assert!(
            out.contains("model: milagro-dev"),
            "Lesson 833: sonnet must be rewritten to milagro-dev, got: {out}"
        );
        assert!(
            !out.contains("model: sonnet"),
            "Lesson 833: raw sonnet must not survive rewrite, got: {out}"
        );
    }

    /// Lesson 833: aliases other than sonnet must also be rewritten.
    /// AP-833-B: we MUST keep this in one place so the default model
    /// can change without rewriting every `.prose` file.
    #[test]
    fn lesson_833_rewrite_all_anthropic_and_openai_aliases() {
        let cases = [
            ("model: opus", "milagro-dev"),
            ("model: haiku", "milagro-dev"),
            ("model: gpt-4", "milagro-dev"),
            ("model: gpt-4o", "milagro-dev"),
            ("model: gpt-5", "milagro-dev"),
            ("model: claude-sonnet", "milagro-dev"),
            ("model: claude-3-5-sonnet", "milagro-dev"),
        ];
        for (input_line, expected) in cases {
            let out = rewrite_prose_source(input_line, None);
            assert!(
                out.contains(&format!("model: {expected}")),
                "Lesson 833: '{input_line}' must rewrite to '{expected}', got: {out}"
            );
        }
    }

    /// Lesson 833: native MAIC model ids must pass through unchanged.
    /// This matters because authors can write `model: milagro-coder`
    /// directly in a `.prose` file (e.g. for code workflows) and we
    /// must NOT silently rewrite it to `milagro-dev`.
    #[test]
    fn lesson_833_native_maic_ids_passthrough() {
        let cases = [
            ("model: milagro-dev", "milagro-dev"),
            ("model: milagro-coder", "milagro-coder"),
            ("model: milagro-oc-qwen", "milagro-oc-qwen"),
        ];
        for (input_line, expected) in cases {
            let out = rewrite_prose_source(input_line, None);
            assert!(
                out.contains(&format!("model: {expected}")),
                "Lesson 833: '{input_line}' must pass through unchanged as '{expected}', got: {out}"
            );
        }
    }

    /// Lesson 833: comment lines mentioning `model: sonnet` must NOT be
    /// rewritten. Example: `28-gas-town.prose` has
    /// `model: sonnet        # Orchestration role - sonnet excels here`.
    /// Rewriting comments would change the author's intent and break
    /// the documentation value of those notes.
    #[test]
    fn lesson_833_comment_lines_preserved_verbatim() {
        let input = "# orchestration role - sonnet excels here\nagent reviewer:\n  model: sonnet\n";
        let out = rewrite_prose_source(input, None);
        assert!(
            out.contains("# orchestration role - sonnet excels here"),
            "Lesson 833: comment line must be preserved verbatim, got: {out}"
        );
        // The actual model: line still gets rewritten.
        assert!(
            out.contains("model: milagro-dev"),
            "Lesson 833: agent model: line must still be rewritten, got: {out}"
        );
        // And only ONE occurrence of `model: ` survives (the rewritten one).
        assert_eq!(
            out.matches("model: ").count(),
            1,
            "Lesson 833: only one model: line should remain after rewrite, got: {out}"
        );
    }

    /// Lesson 833: unknown model aliases fall through to the default
    /// rather than being silently passed through. This is safer — if
    /// a `.prose` file mentions `model: future-model-x`, we'd rather
    /// it run on milagro-dev than fail at runtime with "unknown model".
    #[test]
    fn lesson_833_unknown_alias_falls_through_to_default() {
        let out = rewrite_prose_source("model: future-model-x", None);
        assert!(
            out.contains("model: milagro-dev"),
            "Lesson 833: unknown alias must fall through to default, got: {out}"
        );
    }

    /// Lesson 833: `inject_user_input` must add `input goal: "..."`
    /// as the first non-comment line so OpenProse binds `goal` before
    /// any session/parallel block references it.
    #[test]
    fn lesson_833_inject_user_input_first_non_comment_line() {
        let source = "# comment line\n\nagent foo:\n  model: sonnet\nsession: foo\n  prompt: \"goal\"\n";
        let out = inject_user_input(source, Some("explain the login flow"));
        // input goal must come BEFORE the agent block.
        let input_pos = out.find("input goal:").unwrap();
        let agent_pos = out.find("agent foo:").unwrap();
        assert!(
            input_pos < agent_pos,
            "Lesson 833: input goal must come before agent block (input at {input_pos}, agent at {agent_pos}), got: {out}"
        );
        assert!(
            out.contains("input goal: \"explain the login flow\""),
            "Lesson 833: user text must be quoted into the input decl, got: {out}"
        );
    }

    /// Lesson 833: if the user types a double quote, it must be escaped
    /// so the resulting `.prose` file still parses. (OpenProse's input
    /// declaration uses a quoted string literal.)
    #[test]
    fn lesson_833_inject_user_input_escapes_double_quotes() {
        let source = "agent foo:\n  model: sonnet\n";
        let out = inject_user_input(source, Some("the \"login\" endpoint"));
        assert!(
            out.contains("input goal: \"the \\\"login\\\" endpoint\""),
            "Lesson 833: double quotes in user input must be escaped, got: {out}"
        );
    }

    /// Lesson 833: if the source already has an `input goal:` line,
    /// the user's modal text REPLACES it (so the modal always wins).
    /// Duplicate `input goal:` declarations would confuse the parser.
    #[test]
    fn lesson_833_inject_user_input_replaces_existing_decl() {
        let source = "input goal: \"placeholder\"\n\nagent foo:\n  model: sonnet\n";
        let out = inject_user_input(source, Some("user override"));
        let occurrences = out.matches("input goal:").count();
        assert_eq!(
            occurrences, 1,
            "Lesson 833: must replace (not duplicate) existing input goal decl, got {occurrences} in: {out}"
        );
        assert!(
            out.contains("input goal: \"user override\""),
            "Lesson 833: replacement must use the user's text, got: {out}"
        );
        assert!(
            !out.contains("placeholder"),
            "Lesson 833: old text must be gone, got: {out}"
        );
    }

    /// Lesson 833: empty or None user_input must be a no-op (don't
    /// add a stray empty input declaration).
    #[test]
    fn lesson_833_inject_user_input_no_op_when_empty() {
        let source = "agent foo:\n  model: sonnet\n";
        let out_none = inject_user_input(source, None);
        let out_empty = inject_user_input(source, Some(""));
        let out_whitespace = inject_user_input(source, Some("   \n\t  "));
        assert_eq!(out_none, source, "None must be identity");
        assert_eq!(out_empty, source, "empty string must be identity");
        assert_eq!(out_whitespace, source, "whitespace must be identity");
    }
}
