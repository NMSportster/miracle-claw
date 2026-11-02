// Headless DOM harness for the Miracle Claw Terminal toolbar's Modules
// menu + Run modal, and the Dashboard launchpad's Run modal.
//
// Lesson 1146 (2026-11-02, David): Lesson 245e's rule -- test the
// SHIPPED code path, not a re-implementation -- applies here too. The
// OpenClaw chat toolbar got its own jsdom harness in Lesson 1144; the
// Terminal and the launchpad had NO DOM coverage at all, which is why
// the Terminal's missing tool picker (only `tools[0]` of Calendar's 15
// was reachable) survived for so long.
//
// This harness:
//   * boots a jsdom window with the real DOM globals
//   * installs a fake `window.__TAURI_INTERNALS__.invoke` so
//     `mc_run_module` / `mc_list_installed_modules` are observable
//   * imports the REAL src/modules-menu.js and src/pages/dashboard_launchpad.js
//   * drives the actual dropdown -> modal -> tool-picker -> form -> Run
//     flow and captures the IPC payloads
//
// The production module manifests are reused from the OpenClaw harness so
// all three surfaces are tested against the SAME fixtures.

import { JSDOM } from 'jsdom';
import { LIVE_MANIFESTS } from './mc_toolbar_dom_harness.mjs';

export { LIVE_MANIFESTS };

/**
 * Define a global, tolerating getter-only builtins (Node 22 makes
 * `navigator` and `location` getter-only).
 */
function defineGlobal(key, value) {
  try {
    Object.defineProperty(globalThis, key, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  } catch {
    /* leave the builtin alone */
  }
}

/**
 * Install the browser globals the MC source modules expect.
 *
 * The source modules import from `@tauri-apps/api/core`, which is a
 * thin wrapper over `window.__TAURI_INTERNALS__.invoke`. We stub the
 * INTERNALS object rather than the npm package so the real import
 * path is exercised.
 */
function installTauriStub(dom, opts) {
  const calls = { runs: [], lists: [], other: [] };

  const invoke = async (cmd, args) => {
    if (cmd === 'mc_run_module') {
      calls.runs.push({ cmd, ...args });
      const key = `${args && args.moduleId}/${args && args.tool}`;
      const resp = (opts.runResponses || {})[key];
      if (resp instanceof Error) throw resp;
      if (resp !== undefined) return resp;
      return { outputs: 'ok', duration_ms: 1, run_id: 'test-run' };
    }
    if (cmd === 'mc_list_installed_modules') {
      calls.lists.push({ cmd, ...args });
      return (
        opts.listResponse || {
          modules: opts.modules || Object.values(LIVE_MANIFESTS),
        }
      );
    }
    calls.other.push({ cmd, ...args });
    if (opts.otherResponses && opts.otherResponses[cmd] !== undefined) {
      const r = opts.otherResponses[cmd];
      if (r instanceof Error) throw r;
      return r;
    }
    return null;
  };

  dom.window.__TAURI_INTERNALS__ = { invoke };
  // Some Tauri plugin shims read this.
  dom.window.__TAURI__ = { invoke };
  return calls;
}

/**
 * Mount the Terminal's Modules menu against a real jsdom document.
 *
 * Returns helpers for driving the UI.
 */
export async function mountTerminalModules(opts = {}) {
  const dom = new JSDOM(
    `<!doctype html><html><body>
      <div id="app">
        <button id="mods-btn">Modules</button>
      </div>
    </body></html>`,
    { url: 'http://localhost/', pretendToBeVisual: true }
  );

  const { window } = dom;
  // Expose the jsdom globals the MC source expects.
  const saved = {};
  const globals = [
    'window', 'document', 'navigator', 'HTMLElement', 'Element', 'Node',
    'CustomEvent', 'Event', 'MouseEvent', 'KeyboardEvent', 'FormData',
    'CSS', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame',
    'localStorage', 'location', 'history',
  ];
  for (const key of globals) {
    saved[key] = Object.getOwnPropertyDescriptor(globalThis, key);
    if (key === 'window') {
      defineGlobal('window', window);
    } else if (key === 'document') {
      defineGlobal('document', window.document);
    } else if (window[key] !== undefined) {
      defineGlobal(key, window[key]);
    }
  }
  if (!globalThis.CSS) defineGlobal('CSS', window.CSS);

  const calls = installTauriStub(dom, opts);

  // The module under test.
  const mod = await import('./../modules-menu.js');

  const button = window.document.getElementById('mods-btn');
  const modules = opts.modules || Object.values(LIVE_MANIFESTS);

  await mod.mountModulesMenu(button, {
    surface: 'terminal',
    getModules: async () => modules,
  });

  return {
    dom,
    window,
    document: window.document,
    calls,
    mod,

    /** Wait until a predicate is true (or give up). */
    async waitFor(pred, label = 'condition', tries = 60) {
      for (let i = 0; i < tries; i++) {
        if (pred()) return true;
        await new Promise((r) => setTimeout(r, 5));
      }
      throw new Error(`timed out waiting for ${label}`);
    },

    /** Open the Modules dropdown and wait for it to be populated. */
    async openMenu() {
      button.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      // The menu is built from an async module lookup, so wait for the
      // rows to actually appear rather than guessing at tick counts.
      await this.waitFor(
        () => window.document.querySelectorAll('.mc-modules-menu-item').length > 0,
        'modules menu rows'
      );
      return window.document.querySelector('.mc-modules-menu');
    },

    /** Click a module row by visible name. */
    clickModule(name) {
      const rows = [...window.document.querySelectorAll('.mc-modules-menu-item')];
      const row = rows.find((r) =>
        (r.querySelector('.mc-modules-menu-name')?.textContent || '').includes(name)
      );
      if (!row) return false;
      row.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      return true;
    },

    /** Click the ❔ help chip on a module row. */
    clickHelp(name) {
      const rows = [...window.document.querySelectorAll('.mc-modules-menu-row')];
      const row = rows.find((r) =>
        (r.querySelector('.mc-modules-menu-name')?.textContent || '').includes(name)
      );
      const btn = row?.querySelector('.mc-modules-menu-help');
      if (!btn) return false;
      btn.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      return true;
    },

    /** The open module modal (or null). */
    modal() {
      return window.document.querySelector('.mc-modal-panel');
    },

    /** The open help overlay (or null). */
    helpOverlay() {
      return window.document.querySelector('.mhelp-overlay');
    },

    /** Click a tool in the tool picker by name. */
    pickTool(toolName) {
      const row = window.document.querySelector(
        `.mc-modal-tool-row[data-tool-name="${toolName}"]`
      );
      if (!row) return false;
      row.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      return true;
    },

    /** The names of the tools offered by the picker. */
    pickerTools() {
      return [...window.document.querySelectorAll('.mc-modal-tool-row')].map((r) =>
        r.dataset.toolName
      );
    },

    /** Set a form field's value (and fire input/change). */
    fill(name, value) {
      const el = window.document.querySelector(
        `form.mc-modal-form [name="${name}"]`
      );
      if (!el) return false;
      el.value = value;
      el.dispatchEvent(new window.Event('input', { bubbles: true }));
      el.dispatchEvent(new window.Event('change', { bubbles: true }));
      return true;
    },

    /** Submit the run form and wait for the result. */
    async submit() {
      const form = window.document.querySelector('form.mc-modal-form');
      if (!form) return false;
      form.dispatchEvent(
        new window.Event('submit', { bubbles: true, cancelable: true })
      );
      // Let the invoke promise settle.
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      return true;
    },

    /** The text of the result panel. */
    resultText() {
      return window.document.querySelector('.mc-modal-result')?.textContent || '';
    },

    /** The last run IPC payload. */
    lastRun() {
      return calls.runs[calls.runs.length - 1];
    },

    /**
     * Tear down this mount.
     *
     * IMPORTANT: src/modules-menu.js schedules a `setTimeout(..., 0)`
     * that installs document/window listeners AFTER the dropdown click
     * resolves. If we restore the globals synchronously, that timer
     * fires into a bare Node global and throws
     * "ReferenceError: document is not defined" -- which surfaces as
     * an unrelated failure in the NEXT test (the module list comes
     * back empty because the remote-list promise rejected).
     *
     * So: wait for the timers to drain while the globals are still
     * installed, THEN restore.
     */
    async cleanup() {
      await new Promise((r) => setTimeout(r, 30));
      await new Promise((r) => setTimeout(r, 30));
      for (const [k, desc] of Object.entries(saved)) {
        if (desc === undefined) delete globalThis[k];
        else Object.defineProperty(globalThis, k, desc);
      }
      window.close();
    },
  };
}

/**
 * Mount the launchpad's Run modal.
 *
 * The launchpad's openModuleModal is not exported, so we drive it the
 * way the app does: via the "use-tool" delegated click on a tool card.
 */
export async function mountLaunchpad(opts = {}) {
  const dom = new JSDOM(
    `<!doctype html><html><body>
      <div id="app"></div>
    </body></html>`,
    { url: 'http://localhost/', pretendToBeVisual: true }
  );

  const { window } = dom;
  const saved = {};
  const globals = [
    'window', 'document', 'navigator', 'HTMLElement', 'Element', 'Node',
    'CustomEvent', 'Event', 'MouseEvent', 'KeyboardEvent', 'FormData',
    'CSS', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame',
    'localStorage', 'location', 'history',
  ];
  for (const key of globals) {
    saved[key] = Object.getOwnPropertyDescriptor(globalThis, key);
    if (key === 'window') {
      defineGlobal('window', window);
    } else if (key === 'document') {
      defineGlobal('document', window.document);
    } else if (window[key] !== undefined) {
      defineGlobal(key, window[key]);
    }
  }
  if (!globalThis.CSS) defineGlobal('CSS', window.CSS);

  const calls = installTauriStub(dom, opts);
  const mod = await import('./dashboard_launchpad.js');

  return {
    dom,
    window,
    document: window.document,
    calls,
    mod,

    /**
     * Open the launchpad's Run modal for a module + optional tool.
     *
     * Mirrors the "use-tool" delegated click in the page: the click
     * bubbles to the page root, the handler calls
     * `mc_list_installed_modules`, finds the module, and opens the
     * modal with the requested tool.
     */
    async openRunModal(moduleId, toolName) {
      const root = window.document.getElementById('app');
      const modules = opts.modules || Object.values(LIVE_MANIFESTS);

      // Mount the page for real so the delegated root listener is
      // installed. `mount` re-renders root.innerHTML asynchronously,
      // so we await it and only THEN inject the card the user clicks.
      await mod.dashboardLaunchpadPage.mount(root, {
        user: { email: 'test@example.com' },
      });
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));

      // Use the REAL rendered "Use" button for this module/tool --
      // the launchpad already renders one per tool, and clicking a
      // synthetic duplicate would target whichever card happened to
      // come first in the DOM.
      const wanted = toolName || null;
      const btn = [...root.querySelectorAll('[data-action="use-tool"]')].find((b) => {
        const card = b.closest('[data-module-id]');
        if (!card || card.dataset.moduleId !== moduleId) return false;
        if (!wanted) return true;
        return b.dataset.toolName === wanted;
      });
      if (!btn) return null;
      btn.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      // The handler awaits an async invoke() before opening the modal.
      for (let i = 0; i < 8; i++) {
        await new Promise((r) => setTimeout(r, 0));
        if (window.document.querySelector('.mc-modal-panel')) break;
      }
      // Make sure the module list the handler asks for contains it.
      calls.lists.length = 0;
      return window.document.querySelector('.mc-modal-panel');
    },

    /** Wait until a predicate is true (or give up). */
    async waitFor(pred, label = 'condition', tries = 60) {
      for (let i = 0; i < tries; i++) {
        if (pred()) return true;
        await new Promise((r) => setTimeout(r, 5));
      }
      throw new Error(`timed out waiting for ${label}`);
    },

    modal() {
      return window.document.querySelector('.mc-modal-panel');
    },

    helpOverlay() {
      return window.document.querySelector('.mhelp-overlay');
    },

    /** Click the modal's ❔ Help button. */
    clickHelp() {
      const btn = window.document.querySelector('[data-action="module-help"]');
      if (!btn) return false;
      btn.dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      );
      return true;
    },

    fill(name, value) {
      const el = window.document.querySelector(
        `form.mc-modal-form [name="${name}"]`
      );
      if (!el) return false;
      el.value = value;
      el.dispatchEvent(new window.Event('input', { bubbles: true }));
      el.dispatchEvent(new window.Event('change', { bubbles: true }));
      return true;
    },

    async submit() {
      const form = window.document.querySelector('form.mc-modal-form');
      if (!form) return false;
      form.dispatchEvent(
        new window.Event('submit', { bubbles: true, cancelable: true })
      );
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      await new Promise((r) => setTimeout(r, 0));
      return true;
    },

    resultText() {
      return window.document.querySelector('.mc-modal-result')?.textContent || '';
    },

    lastRun() {
      return calls.runs[calls.runs.length - 1];
    },

    /**
     * Tear down this mount.
     *
     * IMPORTANT: src/modules-menu.js schedules a `setTimeout(..., 0)`
     * that installs document/window listeners AFTER the dropdown click
     * resolves. If we restore the globals synchronously, that timer
     * fires into a bare Node global and throws
     * "ReferenceError: document is not defined" -- which surfaces as
     * an unrelated failure in the NEXT test (the module list comes
     * back empty because the remote-list promise rejected).
     *
     * So: wait for the timers to drain while the globals are still
     * installed, THEN restore.
     */
    async cleanup() {
      await new Promise((r) => setTimeout(r, 30));
      await new Promise((r) => setTimeout(r, 30));
      for (const [k, desc] of Object.entries(saved)) {
        if (desc === undefined) delete globalThis[k];
        else Object.defineProperty(globalThis, k, desc);
      }
      window.close();
    },
  };
}
