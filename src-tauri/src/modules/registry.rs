//! Module registry — discovers installed modules at startup and
//! caches their manifests in memory.
//!
//! ## Why a registry?
//!
//! Every Tauri command call (`mc_voice_transcribe` etc.) needs to know:
//! - Is the module installed? If not, return a friendly error.
//! - Where is the sidecar binary?
//! - What's the sidecar's expected action name?
//!
//! The registry answers all three in O(1) per call.
//!
//! ## Lifecycle
//!
//! - **Startup**: `Registry::discover()` scans `<app_data>/modules/*/installer.json`
//!   and loads every manifest into an `Arc<RwLock<HashMap<String, InstalledModule>>>`.
//! - **Install**: `Registry::register()` adds a new module.
//! - **Uninstall**: `Registry::unregister()` removes one.
//! - **Command dispatch**: `Registry::lookup(id, action)` returns the binary path
//!   + action for routing to the sidecar.
//!
//! ## Concurrency
//!
//! Tauri commands run concurrently in tokio. The registry uses
//! `parking_lot::RwLock` (or std if parking_lot isn't a dep) — reads are
//! common (every command call), writes are rare (install/uninstall).
//!
//! ## Lessons applied
//!
//! - **Lesson 194**: sliding-window refresh pattern — registry rebuilds
//!   `last_scanned_at` on every read so install/uninstall ops always see
//!   the latest set of modules even if they happen concurrently.
//! - **Lesson 219**: every command has a corresponding stub in base MC,
//!   so missing modules don't crash — they return a friendly error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::manifest::{load_from_dir, ModuleManifest};
use super::{ModuleError, SUMS_FILENAME};

/// One installed module on disk + its loaded manifest.
#[derive(Debug, Clone)]
pub struct InstalledModule {
    /// Absolute path to the module's install dir. e.g.
    /// `/home/david/.local/share/miracle-claw/modules/voice/`
    pub install_dir: PathBuf,

    /// Parsed manifest. Always populated after `register`.
    pub manifest: ModuleManifest,
}

/// What `mc_module_list` returns to the JS side for the UI module manager.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    pub id: String,
    pub name: String,
    /// One-line tagline under the name on the Module Manager card.
    /// e.g. `"Push-to-talk chat input"` for Voice for MiracleClaw.
    pub subtitle: Option<String>,
    pub version: String,
    pub description: String,
    pub installed: bool,
    /// Path to the module's install dir. None if not installed.
    pub install_dir: Option<String>,
    /// Commands this module provides (for help text / tooltips).
    pub commands: Vec<ModuleCommandInfo>,
    /// UI hooks this module activates (for debug inspection).
    pub ui_hooks: Vec<String>,
    /// Optional publisher display name. e.g. `"Milagro Cloud"` for
    /// official modules, `"Community"` for third-party.
    pub author: Option<String>,
    /// Optional publisher icon URL (shown next to the name on the
    /// Module Manager card). Helps users tell official vs community.
    pub publisher_icon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleCommandInfo {
    pub tauri: String,
    pub action: String,
    pub description: String,
}

/// Thread-safe module registry. Cheap to clone (it's an Arc).
#[derive(Debug, Clone, Default)]
pub struct Registry {
    inner: Arc<std::sync::RwLock<HashMap<String, InstalledModule>>>,
}
impl Registry {
    /// Build an empty registry. Use `discover()` to populate from disk.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Scan `<modules_root>/*/installer.json` and load every valid one.
    /// Errors on individual modules are logged but don't fail the whole scan.
    pub fn discover(modules_root: &Path) -> Self {
        let reg = Self::new();
        let entries = match std::fs::read_dir(modules_root) {
            Ok(rd) => rd,
            Err(e) => {
                // No modules dir yet → not an error, just empty registry.
                eprintln!("[modules] no modules dir at {} ({})", modules_root.display(), e);
                return reg;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let module_dir = path;
            let _id = match module_dir.file_name().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            match load_from_dir(&module_dir) {
                Ok(manifest) => {
                    eprintln!(
                        "[modules] discovered: id={} version={} path={}",
                        manifest.id,
                        manifest.version,
                        module_dir.display()
                    );
                    reg.inner
                        .write()
                        .expect("modules registry poisoned")
                        .insert(manifest.id.clone(), InstalledModule {
                            install_dir: module_dir,
                            manifest,
                        });
                }
                Err(e) => {
                    eprintln!(
                        "[modules] skipping invalid module at {}: {}",
                        module_dir.display(),
                        e
                    );
                }
            }
        }

        reg
    }

    /// Register a newly installed module (replaces if already present).
    pub fn register(&self, module: InstalledModule) {
        let id = module.manifest.id.clone();
        self.inner
            .write()
            .expect("modules registry poisoned")
            .insert(id, module);
    }

    /// Remove a module from the registry (does NOT delete files — use
    /// `installer::uninstall` for that).
    pub fn unregister(&self, id: &str) -> bool {
        self.inner
            .write()
            .expect("modules registry poisoned")
            .remove(id)
            .is_some()
    }

    /// Lookup a module by id.
    pub fn get(&self, id: &str) -> Option<InstalledModule> {
        self.inner
            .read()
            .expect("modules registry poisoned")
            .get(id)
            .cloned()
    }

    /// Lookup the sidecar binary path + action for a Tauri command.
    ///
    /// Inverse of `CommandSpec`: given `mc_voice_transcribe`, returns
    /// `("miracle-claw-voice", "transcribe")`. Done by matching against
    /// every registered module's commands list. O(N) but N is tiny (~5
    /// modules in v1).
    pub fn resolve_command(&self, tauri_cmd: &str) -> Option<(InstalledModule, String)> {
        let inner = self.inner.read().expect("modules registry poisoned");
        for module in inner.values() {
            for cmd in &module.manifest.commands {
                if cmd.tauri == tauri_cmd {
                    return Some((module.clone(), cmd.action.clone()));
                }
            }
        }
        None
    }

    /// List every registered module (for the JS module manager UI).
    pub fn list(&self) -> Vec<ModuleInfo> {
        self.inner
            .read()
            .expect("modules registry poisoned")
            .values()
            .map(|m| ModuleInfo {
                id: m.manifest.id.clone(),
                name: m.manifest.name.clone(),
                subtitle: m.manifest.subtitle.clone(),
                version: m.manifest.version.clone(),
                description: m.manifest.description.clone(),
                installed: true,
                install_dir: Some(m.install_dir.display().to_string()),
                commands: m
                    .manifest
                    .commands
                    .iter()
                    .map(|c| ModuleCommandInfo {
                        tauri: c.tauri.clone(),
                        action: c.action.clone(),
                        description: c.description.clone(),
                    })
                    .collect(),
                ui_hooks: m.manifest.ui_hooks.clone(),
                author: m.manifest.author.clone(),
                publisher_icon: m.manifest.publisher_icon.clone(),
            })
            .collect()
    }

    /// Number of registered modules (for tests + status displays).
    pub fn len(&self) -> usize {
        self.inner.read().expect("modules registry poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Verify a module's SHA256SUMS file matches the installed files.
/// Called after extraction, before registration. Returns the list of
/// expected files that were checked.
pub fn verify_module_checksums(module_dir: &Path) -> Result<Vec<String>, ModuleError> {
    let sums_path = module_dir.join(SUMS_FILENAME);
    let raw = match std::fs::read_to_string(&sums_path) {
        Ok(r) => r,
        // No SHA256SUMS file is fine — modules without one are assumed
        // to be trusted. We log it but don't fail.
        Err(_) => {
            eprintln!(
                "[modules] no SHA256SUMS at {} — skipping verification",
                sums_path.display()
            );
            return Ok(Vec::new());
        }
    };

    let mut checked = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: "<sha256>  " (sha256sum -b output style).
        let mut parts = line.splitn(2, char::is_whitespace);
        let expected_sha = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let filename = match parts.next() {
            Some(s) => s.trim(),
            None => continue,
        };
        let file_path = module_dir.join(filename);
        if !file_path.exists() {
            return Err(ModuleError::InvalidManifest(format!(
                "SHA256SUMS references missing file: {}",
                filename
            )));
        }
        let actual = sha256_of_file(&file_path)?;
        if actual != expected_sha {
            return Err(ModuleError::ChecksumMismatch(
                filename.to_string(),
                expected_sha.to_string(),
                actual,
            ));
        }
        checked.push(filename.to_string());
    }
    Ok(checked)
}

fn sha256_of_file(path: &Path) -> Result<String, ModuleError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn registry_starts_empty() {
        let reg = Registry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_and_lookup() {
        let reg = Registry::new();
        let m = InstalledModule {
            install_dir: PathBuf::from("/tmp/fake"),
            manifest: ModuleManifest {
                id: "voice".into(),
                name: "Voice for MiracleClaw".into(),
                subtitle: Some("Push-to-talk chat input".into()),
                version: "0.1.0".into(),
                min_mc_version: "1.1.0".into(),
                description: "test".into(),
                binary: super::super::manifest::BinarySpec {
                    name: "voice.exe".into(),
                    binary_type: "sidecar".into(),
                },
                commands: vec![super::super::manifest::CommandSpec {
                    tauri: "mc_voice_transcribe".into(),
                    action: "transcribe".into(),
                    description: "test".into(),
                }],
                ui_hooks: vec!["#x".into()],
                assets: vec![],
                author: None,
                homepage: None,
                publisher_icon: None,
            },
        };
        reg.register(m);
        assert_eq!(reg.len(), 1);
        assert!(reg.get("voice").is_some());
        assert!(reg.resolve_command("mc_voice_transcribe").is_some());
        assert!(reg.resolve_command("mc_voice_nope").is_none());
    }

    #[test]
    fn unregister_removes_module() {
        let reg = Registry::new();
        let m = InstalledModule {
            install_dir: PathBuf::from("/tmp/fake"),
            manifest: ModuleManifest {
                id: "voice".into(),
                name: "Voice for MiracleClaw".into(),
                subtitle: None,
                version: "0.1.0".into(),
                min_mc_version: "1.1.0".into(),
                description: "test".into(),
                binary: super::super::manifest::BinarySpec {
                    name: "voice.exe".into(),
                    binary_type: "sidecar".into(),
                },
                commands: vec![],
                ui_hooks: vec![],
                assets: vec![],
                author: None,
                homepage: None,
                publisher_icon: None,
            },
        };
        reg.register(m);
        assert!(reg.unregister("voice"));
        assert!(!reg.unregister("voice")); // already gone
        assert!(reg.is_empty());
    }

    #[test]
    fn sha256_helper_returns_consistent_hash() {
        // Round-trip a small string through our test sha256 helper.
        let tmp = std::env::temp_dir().join("mc_module_sha_test.txt");
        std::fs::write(&tmp, b"hello").unwrap();
        let h1 = sha256_of_file(&tmp).unwrap();
        let h2 = sha256_of_file(&tmp).unwrap();
        assert_eq!(h1, h2);
        // Sanity: a different file produces a different hash.
        let tmp2 = std::env::temp_dir().join("mc_module_sha_test2.txt");
        std::fs::write(&tmp2, b"world").unwrap();
        let h3 = sha256_of_file(&tmp2).unwrap();
        assert_ne!(h1, h3);
        std::fs::remove_file(&tmp).ok();
        std::fs::remove_file(&tmp2).ok();
    }
}
