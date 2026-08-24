// pages/terminal.js — MC Terminal tab (v1.0.9-rc43).
//
// Public API: { mount(root, ctx), unmount() }
//
// Surface:
//   - Shell picker (cmd / pwsh / wsl on Windows; bash / sh / zsh on Unix).
//     Last selection is persisted to localStorage as `mc.terminal.shell`.
//   - xterm.js Terminal as the output area (rc43, replaces <pre>+stripAnsi).
//   - Single-line input box at the bottom.
//   - "Kill session" button — sends EOF + kill on the backend.
//
// Protocol:
//   1. On mount, call `mc_terminal_start(shell)` → get a session id.
//   2. setInterval every ~100ms: `mc_terminal_poll(id, lastSeenSeq)`.
//      For any new chunks, call `term.write(chunk.data)` — xterm.js
//      handles ANSI parsing (CSI / OSC / SGR / cursor positioning) and
//      redraws natively. No frontend regex needed.
//   3. On Enter in the input: `mc_terminal_write(id, text)`.
//   4. On Kill click: `mc_terminal_kill(id)` and reset.
//
// Unmount: clear the polling interval, dispose xterm.Terminal, kill any
// active session (we don't want orphaned shells running while the user
// is on Settings).
//
// rc43 — xterm.js adoption (was Lesson 215 candidate). Reasons for
// finally doing this:
//   - stripAnsi (rc41) hid the noise but did not fix root cause: Ink
//     whole-screen redraws without CSI interpretation leave residue.
//   - collapseNoise (rc42 plan) would have been ~70% effective. xterm
//     is 100% — it IS a real terminal emulator.
//   - Color, scrollback, resize (fullscreen toggle), hyperlink click
//     all work out of the box.
//
// Pitfalls handled here:
//   - xterm needs a non-zero container at open-time. We open it after
//     innerHTML is set + on next animation frame.
//   - FitAddon.fit() must be called after mount AND after every resize
//     AND after fullscreen toggle. We use a ResizeObserver for that.
//   - For mc-openclaw (TUI), the backend sends \r as Enter (rc40); we
//     preserve that. xterm's onData could feed input too, but for the
//     mc-openclaw shell we drive input through the backend so the
//     backend sees \r instead of \r\n.

import { invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";
// rc53.5 (feature/secrets-vault): mount the secrets page inside an
// overlay when the toolbar button is clicked. Re-uses the existing
// page factory — no duplicate UI code.
import { secretsPage } from "./secrets.js";
// rc53.7 (feature/attach-toolbar): same overlay pattern for the
// attach-zone. We reuse the dashboard's wireAttachZone subcomponent
// so dropping files behaves identically to the dashboard zone.
import { wireAttachZone } from "./dashboard.js";

// Default shell for the Terminal tile. Historically `mc-openclaw` so the
// Terminal button launched the OpenClaw TUI by default. Dashboard v1.0.9-rc45
// splits that into its own "OpenClaw · Terminal" tile; the Terminal tile is
// now the local shell launcher (cmd / bash), so it defaults to the OS shell.
const DEFAULT_SHELL_WIN = "cmd";
const DEFAULT_SHELL_NIX = "bash";

const POLL_INTERVAL_MS = 100;

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function detectOS() {
  if (navigator.userAgent.includes("Windows")) return "windows";
  if (navigator.userAgent.includes("Mac")) return "macos";
  return "linux";
}

function shellOptionsForOS(os) {
  if (os === "windows") {
    return [
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
    const { onBackToDashboard, defaultShell: ctxDefaultShell, initialCommand: ctxInitialCommand, autoOpenOverlay: ctxAutoOpenOverlay } = ctx;

    const os = detectOS();
    const osOptions = shellOptionsForOS(os);
    const allowedShells = new Set(osOptions.map((o) => o.value));
    // Resolution order:
    //   1. ctx.defaultShell (caller asked for a specific shell — dashboard
    //      uses this to drop the user straight into the OpenClaw TUI).
    //   2. localStorage `mc.terminal.shell` (user's last selection).
    //   3. OS default.
    //
    // Migration (v1.0.9-rc45): pre-rc45 the Terminal tile defaulted to
    // mc-openclaw, so installs that have used the app have it saved in
    // localStorage. After the split, "Terminal" is the local shell tile
    // and mc-openclaw lives on its own dashboard button. If we see the
    // old saved value here, fall back to the OS default so the user
    // lands in cmd/bash — the AI shell is one click away on the
    // dashboard.
    const lastShell = safeLocalGet("mc.terminal.shell");
    let initialShell;
    if (ctxDefaultShell && allowedShells.has(ctxDefaultShell)) {
      initialShell = ctxDefaultShell;
    } else if (
      lastShell &&
      allowedShells.has(lastShell) &&
      lastShell !== "mc-openclaw"
    ) {
      initialShell = lastShell;
    } else {
      initialShell = defaultShellForOS(os);
    }

    let sessionId = null;
    let lastSeq = 0;
    let pollTimer = null;
    let ended = false;
    let term = null;        // xterm.Terminal instance
    let fitAddon = null;    // FitAddon instance
    let resizeObserver = null;

    const appendSystem = (msg) => {
      if (!term) return;
      // Yellow brackets, dim grey message — easy to scan in scrollback.
      term.write(`\r\n\x1b[33m[\x1b[0m\x1b[90m${String(msg ?? "")}\x1b[0m\x1b[33m]\x1b[0m\r\n`);
    };

    const startSession = async (shell) => {
      try {
        sessionId = await invoke("mc_terminal_start", { shell });
        lastSeq = 0;
        appendSystem(`Started ${shell} session (id ${sessionId.slice(0, 8)}…)`);
        setStatus("alive", shell);
        // rc53.8 (feature/extras-hub): if the caller passed an initial
        // command (e.g. extras hub launches us with "mlg-doctor"), wait
        // briefly for the shell to settle and then write + enter it.
        // The 350ms delay gives cmd.exe / bash time to print its first
        // prompt before we send input — otherwise some shells swallow
        // the first command line.
        if (ctxInitialCommand && sessionId) {
          setTimeout(async () => {
            try {
              term.write(`\r\n\x1b[36m> ${ctxInitialCommand}\x1b[0m\r\n`);
              await invoke("mc_terminal_write", {
                id: sessionId,
                input: ctxInitialCommand + "\n",
              });
              // Show the command in the visible input too so the user
              // can edit it (and so the input isn't blank if the
              // shell prompt hasn't drawn yet).
              if (input) input.value = "";
            } catch (err) {
              appendSystem(`initial command failed: ${escapeHtml(err)}`);
            }
          }, 350);
        }
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

    const writeChunk = (chunk) => {
      if (!term) return;
      // xterm.js interprets ANSI natively. No stripAnsi needed.
      term.write(chunk.data ?? "");
    };

    const pollOnce = async () => {
      if (ended || !sessionId) return;
      try {
        const res = await invoke("mc_terminal_poll", {
          id: sessionId,
          sinceSeq: lastSeq,
        });
        for (const c of res.chunks || []) {
          if (c.seq <= lastSeq) continue;
          lastSeq = c.seq;
          writeChunk(c);
        }
        if (!res.alive && sessionId) {
          if (res.exit_info) appendSystem(`exit: ${res.exit_info}`);
          sessionId = null;
          setStatus("dead", null, res.exit_info || "exited");
        }
      } catch (err) {
        const msg = String(err);
        if (msg.includes("not found")) {
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
          <span class="terminal-toolbar-spacer"></span>
          <button type="button" id="terminal-attach-btn" class="icon-link"
                  title="Drop files to attach to your next chat message"
                  aria-label="Open attach zone">
            📎 Attach
          </button>
          <button type="button" id="terminal-secrets-btn" class="icon-link"
                  title="Manage secrets vault (rc53.6 — Once / PerSession / Vault)"
                  aria-label="Open secrets vault">
            🔑 Secrets
          </button>
        </div>

        <div
          class="terminal-output xterm-mount"
          id="terminal-output"
          aria-live="polite"
          tabindex="0"
        ></div>

        <form class="terminal-input-row" id="terminal-input-form">
          <input
            type="text"
            id="terminal-input"
            class="terminal-input"
            placeholder="Type a command and press Enter…"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
          />
          <button type="submit" class="primary-button">Send</button>
        </form>

        <div class="dashboard-footer">
          <span class="muted small">
            xterm.js terminal emulator — full PTY rendering, scrollback,
            colors. This is an UNSANDBOXED shell — anything you type
            here runs as your user.
          </span>
        </div>
      </div>
    `;

    // rc43: Initialize xterm.js after innerHTML is set; container is
    // non-zero at this point because the flexbox layout has sized it.
    // FitAddon reads from the container's dimensions.
    const termContainer = document.getElementById("terminal-output");
    term = new Terminal({
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: "block",
      // Generous scrollback — xterm handles its own ring buffer.
      scrollback: 10000,
      // Don't convert our \n into \r\n — backend already does that.
      convertEol: false,
      // Force a sane default theme that matches our dark dashboard.
      theme: {
        background: "#0b1020",
        foreground: "#e5e7eb",
        cursor: "#22c55e",
        cursorAccent: "#0b1020",
        selectionBackground: "rgba(34,197,94,0.30)",
        black: "#0b1020",
        red: "#ef4444",
        green: "#22c55e",
        yellow: "#eab308",
        blue: "#3b82f6",
        magenta: "#a855f7",
        cyan: "#06b6d4",
        white: "#e5e7eb",
        brightBlack: "#475569",
        brightRed: "#f87171",
        brightGreen: "#4ade80",
        brightYellow: "#facc15",
        brightBlue: "#60a5fa",
        brightMagenta: "#c084fc",
        brightCyan: "#22d3ee",
        brightWhite: "#f8fafc",
      },
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.open(termContainer);

    // rc44 focus fix: xterm's internal <textarea class="xterm-helper-textarea">
    // is positioned inside the terminal container and steals keystrokes
    // when it has focus. term.open() focuses it by default, which
    // conflicts with our <input id="terminal-input"> below the terminal.
    // Symptom (David, 2026-08-22 23:38 MDT): click into the input,
    // cursor appears, but typed keys don't register and Enter doesn't
    // submit. The visible cursor is just :focus styling; actual focus
    // is still on xterm's helper textarea (covers the input area
    // visually because they're stacked).
    //
    // Fix: explicitly blur xterm AFTER mount so the input field is the
    // real document.activeElement. Then focus the input on first mount
    // and on every click into the input (defensive — clicks should
    // bubble to set focus, but we make it explicit).
    term.blur();

    // First fit must happen AFTER the container has been measured by
    // the browser (next animation frame is the safe bet). Without
    // this, FitAddon.fit() reports 0x0 and renders an empty terminal.
    requestAnimationFrame(() => {
      try {
        fitAddon.fit();
      } catch (e) {
        // Some browsers throw if container still 0x0; retry on next frame.
        requestAnimationFrame(() => {
          try { fitAddon.fit(); } catch { /* ignore */ }
        });
      }
    });

    // Observe container resizes (fullscreen toggle, window resize,
    // devicePixelRatio change). xterm needs .fit() on every change.
    resizeObserver = new ResizeObserver(() => {
      try { fitAddon.fit(); } catch { /* ignore */ }
    });
    resizeObserver.observe(termContainer);

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

    // rc53.5 (feature/secrets-vault): toolbar 🔑 Secrets button.
    // Opens the secrets page as a fullscreen overlay on top of the
    // terminal. The overlay has its own z-index and Escape closes it.
    // If the user has unsaved typed input, we keep it warm but pause
    // polling while the overlay is up so the terminal doesn't scroll
    // underneath.
    const secretsOverlay = document.createElement("div");
    secretsOverlay.className = "secrets-overlay";
    secretsOverlay.id = "terminal-secrets-overlay";
    secretsOverlay.setAttribute("hidden", "");
    secretsOverlay.innerHTML = `
      <div class="secrets-overlay-header">
        <button class="icon-link" id="terminal-secrets-close"
                title="Close secrets" aria-label="Close secrets">✕</button>
      </div>
      <div class="secrets-overlay-body" id="terminal-secrets-body"></div>
    `;
    root.appendChild(secretsOverlay);
    const secretsBody = document.getElementById("terminal-secrets-body");
    const secretsCloseBtn = document.getElementById("terminal-secrets-close");

    let secretsOpen = false;
    function openSecretsOverlay() {
      // Pause terminal polling so the overlay doesn't render behind
      // a moving terminal. Resume on close.
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
      secretsOpen = true;
      secretsOverlay.removeAttribute("hidden");
      // Mount the secrets page into the overlay body. onBack = close.
      secretsPage.mount(secretsBody, {
        onBackToDashboard: closeSecretsOverlay,
      });
    }
    function closeSecretsOverlay() {
      if (!secretsOpen) return;
      secretsOpen = false;
      secretsPage.unmount();
      secretsBody.innerHTML = "";
      secretsOverlay.setAttribute("hidden", "");
      // Resume polling.
      if (!ended && sessionId && !pollTimer) {
        pollTimer = setInterval(pollOnce, POLL_INTERVAL_MS);
      }
      // rc53.11 (Lesson 247): if the user opened this overlay from the
      // OpenClaw chat window (via the floating 🔑 button in mc-chat-
      // toolbar), the close handler should also navigate them BACK to
      // the chat URL — not leave them stranded on the dashboard. The
      // Rust mc_close_overlay command captures the URL we came from
      // (stashed by mc_open_overlay) and navigates back.
      //
      // Fall back to plain DOM close if the bridge isn't present
      // (e.g. dev/test environments) — the overlay still hides
      // locally, the user just stays on whatever page they were on.
      try {
        const bridge = window.__openclawHostBridge;
        if (bridge && typeof bridge.closeOverlay === 'function') {
          bridge.closeOverlay();
          return;
        }
      } catch (_) { /* fall through */ }
    }
    secretsCloseBtn.addEventListener("click", closeSecretsOverlay);
    // Esc closes the overlay when it's open.
    secretsOverlay.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeSecretsOverlay();
    });
    document.getElementById("terminal-secrets-btn").addEventListener("click", openSecretsOverlay);

    // rc53.7 (feature/attach-toolbar): mount the dashboard's attach
    // zone inside an overlay when the toolbar 📎 button is clicked.
    // Same overlay pattern as secrets — pause terminal polling while
    // open so the overlay doesn't render behind a moving terminal.
    const attachOverlay = document.createElement("div");
    attachOverlay.className = "attach-overlay";
    attachOverlay.id = "terminal-attach-overlay";
    attachOverlay.setAttribute("hidden", "");
    attachOverlay.innerHTML = `
      <div class="attach-overlay-header">
        <span class="attach-overlay-title">📎 Attach files to next chat message</span>
        <button class="icon-link" id="terminal-attach-close"
                title="Close attach zone" aria-label="Close attach zone">✕</button>
      </div>
      <div class="attach-overlay-body" id="terminal-attach-body">
        <div class="attach-zone" id="attach-zone" tabindex="0" role="button"
             aria-label="Drop files here to attach them to your next chat message">
          <div class="attach-zone-empty" id="attach-zone-empty">
            <div class="attach-zone-icon">📎</div>
            <div class="attach-zone-msg">
              <strong>Drop files here</strong> to attach them to your next chat message.
              <div class="muted small">
                Up to 100 MB per file. PDFs, images, code, documents — anything you can drag.
              </div>
            </div>
          </div>
          <div class="attach-queue" id="attach-queue" hidden></div>
          <div class="attach-actions" id="attach-actions" hidden>
            <input type="text" class="attach-message" id="attach-message"
                   placeholder="Optional: a note for the model (e.g. 'summarize this')" />
            <button type="button" class="primary" id="attach-send">Send to chat →</button>
            <button type="button" class="link-button" id="attach-clear">Clear queue</button>
          </div>
          <div class="attach-zone-help muted small" id="attach-zone-help">
            <details>
              <summary>How does this work?</summary>
              <ol>
                <li>Drop one or more files above. They copy into MC's workspace.</li>
                <li>Click <strong>Send to chat</strong>. The OpenClaw chat window opens and the file paths land in your clipboard.</li>
                <li>Click into the chat input and press <strong>Ctrl+V</strong>. The model sees the file paths and reads them with its file tool.</li>
              </ol>
              <p class="muted small">MC can't paste directly into the chat window, so the clipboard is the bridge. One keystroke after each Send.</p>
            </details>
          </div>
          <div class="attach-status muted small" id="attach-status"></div>
        </div>
      </div>
    `;
    root.appendChild(attachOverlay);
    const attachBody = document.getElementById("terminal-attach-body");
    const attachCloseBtn = document.getElementById("terminal-attach-close");

    let attachOpen = false;
    let attachZoneWired = false;
    function openAttachOverlay() {
      // Pause terminal polling while overlay is up (same as secrets).
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
      attachOpen = true;
      attachOverlay.removeAttribute("hidden");
      // Wire the zone lazily — wireAttachZone registers document-level
      // drag listeners, so we only want them live while the overlay
      // is mounted. The next open() reuses the existing wiring if
      // the zone is still in the DOM.
      if (!attachZoneWired) {
        wireAttachZone(attachOverlay);
        attachZoneWired = true;
      }
    }
    function closeAttachOverlay() {
      if (!attachOpen) return;
      attachOpen = false;
      attachOverlay.setAttribute("hidden", "");
      // Resume polling.
      if (!ended && sessionId && !pollTimer) {
        pollTimer = setInterval(pollOnce, POLL_INTERVAL_MS);
      }
      // rc53.11 (Lesson 247): same close-and-return logic as secrets —
      // if opened from the OpenClaw chat window, navigate back to the
      // chat URL rather than stranding the user on the dashboard.
      try {
        const bridge = window.__openclawHostBridge;
        if (bridge && typeof bridge.closeOverlay === 'function') {
          bridge.closeOverlay();
          return;
        }
      } catch (_) { /* fall through */ }
    }
    attachCloseBtn.addEventListener("click", closeAttachOverlay);
    // Esc closes the overlay when it's open.
    attachOverlay.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeAttachOverlay();
    });
    document.getElementById("terminal-attach-btn").addEventListener("click", openAttachOverlay);

    // rc53.9 (Lesson 243): auto-open one of the overlays on mount if
    // requested. Three sources, in priority order:
    //   1. ctx.autoOpenOverlay (caller passed extras via navigate())
    //   2. URL hash #mcAutoOpen=secrets|attach (cross-origin path —
    //      the OpenClaw chat MC-PATCH toolbar navigates to
    //      tauri://localhost/index.html#mcAutoOpen=secrets because
    //      localStorage IS NOT shared across origins, but URL hashes
    //      make it through the cross-scheme WebView2 navigation).
    //   3. localStorage mc.terminal.autoOpenOverlay (same-origin
    //      fallback, e.g. future internal shortcuts).
    //
    // The flag is cleared after consumption so re-mounts (user
    // navigates Terminal → Dashboard → Terminal) don't re-open.
    let desiredAutoOpen = ctxAutoOpenOverlay;
    if (!desiredAutoOpen) {
      try {
        const hashMatch = (window.location.hash || "").match(
          /mcAutoOpen=(secrets|attach)/
        );
        if (hashMatch) desiredAutoOpen = hashMatch[1];
      } catch { /* ignore */ }
    }
    if (!desiredAutoOpen) {
      desiredAutoOpen = safeLocalGet("mc.terminal.autoOpenOverlay");
    }
    if (desiredAutoOpen === "secrets" || desiredAutoOpen === "attach") {
      // Clear all three sources so re-mounts don't re-open the overlay.
      try { safeLocalSet("mc.terminal.autoOpenOverlay", ""); } catch { /* ignore */ }
      try {
        if (window.location.hash && /mcAutoOpen=/.test(window.location.hash)) {
          // Strip just the mcAutoOpen param, leave the rest of the hash alone.
          const newHash = window.location.hash.replace(
            /[#&]?(mcAutoOpen=(secrets|attach))/,
            ""
          );
          history.replaceState(
            null,
            "",
            window.location.pathname + window.location.search + newHash
          );
        }
      } catch { /* ignore — hash strip is best-effort */ }
      // Defer one tick so listeners + DOM are fully settled (overlay
      // divs need to be appended first).
      setTimeout(() => {
        if (desiredAutoOpen === "secrets") openSecretsOverlay();
        else openAttachOverlay();
      }, 0);
    }

    // Fullscreen toggle: adds `terminal-fullscreen-mode` class to the page
    // root. CSS enlarges the output area. ResizeObserver triggers
    // fitAddon.fit() automatically; we also do an explicit fit on
    // next tick to race the layout engine's paint.
    const fullscreenBtn = document.getElementById("terminal-fullscreen");
    const pageRoot = root.querySelector(".terminal-page");
    if (safeLocalGet("mc.terminal.fullscreen") === "1") {
      pageRoot.classList.add("terminal-fullscreen-mode");
      fullscreenBtn.textContent = "⛶";
    }
    fullscreenBtn.addEventListener("click", () => {
      const on = pageRoot.classList.toggle("terminal-fullscreen-mode");
      safeLocalSet("mc.terminal.fullscreen", on ? "1" : "0");
      setTimeout(() => {
        try { fitAddon.fit(); } catch { /* ignore */ }
      }, 0);
    });

    const form = document.getElementById("terminal-input-form");
    const input = document.getElementById("terminal-input");

    // rc44: defensive input focus. xterm's helper textarea can re-grab
    // focus on any xterm interaction (resize, repaint, scroll). We
    // make sure the input always takes focus when interacted with,
    // and start with focus on the input so users can type immediately.
    const focusInput = () => {
      try {
        input.focus({ preventScroll: true });
      } catch {
        input.focus();
      }
    };
    input.addEventListener("mouseup", (e) => {
      // mouseup fires after the browser has already done its focus
      // shift; this is just a safety net for cases where xterm stole
      // focus back between mousedown and mouseup.
      e.preventDefault();
      focusInput();
    });
    input.addEventListener("focus", () => {
      // If xterm's helper textarea grabs focus back, the input won't
      // see the focus event. We force-blink focus here so the user
      // visibly sees they're typing in the right place.
    });

    // Make xterm container clicks release xterm focus so a subsequent
    // click on the input doesn't have to fight for it. Click INTO
    // xterm DOES focus xterm (intentional — that's how you select
    // text); clicks on the input take focus back.
    termContainer.addEventListener("mousedown", () => {
      // Allow click to focus xterm — that's normal terminal behavior.
    });
    input.addEventListener("click", focusInput);
    input.addEventListener("keydown", focusInput);

    // First focus on next tick so the input is ready before we focus.
    setTimeout(focusInput, 0);

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const text = input.value;
      if (!text) return;
      if (!sessionId) {
        appendSystem("no live session — click Restart");
        return;
      }
      // Local echo: render the typed prompt into xterm so the user sees
      // what they typed. Backend doesn't echo (piped stdio).
      term.write(`\r\n\x1b[36m> ${text}\x1b[0m\r\n`);
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
      // Close the secrets overlay if it's open (rc53.5)
      if (secretsOpen) closeSecretsOverlay();
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
      if (resizeObserver) {
        try { resizeObserver.disconnect(); } catch { /* ignore */ }
        resizeObserver = null;
      }
      if (sessionId) {
        try {
          await invoke("mc_terminal_kill", { id: sessionId });
        } catch {
          /* ignore */
        }
        sessionId = null;
      }
      if (term) {
        try { term.dispose(); } catch { /* ignore */ }
        term = null;
      }
    };
  },

  unmount() {
    const root = document.getElementById("root");
    if (root && typeof root._terminalCleanup === "function") {
      const c = root._terminalCleanup;
      delete root._terminalCleanup;
      c();
    }
  },
};