// pages/dashboard.js — post-login landing page (extracted from main.js v1.0.9-rc34).
//
// Public API: { mount(root, ctx), unmount() }
//
// Mount behavior:
//   - Fetches tier + nudge in parallel via Promise.allSettled.
//   - Renders the tier badge, usage bar, and the OpenClaw tile.
//   - Wires interactions (refresh tier, sign out, open OpenClaw).
//   - Shows tier-changed modal if downgraded; nudge modal if applicable.
//
// Unmount behavior:
//   - Removes any modal overlays that survived.
//   - Clears the "tile in-flight" state if a click was pending.
//
// Future hooks:
//   - Settings tile (rc35) will live alongside the OpenClaw tile here.
//   - The tile grid is already structured to accept more tiles.

import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openPalette as openCmdKPalette } from "../cmd_k_palette.js";

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
  return Number(n).toLocaleString("en-US");
}

export const dashboardPage = {
  label: "Dashboard",
  icon: null,
  requiresAuth: true,

  mount(root, ctx = {}) {
    const { onNeedsLogin, onOpenSettings, onOpenTerminal, onOpenOpenClawTerminal, onOpenFiles, onOpenNotebook } = ctx;

    (async () => {
      const [tierResult, nudgeResult] = await Promise.allSettled([
        invoke("mc_get_tier"),
        invoke("mc_get_nudge"),
      ]);

      // If the tier fetch failed with "not logged in", bounce back to login.
      if (tierResult.status === "rejected" &&
          String(tierResult.reason).includes("not logged in")) {
        if (onNeedsLogin) onNeedsLogin();
        return;
      }

      const tier = tierResult.status === "fulfilled" ? tierResult.value : null;
      const nudge = nudgeResult.status === "fulfilled" ? nudgeResult.value : null;

      const tierLabel = tier ? formatTierLabel(tier.tier) : "Unknown";
      const usageText = nudge
        ? `${formatNumber(nudge.used)} / ${formatNumber(nudge.limit)} tokens this period`
        : "—";

      root.innerHTML = `
        <div class="dashboard">
          <header class="dashboard-header">
            <h1 class="logo">MiracleClaw</h1>
            <div class="dashboard-header-actions">
              <button
                type="button"
                class="icon-link cmd-k-hint"
                id="cmd-k-open"
                title="Command palette (Ctrl+K)"
                aria-label="Open command palette"
              ><kbd>Ctrl</kbd>+<kbd>K</kbd></button>
              <button
                type="button"
                class="icon-link"
                id="settings-link"
                title="Open Settings"
                aria-label="Open Settings"
              >⚙</button>
              <div class="tier-badge" id="tier-badge" data-tier="${escapeHtml(
                tier?.tier || "free"
              )}" title="Click to refresh tier from MAIC">
                <span class="tier-label">${escapeHtml(tierLabel)}</span>
              </div>
            </div>
          </header>

          <div class="usage-bar" id="usage-bar" title="Token usage this period">
            <span class="usage-text">${escapeHtml(usageText)}</span>
          </div>

          <div class="tiles">
            <button class="tile tile-primary" id="openclaw-windows-tile" type="button">
              <div class="tile-icon">🦞</div>
              <div class="tile-body">
                <div class="tile-title">OpenClaw · Windows</div>
                <div class="tile-description">
                  Your MAIC chat workspace in a desktop window. Talk to any
                  MAIC model, run code, search the web, work with files.
                  Pro and above unlocks local file tools, command execution,
                  and persistent memory.
                </div>
                <div class="tile-cta">Open in new window →</div>
              </div>
            </button>
            <button class="tile tile-primary" id="openclaw-terminal-tile" type="button">
              <div class="tile-icon">⌨️</div>
              <div class="tile-body">
                <div class="tile-title">OpenClaw · Terminal</div>
                <div class="tile-description">
                  The same MAIC workspace, but in your terminal. Great for
                  SSH sessions, remote boxes, and keyboard-first workflows.
                  Same tools, same models, same memory.
                </div>
                <div class="tile-cta">Launch TUI →</div>
              </div>
            </button>
            <button class="tile" id="local-terminal-tile" type="button">
              <div class="tile-icon">💻</div>
              <div class="tile-body">
                <div class="tile-title">Local Terminal</div>
                <div class="tile-description">
                  Your OS shell — cmd.exe on Windows, bash on macOS and Linux.
                  Pick a different shell from the dropdown inside. Unsandboxed:
                  anything you type runs as you.
                </div>
                <div class="tile-cta">Open local shell →</div>
              </div>
            </button>
            <button class="tile" id="files-tile" type="button">
              <div class="tile-icon">📁</div>
              <div class="tile-body">
                <div class="tile-title">Files</div>
                <div class="tile-description">
                  Browse files in your Documents, Desktop, Downloads, and the
                  MC workspace. Click a file to preview it. Same allowlist as
                  the AI tools — you're always inside a known-safe folder.
                </div>
                <div class="tile-cta">Browse files →</div>
              </div>
            </button>
            <button class="tile" id="notebook-tile" type="button">
              <div class="tile-icon">📓</div>
              <div class="tile-body">
                <div class="tile-title">Notebook</div>
                <div class="tile-description">
                  Saved notes for the things you don't want to lose — research
                  snippets, command line tricks, half-formed ideas. Stored
                  locally on this machine; never leaves the device.
                </div>
                <div class="tile-cta">Open notebook →</div>
              </div>
            </button>
          </div>

          <!-- feature/drag-drop: file attachment staging zone. Drop a
               file from File Explorer onto this panel to stage it for
               the next "Send to chat" click. Files copy into MC's
               workspace inbox so chat tools can read them. -->
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

          <div class="dashboard-footer">
            <button class="link-button" id="refresh-tier">Refresh tier</button>
            <button class="link-button" id="signout">Sign out</button>
          </div>
        </div>
      `;

      // Wire interactions
      document.getElementById("openclaw-windows-tile").addEventListener("click", openOpenClaw);
      if (onOpenOpenClawTerminal) {
        document.getElementById("openclaw-terminal-tile").addEventListener("click", () => onOpenOpenClawTerminal());
      }
      if (onOpenTerminal) {
        document.getElementById("local-terminal-tile").addEventListener("click", () => onOpenTerminal());
      }
      if (onOpenFiles) {
        document.getElementById("files-tile").addEventListener("click", () => onOpenFiles());
      }
      if (onOpenNotebook) {
        document.getElementById("notebook-tile").addEventListener("click", () => onOpenNotebook());
      }
      document.getElementById("tier-badge").addEventListener("click", () => this.refreshTier(root, ctx));
      document.getElementById("refresh-tier").addEventListener("click", () => this.refreshTier(root, ctx));
      document.getElementById("signout").addEventListener("click", () => this.signOut(root, ctx));
      if (onOpenSettings) {
        document.getElementById("settings-link").addEventListener("click", () => onOpenSettings());
      }
      // Command palette button: visible "Ctrl+K" chip in the header so
      // users discover the shortcut. Without this, nobody would know
      // the shortcut exists (rc52 feedback).
      const cmdKBtn = document.getElementById("cmd-k-open");
      if (cmdKBtn) {
        cmdKBtn.addEventListener("click", () => openCmdKPalette());
      }

      // Wire the drag-and-drop attachment zone. Dragover/drop are
      // attached to the zone element so dropping anywhere on the page
      // doesn't accidentally trigger file navigation. The zone is also
      // focusable + click-to-open, but the clickable fallback is
      // intentionally minimal — newbies will discover drag on their own.
      wireAttachZone(root);

      if (tier?.tier_changed) showTierChangedModal(tier);
      if (nudge && nudge.text && nudge.text.length > 0) showNudgeModal(nudge);
    })();
  },

  async refreshTier(root, ctx) {
    try {
      await invoke("mc_refresh_tier");
      // Re-mount ourselves to pick up the new tier.
      this.mount(root, ctx);
    } catch (err) {
      if (String(err).includes("not logged in")) {
        if (ctx.onNeedsLogin) ctx.onNeedsLogin();
      } else {
        showFatal(`Could not refresh tier: ${escapeHtml(err)}`);
      }
    }
  },

  async signOut(root, ctx) {
    if (!confirm("Sign out of MiracleClaw? Your chat history in OpenClaw will remain, but you'll need to log in again next time.")) {
      return;
    }
    try {
      await invoke("maic_logout");
      if (ctx.onNeedsLogin) ctx.onNeedsLogin();
    } catch (err) {
      showFatal(`Could not sign out: ${escapeHtml(err)}`);
    }
  },

  unmount() {
    // Remove any modal overlays the dashboard left behind. The dashboard
    // appends overlays to document.body (not root), so they survive a
    // re-mount and look like ghosts. Tear them down here.
    document.querySelectorAll(".modal-overlay").forEach((el) => el.remove());
  },
};

// --- helpers below are not part of the page API ---

// feature/drag-drop: attachment staging zone. The zone is a self-contained
// subcomponent on the dashboard — it manages its own queue state, drag
// listeners, and the "Send to chat" handoff. Wire once per dashboard mount.
function wireAttachZone(root) {
  const zone = root.querySelector("#attach-zone");
  if (!zone) return;
  const emptyEl = root.querySelector("#attach-zone-empty");
  const queueEl = root.querySelector("#attach-queue");
  const actionsEl = root.querySelector("#attach-actions");
  const statusEl = root.querySelector("#attach-status");
  const messageEl = root.querySelector("#attach-message");
  const sendBtn = root.querySelector("#attach-send");
  const clearBtn = root.querySelector("#attach-clear");
  if (!zone || !emptyEl || !queueEl || !actionsEl || !statusEl || !messageEl || !sendBtn || !clearBtn) {
    return;
  }

  // Counter for "Upload N" dragover pulses. Lets us debounce the
  // status text update so we don't thrash the DOM on every mouse move.
  let dragDepth = 0;
  let lastQueue = [];

  const setStatus = (msg, kind = "info") => {
    statusEl.textContent = msg || "";
    statusEl.dataset.kind = kind;
  };

  const renderQueue = () => {
    if (!lastQueue.length) {
      emptyEl.hidden = false;
      queueEl.hidden = true;
      actionsEl.hidden = true;
      queueEl.innerHTML = "";
      return;
    }
    emptyEl.hidden = true;
    queueEl.hidden = false;
    actionsEl.hidden = false;
    queueEl.innerHTML = lastQueue
      .map(
        (a) => `
        <div class="attach-item" data-id="${escapeHtml(a.id)}">
          <span class="attach-item-icon">${iconFor(a)}</span>
          <span class="attach-item-name" title="${escapeHtml(a.staged_path)}">${escapeHtml(
          a.original_name
        )}</span>
          <span class="attach-item-size muted small">${formatBytes(a.size)}</span>
          <button type="button" class="link-button attach-item-remove" data-id="${escapeHtml(
            a.id
          )}">Remove</button>
        </div>
      `
      )
      .join("");
    queueEl.querySelectorAll(".attach-item-remove").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const removeId = btn.dataset.id;
        try {
          await invoke("mc_remove_attachment", { id: removeId });
          await refreshQueue();
          setStatus("Removed.");
        } catch (err) {
          setStatus(`Could not remove: ${err}`, "error");
        }
      });
    });
  };

  const refreshQueue = async () => {
    try {
      lastQueue = await invoke("mc_list_attachments");
      renderQueue();
    } catch (err) {
      setStatus(`Could not load queue: ${err}`, "error");
    }
  };

  // Prevent the browser's default "open file" behavior on accidental
  // drops outside the zone. Without preventDefault, dropping a file
  // anywhere on the page would navigate the webview to the file URL.
  const swallow = (e) => {
    e.preventDefault();
    e.stopPropagation();
  };
  document.addEventListener("dragenter", swallow);
  document.addEventListener("dragover", swallow);
  document.addEventListener("drop", swallow);

  zone.addEventListener("dragenter", (e) => {
    e.preventDefault();
    e.stopPropagation();
    dragDepth++;
    zone.classList.add("attach-zone-hot");
    setStatus("Drop to attach.");
  });
  zone.addEventListener("dragover", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  });
  zone.addEventListener("dragleave", (e) => {
    e.preventDefault();
    e.stopPropagation();
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) {
      zone.classList.remove("attach-zone-hot");
      setStatus("");
    }
  });
  zone.addEventListener("drop", async (e) => {
    e.preventDefault();
    e.stopPropagation();
    dragDepth = 0;
    zone.classList.remove("attach-zone-hot");
    if (!e.dataTransfer || !e.dataTransfer.files.length) {
      setStatus("Nothing to attach.", "error");
      return;
    }
    setStatus(`Staging ${e.dataTransfer.files.length} file(s)…`);
    let ok = 0;
    let lastErr = null;
    for (const file of e.dataTransfer.files) {
      // Tauri exposes the absolute path on the File object via .path
      // (WebView2 + wry). If unavailable we fall back to a clear error.
      const srcPath = file.path || null;
      if (!srcPath) {
        lastErr = `${file.name}: browser dropped the file without a path; try dragging from File Explorer.`;
        continue;
      }
      try {
        await invoke("mc_stage_attachment", {
          args: { src_path: srcPath, original_name: file.name },
        });
        ok++;
      } catch (err) {
        lastErr = `${file.name}: ${err}`;
      }
    }
    await refreshQueue();
    if (ok > 0 && lastErr) {
      setStatus(`Attached ${ok}, skipped: ${lastErr}`, "error");
    } else if (ok > 0) {
      setStatus(`Attached ${ok} file(s).`);
    } else {
      setStatus(lastErr || "Nothing attached.", "error");
    }
  });

  sendBtn.addEventListener("click", async () => {
    if (!lastQueue.length) {
      setStatus("Nothing to send.", "error");
      return;
    }
    sendBtn.disabled = true;
    setStatus("Opening chat & copying to clipboard…");
    try {
      const payload = await invoke("mc_send_attachments_to_chat", {
        userMessage: messageEl.value || null,
      });
      // Use the OS clipboard via the Tauri clipboard plugin (rc49).
      // navigator.clipboard.writeText() in JS depends on the webview
      // being focused; right after a drop the focus may be elsewhere
      // and the write silently no-ops. The plugin writes via the OS API
      // directly with no focus requirement.
      try {
        await writeText(payload);
        setStatus(
          "Copied to clipboard. OpenClaw window opened — paste with Ctrl+V."
        );
      } catch (clipErr) {
        setStatus(
          `Chat opened, but clipboard copy failed: ${clipErr}. Copy the file paths above manually.`,
          "error"
        );
      }
      // Clear the queue (and the staged files) after a successful send.
      try {
        await invoke("mc_clear_attachments");
      } catch (_) {
        // Non-fatal; user can clear manually if it fails.
      }
      messageEl.value = "";
      await refreshQueue();
    } catch (err) {
      setStatus(`Send failed: ${err}`, "error");
    } finally {
      sendBtn.disabled = false;
    }
  });

  clearBtn.addEventListener("click", async () => {
    try {
      const n = await invoke("mc_clear_attachments");
      await refreshQueue();
      setStatus(`Cleared ${n} attachment(s).`);
    } catch (err) {
      setStatus(`Could not clear: ${err}`, "error");
    }
  });

  refreshQueue();
}

function iconFor(att) {
  const ext = (att.original_name.split(".").pop() || "").toLowerCase();
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"].includes(ext))
    return "🖼";
  if (["pdf"].includes(ext)) return "📕";
  if (["doc", "docx"].includes(ext)) return "📘";
  if (["xls", "xlsx", "csv"].includes(ext)) return "📗";
  if (["ppt", "pptx"].includes(ext)) return "📙";
  if (["zip", "rar", "7z", "tar", "gz"].includes(ext)) return "🗜";
  if (["mp3", "wav", "flac"].includes(ext)) return "🎵";
  if (["mp4", "mov", "avi", "mkv"].includes(ext)) return "🎬";
  if (["js", "ts", "jsx", "tsx", "mjs", "cjs"].includes(ext)) return "🟨";
  if (["py"].includes(ext)) return "🐍";
  if (["rs"].includes(ext)) return "🦀";
  if (["json", "yaml", "yml", "toml"].includes(ext)) return "⚙";
  if (["md", "txt"].includes(ext)) return "📝";
  return "📄";
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

async function openOpenClaw() {
  const tile = document.getElementById("openclaw-windows-tile");
  if (tile) {
    tile.disabled = true;
    const label = tile.querySelector(".tile-cta") || tile;
    label.dataset.oldText = label.dataset.oldText ?? label.textContent;
    label.textContent = "Starting…";
  }
  try {
    const gw = await invoke("start_gateway_after_login");
    console.log("[dashboard] gateway ready:", gw);
    const result = await invoke("openclaw_open_window");
    console.log("[dashboard] openclaw window:", result);
  } catch (err) {
    console.error("[dashboard] could not open OpenClaw:", err);
    showFatal(`Could not open OpenClaw: ${escapeHtml(err)}`);
  } finally {
    if (tile) {
      tile.disabled = false;
      const label = tile.querySelector(".tile-cta") || tile;
      if (label.dataset.oldText) {
        label.textContent = label.dataset.oldText;
        delete label.dataset.oldText;
      }
    }
  }
}

function showTierChangedModal(tierInfo) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal">
      <h2>Your subscription changed</h2>
      <p>Your MAIC tier is now <strong>${escapeHtml(formatTierLabel(tierInfo.tier))}</strong>. Some tools are no longer available.</p>
      <p class="muted small">Local file tools and bash execution require Pro or above.</p>
      <div class="modal-actions">
        <button type="button" id="tier-modal-dismiss">Got it</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);
  document.getElementById("tier-modal-dismiss").addEventListener("click", () => {
    overlay.remove();
  });
}

function showNudgeModal(nudge) {
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  const cta = nudge.kind === "free_cap" || nudge.kind === "paid_100"
    ? `<a href="https://milagrocloud.com/upgrade" target="_blank" rel="noopener" class="cta">Upgrade to Pro →</a>`
    : "";
  overlay.innerHTML = `
    <div class="modal">
      <h2>${escapeHtml(nudge.kind === "free_cap" || nudge.kind === "paid_100" ? "You've hit your limit" : "Token usage update")}</h2>
      <p>${escapeHtml(nudge.text)}</p>
      ${cta}
      <div class="modal-actions">
        <button type="button" id="nudge-modal-dismiss">${cta ? "Maybe later" : "Got it"}</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);
  document.getElementById("nudge-modal-dismiss").addEventListener("click", () => {
    overlay.remove();
  });
}

function showFatal(msg) {
  // Render the fatal overlay into document.body so it survives any
  // unmount that might be racing us.
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
