// ============================================================================
// launcher_info — small constants module shared by lib.rs (main process) and
// launcher.rs (sidecar binary). Kept separate to avoid pulling in Tauri
// runtime crates into the launcher binary, which only needs `std`.
// ============================================================================

/// Port the OpenClaw gateway listens on. Matches tauri.conf.json
/// `app.windows[].url = http://localhost:28789/`. If you change this, update
/// both files AND the CSP `connect-src` allowlist in tauri.conf.json.
pub const OPENCLAW_PORT: u16 = 28789;

/// File name Tauri looks up in `bundle.externalBin`. The actual sidecar
/// binary on disk is named `miracle-claw-launcher` for *nix and
/// `miracle-claw-launcher.exe` on Windows — Tauri appends target-triple
/// suffixes at bundle time, but the short name is what we pass to
/// `Shell::sidecar(...)`.
pub const LAUNCHER_BINARY_NAME: &str = "miracle-claw-launcher";

/// Files we copy from `resources/maic-plugin/` into the user's openclaw
/// extensions directory on first run. Add new entries here when the MAIC
/// plugin gains new files.
pub const MAIC_PLUGIN_FILENAMES: &[&str] = &[
    "openclaw.plugin.json",
    "index.js",
    "package.json",
    "test_plugin.js",
];

#[inline]
pub fn launcher_binary_name() -> &'static str {
    LAUNCHER_BINARY_NAME
}
