//! # MC Module Framework (Lesson 570)
//!
//! Optional, downloadable modules that extend MiracleClaw without bloating
//! the base installer or forcing every MC user to compile heavy native deps.
//!
//! ## Design
//!
//! - Modules live under `<app_data>/modules/<id>/` (Unix:
//!   `~/.local/share/miracle-claw/modules/`; Windows: `%LOCALAPPDATA%`).
//! - Each module ships with an `installer.json` manifest declaring:
//!     - its id, version, display name
//!     - the sidecar binary name (relative to module root)
//!     - the commands it provides (`mc_<module>_<action>`)
//!     - the UI hook selectors it activates (CSS selectors flipped from
//!       dormant to active on install)
//!     - the assets it ships with (models, config files, etc.)
//! - Base MC ships with **stubs** for every module command. The stub checks
//!   if the module is installed; if not, returns a friendly error. If yes,
//!   it spawns the module's sidecar binary via `tauri-plugin-shell` and
//!   pipes the args.
//! - UI hooks are HTMl/CSS in base MC with `data-module-<id>-installed="false"`,
//!   grayed out. When the module installs, MC flips the attribute on every
//!   matching selector. No recompile, no relaunch.
//!
//! ## Why a separate module framework (vs. inlining everything in MC)?
//!
//! - Faster MC rebuilds (no whisper-rs-sys / cpal in MC's compile graph)
//! - Smaller base installer (~55 MB unchanged as we add modules)
//! - Per-user opt-in (users who don't want voice don't pay the 80 MB)
//! - Independent module updates (whisper.cpp CVEs ship as module updates)
//! - Reusable pattern (OCR, TTS, local code search, etc. — 2 days each instead of 5)
//!
//! ## Lessons applied
//!
//! - Lesson 219 (ACL pattern): 4-file capability layout, module commands use
//!   `allow-mc-module-call` umbrella permission
//! - Lesson 169 (`tool_execution: "client"`): module commands follow the same
//!   "client-side execution" model as Tauri tools
//! - Lesson 491 (host bridge overlay): UI hooks use the same overlay-injection
//!   pattern as the "← Dashboard" overlay
//!
//! ## File map (this dir)
//!
//! - `mod.rs` (this file) — module registry + public API
//! - `manifest.rs` — `installer.json` schema + serde types
//! - `registry.rs` — discovers installed modules at startup
//! - `installer.rs` — download/extract/verify/register flow
//! - `dispatcher.rs` — Tauri command `mc_module_call` that routes to sidecar
//! - `ui_hooks.rs` — JS/CSS hook system that dormant-flips on install
//!
//! ## What this is NOT
//!
//! - Not a plugin/sandbox system (modules run as plain binaries, no isolation)
//! - Not auto-updating by default (opt-in via Settings, lessons learned from
//!   auto-relogin NOT forcing silent updates)
//! - Not cryptographically verified at runtime (SHA256SUMS file in module
//!   bundle, validated at install time only)

pub mod manifest;
pub mod registry;
pub mod installer;
pub mod dispatcher;
pub mod ui_hooks;

use std::path::PathBuf;

/// Root directory where modules are installed.
///
/// Unix: `~/.local/share/miracle-claw/modules/`
/// Windows: `%LOCALAPPDATA%\miracle-claw\modules\`
///
/// Resolved by `app.path().app_data_dir()` then appending `modules/`.
pub fn modules_root(app_data_dir: &std::path::Path) -> PathBuf {
    app_data_dir.join("modules")
}

/// Subdirectory of a module's install dir where the sidecar binary lives.
pub fn module_bin_dir(module_install_dir: &std::path::Path) -> PathBuf {
    module_install_dir.join("bin")
}

/// Manifest filename, written by `installer` step at the module root.
pub const MANIFEST_FILENAME: &str = "installer.json";

/// SHA256SUMS filename (same pattern as maic-plugin's SHA256SUMS).
pub const SUMS_FILENAME: &str = "SHA256SUMS";

/// Errors specific to the module framework.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("module '{0}' is not installed")]
    NotInstalled(String),

    #[error("module '{0}' version {1} requires MC base >= {2}")]
    BaseVersionTooOld(String, String, String),

    #[error("module manifest invalid: {0}")]
    InvalidManifest(String),

    #[error("module SHA256 mismatch for '{0}': expected {1}, got {2}")]
    ChecksumMismatch(String, String, String),

    #[error("module download failed: {0}")]
    DownloadFailed(String),

    #[error("module extract failed: {0}")]
    ExtractFailed(String),

    #[error("module sidecar spawn failed: {0}")]
    SpawnFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl serde::Serialize for ModuleError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
