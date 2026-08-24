// src/pages/extras.js — Extras hub (rc53.8, feature/extras-hub)
//
// A single page listing all `mlg-*` commands shipped by
// `miracle-claw-extras` (pip-installed). Each card has:
//   - icon + command name
//   - 1-line description
//   - "Run" button that opens the Terminal page with `cmd` shell
//     (or `bash` on Linux/macOS) and pre-fills the input with the
//     command so the user sees its output stream live.
//
// Page structure mirrors src/pages/secrets.js (back button + title bar
// + content grid + zero new backend commands).
//
// Why a hub instead of 10 palette entries?
//   - Discoverability: users see what's available without memorizing names.
//   - Grouped context: doctor + bench + stats read better together.
//   - Palette can still launch individual commands — the hub is the
//     landing page for "what can extras do?" and the palette is for
//     "I know the exact command I want."

import { navigate } from "../navigation.js";

const MLG_COMMANDS = [
  {
    name: "mlg-doctor",
    icon: "🩺",
    blurb: "Check that your Miracle Claw install is healthy — config, paths, dependencies, network.",
    tags: ["diagnostics", "first run"],
  },
  {
    name: "mlg-stats",
    icon: "📊",
    blurb: "Show usage statistics from local session history — token counts, top models, peak hours.",
    tags: ["analytics", "history"],
  },
  {
    name: "mlg-cost",
    icon: "💵",
    blurb: "Token cost analytics from local history. Per-day, per-model, per-session dollar estimates.",
    tags: ["analytics", "billing"],
  },
  {
    name: "mlg-diff",
    icon: "🔍",
    blurb: "Diff history entries, configs, or snapshots. Useful to see what changed between sessions.",
    tags: ["history", "compare"],
  },
  {
    name: "mlg-export",
    icon: "📤",
    blurb: "Render session history to shareable formats — Markdown, HTML, JSON.",
    tags: ["history", "share"],
  },
  {
    name: "mlg-share",
    icon: "🤝",
    blurb: "Bundle a session snapshot for sharing with collaborators. Produces a .mlgsnap file.",
    tags: ["share", "snapshot"],
  },
  {
    name: "mlg-restore",
    icon: "📥",
    blurb: "Import a .mlgsnap bundle as a local snapshot. Inverse of mlg-share.",
    tags: ["share", "snapshot"],
  },
  {
    name: "mlg-template",
    icon: "🧱",
    blurb: "Scaffold a new project from a built-in or custom template (Python, Tauri, MCP server, etc.).",
    tags: ["scaffold", "new project"],
  },
  {
    name: "mlg-bench",
    icon: "⏱️",
    blurb: "Benchmark prompt latency against local models. Stub in v0.3 — full impl in a later week.",
    tags: ["benchmark", "latency"],
  },
  {
    name: "mlg-config",
    icon: "⚙️",
    blurb: "Interactive config generator. Walks you through the most useful extras.json settings.",
    tags: ["config", "setup"],
  },
];

function detectOS() {
  if (typeof navigator === "undefined") return "linux";
  const ua = navigator.userAgent || "";
  if (/Windows/i.test(ua)) return "windows";
  if (/Mac/i.test(ua)) return "mac";
  return "linux";
}

export const extrasPage = {
  /**
   * Page factory. `mount(root, ctx)` is called by the navigation
   * router; we render the hub into `root` and wire Run handlers that
   * navigate to the Terminal page with the right shell + initial
   * command (consumed by terminal.js's `initialCommand` ctx param,
   * added in rc53.8).
   */
  mount(root, ctx) {
    const os = detectOS();
    const shell = os === "windows" ? "cmd" : "bash";

    root.innerHTML = `
      <section class="page page-extras">
        <header class="page-header">
          <button class="back-btn" type="button" aria-label="Back to dashboard">←</button>
          <h1 class="page-title">🛠 Extras</h1>
          <span class="page-subtitle">Companion CLI tools for Miracle Claw</span>
        </header>
        <p class="extras-intro">
          Each card opens <code>${shell}</code> in the Terminal page and runs the
          command live. Press Esc or click ✕ in the Terminal toolbar to come back.
        </p>
        <div class="extras-grid" id="extras-grid"></div>
        <footer class="extras-footer">
          <small>
            Installed via <code>pip install miracle-claw-extras</code>.
            Update with <code>pip install -U miracle-claw-extras</code>.
          </small>
        </footer>
      </section>
    `;

    // Wire back button.
    const back = root.querySelector(".back-btn");
    if (back && ctx && typeof ctx.onBackToDashboard === "function") {
      back.addEventListener("click", ctx.onBackToDashboard);
    }

    // Render cards.
    const grid = root.querySelector("#extras-grid");
    if (grid) {
      for (const cmd of MLG_COMMANDS) {
        grid.appendChild(renderCard(cmd, shell, ctx));
      }
    }

    // Cleanup for the navigation router — no listeners here that
    // would leak; future work could add hot-reload of the command
    // list if extras are updated without an MC restart.
    root._extrasCleanup = async () => {
      // No-op for v1.
    };
  },
};

function renderCard(cmd, shell, ctx) {
  const card = document.createElement("article");
  card.className = "extras-card";
  card.dataset.cmd = cmd.name;

  const tags = cmd.tags
    .map((t) => `<span class="extras-tag">${escapeHtml(t)}</span>`)
    .join("");

  card.innerHTML = `
    <div class="extras-card-head">
      <span class="extras-icon" aria-hidden="true">${cmd.icon}</span>
      <code class="extras-name">${escapeHtml(cmd.name)}</code>
    </div>
    <p class="extras-blurb">${escapeHtml(cmd.blurb)}</p>
    <div class="extras-tags">${tags}</div>
    <div class="extras-actions">
      <button class="extras-run" type="button" data-cmd="${escapeHtml(cmd.name)}">
        ▶ Run in Terminal
      </button>
    </div>
  `;

  const runBtn = card.querySelector(".extras-run");
  if (runBtn) {
    runBtn.addEventListener("click", () => {
      // Hand off to the Terminal page with initialCommand ctx.
      // The terminal will spawn cmd/bash, wait ~350ms for the
      // prompt, then write `mlg-...` + newline — and the user
      // sees the output stream live.
      if (ctx && typeof ctx.onOpenTerminalWithCommand === "function") {
        ctx.onOpenTerminalWithCommand(cmd.name, shell);
      } else if (ctx && typeof ctx.onOpenTerminal === "function") {
        // Fallback: open terminal without auto-run if the ctx
        // builder doesn't wire onOpenTerminalWithCommand.
        ctx.onOpenTerminal();
      }
    });
  }

  return card;
}

function escapeHtml(s) {
  if (typeof s !== "string") return "";
  return s.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case "&": return "&amp;";
      case "<": return "&lt;";
      case ">": return "&gt;";
      case "\"": return "&quot;";
      case "'": return "&#39;";
      default: return c;
    }
  });
}

// Cmd-K palette integration: each mlg-* command is registered as a
// top-level palette action ("Run mlg-doctor", etc.) so users who know
// the exact command can launch it without opening the hub first.
//
// rc53.8: field name must be `category` to match the palette's
// groupBy/sort logic (was originally `section` — caught at compile).
export const extrasPaletteActions = MLG_COMMANDS.map((cmd) => ({
  id: `extras-${cmd.name}`,
  label: `Run ${cmd.name}`,
  hint: cmd.blurb,
  icon: cmd.icon,
  category: "Extras",
  keywords: [cmd.name, "extras", "cli", ...cmd.tags],
  run: () => {
    const os = detectOS();
    const shell = os === "windows" ? "cmd" : "bash";
    navigate("terminal", { defaultShell: shell, initialCommand: cmd.name });
  },
}));