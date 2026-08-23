// pages/terminal.js — MC Terminal tab (v1.0.9-rc36).
//
// Public API: { mount(root, ctx), unmount() }
//
// Surface:
//   - Shell picker (cmd / pwsh / wsl on Windows; bash / sh / zsh on Unix).
//     Last selection is persisted to localStorage as `mc.terminal.shell`.
//   - Big monospace output area with auto-scroll.
//   - Single-line input box at the bottom.
//   - "Kill session" button — sends EOF + kill on the backend.
//
// Protocol:
//   1. On mount, call `mc_terminal_start(shell)` → get a session id.
//   2. setInterval every ~100ms: `mc_terminal_poll(id, lastSeenSeq)`.
//      Append any new chunks to the output, update lastSeenSeq.
//   3. On Enter in the input: `mc_terminal_write(id, text)`.
//   4. On Kill click: `mc_terminal_kill(id)` and reset.
//
// Unmount: clear the polling interval, kill any active session (we
// don't want orphaned shells running while the user is on Settings).
//
// Xterm.js is NOT used in v1 — plain text is fine for cmd.exe / wsl.
// Adopting xterm later requires dropping it in for the output area
// without breaking the rest.
//
// Lesson 215 — Reader-write the buffer in one place (here, `append`).
// Don't double-track output state in JS or the backend poll will
// disagree with what's on screen.

import { invoke } from "@tauri-apps/api/core";

const DEFAULT_SHELL_WIN = "mc-openclaw";
const DEFAULT_SHELL_NIX = "mc-openclaw";

const POLL_INTERVAL_MS = 100;
const MAX_OUTPUT_CHARS = 200_000; // hard cap, prevents OOM on runaway output

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

// Order matters: first match wins. Detection runs once.
function detectOS() {
  // Tauri exposes OS info via the runtime; we use a coarse UA-style
  // check that works on Windows itself (for our test) and on macOS/Linux.
  if (navigator.userAgent.includes("Windows")) return "windows";
  if (navigator.userAgent.includes("Mac")) return "macos";
  return "linux";
}

function shellOptionsForOS(os) {
  if (os === "windows") {
    return [
      // Lesson 220: mc-openclaw is the new default. It spawns
      // `openclaw tui`, which is the OpenClaw terminal UI — the
      // same chat backend in a terminal-friendly view. Power users
      // can switch to plain cmd/pwsh/wsl via the picker.
      { value: "mc-openclaw", label: "OpenClaw TUI (mc-openclaw)" },
      { value: "cmd", label: "Command Prompt (cmd.exe)" },
      { value: "pwsh", label: "PowerShell 7 (pwsh.exe)" },
      { value: "wsl", label: "WSL bash" },
    ];
  }
  return [
    { value: "mc-openclaw", label: "OpenClaw TUI (mc-openclaw)" },
    { value: "bash", label: "bash" },
    { value: "sh", label: "sh" },
    { value: "zsh", label: "zsh" },
  ];
}

function defaultShellForOS(os) {
  return os === "windows" ? DEFAULT_SHELL_WIN : DEFAULT_SHELL_NIX;
}

// localStorage getter that respects a thrown error (private mode).
function safeLocalGet(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function safeLocalSet(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* private mode — ignore */
  }
}

export const terminalPage = {
  label: "Terminal",
  icon: "💻",
  requiresAuth: true,

  mount(root, ctx = {}) {
    const { onBackToDashboard, onNeedsLogin } = ctx;

    const os = detectOS();
    const osOptions = shellOptionsForOS(os);
    const allowedShells = new Set(osOptions.map((o) => o.value));
    const lastShell = safeLocalGet("mc.terminal.shell");
    const initialShell = lastShell && allowedShells.has(lastShell)
      ? lastShell
      : defaultShellForOS(os);

    let sessionId = null;
    let lastSeq = 0;
    let pollTimer = null;
    let ended = false;
    let allOutputText = ""; // running accumulator for the cap

    const startSession = async (shell) => {
      try {
        sessionId = await invoke("mc_terminal_start", { shell });
        lastSeq = 0;
        appendSystem(`Started ${shell} session (id ${sessionId.slice(0, 8)}…)`);
        setStatus("alive", shell);
      } catch (err) {
        appendSystem(`Failed to start ${shell}: ${escapeHtml(err)}`);
        setStatus("dead", shell, `start failed: ${err}`);
      }
    };

    const killSession = async () => {
      if (!sessionId) return;
      try {
        await invoke("mc_terminal_kill", { id: sessionId });
        appendSystem("Sent kill.");
      } catch (err) {
        appendSystem(`Kill failed: ${escapeHtml(err)}`);
      }
      sessionId = null;
      setStatus("dead", null, "killed");
    };

    const setStatus = (state, shell, detail) => {
      const pill = document.getElementById("terminal-status");
      if (!pill) return;
      pill.dataset.state = state;
      if (state === "alive") {
        pill.textContent = `${shell} · alive`;
      } else {
        pill.textContent = detail ? `ended (${detail})` : "ended";
      }
    };

    const appendChunk = (chunk) => {
      if (chunk.seq <= lastSeq) return;
      lastSeq = chunk.seq;
      const out = document.getElementById("terminal-output");
      if (!out) return;

      // Combine into the running buffer + cap it.
      allOutputText += chunk.data;
      if (allOutputText.length > MAX_OUTPUT_CHARS) {
        allOutputText = allOutputText.slice(-MAX_OUTPUT_CHARS);
        out.textContent = allOutputText;
      } else {
        out.textContent = allOutputText;
      }
      // Auto-scroll only if user is already near the bottom.
      const nearBottom = out.scrollHeight - out.scrollTop - out.clientHeight < 80;
      if (nearBottom) out.scrollTop = out.scrollHeight;
    };

    const appendSystem = (msg) => {
      const out = document.getElementById("terminal-output");
      if (!out) return;
      allOutputText += `[${msg}]\n`;
      out.textContent = allOutputText;
      out.scrollTop = out.scrollHeight;
    };

    const pollOnce = async () => {
      if (ended || !sessionId) return;
      try {
        const res = await invoke("mc_terminal_poll", {
          id: sessionId,
          sinceSeq: lastSeq,
        });
        for (const c of res.chunks || []) {
          appendChunk(c);
        }
        if (!res.alive && sessionId) {
          // session died (natural exit, e.g. cmd.exe closed)
          if (res.exit_info) appendSystem(`exit: ${res.exit_info}`);
          sessionId = null;
          setStatus("dead", null, res.exit_info || "exited");
        }
      } catch (err) {
        const msg = String(err);
        if (msg.includes("not found")) {
          // We were killed from another path; flip to dead.
          sessionId = null;
          setStatus("dead", null, "session gone");
          return;
        }
        appendSystem(`poll error: ${escapeHtml(err)}`);
      }
    };

    root.innerHTML = `
      <div class="dashboard terminal-page">
        <header class="dashboard-header">
          <h1 class="logo">MiracleClaw <span class="muted small">Terminal</span></h1>
          <div class="dashboard-header-actions">
            <span class="tier-badge terminal-status" id="terminal-status"
                  data-state="starting">starting…</span>
            <button
              type="button"
              class="icon-link"
              id="terminal-fullscreen"
              title="Toggle larger terminal area"
              aria-label="Toggle larger terminal area"
            ><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 9V3h6"/><path d="M21 9V3h-6"/><path d="M3 15v6h6"/><path d="M21 15v6h-6"/></svg></button>
            <button
              type="button"
              class="icon-link"
              id="terminal-back"
              title="Back to Dashboard"
              aria-label="Back to Dashboard"
            ><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M19 12H5"/><path d="M12 19l-7-7 7-7"/></svg></button>
          </div>
        </header>

        <div class="terminal-toolbar">
          <label class="terminal-shell-label">
            Shell:
            <select id="terminal-shell-select" class="terminal-shell-select">
              ${osOptions
                .map(
                  (o) => `<option value="${escapeHtml(o.value)}">${escapeHtml(o.label)}</option>`
                )
                .join("")}
            </select>
          </label>
          <button type="button" id="terminal-restart" class="link-button">
            Restart
          </button>
          <button type="button" id="terminal-kill" class="link-button"
                  title="Forcefully end the running shell">
            Kill session
          </button>
        </div>

        <pre
          class="terminal-output"
          id="terminal-output"
          aria-live="polite"
          tabindex="0"
        ></pre>

        <form class="terminal-input-row" id="terminal-input-form">
          <input
            type="text"
            id="terminal-input"
            class="terminal-input"
            placeholder="Type a command and press Enter…"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            autofocus
          />
          <button type="submit" class="primary-button">Send</button>
        </form>

        <div class="dashboard-footer">
          <span class="muted small">
            Output capped at ${(MAX_OUTPUT_CHARS / 1000).toFixed(0)}k chars.
            This is an UNSANDBOXED shell — anything you type here runs as your user.
          </span>
        </div>
      </div>
    `;

    const sel = document.getElementById("terminal-shell-select");
    sel.value = initialShell;
    sel.addEventListener("change", async () => {
      safeLocalSet("mc.terminal.shell", sel.value);
      if (sessionId) await killSession();
      await startSession(sel.value);
    });

    document
      .getElementById("terminal-restart")
      .addEventListener("click", async () => {
        if (sessionId) await killSession();
        await startSession(sel.value);
      });

    document
      .getElementById("terminal-kill")
      .addEventListener("click", killSession);

    document.getElementById("terminal-back").addEventListener("click", () => {
      if (onBackToDashboard) onBackToDashboard();
    });

    // Fullscreen toggle: adds `terminal-fullscreen-mode` class to the page
    // root. CSS enlarges the output area to take ~85vh instead of ~40vh.
    // Persists across navigation; reset by clicking again.
    const fullscreenBtn = document.getElementById("terminal-fullscreen");
    const pageRoot = root.querySelector(".terminal-page");
    if (localStorage.getItem("mc.terminal.fullscreen") === "1") {
      pageRoot.classList.add("terminal-fullscreen-mode");
      fullscreenBtn.textContent = "⛶"; // already correct
    }
    fullscreenBtn.addEventListener("click", () => {
      const on = pageRoot.classList.toggle("terminal-fullscreen-mode");
      localStorage.setItem("mc.terminal.fullscreen", on ? "1" : "0");
    });

    const form = document.getElementById("terminal-input-form");
    const input = document.getElementById("terminal-input");
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const text = input.value;
      if (!text) return;
      if (!sessionId) {
        appendSystem("no live session — click Restart");
        return;
      }
      // Echo locally so the user sees what they typed before output
      // catches up. Remove this once we adopt xterm (which has local
      // echo by default).
      allOutputText += `> ${text}\n`;
      const out = document.getElementById("terminal-output");
      if (out) out.textContent = allOutputText;
      input.value = "";
      try {
        await invoke("mc_terminal_write", { id: sessionId, input: text });
      } catch (err) {
        appendSystem(`write failed: ${escapeHtml(err)}`);
      }
    });

    // Kick off the session + polling
    startSession(initialShell);
    pollTimer = setInterval(pollOnce, POLL_INTERVAL_MS);

    // Keep handlers around for unmount.
    root._terminalCleanup = async () => {
      ended = true;
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
      if (sessionId) {
        try {
          await invoke("mc_terminal_kill", { id: sessionId });
        } catch {
          /* ignore */
        }
        sessionId = null;
      }
    };
  },

  unmount() {
    // Find the root that mounted us (page_registry calls unmount on
    // page unmount, but we need the root's cleanup handler). We store
    // the cleanup on root._terminalCleanup; if root is still around
    // we run it; otherwise we have nothing to do.
    const root = document.getElementById("root");
    if (root && typeof root._terminalCleanup === "function") {
      const c = root._terminalCleanup;
      delete root._terminalCleanup;
      c();
    }
  },
};
