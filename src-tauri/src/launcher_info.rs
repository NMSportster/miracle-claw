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
///
/// Lesson 759 (2026-08-29 18:25 MDT, David): include `miracle-claw-tools.exe`
/// (Windows) / `miracle-claw-tools` (*nix) so the install copies the
/// fresh sidecar next to the plugin JS in the user's AppData. Before
/// this, the sidecar shipped in `<resources>/miracle-claw-tools.exe`
/// (Program Files install path) was the only fresh copy; the AppData
/// sidecar (sitting next to `<APPDATA>/MiracleClaw/extensions/maic/
/// index.js`) was stale from an earlier install and got picked first by
/// the plugin's `toolsBinaryPath()` ancestor walk — meaning apply_patch
/// fixes in newer builds silently never reached end users until they
/// nuked the AppData dir. Adding the sidecar to this list means every
/// upgrade copies the new binary, and `compute_hashes_manifest` notices
/// the SHA change and re-runs the install.
pub const MAIC_PLUGIN_FILENAMES: &[&str] = &[
    "openclaw.plugin.json",
    "index.js",
    "package.json",
    "test_plugin.js",
    "miracle-claw-tools.exe", // Windows-only; missing-file fallback in copy_maic_plugin_if_needed skips it on *nix
    "miracle-claw-tools",     // *nix-only; same fallback
];

#[inline]
pub fn launcher_binary_name() -> &'static str {
    LAUNCHER_BINARY_NAME
}
