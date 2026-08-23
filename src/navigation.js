// navigation.js — single source of truth for moving between pages.
//
// Why this exists:
//   Before rc49, every tile / back button / settings link in every page
//   had its own inline arrow function that re-implemented page context
//   wiring. Adding the Cmd-K palette (rc49) meant one more place to wire
//   the same dance — and a typo in any one of them silently broke a
//   navigation path.
//
// Now:
//   - main.js calls installNavigation({ root, ctxBuilders, ... }) at boot
//     to register the root element and a map of "page id -> build ctx"
//     so any caller (palette, back button, dashboard tile, future deep
//     link) just says navigate('files') and the right context flows.
//
//   - Pages still receive their ctx through mount(root, ctx), same as
//     before. This module does NOT change the page contract — it just
//     removes duplication.
//
//   - The Cmd-K palette (src/cmd_k_palette.js) uses navigate() to jump
//     between pages; tiles and back buttons also call it.

let _root = null;
let _buildCtx = null;
// Map of page id -> async (extra?) -> ctx object. The default page
// builder returns the standard dashboard ctx; pages that want extras
// (e.g. terminal with a forced shell) register a builder that accepts
// the extras and returns the right ctx.
let _builders = new Map();

/**
 * Install navigation. Called once from main.js at boot.
 *
 * @param {object} args
 * @param {HTMLElement} args.root - the app's mount root element
 * @param {Map<string, Function>} args.builders - page id -> (extras?) -> ctx
 * @param {Function} [args.mount] - page_registry.mount (passed in to
 *        avoid a circular import; main.js already has it).
 */
export function installNavigation({ root, builders, mount }) {
  _root = root;
  _builders = builders;
  _mount = mount;
}

let _mount = null;

/**
 * Navigate to a page. `extras` is passed to the page's ctx builder so
 * pages like terminal can receive a defaultShell override.
 *
 * @param {string} pageId
 * @param {object} [extras]
 * @returns {boolean} true if mount succeeded
 */
export function navigate(pageId, extras = {}) {
  if (!_root || !_mount) {
    console.error("[navigation] not installed; call installNavigation first");
    return false;
  }
  const builder = _builders.get(pageId);
  if (!builder) {
    console.error(`[navigation] no builder for page '${pageId}'`);
    return false;
  }
  const ctx = builder(extras) || {};
  return _mount(pageId, _root, ctx);
}

/**
 * Run an action (used by Cmd-K actions that aren't page navigation).
 * Actions are pure functions; the palette passes the function in.
 */
export function runAction(fn) {
  try {
    fn();
  } catch (err) {
    console.error("[navigation] action threw:", err);
  }
}