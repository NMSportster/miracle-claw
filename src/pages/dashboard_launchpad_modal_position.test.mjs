// Lesson 968 (2026-09-11, David): regression test for the launchpad
// module-run modal.
//
// Bug: openModuleModal() in src/pages/dashboard_launchpad.js appends
// `<div class="mc-modal">` to document.body with two children:
//
//   <div class="mc-modal">
//     <div class="mc-modal-backdrop" data-action="modal-close"></div>
//     <div class="mc-modal-panel">...form...</div>
//   </div>
//
// The backdrop is `position: fixed; inset: 0; z-index: 1000` so it
// covers the whole viewport with a 45% black overlay. The panel is
// a NORMAL block sibling — NOT inside the backdrop — and has no
// positioning. So the panel renders in normal document flow at the
// top of the page and ends up HIDDEN behind the fixed backdrop.
// The user sees the gray overlay but no form. Clicking anywhere
// closes the modal (the backdrop's data-action="modal-close" fires
// from any click that bubbles up to the wrapper).
//
// Fix: .mc-modal-panel needs `position: fixed; z-index: 1001;` to
// sit above the backdrop AND be centered independently. The HTML
// structure (panel as sibling of backdrop) is kept — moving the
// panel inside the backdrop would change event delegation and the
// backdrop's data-action still works from the click handler on
// the .mc-modal wrapper.
//
// This test pins:
//   1. The HTML structure (panel is sibling of backdrop).
//   2. The CSS positions the panel on top of the backdrop.
//   3. The panel has z-index higher than the backdrop.
//
// If anyone "refactors" the modal and breaks the visibility, this
// test should fail with a clear message about what the modal must
// look like.

import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, "..", "..");

// Re-read the source files every time so this test catches
// regressions in either the JS or the CSS.
function readLaunchpadJs() {
  return readFileSync(join(projectRoot, "src/pages/dashboard_launchpad.js"), "utf8");
}
function readStylesCss() {
  return readFileSync(join(projectRoot, "src/styles.css"), "utf8");
}

test("openModuleModal renders the panel as a sibling of the backdrop (not nested)", () => {
  const js = readLaunchpadJs();
  // Find the openModuleModal function body and slice from its modal
  // creation. We anchor on the unique string 'className = "mc-modal"'
  // which appears once in dashboard_launchpad.js.
  const modalIdx = js.indexOf('className = "mc-modal"');
  assert.ok(modalIdx > 0, 'openModuleModal must create an element with className = "mc-modal"');
  const slice = js.slice(modalIdx, modalIdx + 700);
  // The backdrop opens, closes, then the panel opens. NOT nested.
  assert.match(slice, /mc-modal-backdrop[^>]*><\/div>/, "backdrop should be self-closed before the panel");
  assert.match(slice, /<div class="mc-modal-panel">/, "panel should be its own div sibling of the backdrop");
  // Sanity: panel must NOT be inside the backdrop tag.
  const backdropClose = slice.indexOf("</div>");
  const panelOpen = slice.indexOf('class="mc-modal-panel"');
  assert.ok(panelOpen > backdropClose, "panel must be a sibling, not nested inside the backdrop");
});

test(".mc-modal-panel is positioned and stacked above the backdrop", () => {
  const css = readStylesCss();
  // Find the .mc-modal-panel block.
  const panelMatch = css.match(/\.mc-modal-panel\s*\{([^}]*)\}/);
  assert.ok(panelMatch, ".mc-modal-panel rule must exist in src/styles.css");
  const block = panelMatch[1];
  // Must be position: fixed (or absolute — fixed handles scroll best).
  assert.match(
    block,
    /position\s*:\s*(fixed|absolute)/,
    `.mc-modal-panel must be position:fixed (or absolute) so it lifts out of normal flow. Current block:\n${block}`
  );
  // Must have a z-index higher than the backdrop's 1000.
  const zMatch = block.match(/z-index\s*:\s*(\d+)/);
  assert.ok(zMatch, `.mc-modal-panel must have a z-index. Current block:\n${block}`);
  const panelZ = Number(zMatch[1]);
  assert.ok(
    panelZ > 1000,
    `mc-modal-panel z-index (${panelZ}) must be > backdrop's 1000, otherwise it hides behind the gray overlay. Current block:\n${block}`
  );
});

test(".mc-modal-backdrop is fixed and full-viewport so it acts as a click target", () => {
  const css = readStylesCss();
  const backdropMatch = css.match(/\.mc-modal-backdrop\s*\{([^}]*)\}/);
  assert.ok(backdropMatch, ".mc-modal-backdrop rule must exist");
  const block = backdropMatch[1];
  assert.match(block, /position\s*:\s*fixed/, "backdrop must be position:fixed");
  assert.match(block, /inset\s*:\s*0/, "backdrop must cover the viewport (inset:0)");
  assert.match(block, /z-index\s*:\s*1000/, "backdrop z-index must be 1000 (or whatever the panel sits above)");
});
