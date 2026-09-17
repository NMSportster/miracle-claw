// Lesson 974 (2026-09-11 15:14 MDT, David): regression test that pins
// the icon-only button structure for terminal + dashboard headers.
// The user reported that the previous `.toolbar-btn` structure (icon
// + text label, ~1 inch wide) made the in-app terminal page header
// overflow and obscured the OS title bar's min/max/close buttons.
// Switching header buttons back to `.icon-link` (32x32, icon-only)
// restores the pre-RC56.2 look AND keeps the SVG icons visible.
//
// Pins:
//   1. terminal.js: terminal-fullscreen + terminal-back are
//      `.icon-link` (not `.toolbar-btn`)
//   2. terminal.js: header buttons contain inline SVG (not emoji +
//      text label spans)
//   3. dashboard.js: mc-voice-fab / mc-modules-btn / settings-link
//      are `.icon-link` (icon-only)
//   4. dashboard.js: header buttons use `.icon-link-glyph` for the
//      emoji span (so the icon stays in a fixed-position span even
//      when the JS replaces its textContent during voice processing)
//   5. styles.css: `.icon-link.is-fullscreen` swaps the icon for ⛶
//      when the terminal is in fullscreen mode (so we don't need
//      JS-side textContent manipulation that nukes the SVG)
//
// Run via: `node --test src/pages/header_buttons_icon_only.test.mjs`

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const repoRoot = new URL("../..", import.meta.url).pathname;

const terminalSrc = await readFile(
  new URL("../pages/terminal.js", import.meta.url),
  "utf8"
);
const dashboardSrc = await readFile(
  new URL("../pages/dashboard.js", import.meta.url),
  "utf8"
);
const stylesSrc = await readFile(
  new URL("../styles.css", import.meta.url),
  "utf8"
);

test("terminal.js: header Fullscreen button is .icon-link (icon-only)", () => {
  // Match the button block: id="terminal-fullscreen" with class="icon-link"
  // and inline SVG (not toolbar-btn + text label).
  assert.match(
    terminalSrc,
    /class="icon-link"[\s\S]{0,400}?id="terminal-fullscreen"/,
    "terminal-fullscreen should be .icon-link, not .toolbar-btn"
  );
  // Make sure it does NOT also carry the .toolbar-btn class
  assert.doesNotMatch(
    terminalSrc,
    /class="toolbar-btn[\s\S]{0,200}?id="terminal-fullscreen"/,
    "terminal-fullscreen should not be .toolbar-btn (Lesson 974)"
  );
  // Should contain inline SVG, not a <span>Fullscreen</span> label
  const fullscreenBlock = terminalSrc.match(
    /id="terminal-fullscreen"[\s\S]{0,500}?<\/button>/
  );
  assert.ok(fullscreenBlock, "could not locate terminal-fullscreen button");
  assert.match(fullscreenBlock[0], /<svg /, "header button should have inline SVG icon");
  assert.doesNotMatch(
    fullscreenBlock[0],
    /<span>Fullscreen<\/span>/,
    "header button should not have a 'Fullscreen' text label (icon-only)"
  );
});

test("terminal.js: header Dashboard button is .icon-link (icon-only)", () => {
  assert.match(
    terminalSrc,
    /class="icon-link"[\s\S]{0,400}?id="terminal-back"/,
    "terminal-back should be .icon-link, not .toolbar-btn"
  );
  const backBlock = terminalSrc.match(
    /id="terminal-back"[\s\S]{0,500}?<\/button>/
  );
  assert.ok(backBlock, "could not locate terminal-back button");
  assert.match(backBlock[0], /<svg /, "header button should have inline SVG icon");
  assert.doesNotMatch(
    backBlock[0],
    /<span>Dashboard<\/span>/,
    "header button should not have a 'Dashboard' text label (icon-only)"
  );
});

test("terminal.js: fullscreen toggle uses .is-fullscreen class, not textContent", () => {
  // The previous code did `fullscreenBtn.textContent = "⛶"` which nuked
  // the SVG. The fix uses a class toggle that the CSS handles.
  assert.doesNotMatch(
    terminalSrc,
    /fullscreenBtn\.textContent\s*=\s*"⛶"/,
    "fullscreenBtn.textContent replacement is removed (Lesson 974)"
  );
  assert.match(
    terminalSrc,
    /fullscreenBtn\.classList\.toggle\("is-fullscreen"/,
    "fullscreenBtn should toggle .is-fullscreen class"
  );
});

test("dashboard.js: header buttons are .icon-link (icon-only)", () => {
  // Each of these should be .icon-link, not .toolbar-btn
  for (const id of ["mc-voice-fab", "mc-modules-btn", "settings-link"]) {
    const re = new RegExp(`class="icon-link"[\\s\\S]{0,300}?id="${id}"`);
    assert.match(
      dashboardSrc,
      re,
      `${id} should be .icon-link (icon-only), not .toolbar-btn (Lesson 974)`
    );
    // Make sure no .toolbar-btn class
    const noToolbar = new RegExp(
      `class="toolbar-btn[\\s\\S]{0,200}?id="${id}"`
    );
    assert.doesNotMatch(
      dashboardSrc,
      noToolbar,
      `${id} should not be .toolbar-btn`
    );
  }
});

test("dashboard.js: header voice JS updates .icon-link-glyph, not button textContent", () => {
  // The voice transcription flow used to do voiceFab.textContent = "🎙…"
  // which wiped out the icon. The fix targets the inner glyph span.
  assert.match(
    dashboardSrc,
    /const voiceGlyph\s*=\s*voiceFab\.querySelector\(\s*"\.icon-link-glyph"\s*\)/,
    "voice handler should target .icon-link-glyph span"
  );
  assert.match(
    dashboardSrc,
    /voiceGlyph\.textContent\s*=\s*"🎙…"/,
    "voice handler should set glyph textContent, not button textContent"
  );
  // And the download variant
  assert.match(
    dashboardSrc,
    /voiceGlyph\.textContent\s*=\s*status\.startsWith\("Downloading"\) \? "📥…" : "🎙…"/,
    "voice handler download status should also target glyph"
  );
});

test("styles.css: .icon-link.is-fullscreen swaps SVG for ⛶ glyph", () => {
  // When the terminal is in fullscreen mode, the button should show
  // ⛶ (exit-fullscreen indicator) instead of the maximize SVG. This
  // is how we avoid nuking the button content via textContent.
  assert.match(
    stylesSrc,
    /\.icon-link\.is-fullscreen\s+svg\s*\{[^}]*display:\s*none/,
    "is-fullscreen state should hide the SVG"
  );
  assert.match(
    stylesSrc,
    /\.icon-link\.is-fullscreen::after\s*\{[^}]*content:\s*"⛶"/,
    "is-fullscreen state should show ⛶ via ::after pseudo-element"
  );
});

test("styles.css: count badge on .icon-link is absolute-positioned", () => {
  // When the modules count badge lives inside an .icon-link button
  // (32x32), it needs absolute positioning so it doesn't push the
  // button wider than 32px.
  assert.match(
    stylesSrc,
    /\.icon-link\s+\.toolbar-btn-count\s*\{[^}]*position:\s*absolute/,
    "toolbar-btn-count inside .icon-link should be absolutely positioned"
  );
});
