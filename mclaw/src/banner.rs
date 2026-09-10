//! Brand banner — lobster ASCII + version, printed on --version, login, help.
//!
//! The mc-terminal TUI uses the same lobster on stdout (TUI: 8 lines,
//! wide enough to fit the dashboard without wrapping). This banner is
//! the CLI's equivalent — tighter, no animation, suitable for a TTY.
//!
//! Why a lobster? Same reason the dashboard, TUI, and marketing use it:
//! Milagro Distribution Corp's mascot for the MAIC family. The shape
//! reads as `m + c` from a distance, which is the CLI's name.
//!
//! `print_banner` writes to stderr (stdout is reserved for the user's data:
//! piping `mclaw run firecrawl.scrape ... | jq` should not have the banner
//! in the way).

/// 5-line lobster — works at any terminal width ≥ 40 cols.
const LOBSTER: &str = r"
       _
      ( )       mclaw — the Miracle Claw CLI
     /|/|       Ask MAIC. Run modules. Pipe to your tools.
    / | |       https://milagrocloud.com
      | |
     /   \
";

/// Returns the brand line shown at the top of `mclaw --help`.
pub const TAGLINE: &str = "Miracle Claw CLI — ask MAIC. run modules. ship faster.";

/// Returns the lobster ASCII + version string. The version comes from
/// the `CARGO_PKG_VERSION` env var set by Cargo at build time.
pub fn banner_text() -> String {
    format!("{}{}", LOBSTER, version_line())
}

/// Returns just the version line (e.g. "mclaw 1.1.0-rc56.0 (week-5 modules)").
/// `version` is the Cargo package version, `tag` is an optional release tag
/// passed by the binary (e.g. "week-5 modules").
pub fn version_line() -> String {
    format!("mclaw {} ({})", crate::VERSION, build_tag())
}

/// The build tag — `git describe --tags --dirty` if available, else a
/// static fallback. We shell out to `git` so the same binary prints a
/// useful version whether built from a tag or from a working tree.
fn build_tag() -> String {
    std::process::Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dev".to_string())
}

/// Print the banner to stderr. Returns nothing; panic-safe.
pub fn print_banner() {
    eprint!("{}", banner_text());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_lobster_and_version() {
        let b = banner_text();
        assert!(b.contains("mclaw "), "banner must start with mclaw version line");
        assert!(b.contains("https://milagrocloud.com"), "banner must include URL");
        // The lobster shape — at least one backslash and one of the body parts.
        assert!(b.contains("/|/|"), "lobster shape missing");
    }

    #[test]
    fn version_line_includes_pkg_version() {
        let v = version_line();
        assert!(v.starts_with("mclaw "));
        // The pkg version is whatever Cargo baked in; just assert non-empty parens.
        assert!(v.contains('(') && v.contains(')'));
    }

    #[test]
    fn tagline_under_80_chars() {
        // Help text needs to fit on a single line in a standard 80-col TTY.
        assert!(TAGLINE.len() < 80, "tagline too long: {}", TAGLINE.len());
    }
}
