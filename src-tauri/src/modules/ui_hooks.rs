//! UI hook system — flips HTML elements from dormant to active when a
//! module is installed.
//!
//! ## Pattern
//!
//! Base MC ships with HTML elements that LOOK like they should work but
//! are dormant until a module is installed. They're tagged with
//! `data-module-<id>-installed="false"`:
//!
//! ```html
//! <button id="terminal-voice-btn"
//!         class="mc-module-hook"
//!         data-module-voice-installed="false">
//!   🎙 Voice
//! </button>
//! ```
//!
//! CSS in base MC:
//!
//! ```css
//! .mc-module-hook[data-module-voice-installed="false"] {
//!   opacity: 0.3;
//!   cursor: not-allowed;
//!   filter: grayscale(100%);
//!   pointer-events: none;
//! }
//! .mc-module-hook[data-module-voice-installed="true"] {
//!   opacity: 1.0;
//!   cursor: pointer;
//!   filter: none;
//! }
//! ```
//!
//! When a module installs, MC emits a "module-installed" event with the
//! module id. JS listens, flips the attribute on every matching selector.
//!
//! ## Why CSS attribute selectors instead of JS class toggles?
//!
//! - **Pure declarative** — designer can read CSS and see dormant/active
//!   states without JS knowledge.
//! - **No layout flicker** on load — CSS applies immediately, before JS runs.
//! - **Self-documenting** in DevTools — the attribute itself tells you
//!   "voice is not installed yet".
//! - **No "flash of active state"** — a class toggle via JS would briefly
//!   show the active state before being reverted to dormant.
//!
//! ## Events (JS-side)
//!
//! MC base listens for these events on `window`:
//!
//! ```js
//! window.addEventListener('mc:module-installed', (e) => {
//!   const { id, hooks } = e.detail;
//!   for (const selector of hooks) {
//!     document.querySelectorAll(selector).forEach(el => {
//!       el.setAttribute(`data-module-${id}-installed`, 'true');
//!     });
//!   }
//! });
//!
//! window.addEventListener('mc:module-uninstalled', (e) => {
//!   const { id, hooks } = e.detail;
//!   for (const selector of hooks) {
//!     document.querySelectorAll(selector).forEach(el => {
//!       el.setAttribute(`data-module-${id}-installed`, 'false');
//!     });
//!   }
//! });
//! ```
//!
//! Emitted from Rust via `tauri::Window::emit("mc:module-installed", payload)`
//! after `installer::install_from_url` completes.

use serde::Serialize;

/// Event payload for `mc:module-installed`.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleInstalledEvent {
    /// Module id. e.g. `"voice"`.
    pub id: String,
    /// Module display name. e.g. `"Voice Input"`.
    pub name: String,
    /// Module version. e.g. `"0.1.0"`.
    pub version: String,
    /// CSS selectors that should be flipped from dormant → active.
    /// Pulled from `manifest::ui_hooks`.
    pub hooks: Vec<String>,
}

/// Event payload for `mc:module-uninstalled`.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleUninstalledEvent {
    pub id: String,
    pub hooks: Vec<String>,
}

/// Generate the CSS attribute name for a given module id.
///
/// e.g. `hook_attr("voice")` → `"data-module-voice-installed"`.
pub fn hook_attr(module_id: &str) -> String {
    format!("data-module-{}-installed", module_id)
}

/// Generate the CSS attribute selector for a given module id.
///
/// e.g. `hook_selector("voice")` → `"[data-module-voice-installed=\"true\"]"`.
pub fn hook_selector_active(module_id: &str) -> String {
    format!("[data-module-{}-installed=\"true\"]", module_id)
}

/// Generate the dormant CSS attribute selector for a given module id.
pub fn hook_selector_dormant(module_id: &str) -> String {
    format!("[data-module-{}-installed=\"false\"]", module_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_attr_format() {
        assert_eq!(hook_attr("voice"), "data-module-voice-installed");
        assert_eq!(hook_attr("ocr"), "data-module-ocr-installed");
        assert_eq!(hook_attr("local-search"), "data-module-local-search-installed");
    }

    #[test]
    fn hook_selector_active_format() {
        assert_eq!(
            hook_selector_active("voice"),
            "[data-module-voice-installed=\"true\"]"
        );
    }

    #[test]
    fn hook_selector_dormant_format() {
        assert_eq!(
            hook_selector_dormant("voice"),
            "[data-module-voice-installed=\"false\"]"
        );
    }

    #[test]
    fn installed_event_serializes() {
        let e = ModuleInstalledEvent {
            id: "voice".into(),
            name: "Voice Input".into(),
            version: "0.1.0".into(),
            hooks: vec!["#terminal-voice-btn".into(), "#mc-voice-fab".into()],
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"id\":\"voice\""));
        assert!(s.contains("\"name\":\"Voice Input\""));
        assert!(s.contains("\"hooks\":[\"#terminal-voice-btn\",\"#mc-voice-fab\"]"));
    }
}
