//! Config — MAIC endpoint + per-user state file paths
//!
//! On first run, we discover the MAIC base URL by checking (in order):
//! 1. `MAIC_BASE_URL` env var
//! 2. The `~/.config/mc/config.toml` (or platform equivalent) `maic_base_url` field
//! 3. Fall back to the production MAIC endpoint
//!
//! All paths use `dirs` crate to honor XDG_CONFIG_HOME / APPDATA / ~/Library/Application Support.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DEFAULT_MAIC_BASE_URL: &str = "https://maicserver.com";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    /// MAIC base URL, e.g. `https://maicserver.com`.
    /// Override with `MAIC_BASE_URL` env var or this file.
    pub maic_base_url: Option<String>,
}

impl Config {
    /// Load config from disk. If file doesn't exist, returns default.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config from {}", path.display()))?;
        let cfg: Self = toml::from_str(&contents)
            .with_context(|| format!("parsing config from {}", path.display()))?;
        Ok(cfg)
    }

    /// Save config to disk. Creates parent dirs if needed.
    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let toml = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(&path, toml)
            .with_context(|| format!("writing config to {}", path.display()))?;
        Ok(())
    }

    /// Resolve the active MAIC base URL (env > file > default).
    pub fn resolved_base_url(&self) -> String {
        if let Ok(env_url) = std::env::var("MAIC_BASE_URL") {
            if !env_url.is_empty() {
                return env_url;
            }
        }
        self.maic_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_MAIC_BASE_URL.to_string())
    }

    /// Where the config file lives on this OS.
    /// Linux:   ~/.config/mc/config.toml (or $XDG_CONFIG_HOME/mc/config.toml)
    /// macOS:   ~/Library/Application Support/mc/config.toml
    /// Windows: %APPDATA%\mclaw\config.toml
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("could not determine user config directory")?;
        // Use "mclaw" as the app dir on Windows to disambiguate from any
        // future "mc" product. Linux/Mac use "mc" per the FHS/XDG convention
        // that the app's name should match the binary name.
        #[cfg(windows)]
        let app_dir = config_dir.join("mclaw");
        #[cfg(not(windows))]
        let app_dir = config_dir.join("mc");
        Ok(app_dir.join(CONFIG_FILE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resolves_to_production_maic() {
        let cfg = Config::default();
        assert_eq!(cfg.resolved_base_url(), DEFAULT_MAIC_BASE_URL);
    }

    #[test]
    fn env_var_overrides_file() {
        let mut cfg = Config::default();
        cfg.maic_base_url = Some("https://staging.maicserver.com".to_string());
        // SAFETY: test-only env mutation; safe because we don't run parallel tests
        unsafe { std::env::set_var("MAIC_BASE_URL", "https://dev.maicserver.com") };
        assert_eq!(cfg.resolved_base_url(), "https://dev.maicserver.com");
        unsafe { std::env::remove_var("MAIC_BASE_URL") };
    }

    #[test]
    fn empty_env_var_falls_through() {
        let mut cfg = Config::default();
        cfg.maic_base_url = Some("https://staging.maicserver.com".to_string());
        unsafe { std::env::set_var("MAIC_BASE_URL", "") };
        assert_eq!(cfg.resolved_base_url(), "https://staging.maicserver.com");
        unsafe { std::env::remove_var("MAIC_BASE_URL") };
    }
}
