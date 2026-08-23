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
