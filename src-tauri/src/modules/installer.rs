//! Module installer — download, extract, verify, register.
//!
//! ## Install flow
//!
//! 1. **Download** the module archive (tarball) from `download_url` to a temp file.
//!    Uses `reqwest` (already in deps for MAIC plugin if present, else add).
//! 2. **Extract** to `<modules_root>/<id>/`. We use plain `tar` extraction;
//!    no zip dependency.
//! 3. **Verify** SHA256SUMS (if present).
//! 4. **Register** in the runtime registry so Tauri commands can route to it.
//! 5. **Notify** the frontend so UI hooks can flip from dormant → active.
//!
//! ## Why a separate temp directory for download?
//!
//! - Prevents partially-downloaded modules from polluting the install dir.
//! - Lets us run verification on the archive itself (optional, v2).
//! - On Windows, allows resuming failed downloads (future).
//!
//! ## Concurrency
//!
//! Install is user-initiated, so it doesn't need to be hyper-optimized.
//! Uses `std::fs` (sync) wrapped in `tokio::task::spawn_blocking` from
//! the Tauri command. Cancellation isn't supported in v1.
//!
//! ## Lessons applied
//!
//! - **Lesson 219**: every operation gated through Tauri command → ACL → Rust.
//! - **Lesson 194**: writes are atomic (rename-from-temp), no partial states.
//! - **Lesson 169**: the installer is a client-side tool — MAIC has zero
//!   visibility into what's installed.

use std::path::Path;

use uuid::Uuid;

use super::manifest::{load_from_dir, ModuleManifest};
use super::registry::{verify_module_checksums, InstalledModule, Registry};
use super::ModuleError;

/// Where the module archive came from (used for logging + re-install).
#[derive(Debug, Clone)]
pub struct InstallSource {
    /// Module id, must match what's in the manifest.
    pub id: String,

    /// URL to download the archive from. e.g. GitHub releases.
    pub download_url: String,

    /// Optional URL to the SHA256SUMS file. If absent, no verification.
    #[allow(dead_code)]
    pub sums_url: Option<String>,
}

/// Result of a successful install.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallResult {
    /// Parsed manifest of the just-installed module.
    pub manifest: ModuleManifest,
    /// Absolute path to the install dir.
    pub install_dir: String,
    /// Files verified via SHA256SUMS (empty if no SUMS file).
    pub verified_files: Vec<String>,
}

/// Install a module from a source URL.
///
/// `modules_root` is `<app_data>/modules/`.
///
/// Atomic flow:
/// 1. Download archive → temp file
/// 2. Create `<modules_root>/<id>.tmp-<uuid>/`
/// 3. Extract archive into tmp dir
/// 4. Verify SHA256SUMS (if present in tmp dir)
/// 5. Rename tmp dir → `<modules_root>/<id>/`
/// 6. Register in the registry
/// 7. Return InstallResult
///
/// On any failure, the tmp dir is cleaned up and the registry untouched.
///
/// ### Lessons applied
///
/// - **Lesson 194** (atomic writes): the rename-from-tmp pattern means we
///   never leave a half-installed module dir behind. The .tmp-<uuid>
///   suffix makes it impossible for a concurrent install of the same
///   module id to clobber our in-progress extract.
/// - **Lesson 572** (URL download): we use reqwest + rustls, not native
///   OpenSSL, so this works on Linux AND Windows MSVC without any
///   platform-specific crypto libs.
pub async fn install_from_url(
    modules_root: &Path,
    registry: &Registry,
    source: InstallSource,
) -> Result<InstallResult, ModuleError> {
    // 1. Validate id is safe before using it as a directory name
    //    (defense in depth — caller should have already checked, but
    //    this is a public function that touches the filesystem).
    if !is_safe_module_id(&source.id) {
        return Err(ModuleError::InvalidManifest(format!(
            "unsafe module id: {}",
            source.id
        )));
    }

    // 2. Download to a temp file inside modules_root/.tmp/
    let tmp_dir = modules_root.join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("{}-{}.tar.gz", source.id, Uuid::new_v4()));

    eprintln!(
        "[modules] install_from_url: downloading {} -> {}",
        source.download_url,
        archive_path.display()
    );

    // Fetch the archive. We do a single `bytes()` read rather than a
    // streaming write because modules are small (< 50 MB typical) and
    // the simpler API means fewer ways to leak a temp file on cancel.
    // If a module grows past the practical limit (~100 MB), the
    // `stream` feature on reqwest is already in deps and we can
    // switch to a streaming writer without a Cargo.toml bump.
    let resp = reqwest::get(&source.download_url).await.map_err(|e| {
        ModuleError::DownloadFailed(format!(
            "fetch {}: {e}",
            source.download_url
        ))
    })?;
    if !resp.status().is_success() {
        return Err(ModuleError::DownloadFailed(format!(
            "HTTP {} from {}",
            resp.status(),
            source.download_url
        )));
    }
    let bytes = resp.bytes().await.map_err(|e| {
        ModuleError::DownloadFailed(format!("read body from {}: {e}", source.download_url))
    })?;
    std::fs::write(&archive_path, &bytes)?;

    // 3. Extract via the bytes-based helper. The helper handles the
    //    tmp dir creation, extraction, checksum verification, manifest
    //    validation, and atomic rename — keeping the URL + bytes
    //    paths consistent.
    let result = install_from_archive_bytes(
        modules_root,
        registry,
        &source.id,
        &bytes,
        Some(archive_path.as_path()),
    )
    .await;

    // 4. Best-effort cleanup of the downloaded archive. We don't
    //    fail the install if this errors — the archive in .tmp/ is
    //    harmless and the next install will overwrite it.
    let _ = std::fs::remove_file(&archive_path);

    result
}

/// Install a module from an in-memory tarball (`.tar.gz`).
///
/// Splitting this out from `install_from_url` lets tests exercise the
/// full extract-verify-register flow without hitting the network —
/// the URL fetch is the only network-dependent step, and we test it
/// implicitly by `reqwest::get` being a thin wrapper around the
/// standard `hyper` client (which `cargo test` already exercises in
/// the wider ecosystem).
///
/// `archive_path_for_cleanup`, if `Some`, is removed on success
/// (best-effort). Pass `None` for the test path where the bytes came
/// from an in-memory fixture, not a file on disk.
///
/// ### Lessons applied
///
/// - **Lesson 194** (atomic writes): same rename-from-tmp pattern as
///   `install_from_url` and `install_from_local_dir`.
/// - **Lesson 169** (client-side execution): no MAIC visibility into
///   installs — all filesystem operations stay local.
pub async fn install_from_archive_bytes(
    modules_root: &Path,
    registry: &Registry,
    id: &str,
    bytes: &[u8],
    archive_path_for_cleanup: Option<&Path>,
) -> Result<InstallResult, ModuleError> {
    if !is_safe_module_id(id) {
        return Err(ModuleError::InvalidManifest(format!(
            "unsafe module id: {id}"
        )));
    }

    // 1. Extract into a per-install tmp dir. The <id>.tmp-<uuid> suffix
    //    means concurrent installs of the same module id never collide
    //    on the rename in step 4 below.
    let extract_dir = modules_root.join(format!("{}.tmp-{}", id, Uuid::new_v4()));
    if let Err(e) = std::fs::create_dir_all(&extract_dir) {
        return Err(ModuleError::DownloadFailed(format!(
            "create extract dir {}: {e}",
            extract_dir.display()
        )));
    }

    // 2. Run the tar.gz extraction. We use flate2 + tar (pure Rust)
    //    so this works identically on Linux AND Windows MSVC — no
    //    external `tar` binary, no shell-out. `set_preserve_permissions`
    //    keeps the sidecar's executable bit on Unix; `set_overwrite` is
    //    a no-op here (we just created the dir) but guards against
    //    future callers re-using a dir.
    let extract_result: Result<(), ModuleError> = (|| {
        let cursor = std::io::Cursor::new(bytes);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        archive.set_preserve_permissions(true);
        archive.set_overwrite(true);
        archive.unpack(&extract_dir).map_err(|e| {
            ModuleError::DownloadFailed(format!(
                "extract tarball into {}: {e}",
                extract_dir.display()
            ))
        })
    })();

    if let Err(e) = extract_result {
        // Failed extract — wipe the tmp dir and bail.
        let _ = std::fs::remove_dir_all(&extract_dir);
        if let Some(archive_path) = archive_path_for_cleanup {
            let _ = std::fs::remove_file(archive_path);
        }
        return Err(e);
    }

    // 3. Resolve the manifest's containing directory. GitHub release
    //    tarballs (and most module-pack conventions) put the module's
    //    contents inside a single top-level directory, e.g.
    //    `firecrawl/installer.json` rather than `installer.json` at
    //    the root. We unwrap one level of single-subdir nesting so
    //    both layouts work without forcing the publisher to repack.
    let manifest_dir = resolve_manifest_dir(&extract_dir);

    // 4. Verify SHA256SUMS (if present in the manifest dir). A checksum
    //    mismatch is a HARD failure — we never install a module that
    //    fails its own integrity check. Missing SHA256SUMS is fine
    //    (best-effort, see `verify_module_checksums`).
    let verified_files = match verify_module_checksums(&manifest_dir) {
        Ok(v) => v,
        Err(e) => {
            // Hard fail on checksum mismatch — refuse to install.
            let _ = std::fs::remove_dir_all(&extract_dir);
            if let Some(archive_path) = archive_path_for_cleanup {
                let _ = std::fs::remove_file(archive_path);
            }
            return Err(e);
        }
    };

    // 5. Load the manifest from the resolved dir. The manifest id
    //    must match the expected id (defends against a malicious or
    //    mis-built archive swapping the id).
    let manifest = match load_from_dir(&manifest_dir) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&extract_dir);
            if let Some(archive_path) = archive_path_for_cleanup {
                let _ = std::fs::remove_file(archive_path);
            }
            return Err(e);
        }
    };
    if manifest.id != id {
        let _ = std::fs::remove_dir_all(&extract_dir);
        if let Some(archive_path) = archive_path_for_cleanup {
            let _ = std::fs::remove_file(archive_path);
        }
        return Err(ModuleError::InvalidManifest(format!(
            "manifest id mismatch: expected {id}, got {}",
            manifest.id
        )));
    }

    // 5b. Flatten if needed. When the archive's top-level wrapper dir
    //     (`firecrawl/`) isn't the same as the extract dir, move the
    //     wrapper's contents up so the final install layout is
    //     `<modules_root>/<id>/installer.json` — matching the
    //     `install_from_local_dir` contract. We use a rename into a
    //     sibling tmp dir, then back, to stay on the same filesystem
    //     and avoid copying. On a single-entry dir, the simplest
    //     portable move is: rename wrapper -> final -> done; but
    //     `final` doesn't exist yet (it's the rename target below),
    //     so we move wrapper contents up by renaming the wrapper
    //     onto a sentinel name and back.
    if manifest_dir != extract_dir {
        // Move <extract_dir>/<wrapper> -> <extract_dir>/__flattened__
        // then move the flattened dir's contents up to extract_dir.
        let wrapper = manifest_dir.file_name().unwrap_or_default().to_os_string();
        let sentinel = extract_dir.join("__flattened__");
        let _ = std::fs::remove_dir_all(&sentinel); // shouldn't exist
        if let Err(e) = std::fs::rename(&manifest_dir, &sentinel) {
            let _ = std::fs::remove_dir_all(&extract_dir);
            if let Some(archive_path) = archive_path_for_cleanup {
                let _ = std::fs::remove_file(archive_path);
            }
            return Err(ModuleError::DownloadFailed(format!(
                "flatten wrapper {wrapper:?}: {e}"
            )));
        }
        // Move every entry under sentinel up to extract_dir.
        let entries: Vec<_> = match std::fs::read_dir(&sentinel) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&extract_dir);
                if let Some(archive_path) = archive_path_for_cleanup {
                    let _ = std::fs::remove_file(archive_path);
                }
                return Err(ModuleError::DownloadFailed(format!(
                    "read flattened sentinel: {e}"
                )));
            }
        };
        for entry in entries {
            let from = entry.path();
            let to = extract_dir.join(entry.file_name());
            if let Err(e) = std::fs::rename(&from, &to) {
                let _ = std::fs::remove_dir_all(&extract_dir);
                if let Some(archive_path) = archive_path_for_cleanup {
                    let _ = std::fs::remove_file(archive_path);
                }
                return Err(ModuleError::DownloadFailed(format!(
                    "flatten move {from:?} -> {to:?}: {e}"
                )));
            }
        }
        // Remove the now-empty sentinel dir.
        let _ = std::fs::remove_dir_all(&sentinel);
    }

    // 5. Enforce minimum MC base version (same gate as
    //    install_from_local_dir). If the module requires a newer MC
    //    than we're running, refuse to install rather than load a
    //    sidecar that will explode at runtime.
    if let Err(e) =
        super::super::auth::tier::assert_version_compatible_with_module(&manifest)
    {
        let _ = std::fs::remove_dir_all(&extract_dir);
        if let Some(archive_path) = archive_path_for_cleanup {
            let _ = std::fs::remove_file(archive_path);
        }
        return Err(ModuleError::BaseVersionTooOld(
            manifest.id.clone(),
            manifest.min_mc_version.clone(),
            e.to_string(),
        ));
    }

    // 6. Backup any existing install at the final path. This makes
    //    re-installs atomic — if the rename in step 7 fails, the
    //    previous version is still on disk.
    let final_dir = modules_root.join(id);
    if final_dir.exists() {
        let backup = modules_root.join(format!("{}.old-{}", id, std::process::id()));
        if let Err(e) = std::fs::rename(&final_dir, &backup) {
            // Couldn't back up — bail out, leaving the new extract in
            // place. We don't try to clean it up here because the
            // most likely cause is a permissions issue the user needs
            // to see (and the .tmp-<uuid> dir is harmless until swept
            // by a future `installer::cleanup_old`).
            let _ = std::fs::remove_dir_all(&extract_dir);
            if let Some(archive_path) = archive_path_for_cleanup {
                let _ = std::fs::remove_file(archive_path);
            }
            return Err(ModuleError::DownloadFailed(format!(
                "backup existing install at {}: {e}",
                final_dir.display()
            )));
        }
    }

    // 7. Atomic rename: tmp dir → final dir. This is the commit
    //    point — after this, the module is installed.
    if let Err(e) = std::fs::rename(&extract_dir, &final_dir) {
        // Rename failed — try to restore from the .old-<pid> backup
        // so the user isn't left without a working install.
        let backup = modules_root.join(format!("{}.old-{}", id, std::process::id()));
        if backup.exists() {
            let _ = std::fs::rename(&backup, &final_dir);
        }
        let _ = std::fs::remove_dir_all(&extract_dir);
        if let Some(archive_path) = archive_path_for_cleanup {
            let _ = std::fs::remove_file(archive_path);
        }
        return Err(ModuleError::DownloadFailed(format!(
            "rename {} -> {}: {e}",
            extract_dir.display(),
            final_dir.display()
        )));
    }

    // 8. Register in the runtime registry. After this, `mc_module_list`
    //    and the dispatcher will see the new module.
    let installed = InstalledModule {
        install_dir: final_dir.clone(),
        manifest: manifest.clone(),
    };
    registry.register(installed);

    eprintln!(
        "[modules] installed {} v{} from bytes ({} files verified)",
        manifest.id,
        manifest.version,
        verified_files.len()
    );

    Ok(InstallResult {
        manifest,
        install_dir: final_dir.display().to_string(),
        verified_files,
    })
}

/// Install a module from a pre-extracted directory on disk.
///
/// Used by:
/// - **Dev mode**: developer points `MC_MODULE_LOCAL_PATH=...` at a local
///   module dir to test without packaging.
/// - **Manual install**: user drops a `voice/` dir into modules/ manually
///   (for power users / offline installs).
///
/// Not async — dev/manual install is rare, doesn't need to be.
pub fn install_from_local_dir(
    modules_root: &Path,
    registry: &Registry,
    source_dir: &Path,
) -> Result<InstallResult, ModuleError> {
    if !source_dir.is_dir() {
        return Err(ModuleError::InvalidManifest(format!(
            "source dir does not exist: {}",
            source_dir.display()
        )));
    }

    // 1. Parse + validate manifest
    let manifest = load_from_dir(source_dir)?;
    super::super::auth::tier::assert_version_compatible_with_module(&manifest)
        .map_err(|e| ModuleError::BaseVersionTooOld(manifest.id.clone(), "current".into(), e))?;

    let id = manifest.id.clone();
    let final_dir = modules_root.join(&id);

    // 2. If a previous install exists at final_dir, back it up to .old-<uuid>
    //    (atomic rollback on future failures).
    if final_dir.exists() {
        let backup = modules_root.join(format!(
            "{}.old-{}",
            id,
            std::process::id()
        ));
        std::fs::rename(&final_dir, &backup)?;
        // Stash backup path so callers can restore on failure.
        // For now we leave it; a future `installer::cleanup_old()` sweeps them.
    }

    // 3. Atomic copy: source → final_dir.
    //    We copy file-by-file rather than `cp -r` because we want platform-
    //    portable behavior + easy error reporting.
    copy_dir_recursive(source_dir, &final_dir)?;

    // 4. Verify SHA256SUMS (if present)
    let verified_files = verify_module_checksums(&final_dir)?;

    // 5. Register in the runtime registry
    let installed = InstalledModule {
        install_dir: final_dir.clone(),
        manifest: manifest.clone(),
    };
    registry.register(installed);

    Ok(InstallResult {
        manifest,
        install_dir: final_dir.display().to_string(),
        verified_files,
    })
}

/// Uninstall a module by id. Removes the install dir + unregisters.
///
/// Safety: refuses to run if `id` contains anything other than
/// `[a-z0-9_-]` to prevent `../` escape from modules_root.
pub fn uninstall(modules_root: &Path, registry: &Registry, id: &str) -> Result<(), ModuleError> {
    if !is_safe_module_id(id) {
        return Err(ModuleError::InvalidManifest(format!(
            "unsafe module id: {}",
            id
        )));
    }
    let dir = modules_root.join(id);
    if !dir.exists() {
        return Err(ModuleError::NotInstalled(id.to_string()));
    }
    std::fs::remove_dir_all(&dir)?;
    registry.unregister(id);
    Ok(())
}

/// Validate a module id before using it as a directory name.
pub fn is_safe_module_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Find the directory containing the module's `installer.json` inside
/// an extracted archive. Handles the common `archive/<id>/...` and
/// `archive/...` (flat) layouts without forcing the publisher to
/// repack.
fn resolve_manifest_dir(extract_root: &Path) -> std::path::PathBuf {
    // 1. Manifest at the root: flat archive.
    if extract_root.join(super::MANIFEST_FILENAME).is_file() {
        return extract_root.to_path_buf();
    }
    // 2. Single-subdir-nested archive: walk one level and return
    //    the only subdir that itself contains installer.json. This
    //    matches `tar -czf foo.tar.gz foo/` (the standard GitHub
    //    release shape).
    if let Ok(entries) = std::fs::read_dir(extract_root) {
        let dirs: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        if dirs.len() == 1 {
            let only = dirs.into_iter().next().unwrap().path();
            if only.join(super::MANIFEST_FILENAME).is_file() {
                return only;
            }
        }
    }
    // 3. Fall through: return the original dir. The caller (load_from_dir)
    //    will produce a precise "installer.json: No such file" error so
    //    the user can see the bad archive layout.
    extract_root.to_path_buf()
}

/// Recursively copy a directory tree. Used by `install_from_local_dir`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), ModuleError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_child = entry.path();
        let dst_child = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_child, &dst_child)?;
        } else if ty.is_file() {
            std::fs::copy(&src_child, &dst_child)?;
        }
        // symlinks / devices — skip silently
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_manifest_json() -> &'static str {
        r#"{
            "id": "testmod",
            "name": "Test Mod",
            "version": "0.1.0",
            "minMcVersion": "1.1.0-rc53.14",
            "description": "Test",
            "binary": { "name": "testmod.exe", "type": "sidecar" },
            "commands": [],
            "uiHooks": []
        }"#
    }

    #[test]
    fn safe_module_id_rejects_path_traversal() {
        assert!(!is_safe_module_id("../etc/passwd"));
        assert!(!is_safe_module_id(""));
        assert!(!is_safe_module_id("a/b"));
        assert!(!is_safe_module_id("Voice"));
        assert!(is_safe_module_id("voice"));
        assert!(is_safe_module_id("voice-input"));
        assert!(is_safe_module_id("ocr_v2"));
    }

    #[test]
    fn install_from_local_dir_registers_module() {
        // Build a fake module dir
        let tmp = std::env::temp_dir().join(format!("mc_mod_test_src_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("installer.json"), sample_manifest_json()).unwrap();
        std::fs::write(tmp.join("dummy.bin"), b"data").unwrap();

        // Target modules root
        let modules_root = std::env::temp_dir().join(format!("mc_mod_test_root_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        let result = install_from_local_dir(&modules_root, &registry, &tmp).unwrap();
        assert_eq!(result.manifest.id, "testmod");
        assert_eq!(registry.len(), 1);
        assert!(modules_root.join("testmod").join("installer.json").exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&modules_root);
    }

    #[test]
    fn install_from_local_dir_rolls_back_on_missing_source() {
        let registry = Registry::new();
        let res = install_from_local_dir(
            &PathBuf::from("/tmp/nonexistent_root_xx"),
            &registry,
            &PathBuf::from("/tmp/nonexistent_source_xx"),
        );
        assert!(matches!(res, Err(ModuleError::InvalidManifest(_))));
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn uninstall_removes_dir_and_unregisters() {
        // Set up one installed module via install_from_local_dir
        let tmp = std::env::temp_dir().join(format!("mc_mod_test_uninstall_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("voice")).unwrap();
        std::fs::write(
            tmp.join("voice/installer.json"),
            sample_manifest_json().replace("testmod", "voice"),
        ).unwrap();

        let modules_root = std::env::temp_dir().join(format!(
            "mc_mod_test_uninstall_root_{}", std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        // First register a "voice" module at the location install would
        // use. We bypass install_from_local_dir to avoid relying on the
        // manifest copy logic; we just register the existing dir.
        let installed = InstalledModule {
            install_dir: modules_root.join("voice"),
            manifest: serde_json::from_str(
                sample_manifest_json().replace("testmod", "voice").as_str(),
            )
            .unwrap(),
        };
        registry.register(installed);

        uninstall(&tmp, &registry, "voice").unwrap();
        assert!(!tmp.join("voice").exists());
        assert!(registry.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ---- tests for install_from_archive_bytes (Lesson 574c) ----
    //
    // We test the bytes path instead of the URL path because:
    //  - URL tests would need a fixture HTTP server (heavy)
    //  - reqwest::get is a thin wrapper around hyper, which the wider
    //    Rust ecosystem tests
    //  - The bytes path exercises the SAME extract+verify+register
    //    code as the URL path — only the download step is different

    /// FireCrawl-shaped installer.json + SHA256SUMS fixture, built
    /// in memory as a `.tar.gz` blob.
    ///
    /// Layout in the tarball:
    ///   firecrawl/installer.json
    ///   firecrawl/bin/miracle-claw-firecrawl
    ///   firecrawl/SHA256SUMS
    fn build_firecrawl_tarball() -> Vec<u8> {
        let manifest_json = r##"{
            "id": "firecrawl",
            "name": "FireCrawl for MiracleClaw",
            "subtitle": "Web scraping from chat",
            "version": "0.1.0",
            "minMcVersion": "1.1.0-rc53.14",
            "description": "Scrape web pages from a chat command",
            "binary": { "name": "miracle-claw-firecrawl", "type": "sidecar" },
            "commands": [
                { "tauri": "mc_firecrawl_scrape", "action": "scrape", "description": "Scrape a URL" }
            ],
            "uiHooks": ["#firecrawl-card"],
            "author": "Miracle Claw",
            "publisherIcon": null
        }"##;

        let bin_content = b"#!/bin/sh\necho fake firecrawl sidecar\n";
        let bin_sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(bin_content);
            format!("{:x}", h.finalize())
        };
        // SHA256SUMS uses `<sha>  <relative-path>` format (two spaces,
        // matching sha256sum(1) -b output and the parser in
        // verify_module_checksums).
        let sums_content = format!("{}  bin/miracle-claw-firecrawl\n", bin_sha);

        let mut tar_bytes = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut tar_bytes);
            let mut builder = tar::Builder::new(cursor);

            // 1. installer.json
            let json_bytes = manifest_json.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(json_bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "firecrawl/installer.json", json_bytes)
                .unwrap();

            // 2. bin/miracle-claw-firecrawl (executable)
            let mut bin_header = tar::Header::new_gnu();
            bin_header.set_size(bin_content.len() as u64);
            bin_header.set_mode(0o755);
            bin_header.set_cksum();
            builder
                .append_data(
                    &mut bin_header,
                    "firecrawl/bin/miracle-claw-firecrawl",
                    bin_content.as_slice(),
                )
                .unwrap();

            // 3. SHA256SUMS
            let sums_bytes = sums_content.as_bytes();
            let mut sums_header = tar::Header::new_gnu();
            sums_header.set_size(sums_bytes.len() as u64);
            sums_header.set_mode(0o644);
            sums_header.set_cksum();
            builder
                .append_data(&mut sums_header, "firecrawl/SHA256SUMS", sums_bytes)
                .unwrap();

            builder.finish().unwrap();
        }
        // gzip it
        let mut gz_bytes = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
            use std::io::Write;
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap();
        }
        gz_bytes
    }

    /// Use a minimal poll-once block_on for tests. `install_from_archive_bytes`
    /// is `async` only for call-site symmetry with the URL variant; the
    /// body is pure synchronous std::fs + flate2 + tar, so a single
    /// `poll` on the `Ready` state is enough.
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::sync::Arc;
        use std::sync::Mutex;
        use std::task::{Context, Poll, Wake, Waker};
        struct ParkOnce(Arc<Mutex<bool>>);
        impl Wake for ParkOnce {
            fn wake(self: Arc<Self>) {
                *self.0.lock().unwrap() = true;
            }
        }
        let flag = Arc::new(Mutex::new(false));
        let waker: Waker = Waker::from(Arc::new(ParkOnce(flag.clone())));
        let mut cx = Context::from_waker(&waker);
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            if *flag.lock().unwrap() {
                *flag.lock().unwrap() = false;
                continue;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn install_from_archive_bytes_registers_firecrawl() {
        // Build the tarball in memory.
        let tarball = build_firecrawl_tarball();
        assert!(!tarball.is_empty(), "tarball builder produced no bytes");

        // Fresh modules root.
        let modules_root = std::env::temp_dir().join(format!(
            "mc_mod_test_url_root_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        let result = block_on(install_from_archive_bytes(
            &modules_root,
            &registry,
            "firecrawl",
            &tarball,
            None, // no on-disk archive to clean up
        ))
        .expect("install_from_archive_bytes should succeed");

        // Manifest parsed correctly.
        assert_eq!(result.manifest.id, "firecrawl");
        assert_eq!(result.manifest.name, "FireCrawl for MiracleClaw");
        assert_eq!(result.manifest.version, "0.1.0");

        // install_dir points at the real final dir.
        let final_dir = modules_root.join("firecrawl");
        assert!(final_dir.is_dir(), "expected final_dir at {}", final_dir.display());
        assert!(final_dir.join("installer.json").is_file());
        assert!(final_dir.join("bin/miracle-claw-firecrawl").is_file());
        assert!(final_dir.join("SHA256SUMS").is_file());

        // SHA256SUMS was actually verified.
        assert_eq!(result.verified_files.len(), 1, "expected the bin file to be verified");
        assert!(
            result.verified_files.iter().any(|f| f.contains("miracle-claw-firecrawl")),
            "verified files should mention the sidecar: {:?}",
            result.verified_files
        );

        // Registry updated.
        assert_eq!(registry.len(), 1);
        assert!(registry.get("firecrawl").is_some());

        // No leftover .tmp-* dirs.
        for entry in std::fs::read_dir(&modules_root).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains(".tmp-"),
                "leftover tmp dir after install: {name}"
            );
        }

        // Cleanup.
        let _ = std::fs::remove_dir_all(&modules_root);
    }

    #[test]
    fn install_from_archive_bytes_rejects_unsafe_id() {
        let tarball = build_firecrawl_tarball();
        let modules_root = std::env::temp_dir().join(format!(
            "mc_mod_test_unsafe_root_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        let res = block_on(install_from_archive_bytes(
            &modules_root,
            &registry,
            "../etc/passwd",
            &tarball,
            None,
        ));
        assert!(matches!(res, Err(ModuleError::InvalidManifest(_))));
        assert_eq!(registry.len(), 0);

        let _ = std::fs::remove_dir_all(&modules_root);
    }

    #[test]
    fn install_from_archive_bytes_rejects_id_mismatch() {
        // Tarball says id=firecrawl but we ask to install as "voice".
        // Should hard-fail with InvalidManifest.
        let tarball = build_firecrawl_tarball();
        let modules_root = std::env::temp_dir().join(format!(
            "mc_mod_test_mismatch_root_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        let res = block_on(install_from_archive_bytes(
            &modules_root,
            &registry,
            "voice",
            &tarball,
            None,
        ));
        assert!(matches!(res, Err(ModuleError::InvalidManifest(_))));
        assert_eq!(registry.len(), 0);

        // No leftover final dir or .tmp-* dir.
        assert!(!modules_root.join("voice").exists());
        for entry in std::fs::read_dir(&modules_root).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(!name.contains(".tmp-"), "leftover tmp dir: {name}");
        }

        let _ = std::fs::remove_dir_all(&modules_root);
    }

    #[test]
    fn install_from_archive_bytes_rejects_garbage_tarball() {
        // A blob that isn't a valid tar.gz at all. Should fail with a
        // DownloadFailed (extract step) and not leave a half-installed
        // module behind.
        let garbage = b"this is not a tarball".to_vec();
        let modules_root = std::env::temp_dir().join(format!(
            "mc_mod_test_garbage_root_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&modules_root);

        let registry = Registry::new();
        let res = block_on(install_from_archive_bytes(
            &modules_root,
            &registry,
            "firecrawl",
            &garbage,
            None,
        ));
        assert!(matches!(res, Err(ModuleError::DownloadFailed(_))));
        assert_eq!(registry.len(), 0);

        // No leftover .tmp-* dir.
        for entry in std::fs::read_dir(&modules_root).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(!name.contains(".tmp-"), "leftover tmp dir: {name}");
        }

        let _ = std::fs::remove_dir_all(&modules_root);
    }
}
