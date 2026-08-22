// pages/settings.js — MiracleClaw Settings page (v1.0.9-rc35).
//
// Public API: { mount(root, ctx), unmount() }
//
// Sections (single scrollable page, in this order):
//   1. Account   — email, tier, endpoint, refresh, sign out
//   2. Memory    — file picker (core files + daily notes), view + edit
//   3. About     — version, build, MAIC provider, "open data folder"
//   4. Sign out  — big red button at bottom (also available from Account)
//
// All file operations go through 5 new Tauri commands:
//   - mc_get_user_info
//   - mc_list_memory_files
//   - mc_read_memory_file(path)
//   - mc_write_memory_file(path, content)
//   - mc_open_data_folder
//
// Path safety is enforced server-side in validate_workspace_md.
// We pass the file's absolute path; the backend rejects anything outside
// the workspace or with a non-.md extension.

import { invoke } from "@tauri-apps/api/core";

// ============================================================================
// Helpers (used only by this page)
// ============================================================================

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatSize(bytes) {
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
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

function formatModified(iso) {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    // Show as "Aug 22, 2026 11:30 UTC" — concise and unambiguous.
    return d.toLocaleString("en-US", {
      year: "numeric", month: "short", day: "numeric",
      hour: "2-digit", minute: "2-digit", timeZone: "UTC",
    }) + " UTC";
  } catch (_) {
    return iso;
  }
}

// ============================================================================
// Page definition
// ============================================================================

export const settingsPage = {
  label: "Settings",
  icon: null,
  requiresAuth: true,

  mount(root, ctx = {}) {
    // Render the skeleton immediately; data fills in async.
    root.innerHTML = renderSkeleton();

    // Wire static interactions (navigation back to dashboard)
    const back = document.getElementById("settings-back");
    if (back) back.addEventListener("click", () => ctx.onBackToDashboard?.());

    const signoutBtn = document.getElementById("signout-btn");
    if (signoutBtn) signoutBtn.addEventListener("click", () => signOut(ctx));

    const refreshBtn = document.getElementById("refresh-tier");
    if (refreshBtn) refreshBtn.addEventListener("click", () => refreshUserInfo(this, root, ctx));

    const openFolderBtn = document.getElementById("open-folder-btn");
    if (openFolderBtn) openFolderBtn.addEventListener("click", () => openDataFolder());

    // Async data loads
    loadUserInfo(this, root, ctx);
    loadMemoryFiles(this, root, ctx);
  },

  unmount() {
    // Close any modal overlays we left behind (matches dashboard.js pattern).
    document.querySelectorAll(".modal-overlay").forEach((el) => el.remove());
    // Drop the cached memory-file content if a textarea was being edited.
    pendingWrite = null;
  },
};

// ============================================================================
// Async loaders
// ============================================================================

async function loadUserInfo(page, root, ctx) {
  const slot = document.getElementById("account-slot");
  if (!slot) return;

  try {
    const info = await invoke("mc_get_user_info");
    slot.innerHTML = renderAccount(info);
    // Re-wire the refresh button (re-render replaced the original).
    const refreshBtn = document.getElementById("refresh-tier");
    if (refreshBtn) refreshBtn.addEventListener("click", () => refreshUserInfo(page, root, ctx));
  } catch (err) {
    if (String(err).includes("not logged in")) {
      ctx.onNeedsLogin?.();
      return;
    }
    slot.innerHTML = renderError(`Could not load account: ${escapeHtml(err)}`);
  }
}

async function refreshUserInfo(page, root, ctx) {
  const btn = document.getElementById("refresh-tier");
  if (btn) {
    btn.disabled = true;
    btn.dataset.oldText = btn.dataset.oldText ?? btn.textContent;
    btn.textContent = "Refreshing…";
  }
  try {
    await invoke("mc_refresh_tier");
    await loadUserInfo(page, root, ctx);
  } catch (err) {
    if (String(err).includes("not logged in")) {
      ctx.onNeedsLogin?.();
      return;
    }
    showFatal(`Refresh failed: ${escapeHtml(err)}`);
  } finally {
    if (btn) {
      btn.disabled = false;
      if (btn.dataset.oldText) {
        btn.textContent = btn.dataset.oldText;
        delete btn.dataset.oldText;
      }
    }
  }
}

async function loadMemoryFiles(page, root, ctx) {
  const slot = document.getElementById("memory-slot");
  if (!slot) return;

  try {
    const files = await invoke("mc_list_memory_files");
    if (files.length === 0) {
      slot.innerHTML = renderMemoryEmpty();
      return;
    }
    slot.innerHTML = renderMemoryList(files);
    // Wire file picker clicks
    slot.querySelectorAll("[data-file-rel]").forEach((el) => {
      el.addEventListener("click", () => openFileInEditor(el.dataset.fileRel, el.dataset.fileAbs, files));
    });
  } catch (err) {
    slot.innerHTML = renderError(`Could not list memory files: ${escapeHtml(err)}`);
  }
}

// ============================================================================
// Renderers
// ============================================================================

function renderSkeleton() {
  return `
    <div class="dashboard">
      <header class="dashboard-header">
        <button type="button" id="settings-back" class="back-link">← Back to Dashboard</button>
        <h1 class="logo">Settings</h1>
      </header>

      <section class="settings-section">
        <h2>Account</h2>
        <div id="account-slot" class="loading-slot">Loading account…</div>
      </section>

      <section class="settings-section">
        <h2>Memory</h2>
        <p class="muted small">
          These are the agent's notes and context files. You can read and edit
          them here; changes save to <code>~/.openclaw/workspace/</code>.
        </p>
        <div id="memory-slot" class="loading-slot">Loading files…</div>
      </section>

      <section class="settings-section">
        <h2>About</h2>
        <div id="about-slot">${renderAbout()}</div>
      </section>

      <section class="settings-section signout-section">
        <button type="button" id="signout-btn" class="danger-button">
          Sign out of MiracleClaw
        </button>
      </section>
    </div>
  `;
}

function renderAccount(info) {
  const tierLabel = formatTierLabel(info.tier);
  const tierClass = `tier-${escapeHtml(info.tier || "free")}`;
  return `
    <div class="account-grid">
      <div class="account-row">
        <div class="account-label">Email</div>
        <div class="account-value">${escapeHtml(info.email || "—")}</div>
      </div>
      <div class="account-row">
        <div class="account-label">Tier</div>
        <div class="account-value">
          <span class="tier-badge ${tierClass}">${escapeHtml(tierLabel)}</span>
        </div>
      </div>
      <div class="account-row">
        <div class="account-label">Provider</div>
        <div class="account-value mono">${escapeHtml(info.endpoint || "—")}</div>
      </div>
      <div class="account-row">
        <div class="account-label">Tier data</div>
        <div class="account-value muted">${escapeHtml(info.cache_age || "—")}</div>
      </div>
      <div class="account-actions">
        <button type="button" id="refresh-tier" class="link-button">
          ↻ Refresh from MAIC
        </button>
      </div>
    </div>
  `;
}

function renderMemoryList(files) {
  const core = files.filter((f) => f.core);
  const daily = files.filter((f) => !f.core);

  return `
    <div class="memory-list">
      ${core.length > 0 ? `
        <div class="memory-group-label">Core files</div>
        ${core.map(renderFileItem).join("")}
      ` : ""}
      ${daily.length > 0 ? `
        <div class="memory-group-label">Daily notes</div>
        ${daily.map(renderFileItem).join("")}
      ` : ""}
    </div>
  `;
}

function renderFileItem(f) {
  const size = formatSize(f.size_bytes);
  const modified = formatModified(f.modified_at);
  return `
    <button type="button" class="memory-file" data-file-rel="${escapeHtml(f.rel_path)}" data-file-abs="${escapeHtml(f.abs_path)}">
      <div class="memory-file-icon">📄</div>
      <div class="memory-file-body">
        <div class="memory-file-name mono">${escapeHtml(f.name)}</div>
        <div class="memory-file-meta muted small">
          ${escapeHtml(f.rel_path)} · ${size} · ${escapeHtml(modified)}
        </div>
      </div>
      <div class="memory-file-arrow">→</div>
    </button>
  `;
}

function renderMemoryEmpty() {
  return `
    <div class="memory-empty">
      <p>No memory files yet.</p>
      <p class="muted small">
        They'll appear here once the workspace is populated.
        Click <em>Open data folder</em> below to open
        <code>~/.openclaw/workspace/</code> in your file manager.
      </p>
    </div>
  `;
}

function renderAbout() {
  return `
    <div class="about-grid">
      <div class="account-row">
        <div class="account-label">App version</div>
        <div class="account-value mono" id="about-version">${escapeHtml(getAppVersion())}</div>
      </div>
      <div class="account-row">
        <div class="account-label">MAIC provider</div>
        <div class="account-value mono">${escapeHtml(window.location.origin === "null" ? "—" : "https://maicserver.com")}</div>
      </div>
      <div class="account-actions">
        <button type="button" id="open-folder-btn" class="link-button">
          📁 Open data folder
        </button>
      </div>
    </div>
  `;
}

function renderError(msg) {
  return `<div class="error">${escapeHtml(msg)}</div>`;
}

// ============================================================================
// File editor
// ============================================================================

// If a save is in flight when unmount runs, we still want to abandon it
// cleanly. This module-local var tracks the latest pending write.
let pendingWrite = null;

async function openFileInEditor(rel, abs, files) {
  // Find the original entry for display (size, modified time)
  const entry = files.find((f) => f.abs_path === abs) || { rel_path: rel, abs_path: abs };

  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal modal-wide">
      <header class="modal-header">
        <h2>${escapeHtml(entry.rel_path || rel)}</h2>
        <button type="button" class="modal-close" id="editor-close">✕</button>
      </header>

      <div class="modal-body">
        <div class="editor-tabs">
          <button type="button" class="tab-button active" data-tab="view">View</button>
          <button type="button" class="tab-button" data-tab="edit">Edit</button>
        </div>

        <div class="tab-panel" data-panel="view">
          <pre class="markdown-view" id="editor-view">Loading…</pre>
        </div>

        <div class="tab-panel hidden" data-panel="edit">
          <textarea class="markdown-edit" id="editor-text" rows="20" spellcheck="false"></textarea>
          <div class="editor-footer">
            <span id="editor-status" class="muted small"></span>
            <button type="button" class="primary-button" id="editor-save">Save</button>
          </div>
        </div>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);

  // Wire tabs
  overlay.querySelectorAll(".tab-button").forEach((btn) => {
    btn.addEventListener("click", () => switchTab(overlay, btn.dataset.tab));
  });

  // Wire close (and cancel any pending write)
  document.getElementById("editor-close").addEventListener("click", () => {
    pendingWrite = null;
    overlay.remove();
  });

  // Wire save
  document.getElementById("editor-save").addEventListener("click", () => saveFromEditor(abs, overlay));

  // Load file contents
  try {
    const content = await invoke("mc_read_memory_file", { path: abs });
    document.getElementById("editor-view").textContent = content;
    document.getElementById("editor-text").value = content;
  } catch (err) {
    document.getElementById("editor-view").textContent = `Error: ${err}`;
    const ta = document.getElementById("editor-text");
    ta.value = "";
    ta.disabled = true;
    const saveBtn = document.getElementById("editor-save");
    saveBtn.disabled = true;
    saveBtn.textContent = "Can't save (read failed)";
  }
}

function switchTab(overlay, name) {
  overlay.querySelectorAll(".tab-button").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === name);
  });
  overlay.querySelectorAll(".tab-panel").forEach((p) => {
    p.classList.toggle("hidden", p.dataset.panel !== name);
  });
}

async function saveFromEditor(absPath, overlay) {
  const ta = document.getElementById("editor-text");
  const status = document.getElementById("editor-status");
  const saveBtn = document.getElementById("editor-save");

  const content = ta.value;
  pendingWrite = { abs: absPath, content };
  saveBtn.disabled = true;
  const oldText = saveBtn.textContent;
  saveBtn.textContent = "Saving…";
  status.textContent = "";

  try {
    await invoke("mc_write_memory_file", { path: absPath, content });
    if (pendingWrite && pendingWrite.abs === absPath) {
      pendingWrite = null;
    }
    status.textContent = `Saved ${new Date().toLocaleTimeString()}`;
    // Update the view tab to reflect new contents
    document.getElementById("editor-view").textContent = content;
  } catch (err) {
    status.textContent = `Save failed: ${err}`;
  } finally {
    saveBtn.disabled = false;
    saveBtn.textContent = oldText;
  }
}

// ============================================================================
// Actions
// ============================================================================

async function signOut(ctx) {
  if (!confirm("Sign out of MiracleClaw? Your chat history in OpenClaw will remain, but you'll need to log in again next time.")) {
    return;
  }
  try {
    await invoke("maic_logout");
    ctx.onNeedsLogin?.();
  } catch (err) {
    showFatal(`Sign out failed: ${escapeHtml(err)}`);
  }
}

async function openDataFolder() {
  try {
    await invoke("mc_open_data_folder");
  } catch (err) {
    showFatal(`Could not open data folder: ${escapeHtml(err)}`);
  }
}

// ============================================================================
// Misc
// ============================================================================

function getAppVersion() {
  // Tauri injects __APP_VERSION__ at build time via tauri.conf.json.
  // If unavailable, fall back to the literal "dev".
  if (typeof window.__APP_VERSION__ === "string") return window.__APP_VERSION__;
  return "dev";
}

function showFatal(msg) {
  const root = document.getElementById("root");
  if (root) {
    root.innerHTML = `
      <div class="fatal">
        <h1>MiracleClaw encountered a problem</h1>
        <pre>${escapeHtml(msg)}</pre>
        <p>Please restart the app. If this keeps happening, file an issue at
          <a href="https://github.com/anomalyco/" target="_blank" rel="noopener">github.com/anomalyco/</a>.
        </p>
      </div>
    `;
  }
}
