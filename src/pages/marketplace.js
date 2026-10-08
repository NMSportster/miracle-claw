// src/pages/marketplace.js — Module Marketplace (rc56.x)
//
// The live, server-driven catalog of MAIC modules. Distinct from `modules.js`
// (the static roadmap / discovery hub). The launchpad's "Add a tool" tile
// lands here.
//
// API surface (Tauri commands wired in src-tauri/src/lib.rs around line 5660):
//   - mc_list_categories()                      → public, no JWT
//   - mc_list_marketplace_modules({ category, sort })  → authed
//   - mc_list_installed_modules()               → authed
//   - mc_install_module({ moduleId })           → authed
//   - mc_uninstall_module({ moduleId })         → authed
//
// UI contract (mirrors modules.js):
//   - register("marketplace", marketplacePage)  in main.js
//   - installNavigation: ["marketplace", () => marketplaceCtx()]
//   - dashboard ctx: onOpenMarketplace: () => navigate("marketplace")
//   - page contract: mount(root, ctx), requiresAuth: true
//
// Filtering: a category sidebar (populated from /v1/categories) plus a
// sort dropdown (top_rated / recent / alphabetical) feed into the same
// list query. "All" + "Top rated" is the default view on first paint.

import { invoke } from "@tauri-apps/api/core";
import { toast } from "../toast.js";

// ============================================================================
// State
// ============================================================================

const state = {
  categories: [],
  modules: [],
  installedIds: new Set(),
  activeCategory: null,   // null = "All"
  activeSort: "top_rated",
  busy: false,
  unmounted: false,
};

// ============================================================================
// Utilities
// ============================================================================

function escapeHtml(s) {
  if (s === null || s === undefined) return "";
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function ratingStars(rating) {
  if (rating === null || rating === undefined) return "";
  const r = Number(rating);
  if (!Number.isFinite(r)) return "";
  const full = Math.floor(r);
  const half = r - full >= 0.5 ? 1 : 0;
  const empty = 5 - full - half;
  return "★".repeat(full) + (half ? "½" : "") + "☆".repeat(empty);
}

async function loadCategories() {
  try {
    const res = await invoke("mc_list_categories");
    // Shape: { categories: [{ id, name, count? }, ...] } or [...]
    const cats = Array.isArray(res) ? res : (res.categories || []);
    state.categories = cats.map((c) => ({
      id: c.id || c.slug || c.name,
      name: c.name || c.id || c.slug || "Unknown",
      count: c.count,
    }));
  } catch (err) {
    // Public endpoint — failure shouldn't block the page.
    console.warn("[marketplace] categories failed:", err);
    state.categories = [];
  }
}

async function loadInstalled() {
  try {
    const res = await invoke("mc_list_installed_modules");
    const mods = Array.isArray(res) ? res : (res.modules || res.installed || []);
    state.installedIds = new Set(mods.map((m) => m.module_id || m.id));
  } catch (err) {
    // Not logged in or transient failure — render the catalog anyway,
    // every Install call will surface its own 401 if needed.
    console.warn("[marketplace] installed failed:", err);
    state.installedIds = new Set();
  }
}

async function loadMarketplace() {
  let bundledNotice = null;
  try {
    const res = await invoke("mc_list_marketplace_modules", {
      category: state.activeCategory || null,
      sort: state.activeSort,
    });
    const mods = Array.isArray(res) ? res : (res.modules || res.items || []);
    state.modules = mods;
    if (res && res.source === "bundled" && res.message) {
      bundledNotice = res.message;
    }
  } catch (err) {
    state.modules = [];
    throw err;
  }
  return bundledNotice;
}

// ============================================================================
// Rendering
// ============================================================================

function renderCategorySidebar() {
  const items = [{ id: null, name: "All" }]
    .concat(state.categories)
    .map((c) => {
      const active = (state.activeCategory || null) === c.id ? " active" : "";
      const count = c.count !== undefined && c.count !== null
        ? `<span class="mc-mkt-count">${escapeHtml(c.count)}</span>`
        : "";
      return `
        <button class="mc-mkt-cat${active}" data-action="set-category" data-cat="${escapeHtml(c.id || "")}">
          <span>${escapeHtml(c.name)}</span>
          ${count}
        </button>
      `;
    })
    .join("");
  return `<nav class="mc-mkt-cats">${items}</nav>`;
}

function renderToolbar() {
  const sorts = [
    { id: "top_rated", label: "Top Rated" },
    { id: "recent", label: "Recently Added" },
    { id: "alphabetical", label: "A → Z" },
  ];
  const sortButtons = sorts.map((s) => {
    const active = state.activeSort === s.id ? " active" : "";
    return `<button class="mc-mkt-sort${active}" data-action="set-sort" data-sort="${s.id}">${s.label}</button>`;
  }).join("");
  return `<div class="mc-mkt-toolbar"><span class="mc-mkt-sort-label">Sort:</span>${sortButtons}</div>`;
}

function renderCard(m) {
  const id = m.module_id || m.id;
  const name = m.name || m.title || id;
  const desc = m.description || m.summary || "";
  // Lesson 1096 (2026-10-01, David): bundled catalog returns
  // `installed: bool` per entry; live MAIC catalog + registry
  // requires merging. We prefer the entry flag (truth from Rust)
  // over the separate installedIds set, which may be stale on
  // first paint.
  const installed = m.installed === true || state.installedIds.has(id);
  const publisher = m.publisher || m.author || "Unknown";
  // Lesson 1096: when installed, show the actually-installed
  // version (which may differ from the catalog's "newest").
  const version = installed && m.installed_version
    ? m.installed_version
    : (m.version || "");
  const category = m.category || (m.categories && m.categories[0]) || "";
  const rating = m.rating !== undefined ? m.rating : (m.avg_rating);
  const downloads = m.download_count !== undefined ? m.download_count : m.installs;
  const icon = m.icon || "🧩";
  const tags = Array.isArray(m.tags) ? m.tags : [];
  // Lesson 1096: subtitle goes under the name on the card; MAIC's
  // catalog didn't ship one but ours does and it's useful.
  const subtitle = m.subtitle || "";
  // rc57.9 (2026-10-02, David): mirror the launchpad card's expand-to-see
  // pattern on the marketplace so users can preview tools and read the
  // "How to use" hint BEFORE installing.
  const tools = (m.client_manifest && m.client_manifest.tools) || [];
  const instructions = (m.client_manifest && m.client_manifest.instructions) || "";

  const ratingLine = (rating !== null && rating !== undefined && Number.isFinite(Number(rating)))
    ? `<span class="mc-mkt-rating" title="${escapeHtml(rating)}">${ratingStars(rating)} <span class="muted">${escapeHtml(Number(rating).toFixed(1))}</span></span>`
    : "";

  const downloadsLine = (downloads !== null && downloads !== undefined)
    ? `<span class="mc-mkt-dl muted small">${escapeHtml(downloads)} installs</span>`
    : "";

  const tagsLine = tags.length
    ? `<div class="mc-mkt-tags">${tags.slice(0, 4).map((t) => `<span class="mc-mkt-tag">${escapeHtml(t)}</span>`).join("")}</div>`
    : "";

  const action = installed
    ? `<button class="mc-mkt-btn mc-mkt-btn-installed" data-action="uninstall" data-id="${escapeHtml(id)}">Installed ✓</button>`
    : `<button class="mc-mkt-btn mc-mkt-btn-install" data-action="install" data-id="${escapeHtml(id)}">Install</button>`;

  const toolsLabel = `${tools.length} tool${tools.length === 1 ? "" : "s"}`;
  const toolRows = tools.map((t) => `
    <div class="mc-tool-row">
      <div class="mc-tool-row-info">
        <div class="mc-tool-row-name">${escapeHtml(t.name)}</div>
        <div class="mc-tool-row-desc">${escapeHtml(t.description || "")}</div>
      </div>
    </div>
  `).join("");

  return `
    <article class="modules-card modules-card-available mc-mkt-card mc-tool-card" data-id="${escapeHtml(id)}" data-expanded="false">
      <button class="modules-card-head mc-tool-card-head" data-action="toggle-mkt-card">
        <span class="modules-card-icon">${escapeHtml(icon)}</span>
        <span class="modules-card-titles mc-tool-card-title">
          <span class="modules-card-name mc-tool-name">${escapeHtml(name)}</span>
          ${subtitle ? `<span class="modules-card-subtitle muted small">${escapeHtml(subtitle)}</span>` : ""}
          <span class="modules-card-desc mc-tool-desc">${escapeHtml(desc)}</span>
        </span>
        <span class="mc-tool-card-meta">
          <span class="mc-tool-tools-count">${toolsLabel}</span>
          <span class="mc-tool-card-caret" aria-hidden="true">▾</span>
        </span>
      </button>
      <div class="mc-tool-card-body" hidden>
        ${publisher || version ? `<div class="muted small">by ${escapeHtml(publisher)}${version ? ` · v${escapeHtml(version)}` : ""}</div>` : ""}
        ${category ? `<div class="muted small">${escapeHtml(category)}</div>` : ""}
        ${ratingLine || downloadsLine ? `<div class="mc-mkt-card-meta">${ratingLine}${downloadsLine}</div>` : ""}
        ${tagsLine}
        ${tools.length ? `<div class="mc-tool-list">${toolRows}</div>` : ""}
        ${instructions ? `
          <details class="mc-tool-instructions">
            <summary>How to use ${escapeHtml(name)}</summary>
            <p>${escapeHtml(instructions)}</p>
          </details>
        ` : ""}
        <div class="modules-card-actions">${action}</div>
      </div>
    </article>
  `;
}

function renderGrid() {
  if (state.modules.length === 0) {
    return `
      <div class="mc-mkt-empty">
        <div class="mc-mkt-empty-emoji">🪹</div>
        <div class="mc-mkt-empty-title">No modules match your selection.</div>
        <div class="mc-mkt-empty-sub muted small">Try a different category or sort.</div>
      </div>
    `;
  }
  return `<div class="mc-mkt-grid">${state.modules.map(renderCard).join("")}</div>`;
}

function renderShell() {
  const noticeHtml = state.bundledNotice
    ? `<div class="mc-mkt-notice muted small" role="status" id="mc-mkt-notice">
        <span class="mc-mkt-notice-text">⚠️ ${escapeHtml(state.bundledNotice)}</span>
        <button class="mc-mkt-notice-dismiss" data-action="dismiss-notice" aria-label="Dismiss">×</button>
      </div>`
    : "";
  return `
    <div class="mc-mkt-page">
      <header class="mc-page-header">
        <button class="mc-back-btn" data-action="back">← Back</button>
        <h1>Marketplace</h1>
        <p class="muted small">Browse and install modules from the MAIC catalog.</p>
      </header>
      ${noticeHtml}
      <div class="mc-mkt-layout">
        <aside class="mc-mkt-sidebar">
          <h3 class="mc-mkt-sidebar-title">Categories</h3>
          ${renderCategorySidebar()}
        </aside>
        <main class="mc-mkt-main">
          ${renderToolbar()}
          <div class="mc-mkt-results" id="mc-mkt-results">
            ${renderGrid()}
          </div>
        </main>
      </div>
    </div>
  `;
}

// ============================================================================
// Actions
// ============================================================================

async function setCategory(root, cat) {
  state.activeCategory = cat || null;
  await refresh(root, { skipCategories: true });
}

async function setSort(root, sort) {
  state.activeSort = sort;
  await refresh(root, { skipCategories: true, skipInstalled: true });
}

async function installModule(root, moduleId) {
  if (state.busy) return;
  state.busy = true;
  try {
    const res = await invoke("mc_install_module", { moduleId });
    // Server returns { success, client_manifest } — we just track the id.
    state.installedIds.add(moduleId);
    toast(`Installed ${moduleId}.`);
    updateGrid(root);
  } catch (err) {
    toast(`Install failed: ${err}`);
  } finally {
    state.busy = false;
  }
}

async function uninstallModule(root, moduleId) {
  if (state.busy) return;
  state.busy = true;
  try {
    await invoke("mc_uninstall_module", { moduleId });
    state.installedIds.delete(moduleId);
    toast(`Uninstalled ${moduleId}.`);
    updateGrid(root);
  } catch (err) {
    toast(`Uninstall failed: ${err}`);
  } finally {
    state.busy = false;
  }
}

function updateGrid(root) {
  const gridHost = root.querySelector("#mc-mkt-results");
  if (gridHost) gridHost.innerHTML = renderGrid();
  // Also re-render toolbar so active sort state stays correct.
  const main = root.querySelector(".mc-mkt-main");
  if (main) {
    const toolbar = main.querySelector(".mc-mkt-toolbar");
    if (toolbar) toolbar.outerHTML = renderToolbar();
  }
}

async function refresh(root, opts = {}) {
  const resultsHost = root.querySelector("#mc-mkt-results");
  if (resultsHost) resultsHost.innerHTML = `<div class="mc-mkt-loading muted small">Loading…</div>`;
  if (!opts.skipCategories) await loadCategories();
  if (!opts.skipInstalled) await loadInstalled();
  let bundledNotice = null;
  try {
    bundledNotice = await loadMarketplace();
  } catch (err) {
    toast(`Marketplace load failed: ${err}`);
    state.modules = [];
  }
  state.bundledNotice = bundledNotice;
  if (state.unmounted) return;
  // Re-render shell so sidebar + grid reflect new state.
  root.innerHTML = renderShell();
}

function wireEvents(root, ctx) {
  root.addEventListener("click", (ev) => {
    const target = ev.target.closest("[data-action]");
    if (!target) return;
    const action = target.dataset.action;
    switch (action) {
      case "back":
        if (ctx && typeof ctx.onBackToDashboard === "function") ctx.onBackToDashboard();
        break;
      case "set-category":
        setCategory(root, target.dataset.cat || null);
        break;
      case "set-sort":
        setSort(root, target.dataset.sort);
        break;
      case "install":
        installModule(root, target.dataset.id);
        break;
      case "uninstall":
        uninstallModule(root, target.dataset.id);
        break;
      case "dismiss-notice":
        // Lesson 1109: dismiss the bundled-catalog warning for this
        // session. If it comes back next time the user reopens the
        // page AND MAIC is still down, that's a server-fix signal,
        // not a UI bug — but at least the page is usable today.
        state.bundledNotice = null;
        const noticeEl = root.querySelector("#mc-mkt-notice");
        if (noticeEl) noticeEl.remove();
        break;
      case "toggle-mkt-card": {
        // rc57.9: mirror the launchpad's expand-to-see pattern on
        // the marketplace so users can preview tools + instructions
        // before installing.
        const card = target.closest(".mc-mkt-card");
        if (!card) return;
        const body = card.querySelector(".mc-tool-card-body");
        if (!body) return;
        const isExpanded = card.dataset.expanded === "true";
        if (isExpanded) {
          body.hidden = true;
          card.dataset.expanded = "false";
        } else {
          body.hidden = false;
          card.dataset.expanded = "true";
        }
        break;
      }
      default:
        break;
    }
  });
}

// ============================================================================
// Page contract
// ============================================================================

async function mount(root, ctx) {
  state.unmounted = false;
  state.modules = [];
  state.categories = [];
  state.installedIds = new Set();
  state.activeCategory = null;
  state.activeSort = "top_rated";
  state.busy = false;

  root.innerHTML = renderShell();
  wireEvents(root, ctx || {});

  // Parallel initial fetch — categories are public, the rest are authed.
  await refresh(root);
}

function unmount() {
  state.unmounted = true;
}

export const marketplacePage = { mount, unmount, requiresAuth: true };