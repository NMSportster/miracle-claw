// page_registry.js — minimal registry for MC's top-level pages.
//
// Why this exists:
//   v1.0.9-rc34: David asked for a page_registry BEFORE adding any more
//   pages or programs (Settings, Terminal, etc.). main.js was 393 lines
//   with login + dashboard + modals all inline. Adding a Settings page
//   meant wedging another 200+ lines in, sharing modals awkwardly.
//
// Design:
//   - PageDef is the shape every page exports.
//   - mount(root, ctx) renders the page into `root` and returns nothing.
//   - unmount() tears down event listeners + any pending async work
//     (timers, abort controllers, modal overlays). MUST be idempotent —
//     the registry may call unmount() more than once on rapid route
//     changes (defense against a bug we don't want to repeat).
//   - requiresAuth tells the router whether to gate the page behind a
//     successful MAIC login. Login page itself has requiresAuth: false.
//   - label + icon are for future sidebar/nav; not used yet.
//
// Lifecycle contract:
//   1. Router resolves which page to show based on auth state.
//   2. If the current page exists and is different, registry calls
//      currentPage.unmount() first.
//   3. New page's mount(root, ctx) runs.
//   4. If a fatal error escapes mount(), registry shows the fatal
//      overlay and leaves the page mounted (so the error is visible).
//
// Future pages (Settings, Terminal) just need to:
//   1. Add a new file under src/pages/
//   2. Call register('settings', { mount, unmount, ... }) at boot
//   3. Done — main.js doesn't change.

export const pages = new Map();

/**
 * Register a page by id. Idempotent — re-registering the same id
 * replaces the previous definition (useful for HMR / tests).
 */
export function register(id, def) {
  if (!id || typeof id !== "string") {
    throw new Error("page_registry.register: id must be a non-empty string");
  }
  if (!def || typeof def.mount !== "function") {
    throw new Error(`page_registry.register('${id}'): def.mount must be a function`);
  }
  // Default unmount is a no-op (pages with no teardown can omit it).
  const full = {
    label: id,
    icon: null,
    requiresAuth: true,
    unmount: () => {},
    ...def,
  };
  pages.set(id, full);
}

/**
 * Look up a registered page by id. Returns undefined if missing.
 */
export function get(id) {
  return pages.get(id);
}

/**
 * Mount the page identified by id into `root`. If a different page
 * is currently mounted, unmount it first.
 *
 * @param {string} id
 * @param {HTMLElement} root
 * @param {object} [ctx] — opaque context passed to mount() (auth state, etc.)
 * @returns {boolean} true if mount succeeded, false if page not found / mount threw
 */
export function mount(id, root, ctx = {}) {
  const def = pages.get(id);
  if (!def) {
    console.error(`[page_registry] no page registered for id '${id}'`);
    return false;
  }

  // If a page is already mounted, unmount it first.
  if (current.id && current.id !== id) {
    unmountCurrent();
  }

  try {
    def.mount(root, ctx);
    current.id = id;
    current.def = def;
    // Lesson 581 (2026-08-25 16:25 MDT, David): re-apply module UI
    // hooks after each page mount. Terminal/Files/etc. buttons render
    // AFTER initModuleRuntime's boot sync, so they keep the template's
    // `data-module-<id>-installed="false"` and stay greyed out even
    // when the module IS installed. Re-running activateUiHooks here
    // flips them to "true" the moment the page becomes visible.
    if (typeof window !== "undefined" && window.__MC_REAPPLY_UI_HOOKS__) {
      try {
        window.__MC_REAPPLY_UI_HOOKS__();
      } catch (e) {
        console.warn("[page_registry] reapply ui hooks failed:", e);
      }
    }
    return true;
  } catch (err) {
    console.error(`[page_registry] mount('${id}') threw:`, err);
    current.id = null;
    current.def = null;
    return false;
  }
}

/**
 * Unmount whatever is currently mounted. Safe to call multiple times.
 */
export function unmountCurrent() {
  if (!current.def) return;
  try {
    current.def.unmount();
  } catch (err) {
    console.error(`[page_registry] unmount('${current.id}') threw:`, err);
  }
  current.id = null;
  current.def = null;
}

/**
 * Return the id of the currently mounted page, or null if nothing mounted.
 */
export function currentId() {
  return current.id;
}

// Track what's mounted so we can unmount it on the next mount().
const current = { id: null, def: null };
