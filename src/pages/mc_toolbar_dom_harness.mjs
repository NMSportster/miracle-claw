// Headless DOM harness for the OpenClaw chat toolbar's Modules menu.
//
// This is NOT a re-implementation: it loads the REAL shipped patch
// (`depot/openclaw-patches/dist/control-ui/mc-toolbar.js`) into a jsdom
// window, stubs only the Tauri IPC boundary, and drives the actual UI
// (open dropdown -> pick module -> pick tool -> fill form -> Run).
//
// Lesson 245e: test the SHIPPED code path, not a re-implementation.
//
// Usage:
//   node src/pages/mc_toolbar_dom_harness.mjs            # run scenarios
//   node src/pages/mc_toolbar_dom_harness.mjs --dump     # print DOM state

import { JSDOM } from 'jsdom';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(HERE, '..', '..');
export const TOOLBAR_SRC = resolve(
  REPO_ROOT,
  'depot/openclaw-patches/dist/control-ui/mc-toolbar.js'
);

// The 9 live MAIC modules, with the exact client_manifest shape the
// production DB returns (module_id + client_manifest.tools[].form_schema).
export const LIVE_MANIFESTS = {
  'ad-performance': {
    module_id: 'ad-performance',
    name: 'Ad Performance',
    description: 'Analyze ad campaign metrics and find underperformers',
    icon: '📊',
    category: 'analytics',
    subcategory: 'ads',
    client_manifest: {
      module_id: 'ad-performance',
      name: 'Ad Performance',
      description: 'Summarize ad campaign performance.',
      icon: '📊',
      category: 'analytics',
      subcategory: 'ads',
      tier_required: 'free',
      instructions: 'Give a date range like "last 7 days" and optionally a campaign ID.',
      tools: [
        {
          name: 'summarize',
          description: 'Summarize ad performance for a date range.',
          form_schema: {
            type: 'object',
            required: ['date_range'],
            properties: {
              date_range: { type: 'string', title: 'Date range', minLength: 1 },
              campaign_id: { type: 'string', title: 'Campaign ID (optional)' },
            },
          },
        },
      ],
    },
  },
  'brand-asset-kit': {
    module_id: 'brand-asset-kit',
    name: 'Brand Asset Kit',
    description: 'Generate on-brand social graphics and bios',
    icon: '🎨',
    category: 'content',
    subcategory: 'brand',
    client_manifest: {
      module_id: 'brand-asset-kit',
      name: 'Brand Asset Kit',
      description: 'Generate brand bios.',
      icon: '🎨',
      category: 'content',
      subcategory: 'brand',
      tier_required: 'free',
      instructions: 'Pick a platform and give keywords to anchor the bio.',
      tools: [
        {
          name: 'bio',
          description: 'Write a short bio for a brand.',
          form_schema: {
            type: 'object',
            required: ['platform'],
            properties: {
              platform: {
                enum: ['twitter', 'linkedin', 'instagram', 'website'],
                type: 'string',
                title: 'Platform',
              },
              keywords: { type: 'string', title: 'Keywords (comma-separated)' },
            },
          },
        },
      ],
    },
  },
  calendar: {
    module_id: 'calendar',
    name: 'Miracle Claw Calendar',
    description: 'Your calendar, on every device. Recurring events, categories, reminders, a free-slot finder, .ics import, and an extractor that reads appointments out of pasted messages.',
    icon: '📅',
    category: 'productivity',
    subcategory: 'calendar',
    client_manifest: {
      module_id: 'calendar',
      name: 'Miracle Claw Calendar',
      description: 'Full calendar control.',
      icon: '📅',
      category: 'productivity',
      subcategory: 'calendar',
      tier_required: 'free',
      instructions: 'Times are ISO 8601. Default timezone is America/Denver.',
      tools: [
        {
          name: 'search_events',
          description: 'Search events by free-text query.',
          form_schema: {
            type: 'object',
            required: ['q'],
            properties: {
              q: { type: 'string', title: 'Search for' },
              limit: { type: 'integer', title: 'How many results', default: 25 },
            },
          },
        },
        {
          name: 'list_events',
          description: 'List events in a window.',
          form_schema: {
            type: 'object',
            required: ['window_start', 'window_end'],
            properties: {
              window_start: { type: 'string', title: 'From' },
              window_end: { type: 'string', title: 'To' },
              calendar_id: { type: 'integer', title: 'Calendar (optional)' },
            },
          },
        },
        {
          name: 'upcoming',
          description: 'List upcoming events.',
          form_schema: {
            type: 'object',
            properties: {
              days: { type: 'integer', title: 'How many days ahead', default: 7 },
              limit: { type: 'integer', title: 'How many to show', default: 10 },
            },
          },
        },
      ],
    },
  },
  'calendar-companion': {
    module_id: 'calendar-companion',
    name: 'Calendar Companion',
    description: 'Smart scheduling with conflict detection',
    icon: '🗓️',
    category: 'productivity',
    subcategory: 'calendar',
    client_manifest: {
      module_id: 'calendar-companion',
      name: 'Calendar Companion',
      description: 'Find a meeting slot.',
      icon: '🗓️',
      category: 'productivity',
      subcategory: 'calendar',
      tier_required: 'free',
      instructions: 'Give the duration in minutes and optional attendees.',
      tools: [
        {
          name: 'find_slot',
          description: 'Find an open meeting slot.',
          form_schema: {
            type: 'object',
            required: ['duration_minutes'],
            properties: {
              duration_minutes: {
                type: 'integer',
                title: 'Duration (minutes)',
                default: 30,
                maximum: 480,
                minimum: 15,
              },
              attendees: { type: 'string', title: 'Attendees (comma-separated emails)' },
            },
          },
        },
      ],
    },
  },
  'data-analyzer': {
    module_id: 'data-analyzer',
    name: 'Data Analyzer',
    description: 'CSV/spreadsheet analysis with plain-English summaries',
    icon: '📈',
    category: 'analytics',
    subcategory: 'data',
    client_manifest: {
      module_id: 'data-analyzer',
      name: 'Data Analyzer',
      description: 'Summarize a CSV.',
      icon: '📈',
      category: 'analytics',
      subcategory: 'data',
      tier_required: 'free',
      instructions: 'Point at a public CSV URL. Optionally say what to focus on.',
      tools: [
        {
          name: 'summarize',
          description: 'Summarize a CSV from a URL.',
          form_schema: {
            type: 'object',
            required: ['csv_url'],
            properties: {
              csv_url: { type: 'string', title: 'CSV URL', minLength: 1 },
              focus: { type: 'string', title: 'What to focus on' },
            },
          },
        },
      ],
    },
  },
  'email-campaigns-pro': {
    module_id: 'email-campaigns-pro',
    name: 'Email Campaigns Pro',
    description: 'Sequenced email drips with A/B subject lines',
    icon: '✉️',
    category: 'content',
    subcategory: 'email',
    client_manifest: {
      module_id: 'email-campaigns-pro',
      name: 'Email Campaigns Pro',
      description: 'Draft an email campaign sequence.',
      icon: '✉️',
      category: 'content',
      subcategory: 'email',
      tier_required: 'free',
      instructions: 'Describe the campaign goal and the audience.',
      tools: [
        {
          name: 'sequence',
          description: 'Draft a multi-email campaign sequence.',
          form_schema: {
            type: 'object',
            required: ['goal'],
            properties: {
              goal: { type: 'string', title: 'Goal', minLength: 1 },
              audience: { type: 'string', title: 'Audience' },
            },
          },
        },
      ],
    },
  },
  firecrawl: {
    module_id: 'firecrawl',
    name: 'Firecrawl',
    description: 'Pull clean content from any web page',
    icon: '🕷️',
    category: 'research',
    subcategory: 'web',
    client_manifest: {
      module_id: 'firecrawl',
      name: 'Firecrawl',
      description: 'Fetch and crawl web pages.',
      icon: '🕷️',
      category: 'research',
      subcategory: 'web',
      tier_required: 'free',
      instructions: 'Give a public http(s) URL. Crawls are limited to 100 pages.',
      tools: [
        {
          name: 'fetch_url',
          description: 'Fetch one public URL and return readable text.',
          form_schema: {
            type: 'object',
            required: ['url'],
            properties: {
              url: { type: 'string', title: 'URL', minLength: 1 },
            },
          },
        },
        {
          name: 'crawl_site',
          description: 'Crawl a site starting at a URL.',
          form_schema: {
            type: 'object',
            required: ['start_url'],
            properties: {
              start_url: { type: 'string', title: 'Start URL', minLength: 1 },
              max_pages: {
                type: 'integer',
                title: 'Max pages',
                default: 10,
                maximum: 100,
                minimum: 1,
              },
            },
          },
        },
      ],
    },
  },
  'marketing-suite': {
    module_id: 'marketing-suite',
    name: 'Marketing Suite',
    description: 'Email campaigns, social posts, and ad copy from a single brief',
    icon: '📣',
    category: 'content',
    subcategory: 'marketing',
    client_manifest: {
      module_id: 'marketing-suite',
      name: 'Marketing Suite',
      description: 'Draft emails and social posts.',
      icon: '📣',
      category: 'content',
      subcategory: 'marketing',
      tier_required: 'free',
      instructions: 'For social posts pick a platform. For emails give key points.',
      tools: [
        {
          name: 'draft_email',
          description: 'Draft an email to an audience.',
          form_schema: {
            type: 'object',
            required: ['to_audience', 'key_points'],
            properties: {
              to_audience: { type: 'string', title: 'To audience', minLength: 1 },
              key_points: { type: 'string', title: 'Key points', minLength: 1 },
              tone: {
                enum: ['friendly', 'professional', 'urgent', 'playful'],
                type: 'string',
                title: 'Tone',
              },
            },
          },
        },
        {
          name: 'social_post',
          description: 'Draft a social post.',
          form_schema: {
            type: 'object',
            required: ['platform', 'topic'],
            properties: {
              platform: {
                enum: ['twitter', 'linkedin', 'facebook', 'instagram'],
                type: 'string',
                title: 'Platform',
              },
              topic: { type: 'string', title: 'Topic', minLength: 1 },
            },
          },
        },
      ],
    },
  },
  'voice-memo': {
    module_id: 'voice-memo',
    name: 'Voice Memo',
    description: 'Transcribe audio memos and extract action items',
    icon: '🎙️',
    category: 'productivity',
    subcategory: 'audio',
    client_manifest: {
      module_id: 'voice-memo',
      name: 'Voice Memo',
      description: 'Transcribe an audio file.',
      icon: '🎙️',
      category: 'productivity',
      subcategory: 'audio',
      tier_required: 'free',
      instructions: 'Give a public URL to an audio file (m4a, mp3, wav).',
      tools: [
        {
          name: 'transcribe',
          description: 'Transcribe an audio file from a URL.',
          form_schema: {
            type: 'object',
            required: ['audio_url'],
            properties: {
              audio_url: { type: 'string', title: 'Audio file URL', minLength: 1 },
            },
          },
        },
      ],
    },
  },
};

export const LIVE_MODULE_IDS = Object.keys(LIVE_MANIFESTS);

/**
 * Mount the real toolbar in jsdom.
 *
 * @param {object} opts
 * @param {object} opts.modules  installed modules (remote MAIC shape)
 * @param {object} opts.runResponses  map of "module/tool" -> response or Error
 * @returns {object} handles for driving the UI
 */
export async function mountToolbar(opts = {}) {
  const modules = opts.modules ?? LIVE_MANIFESTS;
  const localModules = opts.localModules ?? [];
  const runResponses = opts.runResponses ?? {};
  const calls = { runs: [], invocations: [] };

  const dom = new JSDOM(
    `<!doctype html><html><body>
      <textarea id="chat-input"></textarea>
      <div id="host"></div>
    </body></html>`,
    {
      url: 'http://127.0.0.1:28789/',
      pretendToBeVisual: true,
      runScripts: 'outside-only',
    }
  );

  const { window } = dom;
  // jsdom has no layout -> offsetParent is always null. The toolbar's
  // pasteIntoChatInput uses offsetParent to find a visible input, so
  // give elements a truthy offsetParent.
  Object.defineProperty(window.HTMLElement.prototype, 'offsetParent', {
    get() {
      return this.parentNode;
    },
    configurable: true,
  });
  window.HTMLElement.prototype.scrollIntoView = function () {};
  if (!window.navigator.clipboard) {
    Object.defineProperty(window.navigator, 'clipboard', {
      value: { writeText: async () => {} },
      configurable: true,
    });
  }

  window.__TAURI_INTERNALS__ = {
    async invoke(cmd, args) {
      calls.invocations.push({ cmd, args });
      if (cmd === 'mc_list_installed_modules') {
        return { modules: Object.values(modules) };
      }
      if (cmd === 'mc_module_list') {
        return localModules;
      }
      if (cmd === 'mc_run_module') {
        calls.runs.push(args);
        const key = `${args.moduleId}/${args.tool}`;
        const r = runResponses[key];
        if (r instanceof Error) throw r;
        if (r !== undefined) return r;
        return { outputs: { ok: true, echoed: args.inputs }, duration_ms: 1 };
      }
      if (cmd === 'mc_open_overlay') return null;
      if (cmd === 'mc_voice_native_capture') return { text: '' };
      return null;
    },
  };

  const src = await readFile(TOOLBAR_SRC, 'utf8');
  window.eval(src);
  // The toolbar defers mount() to setTimeout(..., 0) when the document
  // is already parsed. Wait for that tick (and the badge refresh).
  await new Promise((r) => setTimeout(r, 25));

  return {
    dom,
    window,
    document: window.document,
    calls,
    /** Open the Modules dropdown and wait for it to populate. */
    async openModules() {
      const btn = window.document.getElementById('mc-oc-modules-btn');
      if (!btn) throw new Error('modules button not mounted');
      btn.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
      // Let the async module fetch settle.
      for (let i = 0; i < 50; i++) {
        await new Promise((r) => setTimeout(r, 0));
        const body = window.document.querySelector('#mc-oc-modules-menu .mc-oc-dropdown-body');
        if (body && !/Loading/.test(body.textContent)) break;
      }
      return window.document.querySelector('#mc-oc-modules-menu .mc-oc-dropdown-body');
    },
    /** Click the module card whose visible title matches `name`. */
    clickModuleCard(name) {
      const rows = window.document.querySelectorAll('#mc-oc-modules-menu .mc-oc-module-row, #mc-oc-modules-menu .mc-oc-dropdown-row, #mc-oc-modules-menu button');
      for (const r of rows) {
        if ((r.textContent || '').includes(name)) {
          r.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
          return true;
        }
      }
      return false;
    },
    /** Click a modal row whose text matches `label`. */
    clickModalRow(label) {
      const rows = window.document.querySelectorAll('.mc-oc-modal-row');
      for (const r of rows) {
        if ((r.textContent || '').includes(label)) {
          r.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
          return true;
        }
      }
      return false;
    },
    /**
     * Click the per-module "❔" help button in the Modules dropdown for
     * the module whose card title matches `name`.
     *
     * The help button lives INSIDE the module card button, so we have to
     * dispatch on it directly -- a click on the card itself would run
     * the module instead of opening help.
     */
    clickHelpButton(name) {
      const cards = window.document.querySelectorAll(
        '#mc-oc-modules-menu .mc-oc-dropdown-item[data-module-id]'
      );
      for (const card of cards) {
        const title = card.querySelector('.mc-oc-dropdown-item-name');
        if (!title || !(title.textContent || '').includes(name)) continue;
        const btn = card.querySelector('.mc-oc-dropdown-item-help');
        if (!btn) return false;
        btn.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true }));
        return true;
      }
      return false;
    },
    /** Help overlay panel, if one is open. */
    helpPanel() {
      return window.document.querySelector('.mc-oc-help-panel');
    },
    /** Visible module name for a given MAIC module id (from the manifests). */
    moduleNameFor(id) {
      const m = LIVE_MANIFESTS[id];
      if (!m) return null;
      const cm = m.client_manifest || m;
      return cm.name || m.name || id;
    },
    /** Current modal panel (if any). */
    modal() {
      return window.document.querySelector('.mc-oc-modal-panel');
    },
    /** Fill a rendered form field by name. */
    fill(name, value) {
      const el = window.document.querySelector(`.mc-oc-modal-panel [name="${name}"]`);
      if (!el) return false;
      if (el.type === 'checkbox') {
        el.checked = !!value;
      } else {
        el.value = String(value);
      }
      return true;
    },
    /** Submit the run form and wait for the result to settle. */
    async submit() {
      const form = window.document.querySelector('.mc-oc-modal-panel form');
      if (!form) throw new Error('no run form mounted');
      form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }));
      for (let i = 0; i < 80; i++) {
        await new Promise((r) => setTimeout(r, 1));
        const res = window.document.querySelector('.mc-oc-modal-result');
        if (res && !/^Running/.test(res.textContent || '')) break;
      }
      return window.document.querySelector('.mc-oc-modal-result');
    },
    resultText() {
      const res = window.document.querySelector('.mc-oc-modal-result');
      return res ? res.textContent : null;
    },
  };
}

if (process.argv[1] && process.argv[1].endsWith('mc_toolbar_dom_harness.mjs')) {
  const h = await mountToolbar();
  const body = await h.openModules();
  console.log('--- dropdown body ---');
  console.log(body ? body.textContent.replace(/\s+/g, ' ').trim() : '(none)');
  console.log('\n--- buttons in dropdown ---');
  for (const b of h.document.querySelectorAll('#mc-oc-modules-menu button')) {
    console.log(' *', JSON.stringify((b.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 120)));
  }
  console.log('\n--- toolbar buttons ---');
  for (const b of h.document.querySelectorAll('#mc-oc-toolbar button, #mc-oc-toolbar a')) {
    console.log(' *', JSON.stringify((b.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 120)), 'id=', b.id, 'title=', b.title);
  }
}
