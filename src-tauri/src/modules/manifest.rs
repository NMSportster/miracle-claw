//! Module manifest schema (installer.json).
//!
//! Every MC module ships with an `installer.json` at the root of its
//! extracted directory. This is the contract between the module author
//! and the MC base binary — it tells MC what the module provides, what
//! UI hooks it activates, and what files it needs.
//!
//! ## Example (Voice for MiracleClaw module)
//!
//! ```json
//! {
//!   "id": "voice",
//!   "name": "Voice for MiracleClaw",
//!   "version": "0.1.0",
//!   "subtitle": "Push-to-talk chat input",
//!   "minMcVersion": "1.1.0-rc53.14",
//!   "description": "Capture audio with cpal, transcribe locally with whisper.cpp, drop the transcript into any chat surface. No audio ever leaves your machine.",
//!   "binary": {
//!     "name": "miracle-claw-voice",
//!     "type": "sidecar"
//!   },
//!   "commands": [
//!     {
//!       "tauri": "mc_voice_transcribe",
//!       "action": "transcribe",
//!       "description": "Capture audio + return Whisper transcript."
//!     },
//!     {
//!       "tauri": "mc_voice_check",
//!       "action": "check",
//!       "description": "Returns voice module install + model readiness state."
//!     }
//!   ],
//!   "uiHooks": [
//!     "#terminal-voice-btn",
//!     "#mc-voice-fab",
//!     ".terminal-fullscreen-mic"
//!   ],
//!   "assets": [
//!     {
//!       "name": "ggml-base.en.bin",
//!       "kind": "model",
//!       "downloadUrl": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
//!       "sizeBytes": 75000000,
//!       "sha256": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin.sha256"
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::{ModuleError, MANIFEST_FILENAME};

/// Top-level manifest — exactly what `installer.json` deserializes to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    /// Unique module id (lowercase, hyphens allowed). Must match the
    /// module's install dir name. e.g. `"voice"`, `"ocr"`, `"tts"`.
    pub id: String,

    /// Human-readable display name shown in the Module Manager card
    /// title + the Settings toggle label. e.g. `"Voice for MiracleClaw"`.
    pub name: String,

    /// Optional one-line tagline under the name. e.g. `"Push-to-talk chat input"`.
    /// Shown on the Module Manager card so users can scan what each
    /// module does without opening docs.
    #[serde(default)]
    pub subtitle: Option<String>,

    /// Semver version. e.g. `"0.1.0"`.
    pub version: String,

    /// Minimum MC base binary version required (inclusive).
    /// Compared against `BUNDLE_VERSION` in `tauri.conf.json`.
    /// e.g. `"1.1.0-rc53.14"`.
    #[serde(rename = "minMcVersion")]
    pub min_mc_version: String,

    /// Short description shown in the Module Manager UI.
    #[serde(default)]
    pub description: String,

    /// Module entry binary. Spawned via `tauri-plugin-shell::ShellExt::sidecar`
    /// when a `mc_<id>_*` command is invoked.
    pub binary: BinarySpec,

    /// List of Tauri commands this module provides.
    /// These are stubs in base MC; they route to the sidecar at runtime.
    #[serde(default)]
    pub commands: Vec<CommandSpec>,

    /// CSS selectors that should be flipped from dormant to active
    /// when this module is installed. Base MC ships these elements
    /// already in the DOM (with `data-module-{id}-installed="false"`);
    /// the installer flips the attribute on every matching selector.
    #[serde(default, rename = "uiHooks")]
    pub ui_hooks: Vec<String>,

    /// Optional assets (models, config files) that ship with the module.
    /// Currently used for documentation/UI; actual asset download is
    /// triggered separately (Lesson 572 plan).
    #[serde(default)]
    pub assets: Vec<AssetSpec>,

    /// Optional module author info.
    #[serde(default)]
    pub author: Option<String>,

    /// Optional homepage / repo URL.
    #[serde(default)]
    pub homepage: Option<String>,

    /// Optional module author / publisher icon URL. Shows next to the
    /// module name on the Module Manager card so users can tell which
    /// modules are official Milagro vs community contributions.
    #[serde(default, rename = "publisherIcon")]
    pub publisher_icon: Option<String>,
}

/// The sidecar binary spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySpec {
    /// Binary filename (no path). On Windows, `.exe` is appended automatically
    /// by `tauri-plugin-shell` if missing.
    pub name: String,

    /// Currently only `"sidecar"` is supported (Tauri-spawned child).
    /// `"native"` reserved for future in-process module support.
    #[serde(rename = "type", default = "default_binary_type")]
    pub binary_type: String,
}

fn default_binary_type() -> String {
    "sidecar".to_string()
}

/// A Tauri command provided by this module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    /// The Tauri command name. Must match a stub in base MC. e.g.
    /// `"mc_voice_transcribe"`.
    pub tauri: String,

    /// Action name passed to the sidecar via stdin JSON. e.g. `"transcribe"`.
    pub action: String,

    /// Short description for dev tooltips / docs.
    #[serde(default)]
    pub description: String,
}

/// An asset the module ships with (typically a model file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetSpec {
    pub name: String,

    /// `"model"`, `"config"`, `"data"`, etc.
    pub kind: String,

    /// Where to download from. Optional if the asset is bundled in the
    /// module archive.
    #[serde(default, rename = "downloadUrl")]
    pub download_url: Option<String>,

    /// Expected file size in bytes. Used to show download progress.
    #[serde(default, rename = "sizeBytes")]
    pub size_bytes: Option<u64>,

    /// Optional SHA256 for integrity check.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Read + parse `installer.json` from a module install directory.
pub fn load_from_dir(module_dir: &Path) -> Result<ModuleManifest, ModuleError> {
    let manifest_path = module_dir.join(MANIFEST_FILENAME);
    let raw = std::fs::read_to_string(&manifest_path).map_err(|e| {
        ModuleError::InvalidManifest(format!(
            "could not read {}: {}",
            manifest_path.display(),
            e
        ))
    })?;
    serde_json::from_str::<ModuleManifest>(&raw).map_err(|e| {
        ModuleError::InvalidManifest(format!("JSON parse failed: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let json = r##"{
            "id": "voice",
            "name": "Voice for MiracleClaw",
            "version": "0.1.0",
            "minMcVersion": "1.1.0-rc53.14",
            "description": "Push-to-talk",
            "binary": { "name": "miracle-claw-voice", "type": "sidecar" },
            "commands": [],
            "uiHooks": []
        }"##;
        let m: ModuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "voice");
        assert_eq!(m.name, "Voice for MiracleClaw");
        assert_eq!(m.binary.binary_type, "sidecar");
        assert!(m.commands.is_empty());
    }

    #[test]
    fn parses_full_voice_manifest() {
        let json = r##"{
            "id": "voice",
            "name": "Voice for MiracleClaw",
            "subtitle": "Push-to-talk chat input",
            "version": "0.1.0",
            "minMcVersion": "1.1.0-rc53.14",
            "description": "Capture audio with cpal, transcribe locally with whisper.cpp.",
            "binary": { "name": "miracle-claw-voice", "type": "sidecar" },
            "commands": [
                { "tauri": "mc_voice_transcribe", "action": "transcribe", "description": "Capture + transcribe." },
                { "tauri": "mc_voice_check", "action": "check", "description": "Status check." }
            ],
            "uiHooks": ["#terminal-voice-btn", "#mc-voice-fab"],
            "assets": [
                {
                    "name": "ggml-base.en.bin",
                    "kind": "model",
                    "downloadUrl": "https://example.com/model.bin",
                    "sizeBytes": 75000000
                }
            ],
            "author": "Milagro Cloud",
            "homepage": "https://milagro.cloud",
            "publisherIcon": "https://milagro.cloud/icons/milagro.svg"
        }"##;
        let m: ModuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.name, "Voice for MiracleClaw");
        assert_eq!(m.subtitle.as_deref(), Some("Push-to-talk chat input"));
        assert_eq!(m.commands.len(), 2);
        assert_eq!(m.ui_hooks.len(), 2);
        assert_eq!(m.assets.len(), 1);
        assert_eq!(m.author.as_deref(), Some("Milagro Cloud"));
        assert_eq!(
            m.publisher_icon.as_deref(),
            Some("https://milagro.cloud/icons/milagro.svg")
        );
    }
}
