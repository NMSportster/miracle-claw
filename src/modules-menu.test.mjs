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

test("module-id alias map dedupes MAIC and local installs (Lesson 1136)", () => {
  // Lesson 1136 (2026-10-08, David): without canonical id
  // dedup, the Terminal toolbar badge said "18 modules installed"
  // (8 MAIC + 11 local + 1 firecrawl in both = 18, with email/voice/
  // calendar counted twice). The fix collapses MAIC's
  // `email-campaigns-pro` and the local registry's `email` to one
  // record, with MAIC winning.
  assert.match(src, /MODULE_ID_ALIASES\s*=\s*\{/, "alias map exists");
  assert.match(src, /email-campaigns-pro[\s\S]+?email[^,}]/, "email-campaigns-pro → email");
  assert.match(src, /calendar-companion[\s\S]+?calendar/, "calendar-companion → calendar");
  assert.match(src, /voice-memo[\s\S]+?voice[^,}]/, "voice-memo → voice");
  assert.match(src, /function canonicalId/, "canonicalId helper exists");
  // The merge in getInstalledModulesMerged must use canonicalId
  // (not raw .id) to dedup.
  const mergeFn = src.match(
    /export async function getInstalledModulesMerged\(\)[\s\S]+?\n\}/
  );
  assert.ok(mergeFn, "getInstalledModulesMerged exists");
  assert.match(
    mergeFn[0],
    /canonicalId\(m\.id\)/,
    "merge uses canonicalId to dedup"
  );
});

test("client_side response shows Open-in-launchpad panel, not raw error (Lesson 1138)", () => {
  // Lesson 1138 (2026-10-08, David): MAIC returns
  // {outputs: {client_side: true, message: "This module runs
  // locally..."}} for firecrawl and friends. The old code dumped
  // that string into the result area, which David read as "the
  // tool errored out". The fix detects client_side and renders a
  // dedicated panel with an "Open in launchpad" button.
  assert.match(
    src,
    /outputs\.client_side\s*===\s*true/,
    "modules-menu.js checks outputs.client_side"
  );
  assert.match(
    src,
    /function renderClientSideResult/,
    "renderClientSideResult helper exists"
  );
  assert.match(
    src,
    /navigate\("dashboard-launchpad"\)/,
    "client-side panel navigates to dashboard-launchpad"
  );
  // mc-toolbar.js (Windows) must do the same.
  const winSrc = readFileSync(
    join(__dirname, "..", "depot", "openclaw-patches", "dist", "control-ui", "mc-toolbar.js"),
    "utf8"
  );
  assert.match(
    winSrc,
    /resp\.outputs\.client_side\s*===\s*true/,
    "mc-toolbar.js checks resp.outputs.client_side"
  );
  assert.match(
    winSrc,
    /Open in launchpad/,
    "mc-toolbar.js shows Open in launchpad button"
  );
});

test("defaultRenderItem shows description + meta line (Lesson 1137)", () => {
  // Lesson 1137 (2026-10-08, David): the Terminal Modules dropdown
  // was sparse (icon + name + version only) vs the Windows chat
  // toolbar dropdown (icon + name + description + tool list). The
  // fix expands defaultRenderItem to return {icon, name, subtitle
  // (description), meta (version + tool list)} and the dropdown
  // template renders both subtitle and meta.
  assert.match(
    src,
    /function defaultRenderItem/,
    "defaultRenderItem exists"
  );
  const renderFn = src.match(
    /function defaultRenderItem\(m\)[\s\S]+?\n\}/
  );
  assert.ok(renderFn, "defaultRenderItem body found");
  assert.match(
    renderFn[0],
    /description/,
    "defaultRenderItem reads description"
  );
  assert.match(
    renderFn[0],
    /toolLabel/,
    "defaultRenderItem builds toolLabel"
  );
  // Dropdown template renders the new .mc-modules-menu-meta element.
  const dropdown = src.match(
    /menu\.innerHTML = items[\s\S]+?\.join\(""\);/
  );
  assert.ok(dropdown, "dropdown template found");
  assert.match(
    dropdown[0],
    /mc-modules-menu-meta/,
    "dropdown renders .mc-modules-menu-meta"
  );
});
