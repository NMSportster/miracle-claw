// Lesson 982 (2026-09-11 21:00 MDT, David): regression test that pins
// the close-handler routing for the secrets + attach overlays in
// terminal.js. The user reported that clicking "Back" or "✕" on a
// locally-opened secrets/attach card bounced them to the Dashboard
// instead of keeping them on the Terminal page.
//
// Root cause: the previous close handlers always called
// `bridge.closeOverlay()` whenever the bridge existed, but the
// bridge exists on the main window REGARDLESS of how the overlay
// was opened. Rust's mc_close_overlay falls back to navigating the
// main window to the Dashboard when no `overlay_return_url` is
// stashed (i.e. the overlay was opened from a local toolbar click,
// not the chat bridge's auto-open path).
//
// Fix: track which opener fired with `secretsOpenedByBridge` /
// `attachOpenedByBridge` flags. Only call bridge.closeOverlay() in
// the close handler when the flag is true. Toolbar click → flag
// stays false → close just hides overlay locally.
//
// Pins:
//   1. openSecretsOverlay accepts an opts object with fromBridge
//   2. secretsOpenedByBridge flag exists
//   3. secretsOpenedByBridge is set from opts.fromBridge
//   4. closeSecretsOverlay only calls bridge.closeOverlay() when
//      secretsOpenedByBridge is true
//   5. closeSecretsOverlay resets secretsOpenedByBridge to false
//      after the call
//   6. local toolbar click passes fromBridge: false
//   7. auto-open path passes fromBridge: true
//   8. openAttachOverlay + attachOpenedByBridge mirror the pattern
//   9. closeAttachOverlay only calls bridge.closeOverlay() when
//      attachOpenedByBridge is true
//   10. local attach toolbar click passes fromBridge: false
//
// Run via: `node --test src/pages/terminal_overlay_close_routing.test.mjs`

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const terminalSrc = await readFile(
  new URL("./terminal.js", import.meta.url),
  "utf8"
);

test("terminal.js: openSecretsOverlay accepts fromBridge option", () => {
  assert.match(
    terminalSrc,
    /function\s+openSecretsOverlay\s*\(\s*\{\s*fromBridge\s*=\s*false\s*\}\s*=\s*\{\}\s*\)/,
    "openSecretsOverlay should accept { fromBridge = false } = {} opts"
  );
});

test("terminal.js: secretsOpenedByBridge flag declared", () => {
  assert.match(
    terminalSrc,
    /let\s+secretsOpenedByBridge\s*=\s*false\s*;/,
    "secretsOpenedByBridge flag should be declared"
  );
});

test("terminal.js: openSecretsOverlay sets secretsOpenedByBridge from opts", () => {
  // Inside the openSecretsOverlay function body, the flag should
  // be assigned from opts.fromBridge. Match the assignment line
  // (not the declaration line — the declaration uses `= false`).
  assert.match(
    terminalSrc,
    /secretsOpenedByBridge\s*=\s*fromBridge\s*;/,
    "secretsOpenedByBridge should be assigned from opts.fromBridge"
  );
});

test("terminal.js: closeSecretsOverlay only calls bridge.closeOverlay when flag is true", () => {
  // The close handler must guard the bridge call with `if (secretsOpenedByBridge) {`
  // — not call it unconditionally like the pre-Lesson-982 code did.
  // Find the closeSecretsOverlay function body and assert the guard.
  const closeFn = terminalSrc.match(
    /function\s+closeSecretsOverlay\s*\(\s*\)\s*\{([\s\S]*?)\n\s{4}\}\s*\n/
  );
  assert.ok(closeFn, "closeSecretsOverlay function not found");
  const body = closeFn[1];
  assert.match(
    body,
    /if\s*\(\s*secretsOpenedByBridge\s*\)/,
    "closeSecretsOverlay should guard bridge.closeOverlay with if (secretsOpenedByBridge)"
  );
  assert.match(
    body,
    /bridge\.closeOverlay\s*\(\s*\)/,
    "closeSecretsOverlay should still call bridge.closeOverlay (inside the guard)"
  );
});

test("terminal.js: closeSecretsOverlay resets secretsOpenedByBridge after guard", () => {
  // After the bridge call, the flag must be reset to false so a
  // subsequent local toolbar open doesn't inherit a stale true.
  // Look for: secretsOpenedByBridge = false; inside the if-block.
  const closeFn = terminalSrc.match(
    /function\s+closeSecretsOverlay\s*\(\s*\)\s*\{([\s\S]*?)\n\s{4}\}\s*\n/
  );
  assert.ok(closeFn, "closeSecretsOverlay function not found");
  assert.match(
    closeFn[1],
    /if\s*\(\s*secretsOpenedByBridge\s*\)\s*\{[\s\S]*?secretsOpenedByBridge\s*=\s*false\s*;/,
    "secretsOpenedByBridge should be reset to false inside the guard block"
  );
});

test("terminal.js: local secrets toolbar click passes fromBridge: false", () => {
  // The terminal-secrets-btn click handler should call openSecretsOverlay
  // with fromBridge: false (NOT just openSecretsOverlay() — that would
  // re-introduce the bug because default fromBridge is false but the
  // intent is explicit).
  assert.match(
    terminalSrc,
    /terminal-secrets-btn[\s\S]{0,200}?openSecretsOverlay\s*\(\s*\{\s*fromBridge\s*:\s*false\s*\}\s*\)/,
    "terminal-secrets-btn click should pass { fromBridge: false }"
  );
});

test("terminal.js: auto-open path passes fromBridge: true", () => {
  // The setTimeout in the auto-open block should call openSecretsOverlay
  // and openAttachOverlay with fromBridge: true (the chat bridge case).
  const autoOpenBlock = terminalSrc.match(
    /setTimeout\s*\(\s*\(\s*\)\s*=>\s*\{([\s\S]*?)\},\s*0\s*\)/
  );
  assert.ok(autoOpenBlock, "auto-open setTimeout block not found");
  assert.match(
    autoOpenBlock[1],
    /openSecretsOverlay\s*\(\s*\{\s*fromBridge\s*:\s*true\s*\}\s*\)/,
    "auto-open should call openSecretsOverlay with { fromBridge: true }"
  );
  assert.match(
    autoOpenBlock[1],
    /openAttachOverlay\s*\(\s*\{\s*fromBridge\s*:\s*true\s*\}\s*\)/,
    "auto-open should call openAttachOverlay with { fromBridge: true }"
  );
});

test("terminal.js: openAttachOverlay accepts fromBridge option", () => {
  assert.match(
    terminalSrc,
    /function\s+openAttachOverlay\s*\(\s*\{\s*fromBridge\s*=\s*false\s*\}\s*=\s*\{\}\s*\)/,
    "openAttachOverlay should accept { fromBridge = false } = {} opts"
  );
});

test("terminal.js: attachOpenedByBridge flag declared + used", () => {
  assert.match(
    terminalSrc,
    /let\s+attachOpenedByBridge\s*=\s*false\s*;/,
    "attachOpenedByBridge flag should be declared"
  );
  assert.match(
    terminalSrc,
    /attachOpenedByBridge\s*=\s*fromBridge\s*;/,
    "attachOpenedByBridge should be assigned from opts.fromBridge"
  );
  const closeFn = terminalSrc.match(
    /function\s+closeAttachOverlay\s*\(\s*\)\s*\{([\s\S]*?)\n\s{4}\}\s*\n/
  );
  assert.ok(closeFn, "closeAttachOverlay function not found");
  assert.match(
    closeFn[1],
    /if\s*\(\s*attachOpenedByBridge\s*\)/,
    "closeAttachOverlay should guard bridge.closeOverlay with if (attachOpenedByBridge)"
  );
  assert.match(
    closeFn[1],
    /bridge\.closeOverlay\s*\(\s*\)/,
    "closeAttachOverlay should still call bridge.closeOverlay (inside the guard)"
  );
});

test("terminal.js: local attach toolbar click passes fromBridge: false", () => {
  assert.match(
    terminalSrc,
    /terminal-attach-btn[\s\S]{0,200}?openAttachOverlay\s*\(\s*\{\s*fromBridge\s*:\s*false\s*\}\s*\)/,
    "terminal-attach-btn click should pass { fromBridge: false }"
  );
});
