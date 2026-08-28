// cmd_k_palette.js — global Ctrl+K (Cmd+K on macOS) command palette.
//
// rc49: power-user shortcut for jumping between pages, running common
// actions, and (later) finding files. The palette is mounted into
// document.body so it overlays whatever page is active. Pressing
// Ctrl+K (or Cmd+K on macOS) toggles it. Inside the palette:
//   - Up/Down arrows move selection
//   - Enter runs the highlighted action
//   - Esc closes
//   - Typing updates the fuzzy filter
//
// Why it lives at the global level instead of inside one page:
//   The whole point of a command palette is that it works FROM anywhere.
//   Putting it inside dashboard would mean it doesn't work while the
//   user is in Files or Notebook. We want Ctrl+K to feel like an OS
//   shortcut, not a dashboard feature.
//
// Sources (commands) include:
//   - Pages: Dashboard, Settings, Files, Notebook, Local Terminal,
//     OpenClaw (Windows), OpenClaw TUI (Terminal)
//   - Actions: "Open data folder", "Refresh tier", "Sign out",
//     "Toggle attach zone focus" (so newbies can learn the shortcut)
//
// Fuzzy match: case-insensitive substring with a small scoring boost
// for matches that hit word boundaries (so "td" matches "Terminal
// Dashboard" before matching "foobar"). 60 results max so the palette
// stays snappy even with hundreds of commands.

import { invoke } from "@tauri-apps/api/core";
// rc53.8 (feature/extras-hub): palette actions for mc-* commands.
// Each action opens the Terminal page with the command pre-filled,
// matching the "Run in Terminal" button on the Extras hub page.
import { extrasPaletteActions } from "./pages/extras.js";

// Module-level state. We attach at most one palette overlay; toggling
// just shows/hides it. The palette doesn't persist user input across
// opens — each open is a fresh search.
let _deps = null;       // { navigate, runAction, open }
let _paletteEl = null;  // root overlay element
let _inputEl = null;
let _resultsEl = null;
let _statusEl = null;
let _items = [];        // current filtered list
let _selectedIdx = 0;
let _lastQuery = "";
// Palette gates itself on auth: the Ctrl+K shortcut does nothing
// until enable() is called (called by main.js right after a successful
// login). This is what stops the palette from popping up on the
// login page — login has its own input focus needs and the palette
// overlay would steal clicks AND make login fail.
let _enabled = false;

/**
 * Initialize the palette. Attaches the global keyboard listener and
 * creates (but doesn't show) the overlay element.
 *
 * @param {object} deps
 * @param {Function} deps.navigate  navigate(pageId, extras?) — see navigation.js
 * @param {Function} deps.runAction runAction(fn) — see navigation.js
 * @param {Function} [deps.open]    optional no-op for compat
 */
export function initPalette(deps) {
  _deps = deps;
  ensureOverlay();
  document.addEventListener("keydown", onGlobalKeydown, true);
  // Belt-and-suspenders: also listen on the overlay's own keydown. If
  // the input ever loses focus (autofocus war with a login form, blur
  // after open, etc.) and Esc is pressed, the overlay-level handler
  // will still close it.
  _paletteEl.addEventListener("keydown", onOverlayKeydown);
}

/**
 * Enable the palette (Ctrl+K shortcut starts working). Call this after
 * the user successfully logs in. To disable (e.g. before logging out),
 * call disable().
 */
export function enable() {
  _enabled = true;
}

/**
 * Disable the palette (Ctrl+K shortcut stops working). If the palette
 * is currently open, close it first.
 */
export function disable() {
  _enabled = false;
  if (_paletteEl && !_paletteEl.hidden) close();
}

function ensureOverlay() {
  if (_paletteEl) return;
  _paletteEl = document.createElement("div");
  _paletteEl.className = "cmd-k-overlay";
  _paletteEl.hidden = true;
  _paletteEl.setAttribute("role", "dialog");
  _paletteEl.setAttribute("aria-label", "Command palette");
  _paletteEl.innerHTML = `
    <div class="cmd-k-panel">
      <input type="text" class="cmd-k-input" id="cmd-k-input"
             placeholder="Type a command or search…" autocomplete="off"
             spellcheck="false" />
      <div class="cmd-k-results" id="cmd-k-results"></div>
      <div class="cmd-k-status muted small" id="cmd-k-status">
        ↑↓ to move · Enter to run · Esc to close
      </div>
    </div>
  `;
  document.body.appendChild(_paletteEl);

  _inputEl = _paletteEl.querySelector("#cmd-k-input");
  _resultsEl = _paletteEl.querySelector("#cmd-k-results");
  _statusEl = _paletteEl.querySelector("#cmd-k-status");

  _inputEl.addEventListener("input", () => refreshItems(_inputEl.value));
  _inputEl.addEventListener("keydown", onInputKeydown);
  // Click outside the panel closes; click inside keeps open.
  _paletteEl.addEventListener("mousedown", (e) => {
    if (e.target === _paletteEl) close();
  });
}

function onGlobalKeydown(e) {
  // Ctrl+K / Cmd+K shortcut. Gated on _enabled so the palette doesn't
  // pop up on the login page (where the overlay would steal the
  // email/password input clicks and lock the user out — the exact
  // bug David hit in rc49).
  const isMac = navigator.platform.toLowerCase().includes("mac");
  const accel = isMac ? e.metaKey : e.ctrlKey;
  if (!_enabled) return;
  if (
    accel &&
    !e.altKey &&
    (e.key === "k" || e.key === "K") &&
    !(isMac ? e.ctrlKey : false) &&
    !(e.shiftKey && !e.key.startsWith("Shift"))
  ) {
    e.preventDefault();
    e.stopPropagation();
    toggle();
  }
}

function onInputKeydown(e) {
  if (e.key === "Escape") {
    e.preventDefault();
    close();
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    moveSelection(1);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    moveSelection(-1);
  } else if (e.key === "Enter") {
    e.preventDefault();
    runSelected();
  }
}

/**
 * Same keys as onInputKeydown, but at the overlay level. Catches the
 * case where the input lost focus mid-palette (rare, but possible
 * after a tab away or a focus war with an autofocus'd login field).
 */
function onOverlayKeydown(e) {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    close();
  } else if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter") {
    // If the input doesn't have focus, forward the key back into it
    // so the user sees cursor movement + has working Enter without
    // having to click first. Re-dispatching is the simplest path.
    if (document.activeElement !== _inputEl) {
      e.preventDefault();
      _inputEl.focus();
      _inputEl.dispatchEvent(
        new KeyboardEvent("keydown", { ...e, bubbles: true })
      );
    }
    // Otherwise let the input's own listener handle it.
  }
}

function toggle() {
  if (_paletteEl.hidden) open();
  else close();
}

/**
 * Public open() for callers (e.g. dashboard's "Ctrl+K" header button)
 * that want to programmatically show the palette. No-op if the palette
 * hasn't been enabled yet (i.e. user is on the login page).
 */
export function openPalette() {
  if (!_enabled) return;
  open();
}

function open() {
  ensureOverlay();
  _paletteEl.hidden = false;
  _inputEl.value = "";
  _inputEl.focus();
  refreshItems("");
}

function close() {
  if (_paletteEl) _paletteEl.hidden = true;
  // Blur so subsequent keystrokes (e.g. typing in chat) don't leak
  // into the palette's input after a page transition.
  if (_inputEl) _inputEl.blur();
}

function refreshItems(query) {
  _lastQuery = query;
  const all = buildCommands();
  const q = query.trim().toLowerCase();
  let scored;
  if (!q) {
    // No query: show everything, sorted by category then label.
    scored = all.map((c) => ({ ...c, score: 0 }));
  } else {
    scored = [];
    for (const cmd of all) {
      const s = scoreMatch(cmd, q);
      if (s > 0) scored.push({ ...cmd, score: s });
    }
    scored.sort((a, b) => b.score - a.score);
  }
  _items = scored.slice(0, 60);
  _selectedIdx = 0;
  renderItems();
  setStatus(
    _items.length
      ? `${_items.length} result${_items.length === 1 ? "" : "s"} · ↑↓ to move · Enter to run · Esc to close`
      : "No matches. Press Esc to close."
  );
}

function scoreMatch(cmd, q) {
  const hayLabel = cmd.label.toLowerCase();
  const hayKey = (cmd.keywords || []).join(" ").toLowerCase();
  // Exact substring match scores highest. Word-boundary match (the
  // query starts at the beginning of a word in the label or keywords)
  // scores higher than a mid-word hit. Otherwise substring match.
  if (hayLabel === q) return 1000;
  if (hayLabel.startsWith(q)) return 500;
  if (hayLabel.includes(q)) {
    // bump if at a word boundary
    const idx = hayLabel.indexOf(q);
    const prev = idx > 0 ? hayLabel[idx - 1] : " ";
    return /\s|[:/]/.test(prev) ? 300 : 150;
  }
  if (hayKey.includes(q)) return 100;
  // Per-character subsequence match against the label. Cheap fallback
  // for fuzzy "I know it has these letters somewhere" matching.
  let qi = 0;
  for (let i = 0; i < hayLabel.length && qi < q.length; i++) {
    if (hayLabel[i] === q[qi]) qi++;
  }
  return qi === q.length ? 30 : 0;
}

function buildCommands() {
  // Static command set. Future additions:
  //   - File search via mc_ui_list_allowed_roots (walk dirs, dedupe names)
  //   - Recent items (track last 5 page visits in localStorage)
  //   - Model switcher (mc_set_model)
  const cmds = [
    {
      id: "page.dashboard",
      label: "Dashboard",
      category: "Page",
      icon: "🏠",
      keywords: ["home", "main", "tiles"],
      run: () => _deps.navigate("dashboard"),
    },
    {
      id: "page.files",
      label: "Files",
      category: "Page",
      icon: "📁",
      keywords: ["browse", "open", "folders"],
      run: () => _deps.navigate("files"),
    },
    {
      id: "page.notebook",
      label: "Notebook",
      category: "Page",
      icon: "📓",
      keywords: ["notes", "markdown", "scratch"],
      run: () => _deps.navigate("notebook"),
    },
    {
      id: "page.settings",
      label: "Settings",
      category: "Page",
      icon: "⚙️",
      keywords: ["config", "preferences", "options"],
      run: () => _deps.navigate("settings"),
    },
    {
      id: "page.provider-keys",
      label: "Provider Keys",
      category: "Page",
      icon: "🔑",
      keywords: ["api key", "openai", "anthropic", "ollama", "byo", "bring your own", "secret"],
      run: () => _deps.navigate("provider-keys"),
    },
    {
      id: "page.terminal.local",
      label: "Local Terminal",
      category: "Page",
      icon: "💻",
      keywords: ["cmd", "bash", "shell", "command line"],
      run: () => _deps.navigate("terminal", { defaultShell: undefined }),
    },
    {
      id: "page.terminal.openclaw",
      label: "OpenClaw · TUI",
      category: "Page",
      icon: "⌨️",
      keywords: ["terminal", "tui", "openclaw"],
      run: () => _deps.navigate("terminal", { defaultShell: "mc-openclaw" }),
    },
    {
      id: "page.openclaw.windows",
      label: "OpenClaw · Chat",
      category: "Page",
      icon: "🦞",
      keywords: ["chat", "window", "openclaw", "main"],
      run: async () => {
        try {
          const gw = await invoke("start_gateway_after_login");
          await invoke("openclaw_open_window");
        } catch (err) {
          console.error("[palette] openclaw failed:", err);
          alert(`Could not open OpenClaw chat: ${err}`);
        }
      },
    },
    {
      id: "action.refresh-tier",
      label: "Refresh tier",
      category: "Action",
      icon: "🔄",
      keywords: ["tier", "plan", "subscription", "reload"],
      run: async () => {
        try {
          await invoke("mc_refresh_tier");
          // Re-navigate to dashboard if we're on it; otherwise the
          // user can press Ctrl+K again to navigate.
          setStatus("Tier refreshed.");
        } catch (err) {
          setStatus(`Refresh failed: ${err}`, true);
        }
      },
    },
    {
      id: "action.open-data-folder",
      label: "Open MC data folder",
      category: "Action",
      icon: "📂",
      keywords: ["data", "folder", "files", "appdata", "config"],
      run: async () => {
        try {
          await invoke("mc_open_data_folder");
          setStatus("Opened in file explorer.");
        } catch (err) {
          setStatus(`Open failed: ${err}`, true);
        }
      },
    },
    {
      id: "action.toggle-theme-hint",
      label: "Show keyboard shortcuts",
      category: "Help",
      icon: "❓",
      keywords: ["help", "shortcuts", "keys", "how"],
      run: () => {
        showShortcutsModal();
      },
    },
    {
      id: "action.sign-out",
      label: "Sign out",
      category: "Action",
      icon: "🚪",
      keywords: ["logout", "signout", "exit"],
      run: async () => {
        if (!confirm("Sign out of MiracleClaw? You'll need to log in again.")) {
          return;
        }
        try {
          await invoke("maic_logout");
          // Bounce to login via the navigation system. We could call
          // navigate('login') but that doesn't disable the palette —
          // main.js's mountLogin() does. The palette import isn't
          // supposed to reach into main.js, so we route through a
          // window-exposed helper that main.js sets up.
          if (window.__mc_mountLogin) {
            window.__mc_mountLogin();
          } else {
            // Fallback if main.js hasn't wired it up (e.g. tests).
            _deps.navigate("login");
          }
        } catch (err) {
          setStatus(`Sign out failed: ${err}`, true);
        }
      },
    },
  ];
  // rc53.8 (feature/extras-hub): append one palette action per mc-*
  // command. Filtered by fuzzy match on label/keywords so users can
  // type "mc-doc" or just "doctor" to find it.
  cmds.push(...extrasPaletteActions);
  return cmds;
}

function renderItems() {
  if (!_items.length) {
    _resultsEl.innerHTML = `<div class="cmd-k-empty muted small">No matches.</div>`;
    return;
  }
  _resultsEl.innerHTML = _items
    .map(
      (item, i) => `
        <div class="cmd-k-item${i === _selectedIdx ? " selected" : ""}"
             data-idx="${i}" role="option" aria-selected="${i === _selectedIdx}">
          <span class="cmd-k-icon">${item.icon || "·"}</span>
          <span class="cmd-k-label">${escapeHtml(item.label)}</span>
          <span class="cmd-k-category muted small">${escapeHtml(item.category || "")}</span>
        </div>
      `
    )
    .join("");
  _resultsEl.querySelectorAll(".cmd-k-item").forEach((el) => {
    el.addEventListener("click", () => {
      _selectedIdx = Number(el.dataset.idx);
      runSelected();
    });
    el.addEventListener("mouseenter", () => {
      _selectedIdx = Number(el.dataset.idx);
      updateSelectedClass();
    });
  });
}

function updateSelectedClass() {
  _resultsEl
    .querySelectorAll(".cmd-k-item")
    .forEach((el, i) => {
      el.classList.toggle("selected", i === _selectedIdx);
      el.setAttribute("aria-selected", i === _selectedIdx ? "true" : "false");
    });
}

function moveSelection(delta) {
  if (!_items.length) return;
  _selectedIdx = (_selectedIdx + delta + _items.length) % _items.length;
  updateSelectedClass();
  // Scroll selected into view if needed.
  const el = _resultsEl.querySelector(".cmd-k-item.selected");
  if (el && el.scrollIntoView) {
    el.scrollIntoView({ block: "nearest" });
  }
}

async function runSelected() {
  const item = _items[_selectedIdx];
  if (!item) return;
  close();
  try {
    await item.run();
  } catch (err) {
    console.error("[palette] command failed:", err);
    alert(`Command failed: ${err}`);
  }
}

function setStatus(msg, isError) {
  if (!_statusEl) return;
  _statusEl.textContent = msg;
  _statusEl.dataset.kind = isError ? "error" : "info";
}

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function showShortcutsModal() {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal">
      <h2>Keyboard shortcuts</h2>
      <table class="shortcut-table">
        <tr><td><kbd>Ctrl</kbd>+<kbd>K</kbd></td><td>Open command palette</td></tr>
        <tr><td><kbd>Esc</kbd></td><td>Close palette / modals</td></tr>
        <tr><td><kbd>↑</kbd> <kbd>↓</kbd></td><td>Move selection in palette</td></tr>
        <tr><td><kbd>Enter</kbd></td><td>Run selected command</td></tr>
      </table>
      <p class="muted small">The palette works from any page. New in rc49.</p>
      <div class="modal-actions">
        <button type="button" id="shortcuts-modal-dismiss">Got it</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);
  overlay.querySelector("#shortcuts-modal-dismiss").addEventListener("click", () => {
    overlay.remove();
  });
}