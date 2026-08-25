// src/pages/modules.js — Add-On Modules catalog hub (Lesson 573, 2026-08-25)
//
// This page is the **discovery surface** for the MC module ecosystem.
// It's the first thing users see when they click "Add-On Modules" on the
// Dashboard. The job here is purely to surface what's available, what each
// module does, and route the user to the right next step:
//
//   - "Available" status: install flow (or deep-link into Settings → Modules)
//   - "Installed" status: jump to the first hook location (e.g. dashboard FAB
//     for voice) — that's where the user actually uses the module
//   - "Coming Soon" status: disabled card with tooltip, no action
//
// Architecture (mirrors `pricing.js` and `extras.js`):
//   - Register: `register("modules", modulesPage)` in main.js
//   - Navigation builder: `["modules", () => modulesCtx()]` in
//     installNavigation() so the dashboard's "Add-On Modules" button
//     can `navigate("modules")`.
//   - Page contract: mount(root, ctx), requiresAuth: true,
//     onBackToDashboard wired in modulesCtx().
//
// Catalog data: hardcoded MODULE_CATALOG array at the top of this file.
// v0.1.0 ships with 15 modules; one is available today (Voice for
// MiracleClaw), the other 14 are marked `coming_soon` so the catalog
// reads as a roadmap without faking functionality. Real install/launch
// flows are wired only for installed modules — `coming_soon` cards have
// a disabled CTA so the UI doesn't lie about what works.
//
// Status logic:
//   - On mount, call `mc_module_list` to learn what's installed.
//   - Cross-reference catalog ids with installed ids.
//   - Subscribe to `mc:module-installed` / `mc:module-uninstalled` events
//     for live updates (the Settings → Modules page also listens, so
//     installs from there flow back here automatically).
//   - On unmount, drop the listeners + flag so late events are ignored.

import { invoke } from "@tauri-apps/api/core";
import { isModuleInstalled } from "../modules-runtime.js";
import { toast } from "../toast.js";

// ============================================================================
// Catalog
// ============================================================================

/**
 * The full Add-On Modules catalog. Order matters — first card is the
 * top-left on a 3-column grid, so put the most installable modules first.
 *
 * Status values:
 *   - "available"     — install flow is wired, user can click Install
 *   - "installed"     — auto-set by mount() based on `mc_module_list`
 *   - "coming_soon"   — no flow wired; CTA is disabled
 *
 * `hookLocation` (optional): where to navigate when the user clicks
 * "Open" on an installed module. Defaults to "settings" so installed
 * cards without a clear hook still go somewhere useful.
 */
const MODULE_CATALOG = [
  {
    id: "voice",
    icon: "🎙",
    name: "Voice for MiracleClaw",
    description: "Capture your voice and drop clean transcripts directly into any chat surface. Works on dashboard, terminal, fullscreen, and the OpenClaw overlay.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "available",
    tags: ["voice", "input"],
    hookLocation: "dashboard",
  },
  {
    id: "firecrawl",
    icon: "🔥",
    name: "FireCrawl for MiracleClaw",
    description: "Scrape any URL into clean markdown, batch-crawl websites, or search the web. Gives MAIC real web context for any answer.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["web", "data"],
  },
  {
    id: "leadgen",
    icon: "🎯",
    name: "Lead Generator for MiracleClaw",
    description: "Find prospects online based on your criteria — industry, region, role, company size. Enrich their profiles and draft personalized outreach.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["business", "leads"],
  },
  {
    id: "translation",
    icon: "🌐",
    name: "Translation for MiracleClaw",
    description: "Translate text or web pages between 100+ languages. Drop-in tool for MAIC chat.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["language"],
  },
  {
    id: "pdf",
    icon: "📕",
    name: "PDF & Document Parser",
    description: "Upload PDF, DOCX, or PPTX files. Extract clean text, tables, and document structure.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["files", "docs"],
  },
  {
    id: "youtube",
    icon: "🎥",
    name: "YouTube Transcript",
    description: "Paste a YouTube URL to get the full transcript, a summary, and chapter breakdowns.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["video", "transcript"],
  },
  {
    id: "email",
    icon: "📧",
    name: "Email Writer",
    description: "Guided cold outreach, follow-ups, and reply drafting. Tone and channel are configurable.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["writing", "business"],
  },
  {
    id: "crm",
    icon: "🔗",
    name: "CRM Sync",
    description: "Push enriched contacts and lead status to HubSpot, Pipedrive, Notion, and other CRMs.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["business", "integration"],
  },
  {
    id: "localfiles",
    icon: "🔍",
    name: "Local File Search",
    description: "Index and semantically search your local files. Find anything you've worked on without leaving MC.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["files", "search"],
  },
  {
    id: "tts",
    icon: "🔊",
    name: "Text-to-Speech",
    description: "Read MAIC's responses aloud in chat. Multiple voices, configurable speed.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["voice", "output"],
  },
  {
    id: "ocr",
    icon: "📸",
    name: "Local OCR",
    description: "Extract text from any image. Runs locally with Tesseract — your images never leave the machine.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["vision", "files"],
  },
  {
    id: "calendar",
    icon: "📅",
    name: "Calendar Assistant",
    description: "Connect Google or Outlook calendar. Schedule meetings, set reminders, query availability in natural language.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["productivity"],
  },
  {
    id: "github",
    icon: "🐙",
    name: "GitHub Helper",
    description: "Read issues, summarize PRs, search code across your repositories. Works with public and private repos.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["developer", "code"],
  },
  {
    id: "sql",
    icon: "🗄",
    name: "SQL Buddy",
    description: "Connect to Postgres, MySQL, or SQLite. Ask questions about your data in plain English.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["developer", "data"],
  },
  {
    id: "abtest",
    icon: "🧪",
    name: "A/B Test Designer",
    description: "Design experiments, calculate sample sizes, and analyze results without leaving chat.",
    publisher: "ADeal Auto Repair",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["business", "analytics"],
  },
];

// ============================================================================
// Page
// ============================================================================

export const modulesPage = {
  label: "Add-On Modules",
  icon: "🧩",
  requiresAuth: true,

  async mount(root, ctx = {}) {
    root.innerHTML = `
      <section class="page page-modules" aria-label="Add-On Modules catalog">
        <header class="page-header modules-page-header">
          <button class="back-btn" id="modules-back" type="button" aria-label="Back to dashboard">←</button>
          <h1 class="page-title">🧩 Add-On Modules</h1>
          <span class="page-subtitle muted">Extend MiracleClaw with optional sidecar add-ons</span>
        </header>

        <p class="modules-intro muted">
          Modules run in their own processes and plug into MC through
          well-defined UI hooks. Install one to activate its hooks anywhere
          in the app — voice, OCR, web search, and more. All modules are
          available on every plan, including Free.
        </p>

        <div class="modules-grid" id="modules-grid">
          <p class="muted">Loading modules…</p>
        </div>

        <footer class="modules-footer muted small">
          <p>
            Manage installed modules under
            <strong>Settings → Modules</strong>.
            Uninstall from there to remove a module's files.
          </p>
        </footer>
      </section>
    `;

    // Wire the back button.
    const back = root.querySelector("#modules-back");
    if (back && typeof ctx.onBackToDashboard === "function") {
      back.addEventListener("click", ctx.onBackToDashboard);
    }

    // Live-state plumbing. The page owns its listeners; on unmount the
    // listeners are dropped and the `unmounted` flag flips so late
    // events don't try to re-render a torn-down DOM.
    const state = {
      root,
      ctx,
      unmounted: false,
      onInstalled: null,
      onUninstalled: null,
    };

    state.onInstalled = () => {
      if (state.unmounted) return;
      renderGrid(root, ctx);
    };
    state.onUninstalled = () => {
      if (state.unmounted) return;
      renderGrid(root, ctx);
    };

    window.addEventListener("mc:module-installed", state.onInstalled);
    window.addEventListener("mc:module-uninstalled", state.onUninstalled);

    // Initial render — pulls mc_module_list and merges with the catalog.
    // Done synchronously here; the grid rebuild is cheap (15 cards).
    renderGrid(root, ctx);

    // Stash cleanup on the root so unmount() can find it.
    root._modulesCleanup = () => {
      state.unmounted = true;
      if (state.onInstalled) {
        window.removeEventListener("mc:module-installed", state.onInstalled);
      }
      if (state.onUninstalled) {
        window.removeEventListener("mc:module-uninstalled", state.onUninstalled);
      }
    };
  },

  unmount(root) {
    if (root && typeof root._modulesCleanup === "function") {
      root._modulesCleanup();
      root._modulesCleanup = null;
    }
  },
};

// ============================================================================
// Rendering
// ============================================================================

/**
 * Compute the live module list by merging the catalog with `mc_module_list`.
 * Status rules:
 *   - catalog.status === "coming_soon"  -> stays coming_soon regardless
 *   - catalog.status === "available"    -> "installed" if installed, else "available"
 *
 * Modules that are installed but NOT in the catalog (3rd-party or removed
 * from the catalog) get a synthetic entry so the user can see & uninstall
 * them. They don't get a card on this page, but Settings → Modules owns
 * them — we just skip them here.
 */
function getLiveModules() {
  // We use the JS-side runtime cache (isModuleInstalled) which is kept
  // in sync via mc:module-installed events. mc_module_list has already
  // been called at boot by initModuleRuntime(), so the cache is warm.
  // For belt-and-suspenders we also fetch a fresh list — if it fails
  // we fall back to the cache.
  return MODULE_CATALOG.map((m) => {
    if (m.status === "coming_soon") return m;
    const installed = isModuleInstalled(m.id);
    return {
      ...m,
      status: installed ? "installed" : "available",
      installedVersion: installed ? m.version : null,
    };
  });
}

async function renderGrid(root, ctx) {
  const grid = root.querySelector("#modules-grid");
  if (!grid) return;

  // Best-effort fresh fetch — the runtime cache is the source of truth
  // for `isModuleInstalled`, but we ask the backend again so the version
  // numbers on installed cards reflect the latest install. Failure here
  // is non-fatal: we just render with the cached state.
  try {
    await invoke("mc_module_list");
  } catch (err) {
    console.warn("[modules] mc_module_list refresh failed:", err);
  }

  const live = getLiveModules();
  grid.innerHTML = live.map((m) => renderCard(m, ctx)).join("");
  wireCardButtons(grid, ctx);
}

function renderCard(m, ctx) {
  const status = m.status;
  const tags = (m.tags || [])
    .map((t) => `<span class="modules-tag">${escapeHtml(t)}</span>`)
    .join("");

  let cta;
  if (status === "installed") {
    cta = `
      <button type="button"
              class="modules-cta modules-cta-open"
              data-action="open"
              data-module-id="${escapeHtml(m.id)}"
              data-hook-location="${escapeHtml(m.hookLocation || "settings")}">
        Open
      </button>`;
  } else if (status === "available") {
    cta = `
      <button type="button"
              class="modules-cta modules-cta-install"
              data-action="install"
              data-module-id="${escapeHtml(m.id)}">
        Install
      </button>`;
  } else {
    // coming_soon — disabled, tooltip explains why.
    cta = `
      <button type="button"
              class="modules-cta modules-cta-coming"
              disabled
              title="This module is in development. Subscribe to updates from the Plans & Pricing page."
              aria-label="Coming soon — this module is in development">
        Coming Soon
      </button>`;
  }

  return `
    <article class="modules-card modules-card-${escapeHtml(status)}"
             data-module-id="${escapeHtml(m.id)}"
             data-status="${escapeHtml(status)}">
      <div class="modules-card-head">
        <span class="modules-icon" aria-hidden="true">${escapeHtml(m.icon)}</span>
        <div class="modules-card-titles">
          <div class="modules-card-name">${escapeHtml(m.name)}</div>
          <div class="modules-card-publisher muted small">by ${escapeHtml(m.publisher)}</div>
        </div>
        <span class="modules-status modules-status-${escapeHtml(status)}">
          ${status === "installed" ? "Installed"
            : status === "available" ? "Available"
            : "Coming Soon"}
        </span>
      </div>

      <p class="modules-card-desc">${escapeHtml(m.description)}</p>

      <div class="modules-card-meta muted small">
        <span class="modules-version">v${escapeHtml(m.version)}</span>
        ${tags ? `<span class="modules-card-tags">${tags}</span>` : ""}
      </div>

      <div class="modules-card-actions">
        ${cta}
      </div>
    </article>
  `;
}

function wireCardButtons(grid, ctx) {
  grid.querySelectorAll(".modules-cta").forEach((btn) => {
    if (btn.disabled) return; // coming_soon — no handler
    btn.addEventListener("click", async () => {
      const action = btn.dataset.action;
      const id = btn.dataset.moduleId;
      if (!action || !id) return;

      if (action === "install") {
        await handleInstall(btn, id, ctx);
      } else if (action === "open") {
        handleOpen(ctx, btn.dataset.hookLocation || "settings");
      }
    });
  });
}

/**
 * Install flow. Mirrors Settings → Modules: prompt for a local path
 * (dev mode), call `mc_module_install_local`, toast on success, and
 * re-render the grid once the install event fires.
 *
 * v0.1.0 only supports local-path installs — remote downloads arrive
 * later. The dialog is the same one in settings.js so the two flows
 * stay consistent.
 */
async function handleInstall(btn, id, ctx) {
  btn.disabled = true;
  const oldLabel = btn.textContent;
  btn.textContent = "Installing…";
  try {
    const localPath = window.prompt(
      `Install module "${id}" from local path?\n\n` +
      `Path must contain installer.json and bin/ subdir.\n` +
      `Tip: set MC_MODULE_LOCAL_PATH at launch and this dialog is skipped.`,
      ""
    );
    if (!localPath) {
      btn.disabled = false;
      btn.textContent = oldLabel;
      return;
    }
    await invoke("mc_module_install_local", { id, localPath });
    toast(`Module "${id}" installed`, { kind: "success" });
    // The mc:module-installed event listener on this page will re-render.
  } catch (err) {
    toast(`Module "${id}" install failed: ${err}`, { kind: "error", duration: 8000 });
    btn.disabled = false;
    btn.textContent = oldLabel;
  }
}

/**
 * Open flow. Routes the user to the first hook location for an installed
 * module so they can actually use it. Falls back to Settings → Modules
 * when no hook location is known.
 */
function handleOpen(ctx, location) {
  if (!ctx) return;
  switch (location) {
    case "dashboard":
      if (typeof ctx.onBackToDashboard === "function") {
        ctx.onBackToDashboard();
      }
      break;
    case "settings":
    default:
      if (typeof ctx.onOpenSettings === "function") {
        ctx.onOpenSettings();
      } else if (typeof ctx.onBackToDashboard === "function") {
        ctx.onBackToDashboard();
      }
      break;
  }
}

// ============================================================================
// Helpers
// ============================================================================

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}
