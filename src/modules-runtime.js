// Module runtime — JS-side wiring for the MC Module Framework.
//
// Responsibilities:
//   - Maintain the live module registry on the JS side (mirror of
//     Rust `modules::registry::Registry`)
//   - Subscribe to `mc:module-installed` / `mc:module-uninstalled`
//     events from Rust
//   - On install: flip `data-module-voice-installed="false"` attrs
//     to `"true"` on every element matching the manifest's `uiHooks`
//   - On uninstall: flip back to `"false"`
//   - Expose a `invokeModule(command, params)` helper that calls
//     `mc_module_call` and resolves to the module's `result`
//
// Why a JS-side mirror instead of calling `mc_module_list` every time?
//   - Hot path: many chat surfaces (terminal toolbar, FAB, fullscreen,
//     OpenClaw chat overlay) need to know "is voice installed?" in
//     <50ms. RPC roundtrip + ACL check is fine but polling is wasteful.
//   - Event-driven updates: install/uninstall events arrive within ms,
//     no polling needed.
//   - Single listener: one place to add logging, retry, future
//     install-from-URL UX etc.
//
// This module is loaded once from src/main.js before `boot()`. It's
// safe to call before the Tauri backend is reachable — it just
// silently waits for events.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// Installed modules: { [id]: { name, version, hooks: string[] } }
const installed = new Map();

let listenerInstalled = false;
let pendingInvokes = new Map(); // requestId -> {resolve, reject}

/**
 * Initialize the module runtime. Sets up event listeners. Safe to
 * call multiple times — only the first call wires listeners.
 */
export async function initModuleRuntime() {
  if (listenerInstalled) return;
  listenerInstalled = true;

  // mc:module-installed — payload: { id, name, version, hooks }
  await listen("mc:module-installed", (evt) => {
    const { id, name, version, hooks } = evt.payload || {};
    if (!id) return;
    installed.set(id, { name, version, hooks: hooks || [] });
    console.log(`[modules] installed ${id} v${version} (${name})`);
    activateUiHooks(id, true);
    // Bubble up for any page that wants to know (e.g. settings
    // module manager card refresh).
    window.dispatchEvent(
      new CustomEvent("mc:module-installed", { detail: evt.payload })
    );
  });

  // mc:module-uninstalled — payload: { id }
  await listen("mc:module-uninstalled", (evt) => {
    const { id } = evt.payload || {};
    if (!id) return;
    installed.delete(id);
    console.log(`[modules] uninstalled ${id}`);
    activateUiHooks(id, false);
    window.dispatchEvent(
      new CustomEvent("mc:module-uninstalled", { detail: evt.payload })
    );
  });

  // Initial sync: ask Rust what's already installed (modules
  // discovered at startup before JS listeners were attached).
  try {
    const list = await invoke("mc_module_list");
    for (const m of list || []) {
      installed.set(m.id, {
        name: m.name,
        version: m.version,
        hooks: m.ui_hooks || [],
      });
      activateUiHooks(m.id, true);
    }
    if (installed.size > 0) {
      console.log(
        `[modules] startup: ${installed.size} installed:`,
        Array.from(installed.keys()).join(", ")
      );
    }
  } catch (e) {
    console.warn("[modules] mc_module_list failed:", e);
  }
}

/**
 * Is a given module installed right now?
 */
export function isModuleInstalled(id) {
  return installed.has(id);
}

/**
 * Iterate installed modules (for settings UI, status bar, etc.).
 */
export function listInstalledModules() {
  return Array.from(installed.entries()).map(([id, info]) => ({
    id,
    ...info,
  }));
}

/**
 * Invoke a module command via `mc_module_call`. Returns the
 * module's `result` payload on success, throws on error.
 */
export async function invokeModule(command, params = {}) {
  const t0 = performance.now();
  try {
    const resp = await invoke("mc_module_call", {
      command,
      params,
    });
    const elapsed = (performance.now() - t0).toFixed(0);
    console.log(`[modules] ${command} ok in ${elapsed}ms`, resp);
    return resp;
  } catch (e) {
    const elapsed = (performance.now() - t0).toFixed(0);
    console.error(`[modules] ${command} failed in ${elapsed}ms:`, e);
    throw e;
  }
}

/**
 * Flip `data-module-<id>-installed` attrs on every element matching
 * the manifest's `uiHooks` selectors. Live elements get `true`,
 * dormant ones get `false`.
 *
 * CSS uses `[data-module-voice-installed="true"]` to enable the
 * buttons; `[data-module-voice-installed="false"]` keeps them grey.
 *
 * Why data attrs instead of adding/removing classes:
 *   - Three states (true/false/never-installed) collapse to one attr
 *   - Easier to read from CSS and devtools
 *   - Survives page reloads (we re-sync via mc_module_list at boot)
 */
function activateUiHooks(moduleId, live) {
  const attr = `data-module-${moduleId}-installed`;
  const val = live ? "true" : "false";

  // Find selectors from the manifest. If we don't have them (e.g.
  // uninstall event without manifest data), fall back to the
  // element-by-element discovery below.
  const info = installed.get(moduleId) || uninstalledHooksCache.get(moduleId);
  const selectors = (info && info.hooks) || [];

  // Method 1: selectors from manifest — fast + covers selectors that
  // don't exist yet (terminal toolbar might mount the voice btn
  // after install).
  for (const sel of selectors) {
    try {
      for (const el of document.querySelectorAll(sel)) {
        el.setAttribute(attr, val);
      }
    } catch (_) {
      // Selector might be malformed — ignore and fall through.
    }
  }

  // Method 2: any element that already has the attr (installed from
  // a previous page). Catches the case where the JS gets uninstall
  // before the manifest hooks are cached.
  for (const el of document.querySelectorAll(`[${attr}]`)) {
    el.setAttribute(attr, val);
  }
}

// Cache of selectors for modules we've seen installed/uninstalled
// (so uninstall still flips elements even though we removed the
// entry from `installed`).
const uninstalledHooksCache = new Map();