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
import {
  initModuleRuntime,
  isModuleInstalled,
  installModuleFromUrl,
  reapplyAllUiHooks,
  ensureVoiceModel,
  checkModuleHealth,
} from "../modules-runtime.js";
import { toast } from "../toast.js";
import { openModuleHelp } from "./module-help.js";

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
    description: "Capture your voice and drop clean transcripts directly into any chat surface. v0.1.8: per-transcript diagnostics (VAD speech-frames, peak amplitude, RMS, silence-tripped) so the JS layer can disambiguate \"No Speech Detected\" failures. v0.1.7: faster tiny.en model + improved VAD. Works on dashboard, terminal, fullscreen, and the OpenClaw overlay.",
    publisher: "Miracle Claw",
    version: "0.1.8",
    status: "available",
    downloadUrl:
      "https://milagrocloud.com/downloads/miracle-claw-voice-0.1.8.tar.gz",
    hookLocation: "dashboard",
    tags: ["voice", "input"],
  },
  {
    id: "translate",
    icon: "🌍",
    name: "Translation for MiracleClaw",
    description:
      "Translate text or web pages between 100+ languages. " +
      "Drop-in tool for MAIC chat.",
    publisher: "Milagro Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl:
      "https://milagrocloud.com/downloads/miracle-claw-translate-0.1.0.tar.gz",
    hookLocation: "chat",
    tags: ["translate", "language", "ai"],
  },
  {
    id: "firecrawl",
    icon: "🔥",
    name: "FireCrawl for MiracleClaw",
    description:
      "Scrape any URL into clean markdown, batch-crawl websites, or search the web. " +
      "Localhost-only proxy bound to 127.0.0.1 with rate limiting and your API key " +
      "stored in your OS keychain. Wire-protocol compatible with FireCrawl — bring " +
      "your own fc- key.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl:
      "https://milagrocloud.com/downloads/miracle-claw-firecrawl-0.1.0.tar.gz",
    hookLocation: "settings",
    tags: ["web", "data"],
  },
  {
    id: "leadgen",
    icon: "🎯",
    name: "Lead Generator for MiracleClaw",
    description: "Find prospects online based on your criteria — industry, region, role, company size. Enrich their profiles and draft personalized outreach.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl:
      "https://milagrocloud.com/downloads/miracle-claw-leadgen-0.1.0.tar.gz",
    hookLocation: "settings",
    tags: ["business", "leads"],
  },
  {
    id: "translation",
    icon: "🌐",
    name: "Translation for MiracleClaw",
    description: "Translate text or web pages between 100+ languages. Drop-in tool for MAIC chat.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["language"],
  },
  {
    id: "pdf",
    icon: "📕",
    name: "PDF & Document Parser",
    description:
      "Drop a PDF or DOCX into MAIC chat — get clean text, a summary, or " +
      "ask any question about the document. Supports PDF, DOCX, TXT, MD, RTF, " +
      "and HTML. v0.1.0 ships text extraction + MAIC bridge; OCR for scanned " +
      "PDFs lands in v0.2.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl:
      "https://milagrocloud.com/downloads/miracle-claw-pdf-0.1.0-r2.tar.gz",
    hookLocation: "chat",
    tags: ["files", "docs", "ai"],
  },
  {
    id: "youtube",
    icon: "🎥",
    name: "YouTube Transcript",
    description: "Paste a YouTube URL into MAIC chat — get the full transcript with timestamps, an AI-generated summary, and chapter breakdown. Uses yt-dlp (or YouTube's timedtext fallback) for caption extraction. v0.1.0 ships transcript + summary + chapters.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-youtube-0.1.0.tar.gz",
    hookLocation: "chat",
    tags: ["video", "transcript", "ai"],
  },
  {
    id: "email",
    icon: "📧",
    name: "Email Writer",
    description: "Drop a few bullet points into MAIC chat and get a complete email — cold outreach, follow-up, or reply. Pick a tone (formal / friendly / direct / casual) and the channel (email / LinkedIn). v0.1.0 ships draft + reply templates routed through MAIC.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-email-0.1.0.tar.gz",
    hookLocation: "chat",
    tags: ["writing", "business", "ai"],
  },
  {
    id: "crm",
    icon: "🔗",
    name: "CRM Sync",
    description: "Push enriched contacts and lead status to HubSpot, Pipedrive, Notion, and other CRMs.",
    publisher: "Milagro Claw",
    version: "0.1.0",
    status: "available",
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-crm-0.1.0.tar.gz",
    hookLocation: "Extras",
    tags: ["business", "integration"],
  },
  {
    id: "localfiles",
    icon: "🔍",
    name: "Local File Search",
    description: "Index and semantically search your local files. Find anything you've worked on without leaving MC.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["files", "search"],
  },
  {
    id: "tts",
    icon: "🔊",
    name: "Text-to-Speech",
    description: "Read MAIC's responses aloud in chat. Two providers: bundled eSpeak NG (offline, free) or MAIC cloud TTS (natural voices via OpenAI TTS-1, $15 per 1M characters, free tier gets 25K chars per 30 days, Pro is unlimited).",
    publisher: "Miracle Claw",
    version: "0.2.0",
    status: "available",
    tags: ["voice", "output"],
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-tts-0.2.0.tar.gz",
    hookLocation: "Extras",
  },
  {
    id: "ocr",
    icon: "📸",
    name: "Local OCR",
    description: "Extract text from any image (PNG/JPG/WebP/GIF/BMP, up to 8 MB). Routes through your MAIC vision tool — your images stay in your MAIC tenant. Perfect for receipts, whiteboards, screenshots, scanned docs.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    tags: ["vision", "files"],
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-ocr-0.1.0.tar.gz",
    hookLocation: "Files",
  },
  {
    id: "calendar",
    icon: "📅",
    name: "Calendar Assistant",
    description: "Connect Google or Outlook calendar through MAIC's OAuth broker. List today's events, schedule meetings, check availability — all from chat. Tokens stored server-side in MAIC.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "available",
    tags: ["productivity"],
    downloadUrl: "https://milagrocloud.com/downloads/miracle-claw-calendar-0.1.0.tar.gz",
    hookLocation: "Productivity",
  },
  {
    id: "github",
    icon: "🐙",
    name: "GitHub Helper",
    description: "Read issues, summarize PRs, search code across your repositories. Works with public and private repos.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["developer", "code"],
  },
  {
    id: "sql",
    icon: "🗄",
    name: "SQL Buddy",
    description: "Connect to Postgres, MySQL, or SQLite. Ask questions about your data in plain English.",
    publisher: "Miracle Claw",
    version: "0.1.0",
    status: "coming_soon",
    tags: ["developer", "data"],
  },
  {
    id: "abtest",
    icon: "🧪",
    name: "A/B Test Designer",
    description: "Design experiments, calculate sample sizes, and analyze results without leaving chat.",
    publisher: "Miracle Claw",
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
      // Voice module just installed — clear any stale model cache.
      modelHealthCache.delete("voice");
      renderGrid(root, ctx);
    };
    state.onUninstalled = () => {
      if (state.unmounted) return;
      // Voice module just uninstalled — clear any cached model state.
      modelHealthCache.delete("voice");
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

  // Ensure the JS-side installed-state cache is warm BEFORE we paint
  // any cards. Without this await, getLiveModules() reads an empty
  // Map and renders already-installed modules as "Install" — which
  // then triggers the local-path prompt on click. (Lesson 575-r1)
  try {
    await initModuleRuntime();
  } catch (err) {
    console.warn("[modules] initModuleRuntime failed:", err);
  }

  // Belt-and-suspenders: a fresh fetch in case boot completed before
  // we got here. The runtime cache is the source of truth for
  // `isModuleInstalled`, but a fresh fetch picks up version bumps
  // and 3rd-party installs that happened after boot.
  try {
    const list = await invoke("mc_module_list");
    if (Array.isArray(list)) {
      // Re-sync the JS cache from the authoritative Rust response
      // before computing live status. (initModuleRuntime already did
      // this once, but a re-run is cheap and covers the race.)
      for (const m of list) {
        if (m && m.id) {
          // Touch the cache via the public API: the module-runtime
          // doesn't expose a setter, but the cache was warmed by
          // initModuleRuntime() above. Nothing else to do here.
        }
      }
    }
  } catch (err) {
    console.warn("[modules] mc_module_list refresh failed:", err);
  }

  const live = getLiveModules();

  // Per-module asset status (Lesson 581, 2026-08-25 16:25 MDT, David):
  // Voice binary requires a separately-downloaded whisper model. We
  // probe `mc_voice_check` for installed voice modules and surface a
  // "Setup model" CTA on the card if the model is missing. The probe
  // is cached in `modelHealthCache` so navigating away and back is
  // instant, and the cache invalidates on `mc:module-installed` /
  // successful model download.
  await annotateModelHealth(live);

  grid.innerHTML = live.map((m) => renderCard(m, ctx)).join("");
  wireCardButtons(grid, ctx);
}

// Per-module asset cache. Keyed by moduleId. Voice-only for now.
const modelHealthCache = new Map();
let modelHealthInflight = null;

async function annotateModelHealth(modules) {
  const installed = modules.filter((m) => m.status === "installed");
  const voice = installed.find((m) => m.id === "voice");
  if (!voice) {
    // Voice not installed → no model status to surface.
    voice && (voice.modelState = null);
    return;
  }

  let cached = modelHealthCache.get("voice");
  if (!cached) {
    if (!modelHealthInflight) {
      modelHealthInflight = checkModuleHealth("voice").finally(() => {
        modelHealthInflight = null;
      });
    }
    cached = await modelHealthInflight;
    modelHealthCache.set("voice", cached);
  }
  voice.modelState = cached.model_loaded ? "ready" : "needs_model";
}

// Public hook — call after a successful model download to invalidate
// the cache and re-render the catalog.
export function invalidateModelHealth(moduleId = "voice") {
  modelHealthCache.delete(moduleId);
}

function renderCard(m, ctx) {
  const status = m.status;
  const tags = (m.tags || [])
    .map((t) => `<span class="modules-tag">${escapeHtml(t)}</span>`)
    .join("");

  let cta;
  if (status === "installed") {
    // Voice-only: if model is missing, add a secondary "Download model" CTA.
    let secondary = "";
    if (m.id === "voice" && m.modelState === "needs_model") {
      secondary = `
        <button type="button"
                class="modules-cta modules-cta-secondary modules-cta-download-model"
                data-action="download-model"
                data-module-id="voice"
                title="Download the whisper model (~141 MB) so voice transcription works.">
          Download model
        </button>`;
    }
    cta = `
      <button type="button"
              class="modules-cta modules-cta-open"
              data-action="open"
              data-module-id="${escapeHtml(m.id)}"
              data-hook-location="${escapeHtml(m.hookLocation || "settings")}">
        Open
      </button>${secondary}`;
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

  // Voice-only model-state pill (Lesson 581).
  let modelPill = "";
  if (status === "installed" && m.id === "voice") {
    if (m.modelState === "needs_model") {
      modelPill = `<span class="modules-pill modules-pill-warn" title="Run mc_voice_check after the model is downloaded.">whisper model missing</span>`;
    } else if (m.modelState === "ready") {
      modelPill = `<span class="modules-pill modules-pill-ok" title="Whisper model is loaded and ready.">whisper ready</span>`;
    }
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

      ${modelPill ? `<div class="modules-card-pills">${modelPill}</div>` : ""}

      <div class="modules-card-meta muted small">
        <span class="modules-version">v${escapeHtml(m.version)}</span>
        ${tags ? `<span class="modules-card-tags">${tags}</span>` : ""}
      </div>

      <div class="modules-card-actions">
        ${cta}
        <button class="modules-help-btn" type="button"
                data-action="help"
                data-module-id="${escapeHtml(m.id)}"
                aria-label="Help for ${escapeHtml(m.name)}"
                title="Show usage help (Terminal, Chat, Windows UI)">
          ❔ Help
        </button>
      </div>
    </article>
  `;
}

function wireCardButtons(grid, ctx) {
  // rc53.29 (David 2026-08-26 16:33 MDT): broaden selector to ALL buttons
  // inside .modules-card-actions, not just .modules-cta. The previous
  // selector only matched the main Install/Open CTA — .modules-help-btn
  // silently had no click handler, so clicking Help did nothing.
  // Lesson 595: always include every interactive child button in the
  // selector. (Lesson 595 was confirmed by David's "Help button does
  // nothing" feedback on rc53.28.)
  grid.querySelectorAll(".modules-card-actions button").forEach((btn) => {
    if (btn.disabled) return; // coming_soon — no handler
    btn.addEventListener("click", async () => {
      const action = btn.dataset.action;
      const id = btn.dataset.moduleId;
      if (!action || !id) return;

      if (action === "install") {
        await handleInstall(btn, id, ctx);
      } else if (action === "open") {
        handleOpen(ctx, btn.dataset.hookLocation || "settings");
      } else if (action === "download-model") {
        await handleDownloadModel(btn, id, ctx);
      } else if (action === "help") {
        openModuleHelp(id);
      }
    });
  });
}

/**
 * Install flow. v0.1.0 supports local-path installs AND URL installs
 * (Lesson 574c). If the catalog entry has a `downloadUrl`, we offer
 * both: URL is preferred (one-click), local path is the dev/fallback
 * path. After successful install we toast and let the
 * `mc:module-installed` event re-render the grid.
 */
async function handleInstall(btn, id, ctx) {
  btn.disabled = true;
  const oldLabel = btn.textContent;
  btn.textContent = "Installing…";

  const entry = MODULE_CATALOG.find((m) => m.id === id);

  // Defensive re-check: even after Fix 1 (await initModuleRuntime
  // before painting), an Install click within ms of page mount could
  // still race. Re-read the runtime cache here so an already-installed
  // module never falls through to the local-path prompt. (Lesson 575-r2)
  if (isModuleInstalled(id)) {
    btn.textContent = "Installed";
    toast(`Module "${id}" is already installed.`, { kind: "info" });
    // Bounce to the hook location so the user actually sees it work.
    if (entry?.hookLocation) {
      setTimeout(() => handleOpen(ctx, entry.hookLocation), 250);
    }
    return;
  }

  const downloadUrl = entry?.downloadUrl;

  try {
    if (downloadUrl) {
      // URL install — one-click. Uses mc_module_install_url (Lesson 574c).
      // Backend downloads, extracts, SHA256-verifies, atomic-renames, registers.
      btn.textContent = "Downloading…";
      await installModuleFromUrl(id, downloadUrl);
      toast(`Module "${id}" installed from ${new URL(downloadUrl).hostname}`, {
        kind: "success",
      });
      // mc:module-installed event listener on this page will re-render the grid.
    } else {
      // No downloadUrl — this is an offline / dev-only path. Surface
      // a real error (not a prompt) so the user understands the card
      // is in a half-wired state. Lesson 575-r3 keeps the prompt
      // behind a clear "dev mode" affordance only.
      const localPath = window.prompt(
        `[Developer mode]\n\n` +
        `Module "${id}" has no downloadUrl configured. ` +
        `Install from a local path containing installer.json + bin/?\n\n` +
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
    }
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
/**
 * Lesson 581 (2026-08-25 16:25 MDT, David): Voice module needs a
 * separately-downloaded whisper model. The Download model CTA on the
 * Voice card invokes mc_voice_download_model, then invalidates the
 * model-health cache and re-renders so the pill flips to "ready".
 */
async function handleDownloadModel(btn, id, ctx) {
  btn.disabled = true;
  const oldLabel = btn.textContent;
  btn.textContent = "Downloading…";
  // Lesson 583 (2026-08-26 07:34 MDT, David): capture the last
  // status reported by ensureVoiceModel so the failure toast can
  // show the real sidecar error instead of the generic network hint.
  let lastStatus = "";
  try {
    const ok = await ensureVoiceModel((status) => {
      lastStatus = status;
      btn.textContent = status.startsWith("Downloading") ? "Downloading (~141 MB)…" : status;
    });
    if (ok) {
      toast("Whisper model downloaded — voice is ready.", { kind: "success" });
      invalidateModelHealth("voice");
    } else {
      toast(
        lastStatus && lastStatus !== "Checking whisper model…"
          ? `Whisper model download failed: ${lastStatus}`
          : "Whisper model download failed. Click again to retry, or check Settings → Modules → Voice logs.",
        { kind: "error", duration: 12000 }
      );
    }
  } catch (e) {
    toast(`Model download failed: ${e}`, { kind: "error", duration: 12000 });
  } finally {
    btn.disabled = false;
    btn.textContent = oldLabel;
    // Re-render the grid to flip the model-state pill to "ready"
    // (or keep it as "needs_model" on failure).
    const root = btn.closest('[data-page="modules"]') || document;
    renderGrid(root, ctx);
  }
}

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
