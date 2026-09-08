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
}
