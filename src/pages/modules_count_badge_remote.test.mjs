// Lesson 976 (2026-09-11 18:05 MDT, David): regression tests for
// the toolbar Modules count badge + dropdown positioning + no-tools
// fallback modal. After Lesson 975 the dropdown showed installed
// modules, but the count badge still read from `listInstalledModules()`
// (local Map) and showed "0" when 2 modules were actually installed
// via MAIC. Plus the dropdown was clipping off the right edge of the
// viewport when the button was near the right side. Plus clicking a
// module with no `client_manifest.tools` did nothing visible (a
// toast in the corner that was easy to miss).
//
// Pins:
//   1. `getInstalledModulesMerged` is exported from modules-menu.js
//      and is the canonical way to count installed modules.
//   2. terminal.js count badge uses `getInstalledModulesMerged`,
//      NOT `listInstalledModules` (so the badge and dropdown agree).
//   3. dashboard.js count badge uses `getInstalledModulesMerged`,
//      NOT `listInstalledModules`.
//   4. The dropdown right-aligns when it would extend past the
//      viewport right edge.
//   5. The dropdown flips above the button when it would extend
//      past the viewport bottom edge.
//   6. When a module has no `client_manifest.tools`, the modal
//      shows an info modal with description + hint, not a silent
//      toast bail.
//   7. The info modal has a Close / OK action.
//   8. The .mc-modal-info CSS class exists and is styled.
//
// Run via: `node --test src/pages/modules_count_badge_remote.test.mjs`

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const modulesMenuSrc = await readFile(
  new URL("../modules-menu.js", import.meta.url),
  "utf8"
);
const terminalSrc = await readFile(
  new URL("./terminal.js", import.meta.url),
  "utf8"
);
const dashboardSrc = await readFile(
  new URL("./dashboard.js", import.meta.url),
  "utf8"
);
const stylesSrc = await readFile(
  new URL("../styles.css", import.meta.url),
  "utf8"
);

test("getInstalledModulesMerged is exported from modules-menu.js", () => {
  // Lesson 976: the shared helper. Both the dropdown and the count
  // badge import it so the two surfaces always agree.
  assert.match(
    modulesMenuSrc,
    /export\s+(async\s+)?function\s+getInstalledModulesMerged\b/,
    "getInstalledModulesMerged should be an exported function"
  );
  // It must query both sources.
  assert.match(
    modulesMenuSrc,
    /export\s+(async\s+)?function\s+getInstalledModulesMerged[\s\S]*?listInstalledModules\(\)/,
    "getInstalledModulesMerged should call listInstalledModules() (local)"
  );
  assert.match(
    modulesMenuSrc,
    /export\s+(async\s+)?function\s+getInstalledModulesMerged[\s\S]*?invoke\(\s*["']mc_list_installed_modules["']\s*\)/,
    "getInstalledModulesMerged should call invoke('mc_list_installed_modules') (remote)"
  );
  // Graceful fallback: catches the remote error and returns local.
  assert.match(
    modulesMenuSrc,
    /export\s+(async\s+)?function\s+getInstalledModulesMerged[\s\S]*?try\s*\{[\s\S]*?\}\s*catch\s*\([\s\S]*?return localItems/,
    "getInstalledModulesMerged should fall back to local list on remote error"
  );
});

test("terminal.js count badge uses getInstalledModulesMerged (not listInstalledModules)", () => {
  // Lesson 976: David reported badge showed "0" but dropdown showed
  // 2 installed. The cause was that the count was reading the local
  // Map (empty for MAIC-installed modules). Fix: use the shared
  // merged helper.
  assert.match(
    terminalSrc,
    /import\s*\{[^}]*getInstalledModulesMerged[^}]*\}\s*from\s*["']\.\.\/modules-menu\.js["']/,
    "terminal.js should import getInstalledModulesMerged from modules-menu.js"
  );
  // The count update function should call it, not listInstalledModules.
  assert.match(
    terminalSrc,
    /async\s+function\s+updateTerminalModulesCount[\s\S]*?await\s+getInstalledModulesMerged\s*\(\s*\)/,
    "updateTerminalModulesCount should await getInstalledModulesMerged()"
  );
});

test("dashboard.js count badge uses getInstalledModulesMerged (not listInstalledModules)", () => {
  // Same fix for the dashboard's inline Modules button (Lesson 976).
  assert.match(
    dashboardSrc,
    /import\s*\{[^}]*getInstalledModulesMerged[^}]*\}\s*from\s*["']\.\.\/modules-menu\.js["']/,
    "dashboard.js should import getInstalledModulesMerged from modules-menu.js"
  );
  assert.match(
    dashboardSrc,
    /async\s+function\s+updateModulesCount[\s\S]*?await\s+getInstalledModulesMerged\s*\(\s*\)/,
    "updateModulesCount should await getInstalledModulesMerged()"
  );
});

test("modules-menu dropdown right-aligns when it would clip the viewport", () => {
  // Lesson 976: David reported the menu was half off-screen to the
  // right when the Modules button was near the right edge. The fix
  // is to measure the menu after attach and switch to
  // `right: <distance-from-right>` when it would clip.
  // Look for the viewport-edge check + the right-align code path.
  assert.match(
    modulesMenuSrc,
    /rect\.left\s*\+\s*menuWidth\s*>\s*window\.innerWidth\s*-\s*margin/,
    "should check if menu would clip the viewport right edge"
  );
  // When clipping, switch to right-align.
  assert.match(
    modulesMenuSrc,
    /menu\.style\.left\s*=\s*["']auto["'][\s\S]*?menu\.style\.right\s*=\s*`\$\{window\.innerWidth\s*-\s*rect\.right\}/,
    "should right-align the menu when it would clip"
  );
});

test("modules-menu dropdown flips above the button when it would clip the viewport bottom", () => {
  // Lesson 976: when the button is near the bottom of the viewport,
  // the menu extends past the visible area. The fix is to flip the
  // menu above the button when there's not enough room below.
  assert.match(
    modulesMenuSrc,
    /maxMenuHeight\s*<\s*200/,
    "should detect when not enough room below the button"
  );
  assert.match(
    modulesMenuSrc,
    /menu\.style\.top\s*=\s*`\$\{rect\.top\s*-\s*neededHeight/,
    "should flip the menu above the button when not enough room below"
  );
});

test("openModuleModal shows info modal (not silent toast) when tools array is empty", () => {
  // Lesson 976: when a module has no `client_manifest.tools`, the
  // old code toasted `${name} has no tools to run.` and bailed. The
  // toast was easy to miss in the corner. New: open an info modal
  // that explains what the module is and where to use it.
  assert.match(
    modulesMenuSrc,
    /function\s+openModuleInfoModal\s*\(/,
    "should have openModuleInfoModal helper"
  );
  // openModuleModal should call openModuleInfoModal when tools is empty.
  assert.match(
    modulesMenuSrc,
    /if\s*\(\s*tools\.length\s*===\s*0\s*\)\s*\{[\s\S]*?openModuleInfoModal\s*\(/,
    "openModuleModal should call openModuleInfoModal when tools is empty"
  );
  // No more "has no tools to run" toast bail.
  assert.doesNotMatch(
    modulesMenuSrc,
    /toast\(`\$\{module\.name\}\s+has\s+no\s+tools\s+to\s+run`/,
    "should not have the old silent 'has no tools to run' toast bail"
  );
});

test("openModuleInfoModal has a Close / OK action", () => {
  // The info modal needs a way to dismiss. A primary "OK" button
  // with data-action="modal-close" satisfies the existing modal
  // click handler in closeModuleModal().
  assert.match(
    modulesMenuSrc,
    /function\s+openModuleInfoModal\s*\([\s\S]*?data-action=["']modal-close["'][\s\S]*?class=["']mc-primary["']/,
    "openModuleInfoModal should have a primary 'OK' button with modal-close action"
  );
  // Should include a hint pointing at the launchpad.
  assert.match(
    modulesMenuSrc,
    /function\s+openModuleInfoModal\s*\([\s\S]*?launchpad/,
    "openModuleInfoModal should mention the launchpad as where to use the module"
  );
});

test(".mc-modal-info CSS class is defined and styled", () => {
  // The info modal needs styling. Lesson 976: pin that the CSS class
  // exists with reasonable padding/typography so the modal doesn't
  // render as a flat blob.
  assert.match(
    stylesSrc,
    /\.mc-modal-info\s*\{[^}]*padding/s,
    ".mc-modal-info should have padding"
  );
  assert.match(
    stylesSrc,
    /\.mc-modal-info-desc\s*\{/,
    ".mc-modal-info-desc should have its own rule (description text)"
  );
  assert.match(
    stylesSrc,
    /\.mc-modal-info-hint\s*\{/,
    ".mc-modal-info-hint should have its own rule (the launchpad hint)"
  );
});
