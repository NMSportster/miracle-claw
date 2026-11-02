// Regression coverage for the Terminal module toolbar and the
// Dashboard launchpad Run modal.
//
// Lesson 1146 (2026-11-02, David): the OpenClaw chat toolbar got
// jsdom coverage in Lesson 1144. The Terminal and the launchpad had
// NONE -- which is exactly why these bugs survived:
//
//   * The Terminal's `openModuleModal()` hardcoded `tools[0]`, so
//     Calendar's 15 tools were reduced to 1 (search_events). 14 tools
//     were unreachable from the Terminal.
//   * Neither surface had a module-level Help affordance wired to the
//     canonical catalog in src/data/module-help.js.
//   * The Terminal's error mapper knew only 401/429/network; the
//     launchpad's knew 401/403/404/409/422/429/502/503/504 but NOT
//     501 -- so `calendar-companion.find_slot` (a deliberate
//     COMING_SOON) rendered as a bare "MAIC returned 501."
//   * Both surfaces had a latent bug where an optional blank INTEGER
//     field was sent as 0 instead of being omitted.
//
// These tests drive the REAL source modules through a jsdom DOM, the
// same way src/pages/mc_toolbar_dom_harness.mjs does for the shipped
// OpenClaw patch. No re-implementation.
//
// Run via: `npm run test:js`.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  mountTerminalModules,
  mountLaunchpad,
  LIVE_MANIFESTS,
} from './mc_toolbar_surfaces_harness.mjs';

const ALL_MODULES = Object.values(LIVE_MANIFESTS);
const settle = (ms = 0) => new Promise((r) => setTimeout(r, ms));

// =========================================================================
// Terminal: the tool picker
// =========================================================================

test("Terminal dropdown opens and lists every live MAIC module", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    const ids = [
      ...h.document.querySelectorAll('.mc-modules-menu-item'),
    ].map((n) => n.dataset.moduleId);
    for (const id of Object.keys(LIVE_MANIFESTS)) {
      assert.ok(ids.includes(id), `missing module ${id}`);
    }
  } finally {
    await h.cleanup();
  }
});

test("Terminal offers a tool picker instead of silently using tools[0]", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);

    const offered = h.pickerTools();
    assert.ok(
      offered.length > 1,
      `Calendar should offer multiple tools, got ${JSON.stringify(offered)}`
    );
    assert.ok(offered.includes('search_events'));
    assert.ok(offered.includes('list_events'));
  } finally {
    await h.cleanup();
  }
});

test("Terminal tool picker swaps the form and submits the CHOSEN tool", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);

    // The default is the first tool.
    assert.equal(
      h.document.querySelector('form.mc-modal-form')?.dataset.tool,
      'search_events'
    );

    // Pick a different one.
    assert.ok(h.pickTool('list_events'));
    await settle(10);
    assert.equal(
      h.document.querySelector('form.mc-modal-form')?.dataset.tool,
      'list_events'
    );

    // The fields changed to match the new tool's schema.
    const names = [
      ...h.document.querySelectorAll('form.mc-modal-form [name]'),
    ].map((e) => e.name);
    assert.ok(names.includes('window_start'));
    assert.ok(names.includes('window_end'));
    assert.ok(!names.includes('q'), 'search_events field leaked into list_events');

    h.fill('window_start', '2026-11-01');
    h.fill('window_end', '2026-11-30');
    await h.submit();

    const run = h.lastRun();
    assert.equal(run.moduleId, 'calendar');
    assert.equal(run.tool, 'list_events');
    assert.equal(run.inputs.window_start, '2026-11-01');
    assert.equal(run.inputs.window_end, '2026-11-30');
  } finally {
    await h.cleanup();
  }
});

test("Terminal never submits tool: undefined", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);
    h.fill('q', 'dentist');
    await h.submit();
    const run = h.lastRun();
    assert.ok(run, 'no run was issued');
    assert.equal(typeof run.tool, 'string');
    assert.ok(run.tool.length > 0);
  } finally {
    await h.cleanup();
  }
});

test("Terminal omits blank optional INTEGER fields instead of sending 0", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);
    h.pickTool('list_events');
    await settle(10);
    h.fill('window_start', '2026-11-01');
    h.fill('window_end', '2026-11-30');
    // calendar_id is an OPTIONAL integer -- leave it blank.
    await h.submit();

    const run = h.lastRun();
    assert.ok(
      !('calendar_id' in run.inputs),
      `blank optional integer must be omitted, got ${JSON.stringify(run.inputs)}`
    );
  } finally {
    await h.cleanup();
  }
});

test("Terminal keeps a filled optional INTEGER field as a number", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);
    h.pickTool('list_events');
    await settle(10);
    h.fill('window_start', '2026-11-01');
    h.fill('window_end', '2026-11-30');
    h.fill('calendar_id', '7');
    await h.submit();

    const run = h.lastRun();
    assert.strictEqual(run.inputs.calendar_id, 7);
    assert.equal(typeof run.inputs.calendar_id, 'number');
  } finally {
    await h.cleanup();
  }
});

// =========================================================================
// Terminal: module-level Help
// =========================================================================

test("Terminal dropdown renders a Help chip for every live module", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    const rows = [...h.document.querySelectorAll('.mc-modules-menu-row')];
    assert.equal(rows.length, Object.keys(LIVE_MANIFESTS).length);
    for (const row of rows) {
      assert.ok(
        row.querySelector('.mc-modules-menu-help'),
        `missing Help chip for ${row.dataset.moduleId}`
      );
    }
  } finally {
    await h.cleanup();
  }
});

test("Terminal Help chip opens the canonical help overlay and does NOT run the module", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    assert.ok(h.clickHelp('Miracle Claw Calendar'));
    await settle(30);

    const overlay = h.helpOverlay();
    assert.ok(overlay, 'no help overlay opened');
    assert.equal(h.calls.runs.length, 0, 'Help must not execute the module');

    const text = overlay.textContent || '';
    // Canonical catalog content for the live Calendar module.
    assert.match(text, /calendar/i);
  } finally {
    await h.cleanup();
  }
});

test("Terminal Help chip is a separate control, not the run button", async () => {
  const h = await mountTerminalModules({ modules: ALL_MODULES });
  try {
    await h.openMenu();
    // A nested button inside a button is invalid HTML and swallows
    // clicks in some engines -- the row must not be a <button> with a
    // <button> inside it.
    for (const row of h.document.querySelectorAll('.mc-modules-menu-row')) {
      assert.equal(row.tagName, 'DIV', 'menu row should be a div, not a button');
      assert.equal(
        row.querySelectorAll('button button').length,
        0,
        'no nested buttons allowed'
      );
    }
  } finally {
    await h.cleanup();
  }
});

// =========================================================================
// Terminal: error mapping
// =========================================================================

async function runTerminalExpectingError(errorObj) {
  const h = await mountTerminalModules({
    modules: ALL_MODULES,
    runResponses: {
      'calendar/search_events': errorObj,
    },
  });
  await h.openMenu();
  h.clickModule('Miracle Claw Calendar');
  await settle(30);
  h.fill('q', 'dentist');
  await h.submit();
  await settle(10);
  const text = h.resultText();
  await h.cleanup();
  return text;
}

test("Terminal 501 COMING_SOON is explained, not shown as a bare code", async () => {
  const text = await runTerminalExpectingError(
    new Error(
      'HTTP 501: {"detail":"This module is in private beta. Join the waitlist at https://miracleclaw.com/waitlist"}'
    )
  );
  assert.match(text, /not available yet/i);
  assert.match(text, /private beta/i);
  assert.doesNotMatch(text, /MAIC returned 501/);
});

test("Terminal 422 explains that the values were rejected", async () => {
  const text = await runTerminalExpectingError(
    new Error('HTTP 422: {"detail":"q is required"}')
  );
  assert.match(text, /invalid input/i);
  // The server's own detail is surfaced so the user knows WHAT to fix.
  assert.match(text, /q is required/);
});

test("Terminal 404 says the module isn't installed", async () => {
  const text = await runTerminalExpectingError(
    new Error('HTTP 404: {"detail":"module \'ghost\' not found"}')
  );
  assert.match(text, /isn't installed/i);
});

test("Terminal keeps the raw error behind a Show details disclosure", async () => {
  const h = await mountTerminalModules({
    modules: ALL_MODULES,
    runResponses: {
      'calendar/search_events': new Error('HTTP 500: <html>boom</html>'),
    },
  });
  try {
    await h.openMenu();
    h.clickModule('Miracle Claw Calendar');
    await settle(30);
    h.fill('q', 'dentist');
    await h.submit();
    await settle(10);

    const details = h.document.querySelector('.mc-modal-result-detail');
    assert.ok(details, 'expected a <details> block');
    assert.match(details.textContent || '', /boom/);
  } finally {
    await h.cleanup();
  }
});

// =========================================================================
// Launchpad
// =========================================================================

test("Launchpad Run modal submits the requested tool", async () => {
  const h = await mountLaunchpad({ modules: ALL_MODULES });
  try {
    await h.openRunModal('calendar', 'list_events');
    const modal = h.modal();
    assert.ok(modal, 'launchpad Run modal did not open');
    assert.equal(
      h.document.querySelector('form.mc-modal-form')?.dataset.tool,
      'list_events'
    );

    h.fill('window_start', '2026-11-01');
    h.fill('window_end', '2026-11-30');
    await h.submit();

    const run = h.lastRun();
    assert.ok(run, 'no run issued');
    assert.equal(run.moduleId, 'calendar');
    assert.equal(run.tool, 'list_events');
    assert.ok(
      !('calendar_id' in run.inputs),
      'blank optional integer must be omitted'
    );
  } finally {
    await h.cleanup();
  }
});

test("Launchpad Run modal has a module-level Help button", async () => {
  const h = await mountLaunchpad({ modules: ALL_MODULES });
  try {
    await h.openRunModal('calendar', 'search_events');
    assert.ok(h.clickHelp(), 'no Help button in the launchpad Run modal');
    await settle(30);
    assert.ok(h.helpOverlay(), 'Help overlay did not open');
    assert.equal(h.calls.runs.length, 0, 'Help must not execute the module');
  } finally {
    await h.cleanup();
  }
});

test("Launchpad 501 COMING_SOON is explained", async () => {
  const h = await mountLaunchpad({
    modules: ALL_MODULES,
    runResponses: {
      'calendar-companion/find_slot': new Error(
        'HTTP 501: {"detail":"find_slot is coming soon"}'
      ),
    },
  });
  try {
    await h.openRunModal('calendar-companion', 'find_slot');
    await h.submit();
    await settle(10);
    const text = h.resultText();
    assert.match(text, /not available yet/i);
    assert.doesNotMatch(text, /MAIC returned 501/);
  } finally {
    await h.cleanup();
  }
});

test("Launchpad surfaces the server's validation detail", async () => {
  const h = await mountLaunchpad({
    modules: ALL_MODULES,
    runResponses: {
      'firecrawl/fetch_url': new Error(
        'HTTP 422: {"detail":"url: required field missing"}'
      ),
    },
  });
  try {
    await h.openRunModal('firecrawl', 'fetch_url');
    // `url` is required, so fill it to get past the pre-submit
    // validity check and reach the server call.
    h.fill('url', 'https://example.com');
    await h.submit();
    await settle(10);
    const text = h.resultText();
    assert.match(text, /invalid input/i);
    assert.match(text, /required field missing/);
  } finally {
    await h.cleanup();
  }
});
