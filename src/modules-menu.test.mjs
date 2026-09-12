// Lesson 969 (2026-09-11, David): regression test for the shared
// modules-menu helper. Pins:
//   1. openModuleModal HTML structure (panel sibling of backdrop, has
//      result-actions row, has Send-to-Chat / Copy / Close buttons)
//   2. sendResultToChat fallback chain: tries chat input first
//   3. form_schema → input rendering (a real firecrawl shape)
//
// Run via: `npm run test:js` (added to package.json scripts).
//
// We can't import the real modules-menu.js because it pulls in
// @tauri-apps/api/core (window.__TAURI__). Instead, mirror the
// structural expectations in node:test using a JSDOM-free approach:
// read the source file, assert the contract is preserved.

import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(join(__dirname, "modules-menu.js"), "utf8");

test("openModuleModal renders panel as sibling of backdrop", () => {
  // Lesson 968 sibling rule: the modal panel must NOT be a child of
  // the backdrop (which would break event delegation) — it's a
  // sibling. Verify the template still keeps them as siblings.
  assert.match(src, /modal\.className = "mc-modal"/, "has outer .mc-modal wrapper");
  assert.match(src, /class="mc-modal-backdrop"/, "has .mc-modal-backdrop");
  assert.match(src, /class="mc-modal-panel"/, "has .mc-modal-panel");
  // The panel must come after the backdrop in the template (siblings,
  // panel as second child of the wrapper) — not nested inside the
  // backdrop's tag.
  const backdropIdx = src.indexOf("mc-modal-backdrop");
  const panelIdx = src.indexOf("mc-modal-panel");
  assert.ok(backdropIdx > 0);
  assert.ok(panelIdx > backdropIdx, "panel appears after backdrop in template");
  // No nested `<div class="mc-modal-backdrop"><div class="mc-modal-panel">`
  // Find the end of the backdrop opening tag — the next `</div>` after
  // the backdrop's opening tag must come BEFORE the panel starts.
  const backdropOpenEnd = src.indexOf("</div>", backdropIdx);
  assert.ok(backdropOpenEnd > 0);
  assert.ok(panelIdx > backdropOpenEnd, "backdrop closes before panel opens");
});

test("result-actions row has Send to Chat, Copy, and Close buttons", () => {
  assert.match(
    src,
    /data-result-action="send-chat"/,
    "Send to Chat button present"
  );
  assert.match(src, /data-result-action="copy"/, "Copy button present");
  assert.match(src, /data-result-action="close"/, "Close button present");
});

test("sendResultToChat tries chat input before clipboard", () => {
  // Lesson 574 fallback chain. The function MUST query the dashboard
  // chat input first, then textarea[name='message'], then fall back to
  // writeText().
  assert.match(
    src,
    /querySelector\("#dashboard-chat-input"\)/,
    "tries #dashboard-chat-input first"
  );
  assert.match(
    src,
    /querySelector\("textarea\[name='message'\]"\)/,
    "falls back to textarea[name='message']"
  );
  assert.match(src, /writeText\(text\)/, "falls back to clipboard writeText");
});

test("mountModulesMenu inserts dropdown as sibling of trigger", () => {
  // Lesson 968 sibling rule applies here too: the dropdown is a
  // sibling of the trigger button in the DOM. Both share the same
  // parent (the toolbar).
  assert.match(
    src,
    /insertBefore\(menu, buttonEl\.nextSibling\)/,
    "menu is inserted as a sibling of the trigger button"
  );
  // Marked with the attribute so outside-click handler can ignore
  // clicks on the trigger itself.
  assert.match(
    src,
    /setAttribute\("data-modules-menu-trigger"/,
    "trigger gets data-modules-menu-trigger"
  );
});

test("form_schema renderer handles URL + integer + boolean fields", () => {
  // Mirror the launchpad's form_schema test against the shared
  // renderField. Same expectations as the launchpad test:
  //   - "string" property becomes a text input
  //   - "integer"/"number" property becomes a number input
  //   - "boolean" property becomes a hidden + checkbox pair
  // We don't import the function (it requires a DOM), so we just
  // assert the renderer is present and handles the three types.
  assert.match(src, /function renderField/);
  assert.match(src, /type="number"/);
  assert.match(src, /type="checkbox"/);
  assert.match(src, /type="text"/);
  // Coercion rules for booleans and numbers are present
  assert.match(src, /inputs\[f\.name\] = !!form\.querySelector/);
  assert.match(src, /Number\(inputs\[f\.name\]\)/);
});

test("mountModulesMenu click handler uses cached module, not re-fetch (Lesson 980)", () => {
  // Bug 2: clicking a module in the terminal toolbar didn't open the
  // form because the click handler re-fetched the module list and the
  // lookup failed silently. Fix: use the cached object from
  // getInstalledModulesMerged() and only re-fetch as a fallback.
  // Verify the click handler references menu._items and the
  // cached._raw path.
  assert.match(
    src,
    /menu\._items/,
    "click handler reads cached items from menu"
  );
  assert.match(
    src,
    /cached\._raw/,
    "click handler unwraps _raw to get the MAIC module shape"
  );
  // normalizeRemoteModule must preserve _raw so the click handler can
  // find the original MAIC shape (with client_manifest.tools).
  const normalizeFn = src.match(/function normalizeRemoteModule[\s\S]+?\n\s{2}\};/);
  assert.ok(normalizeFn, "normalizeRemoteModule function exists");
  assert.match(
    normalizeFn[0],
    /_raw:\s*m/,
    "normalizeRemoteModule preserves _raw reference to the original MAIC entry"
  );
});

test("bridge.json capability lists allow-mc-list-installed-modules (Lesson 980)", () => {
  // Bug 4: the OpenClaw chat Modules dropdown said "No modules"
  // because the bridge.json capability didn't allow
  // mc_list_installed_modules. The Tauri ACL rejected the IPC and
  // the toolbar's catch returned [], showing the empty state.
  const bridgeJson = readFileSync(
    join(__dirname, "..", "src-tauri", "capabilities", "bridge.json"),
    "utf8"
  );
  assert.match(
    bridgeJson,
    /"allow-mc-list-installed-modules"/,
    "bridge.json permits mc_list_installed_modules from the chat origin"
  );
});
