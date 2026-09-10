// pages/dashboard_launchpad.js — rc56.0 launchpad landing page.
//
// Public API: { mount(root, ctx), unmount() }
//
// Render shape (5 sections, locked in mc-module-driven-surfaces-2026-09.md):
//   1. Topbar: 🦞 Miracle Claw | greeting | user menu | settings
//   2. Tier row: [Free ████░░░░ 12,400/50K]  [Plans & Pricing →]
//   3. Container heading: "Good {morning|afternoon|evening}, *David*."
//   4. Launch tiles (3): Windows / Terminal / Commands
//   5. Your Tools: grid of installed modules + [+ Add a tool]
//   6. Recent: last 3 module invocations
//
// Data sources (all proxied through Rust Tauri commands, never direct HTTP):
//   - mc_list_categories         → 16 categories
//   - mc_list_marketplace        → browse marketplace
//   - mc_list_installed_modules  → user's installed modules + client_manifest
//   - mc_install_module(id)      → install + return manifest
//   - mc_uninstall_module(id)    → uninstall
//
// Mount behavior:
//   - Fetches tier + quota + installed modules in parallel.
//   - Renders the 5 sections in order.
//   - Wires launch tiles to navigate('chat' | 'terminal' | ?).
//   - Wires module tiles to a modal that renders form_schema from the
//     module's client_manifest.
//
// Unmount behavior:
//   - Closes any open modal.
//   - Aborts any in-flight fetches via AbortController.

import { invoke } from "@tauri-apps/api/core";
import { navigate } from "../navigation.js";
import { toast } from "../toast.js";

// --- helpers -------------------------------------------------------------

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatTierLabel(tier) {
  switch (tier) {
    case "free": return "Free";
    case "pro": return "Pro";
    case "pro_plus": return "Pro Plus";
    case "team": return "Team";
    case "enterprise": return "Enterprise";
    default: return tier || "Unknown";
  }
}

function formatNumber(n) {
  if (n == null) return "—";
  return n.toLocaleString();
}

function greet() {
  const h = new Date().getHours();
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

function firstName(displayName) {
  if (!displayName) return "friend";
  return displayName.split(/\s+/)[0];
}

// --- state ---------------------------------------------------------------

let _abortController = null;
let _openModal = null;

function _closeModal() {
  if (_openModal) {
    _openModal.remove();
    _openModal = null;
  }
}

function unmount() {
  if (_abortController) _abortController.abort();
  _abortController = null;
  _closeModal();
}

// --- section renderers ---------------------------------------------------

function renderTopbar(displayName) {
  return `
    <header class="mc-topbar">
      <div class="mc-topbar-left">
        <span class="mc-brand">🦞 Miracle Claw</span>
      </div>
      <div class="mc-topbar-right">
        <button class="mc-user" data-action="user-menu">
          <span class="mc-user-name">${escapeHtml(displayName || "Account")}</span>
          <span class="mc-user-caret">▾</span>
        </button>
        <button class="mc-iconbtn" data-action="settings" title="Settings">⚙</button>
      </div>
    </header>
  `;
}

function renderTierRow(tier, quota) {
  const tierLabel = formatTierLabel(tier);
  const used = quota?.used ?? 0;
  const limit = quota?.limit ?? 0;
  const pct = limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  const bar = limit > 0
    ? `<div class="mc-bar"><div class="mc-bar-fill" style="width:${pct}%"></div></div>`
    : `<div class="mc-bar-empty">No quota loaded</div>`;
  return `
    <div class="mc-tier-row">
      <div class="mc-tier-badge" data-tier="${escapeHtml(tier)}">
        <span class="mc-tier-label">${escapeHtml(tierLabel)}</span>
        ${bar}
        <span class="mc-tier-numbers">${formatNumber(used)}/${formatNumber(limit)}</span>
      </div>
      <button class="mc-link" data-action="open-plans">Plans and Pricing →</button>
    </div>
  `;
}

function renderGreeting(displayName) {
  return `
    <h1 class="mc-greeting">${escapeHtml(greet())}, <em>${escapeHtml(firstName(displayName))}</em>.</h1>
  `;
}

function renderLaunchTiles() {
  return `
    <section class="mc-launch">
      <div class="mc-launch-grid">
        <button class="mc-launch-tile" data-action="launch-chat">
          <div class="mc-launch-icon">🦞</div>
          <div class="mc-launch-title">Miracle Claw · Windows</div>
          <div class="mc-launch-sub">Chat with the local model</div>
        </button>
        <button class="mc-launch-tile" data-action="launch-terminal">
          <div class="mc-launch-icon">⌨️</div>
          <div class="mc-launch-title">Miracle Claw · Terminal</div>
          <div class="mc-launch-sub">Run mc-terminal in a shell</div>
        </button>
        <button class="mc-launch-tile" data-action="launch-commands">
          <div class="mc-launch-icon">🖥️</div>
          <div class="mc-launch-title">Miracle Claw · Commands</div>
          <div class="mc-launch-sub">One-shot <code>mclaw run …</code> invocations</div>
        </button>
      </div>
    </section>
  `;
}

function renderYourTools(installed) {
  const count = installed.length;
  const tiles = installed.map((m) => {
    const emoji = m.icon || "📦";
    const tools = m.tools || [];
    const firstTool = tools[0]?.name;
    return `
      <div class="mc-tool-tile" data-module-id="${escapeHtml(m.module_id)}">
        <div class="mc-tool-emoji">${emoji}</div>
        <div class="mc-tool-name">${escapeHtml(m.name)}</div>
        <div class="mc-tool-desc">${escapeHtml(m.description || "")}</div>
        ${firstTool
          ? `<button class="mc-tool-run" data-action="run-tool">Run</button>`
          : `<button class="mc-tool-run" disabled>No tools</button>`}
      </div>
    `;
  }).join("");

  const addTile = `
    <button class="mc-tool-tile mc-tool-add" data-action="add-tool">
      <div class="mc-tool-emoji">＋</div>
      <div class="mc-tool-name">Add a tool</div>
      <div class="mc-tool-desc">Browse the marketplace</div>
    </button>
  `;

  return `
    <section class="mc-tools">
      <div class="mc-tools-header">
        <h2>Your Tools</h2>
        <span class="mc-tools-count">${count} installed</span>
        <button class="mc-link" data-action="add-tool">+ Add a tool</button>
      </div>
      <div class="mc-tools-grid">
        ${tiles}
        ${addTile}
      </div>
    </section>
  `;
}

function renderRecent(runs) {
  const items = (runs || []).slice(0, 3);
  if (items.length === 0) {
    return `
      <section class="mc-recent">
        <h2>Recent</h2>
        <div class="mc-recent-empty">No invocations yet — install a tool above to get started.</div>
      </section>
    `;
  }
  const rows = items.map((r) => `
    <div class="mc-recent-row">
      <span class="mc-recent-when">${escapeHtml(r.relative_time || r.created_at || "")}</span>
      <span class="mc-recent-name">${escapeHtml(r.module_id || "")}.${escapeHtml(r.tool_name || "")}</span>
      <span class="mc-recent-status" data-status="${escapeHtml(r.status || "ok")}">${escapeHtml(r.status || "ok")}</span>
    </div>
  `).join("");
  return `
    <section class="mc-recent">
      <h2>Recent</h2>
      <div class="mc-recent-list">${rows}</div>
    </section>
  `;
}

// --- modal: module form --------------------------------------------------

function openModuleModal(module) {
  _closeModal();
  const tools = module.tools || [];
  if (tools.length === 0) {
    toast(`${module.name} has no tools to run.`);
    return;
  }
  const firstTool = tools[0];
  const fields = (firstTool.form_schema?.fields || []).map((f) => {
    const id = `f-${f.name}`;
    const label = `<label for="${id}">${escapeHtml(f.label || f.name)}${f.required ? " *" : ""}</label>`;
    let input;
    if (f.type === "textarea") {
      input = `<textarea id="${id}" name="${escapeHtml(f.name)}"${f.required ? " required" : ""}></textarea>`;
    } else if (f.type === "select") {
      const opts = (f.options || []).map((o) =>
        `<option value="${escapeHtml(o.value)}">${escapeHtml(o.label)}</option>`).join("");
      input = `<select id="${id}" name="${escapeHtml(f.name)}"${f.required ? " required" : ""}>${opts}</select>`;
    } else {
      input = `<input id="${id}" name="${escapeHtml(f.name)}" type="${escapeHtml(f.type || "text")}"${f.required ? " required" : ""} />`;
    }
    return `<div class="mc-field">${label}${input}</div>`;
  }).join("");

  const modal = document.createElement("div");
  modal.className = "mc-modal";
  modal.innerHTML = `
    <div class="mc-modal-backdrop" data-action="modal-close"></div>
    <div class="mc-modal-panel">
      <header class="mc-modal-header">
        <h3>${escapeHtml(module.name)} · ${escapeHtml(firstTool.name)}</h3>
        <button class="mc-iconbtn" data-action="modal-close" title="Close">×</button>
      </header>
      <form class="mc-modal-form" data-module="${escapeHtml(module.module_id)}" data-tool="${escapeHtml(firstTool.name)}">
        ${fields || `<p class="mc-modal-empty">${escapeHtml(firstTool.description || "No inputs needed.")}</p>`}
        <footer class="mc-modal-footer">
          <button type="button" data-action="modal-close">Cancel</button>
          <button type="submit" class="mc-primary">Run</button>
        </footer>
      </form>
      <div class="mc-modal-result" hidden></div>
    </div>
  `;
  document.body.appendChild(modal);
  _openModal = modal;

  modal.addEventListener("click", (ev) => {
    const target = ev.target.closest("[data-action]");
    if (!target) return;
    if (target.dataset.action === "modal-close") {
      _closeModal();
    }
  });

  const form = modal.querySelector("form.mc-modal-form");
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const fd = new FormData(form);
    const inputs = {};
    for (const [k, v] of fd.entries()) inputs[k] = v;
    const result = modal.querySelector(".mc-modal-result");
    result.hidden = false;
    result.textContent = "Running…";
    try {
      // Module invocation goes through MAIC; result is the rendered output.
      // We don't have a dedicated Rust command yet for this — wire it when
      // Week 5 lands the mclaw CLI integration. For now, just acknowledge.
      result.textContent = `Submitted ${module.module_id}.${firstTool.name} with ${Object.keys(inputs).length} field(s).`;
    } catch (err) {
      result.textContent = `Error: ${err?.message || err}`;
    }
  });
}

// --- mount ---------------------------------------------------------------

async function mount(root, ctx) {
  _abortController = new AbortController();
  const signal = _abortController.signal;

  // Initial shell — render before we know the data, so the user sees
  // something immediately.
  root.innerHTML = `
    <div class="mc-launchpad">
      <div class="mc-loading">Loading your dashboard…</div>
    </div>
  `;

  // Fetch in parallel. Use settled so one failure doesn't break the rest.
  const [tierRes, quotaRes, installedRes, userRes] = await Promise.allSettled([
    invoke("mc_refresh_tier").catch(() => null),
    invoke("mc_get_quota").catch(() => null),
    invoke("mc_list_installed_modules").catch(() => ({ modules: [], grouped_by_category: {} })),
    invoke("mc_get_user_info").catch(() => null),
  ]);

  if (signal.aborted) return;

  const tier = tierRes.status === "fulfilled" ? (tierRes.value?.tier || "free") : "free";
  const quota = quotaRes.status === "fulfilled" ? quotaRes.value : null;
  const installed = installedRes.status === "fulfilled"
    ? (installedRes.value?.modules || [])
    : [];
  const user = userRes.status === "fulfilled" ? userRes.value : null;
  const displayName = user?.name || user?.email || ctx?.user?.name || "";

  // Recent invocations: not wired in Week 2 yet (needs module_runs table
  // populated by mclaw CLI in Week 5). Show empty state for now.
  const recent = [];

  root.innerHTML = `
    <div class="mc-launchpad">
      ${renderTopbar(displayName)}
      ${renderTierRow(tier, quota)}
      <main class="mc-container">
        ${renderGreeting(displayName)}
        ${renderLaunchTiles()}
        ${renderYourTools(installed)}
        ${renderRecent(recent)}
      </main>
    </div>
  `;

  // Wire interactions.
  root.addEventListener("click", _onClick);
}

function _onClick(ev) {
  const target = ev.target.closest("[data-action]");
  if (!target) return;
  const action = target.dataset.action;
  switch (action) {
    case "settings":
      navigate("settings");
      break;
    case "user-menu":
      // Lightweight stub — could expand to a popover later. For now, just
      // surface a hint that account menu is forthcoming.
      toast("Account menu — coming soon.");
      break;
    case "open-plans":
      navigate("plans");
      break;
    case "launch-chat":
      navigate("chat");
      break;
    case "launch-terminal":
      navigate("terminal");
      break;
    case "launch-commands":
      // For Week 2, route to extras hub; replace with `mclaw run` UI in Week 5.
      navigate("extras");
      break;
    case "add-tool":
      navigate("marketplace");
      break;
    case "run-tool": {
      const installedCard = target.closest("[data-module-id]");
      if (!installedCard) return;
      const moduleIdFromCard = installedCard.dataset.moduleId;
      // We need the full client_manifest to render the form. Re-fetch is
      // wasteful — but for Week 2 we keep it simple by re-listing installed.
      invoke("mc_list_installed_modules").then((res) => {
        const mod = (res.modules || []).find((m) => m.module_id === moduleIdFromCard);
        if (mod) openModuleModal(mod);
      }).catch((err) => toast(`Failed to load module: ${err}`));
      break;
    }
    default:
      break;
  }
}

export const dashboardLaunchpadPage = { mount, unmount, requiresAuth: true };
