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
 * Install a module from a remote URL (e.g. a GitHub release tarball).
 *
 * Wraps the `mc_module_install_url` Tauri command, which
 * - downloads the .tar.gz via reqwest (rustls, no native OpenSSL)
 * - extracts to `<modules_root>/<id>.tmp-<uuid>/`
 * - verifies SHA256SUMS if present
 * - atomically renames to `<modules_root>/<id>/`
 * - registers in the runtime registry
 * - emits `mc:module-installed` (handled by `initModuleRuntime`)
 *
 * The Tauri command returns an `InstallResult` JSON object; on success
 * we surface the installed manifest's id/version so the caller can
 * log or update UI.
 *
 * @param {string} id   module id (must match the archive's installer.json id)
 * @param {string} url  full https:// URL to the .tar.gz archive
 * @returns {Promise<{id: string, version: string, installDir: string, verifiedFiles: string[]}>}
 * @throws  on network failure, HTTP non-2xx, bad tarball, checksum
 *          mismatch, or manifest id mismatch
 *
 * v1.1.0-rc53.17 (Lesson 574c).
 */
export async function installModuleFromUrl(id, url) {
  if (!id || typeof id !== "string") {
    throw new Error("installModuleFromUrl: `id` is required");
  }
  if (!url || typeof url !== "string") {
    throw new Error("installModuleFromUrl: `url` is required");
  }
  if (!/^https?:\/\//i.test(url)) {
    throw new Error(
      `installModuleFromUrl: url must be http(s), got "${url.slice(0, 64)}…"`
    );
  }
  const t0 = performance.now();
  try {
    const result = await invoke("mc_module_install_url", { id, url });
    const elapsed = (performance.now() - t0).toFixed(0);
    console.log(
      `[modules] install ${id} from URL ok in ${elapsed}ms (${result?.verifiedFiles?.length || 0} files verified)`,
      result
    );
    return {
      id: result?.manifest?.id || id,
      version: result?.manifest?.version || "",
      installDir: result?.installDir || "",
      verifiedFiles: result?.verifiedFiles || [],
    };
  } catch (e) {
    const elapsed = (performance.now() - t0).toFixed(0);
    console.error(
      `[modules] install ${id} from URL failed in ${elapsed}ms:`,
      e
    );
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

/**
 * Re-apply `data-module-<id>-installed="true|false"` to every
 * currently mounted DOM element that has a `uiHooks` selector.
 *
 * WHY this exists (Lesson 581, 2026-08-25 16:25 MDT, David):
 *   `activateUiHooks()` runs once at boot, BEFORE lazily-mounted
 *   pages (Terminal, Files) are navigated to. Their buttons render
 *   with the template's hardcoded `data-module-voice-installed="false"`
 *   and never get flipped — the click handler returns early because
 *   `pointer-events: none` (CSS), so the button feels dead even
 *   though voice IS installed.
 *
 *   Calling this after every page mount fixes it: the buttons now
 *   re-sync with the JS-side `installed` Map at mount time.
 *
 *   Also dispatches a `mc:ui-hooks-applied` event so individual
 *   pages can run per-module logic (e.g. update voice module card
 *   status pill from "needs model" to "ready").
 *
 * Safe to call repeatedly — idempotent.
 */
export function reapplyAllUiHooks() {
  for (const id of installed.keys()) {
    activateUiHooks(id, true);
  }
  // Also flip uninstalled hooks back to false in case they got
  // rendered between uninstall + page-mount.
  for (const id of uninstalledHooksCache.keys()) {
    activateUiHooks(id, false);
  }
  window.dispatchEvent(new CustomEvent("mc:ui-hooks-applied"));
  console.log(
    `[modules] ui-hooks reapplied (${installed.size} installed, ${uninstalledHooksCache.size} uninstalled caches)`
  );
}
/**
 * Module-asset health — generic "does this module have its required
 * model/files?" query.
 *
 * Today: hardcoded for voice (mc_voice_check returns
 * { model_loaded, model_path, audio_host, input_device }).
 * Future: dispatcher grows a generic mc_module_check that returns
 * each module's full status payload, and individual modules decide
 * what's "missing".
 *
 * @param {string} moduleId — currently only "voice" is supported.
 * @returns {Promise<{ok: boolean, model_loaded: boolean, model_path: string|null, raw: object}>}
 */
export async function checkModuleHealth(moduleId) {
  if (moduleId !== "voice") {
    return { ok: false, model_loaded: null, model_path: null, raw: { reason: "unsupported" } };
  }
  try {
    const raw = await invokeModule("mc_voice_check", {});
    return {
      ok: !!raw?.model_loaded,
      model_loaded: !!raw?.model_loaded,
      model_path: raw?.model_path || null,
      raw,
    };
  } catch (e) {
    return { ok: false, model_loaded: false, model_path: null, raw: { error: String(e) } };
  }
}

/**
 * Ensure the voice whisper model is downloaded. Idempotent.
 * Returns true if model is now available (either pre-existing or
 * freshly downloaded), false if download failed.
 *
 * Triggers a toast notification via the caller-supplied onStatus
 * callback (e.g. "Downloading model…", "Ready", "Failed").
 *
 * Lesson 581 (2026-08-25 16:25 MDT, David): Voice binary ships with
 * `mc_voice_download_model` action that downloads
 * ggml-base.en.bin (~75MB) from huggingface.co/.../whisper.cpp/ with
 * SHA256 verification. This helper wires it into the JS flow so
 * users get a button instead of a cryptic binary error.
 *
 * @param {(status: string) => void} onStatus — UI status callback
 * @returns {Promise<boolean>}
 */
export async function ensureVoiceModel(onStatus) {
  const setStatus = (s) => { try { onStatus?.(s); } catch {} };

  setStatus("Checking whisper model…");
  const health = await checkModuleHealth("voice");
  if (health.model_loaded) {
    setStatus("Whisper model ready");
    return true;
  }

  setStatus("Downloading whisper model (~75MB)…");
  try {
    await invokeModule("mc_voice_download_model", {});
    // Verify the download succeeded.
    const recheck = await checkModuleHealth("voice");
    if (recheck.model_loaded) {
      setStatus("Whisper model ready");
      return true;
    }
    setStatus("Whisper model download did not complete");
    return false;
  } catch (e) {
    setStatus(`Whisper model download failed: ${e}`);
    return false;
  }
}
