// Regression tests for the chat-toolbar module HELP surface.
//
// Lesson 1144 (2026-11-02, David): the chat toolbar had NO module-level
// Help affordance at all. The Modules PAGE card has a "❔ Help"
// button, but a user working in the chat window had no way to read a
// module's instructions, examples, or troubleshooting notes without
// leaving the conversation.
//
// The canonical catalog (src/data/module-help.js) also covered an
// OBSOLETE local module set -- voice, translate, pdf, ocr, youtube,
// email, leadgen, crm, tts -- instead of the nine MAIC modules that
// actually ship:
//
//   ad-performance  brand-asset-kit  calendar
//   calendar-companion  data-analyzer  email-campaigns-pro
//   firecrawl  marketing-suite  voice-memo
//
// Seven of those nine had no help entry, so their Help button was dead.
//
// These tests drive the REAL shipped toolbar through jsdom.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { mountToolbar, LIVE_MANIFESTS } from './mc_toolbar_dom_harness.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '../..');

const LIVE_IDS = [
  'ad-performance',
  'brand-asset-kit',
  'calendar',
  'calendar-companion',
  'data-analyzer',
  'email-campaigns-pro',
  'firecrawl',
  'marketing-suite',
  'voice-memo',
];

// ---------------------------------------------------------------------------
// 1. Every live MAIC module appears with a ❔ help button.
// ---------------------------------------------------------------------------
test('every live MAIC module appears in the dropdown with a help button', async () => {
  const h = await mountToolbar({ modules: LIVE_MANIFESTS });
  await h.openModules();

  const cards = h.document.querySelectorAll(
    '#mc-oc-modules-menu .mc-oc-dropdown-item[data-module-id]'
  );
  assert.equal(cards.length, LIVE_IDS.length, 'all nine live modules should be listed');

  for (const id of LIVE_IDS) {
    const card = h.document.querySelector(
      `#mc-oc-modules-menu .mc-oc-dropdown-item[data-module-id="${id}"]`
    );
    assert.ok(card, `card should exist for ${id}`);
    const help = card.querySelector('.mc-oc-dropdown-item-help');
    assert.ok(help, `card for ${id} should have a ❔ help button`);
    assert.match(
      help.getAttribute('aria-label') || '',
      /help/i,
      `help button for ${id} should be labelled for screen readers`
    );
  }
});

// ---------------------------------------------------------------------------
// 2. Clicking ❔ opens help and does NOT run the module.
// ---------------------------------------------------------------------------
test('clicking ❔ opens the help overlay without running the module', async () => {
  const h = await mountToolbar({ modules: LIVE_MANIFESTS });
  await h.openModules();

  const before = h.calls.runs.length;
  assert.ok(h.clickHelpButton('Miracle Claw Calendar'), 'help button should be clickable');

  const panel = h.helpPanel();
  assert.ok(panel, 'a help overlay should open');
  const txt = panel.textContent || '';
  assert.ok(txt.length > 120, `help overlay should be substantial (got ${txt.length} chars)`);
  assert.match(txt, /Troubleshooting/i, 'help should include troubleshooting');
  assert.match(txt, /Example/i, 'help should include examples');
  assert.match(txt, /calendar/i, 'help should mention the module');

  assert.equal(
    h.calls.runs.length,
    before,
    'clicking ❔ must not execute the module (it lives inside the card button)'
  );
});

// ---------------------------------------------------------------------------
// 3. Each of the nine live modules opens a populated help overlay.
// ---------------------------------------------------------------------------
test('all nine live modules open a populated help overlay', async () => {
  for (const id of LIVE_IDS) {
    const h = await mountToolbar({ modules: LIVE_MANIFESTS });
    await h.openModules();

    const manifest = LIVE_MANIFESTS[id];
    const name = manifest.client_manifest?.name || manifest.name || id;
    assert.ok(h.clickHelpButton(name), `help button should exist for ${id}`);

    const panel = h.helpPanel();
    assert.ok(panel, `help overlay should open for ${id}`);
    const txt = panel.textContent || '';
    assert.ok(txt.length > 80, `help overlay for ${id} should not be empty (got ${txt.length})`);
    assert.match(txt, /Troubleshooting/i, `help for ${id} should include troubleshooting`);
  }
});

// ---------------------------------------------------------------------------
// 4. The canonical catalog covers all nine live ids, completely.
// ---------------------------------------------------------------------------
test('src/data/module-help.js covers all nine live MAIC ids', async () => {
  const mod = await import('../data/module-help.js');
  for (const id of LIVE_IDS) {
    const help = mod.getModuleHelp(id);
    assert.ok(help, `help entry should exist for ${id}`);
    assert.ok(help.whoFor, `${id}.whoFor`);
    assert.ok(help.whyUseIt, `${id}.whyUseIt`);
    assert.ok(help.whatItDoes, `${id}.whatItDoes`);
    assert.ok(Array.isArray(help.examples) && help.examples.length, `${id}.examples`);
    assert.ok(
      Array.isArray(help.troubleshooting) && help.troubleshooting.length,
      `${id}.troubleshooting`
    );
    assert.ok(
      Array.isArray(help.windowsUI) && help.windowsUI.length,
      `${id}.windowsUI`
    );
  }
});

test('the legacy local module help entries are still reachable', async () => {
  // The old local add-ons are still in the runtime registry and show up
  // in the dropdown as offline copies. Their help must not have been
  // lost when the MAIC catalog was added.
  const mod = await import('../data/module-help.js');
  for (const id of ['voice', 'translate', 'pdf', 'ocr', 'youtube', 'email', 'leadgen', 'crm', 'tts']) {
    assert.ok(mod.getModuleHelp(id), `legacy help entry should still exist for ${id}`);
  }
});

// ---------------------------------------------------------------------------
// 5. The toolbar's embedded copy is in sync with the canonical source.
// ---------------------------------------------------------------------------
test('toolbar embedded help is in sync with the canonical source', async () => {
  const run = promisify(execFile);
  const { stdout, stderr } = await run(
    'node',
    ['scripts/sync-toolbar-module-help.mjs', '--check'],
    { cwd: REPO_ROOT }
  );
  assert.match(stdout + stderr, /current/i);
});

test('toolbar embeds help for all nine live module ids', async () => {
  const path = resolve(
    REPO_ROOT,
    'depot/openclaw-patches/dist/control-ui/mc-toolbar.js'
  );
  const src = await readFile(path, 'utf8');
  const start = src.indexOf('var TOOLBAR_MODULE_HELP');
  const end = src.indexOf('// <<< END GENERATED: MODULE-HELP');
  assert.ok(start > -1 && end > start, 'toolbar should contain the generated help block');

  const block = src.slice(start, end);
  for (const id of LIVE_IDS) {
    assert.ok(block.includes(`"${id}"`), `toolbar should embed help for ${id}`);
  }
});

test('the generated help block is regenerated from source, not hand-edited', async () => {
  const path = resolve(
    REPO_ROOT,
    'depot/openclaw-patches/dist/control-ui/mc-toolbar.js'
  );
  const src = await readFile(path, 'utf8');
  assert.match(
    src,
    /GENERATED by scripts\/sync-toolbar-module-help\.mjs — DO NOT EDIT BY HAND/,
    'the help block should be marked as generated'
  );
});

// ---------------------------------------------------------------------------
// 6. Local-only modules (no tools) still get help and degrade gracefully.
// ---------------------------------------------------------------------------
test('a local-only module with no tools opens info, not a broken run form', async () => {
  const localModules = [
    {
      id: 'crm',
      name: 'CRM',
      version: '0.3.1',
      icon: '📇',
      description: 'Customer records from the shop',
    },
  ];
  const h = await mountToolbar({ modules: {}, localModules });
  await h.openModules();

  const card = h.document.querySelector(
    '#mc-oc-modules-menu .mc-oc-dropdown-item[data-module-id="crm"]'
  );
  assert.ok(card, 'local-only module should appear in the dropdown');

  card.dispatchEvent(new h.window.MouseEvent('click', { bubbles: true }));
  const panel = h.document.querySelector('.mc-oc-modal-panel');
  assert.ok(panel, 'clicking a no-tools module should open a modal');
  assert.match(
    panel.textContent || '',
    /does not expose client-side tools/i,
    'should show the info modal, not a run form'
  );
  assert.ok(
    !h.document.querySelector('.mc-oc-modal-panel form'),
    'should not render a run form for a module with no tools'
  );
});

test('a local-only module still gets a help overlay', async () => {
  const localModules = [
    { id: 'crm', name: 'CRM', version: '0.3.1', icon: '📇', description: 'Customer records' },
  ];
  const h = await mountToolbar({ modules: {}, localModules });
  await h.openModules();
  assert.ok(h.clickHelpButton('CRM'), 'help button should exist');
  assert.ok(h.helpPanel(), 'help overlay should open for a local-only module');
});

// ---------------------------------------------------------------------------
// 7. A module with no help content and no instructions still opens a
//    non-empty overlay -- never a blank dialog.
// ---------------------------------------------------------------------------
test('a module with no help content still opens a populated overlay', async () => {
  const h = await mountToolbar({
    modules: {
      mystery: {
        module_id: 'mystery',
        name: 'Mystery Module',
        version: '1.0.0',
        icon: '🧿',
        description: 'A module with no help content at all',
        client_manifest: {
          name: 'Mystery Module',
          version: '1.0.0',
          tools: [],
        },
      },
    },
  });
  await h.openModules();
  assert.ok(h.clickHelpButton('Mystery Module'), 'help button should exist');
  const panel = h.helpPanel();
  assert.ok(panel, 'help overlay should open');
  assert.ok(
    (panel.textContent || '').length > 20,
    'overlay should not be blank even with no help content'
  );
});
