// src/pages/module-help.js — Add-On Module help overlay (rc53.28, 2026-08-26;
// enriched for non-CLI users in rc53.29, 2026-08-26, per David:
//
//   "have to do a good job of explaining these programs for the newer
//    type user that has never used commands, veteran cli users should
//    have no trouble"
//
// Renders a full-screen overlay with plain-language framing for every
// Add-On Module card. Content comes from src/data/module-help.js.
//
// Usage:
//   import { openModuleHelp } from "./module-help.js";
//   openModuleHelp("voice");
//
// The overlay is a single root div appended to <body> and removed on
// close (Escape key, click backdrop, click close button). Re-opening
// the same module reuses no state — each open() builds a fresh tree.
//
// Schema (each module can use any subset of these):
//   whoFor        — one-line audience tag, e.g. "Anyone who types
//                    faster than they talk"
//   whyUseIt      — one-line value prop in plain English, no jargon
//   whatItDoes    — one-liner shown as the overlay's subtitle
//   windowsUI     — list of {surface, steps} for the Tauri desktop UI
//                    surfaces (Dashboard / Terminal / Files / Settings /
//                    Chat / Extras hub). Each surface = a clickable flow.
//   examples      — list of natural-language prompts a user can paste
//                    into MAIC chat. The most important section for
//                    newer users — shows what the AI can actually do.
//   terminal      — list of {cmd, desc} of mc-* slash commands or
//                    invokeModule calls usable in the Terminal page.
//   chat          — list of @module-slash commands usable in MAIC chat.
//                    Empty if the module has no chat-side surface.
//   troubleshooting — list of {problem, fix} for the most common gotchas.
//
// All content is plain text — no markdown, no HTML. The renderer escapes
// it for safety. Keep examples concrete (real commands, real expected
// outputs) so the user can copy-paste-test.
import { getModuleHelp } from "../data/module-help.js";

function escapeHtml(s) {
  if (s == null) return "";
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderSections(help) {
  const blocks = [];

  // rc53.29: plain-language framing for newer / non-CLI users
  // (David 2026-08-26 16:33 MDT: "have to do a good job of explaining
  // these programs for the newer type user that has never used
  // commands, veteran cli users should have no trouble"). whoFor and
  // whyUseIt are short, human-readable intros that set context
  // BEFORE the technical sections.
  if (help.whoFor || help.whyUseIt) {
    const who = help.whoFor
      ? `<div class="mhelp-intro-who"><span class="mhelp-intro-label">Who's it for:</span> ${escapeHtml(help.whoFor)}</div>`
      : "";
    const why = help.whyUseIt
      ? `<div class="mhelp-intro-why"><span class="mhelp-intro-label">Why you'd use it:</span> ${escapeHtml(help.whyUseIt)}</div>`
      : "";
    blocks.push(`<section class="mhelp-section mhelp-intro">${who}${why}</section>`);
  }

  if (help.windowsUI && help.windowsUI.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">🖥️ Using it in MiracleClaw</h2>
        ${help.windowsUI
          .map(
            (s) => `
          <div class="mhelp-subsection">
            <h3 class="mhelp-h3">${escapeHtml(s.surface)}</h3>
            <ol class="mhelp-steps">
              ${s.steps.map((step) => `<li>${escapeHtml(step)}</li>`).join("")}
            </ol>
          </div>
        `
          )
          .join("")}
      </section>
    `);
  }

  if (help.examples && help.examples.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">💡 Example things to ask MAIC</h2>
        <ul class="mhelp-examples">
          ${help.examples
            .map((ex) => `<li><span class="mhelp-example-prompt">${escapeHtml(ex)}</span></li>`)
            .join("")}
        </ul>
      </section>
    `);
  }

  if (help.terminal && help.terminal.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">⌨️ Command-line reference <span class="mhelp-tag">for power users</span></h2>
        <table class="mhelp-cmd-table">
          <thead><tr><th>Command</th><th>What it does</th></tr></thead>
          <tbody>
            ${help.terminal
              .map(
                (c) => `
              <tr>
                <td><code class="mhelp-cmd">${escapeHtml(c.cmd)}</code></td>
                <td>${escapeHtml(c.desc)}</td>
              </tr>
            `
              )
              .join("")}
          </tbody>
        </table>
      </section>
    `);
  }

  if (help.chat && help.chat.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">💬 Slash commands in MAIC Chat <span class="mhelp-tag">for power users</span></h2>
        <table class="mhelp-cmd-table">
          <thead><tr><th>Command</th><th>What it does</th></tr></thead>
          <tbody>
            ${help.chat
              .map(
                (c) => `
              <tr>
                <td><code class="mhelp-cmd">${escapeHtml(c.cmd)}</code></td>
                <td>${escapeHtml(c.desc)}</td>
              </tr>
            `
              )
              .join("")}
          </tbody>
        </table>
      </section>
    `);
  }

  if (help.troubleshooting && help.troubleshooting.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">🔧 If something goes wrong</h2>
        <dl class="mhelp-ts">
          ${help.troubleshooting
            .map(
              (t) => `
            <dt class="mhelp-ts-q">${escapeHtml(t.problem)}</dt>
            <dd class="mhelp-ts-a">${escapeHtml(t.fix)}</dd>
          `
            )
            .join("")}
        </dl>
      </section>
    `);
  }

  return blocks.join("");
}

function buildOverlay(id, help, moduleMeta) {
  const overlay = document.createElement("div");
  overlay.className = "mhelp-overlay";
  overlay.setAttribute("role", "dialog");
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("aria-label", `Help for ${moduleMeta.name}`);
  overlay.innerHTML = `
    <div class="mhelp-backdrop" data-close="1"></div>
    <div class="mhelp-panel">
      <header class="mhelp-header">
        <span class="mhelp-icon" aria-hidden="true">${escapeHtml(moduleMeta.icon || "❔")}</span>
        <div class="mhelp-titles">
          <h1 class="mhelp-title">${escapeHtml(moduleMeta.name)}</h1>
          <span class="mhelp-subtitle">${escapeHtml(help.whatItDoes)}</span>
        </div>
        <button class="mhelp-close" type="button" aria-label="Close help" data-close="1">✕</button>
      </header>
      <div class="mhelp-body">
        ${renderSections(help)}
      </div>
      <footer class="mhelp-footer">
        <span class="mhelp-meta muted small">
          v${escapeHtml(moduleMeta.version)} · ${escapeHtml(moduleMeta.publisher || "")}
        </span>
        <button class="mhelp-ok" type="button" data-close="1">Got it</button>
      </footer>
    </div>
  `;
  return overlay;
}

function attachHandlers(overlay, onClose) {
  const onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };
  overlay.addEventListener("click", (e) => {
    if (e.target.dataset.close === "1") {
      onClose();
    }
  });
  document.addEventListener("keydown", onKey);
  overlay._mhelpCleanup = () => {
    document.removeEventListener("keydown", onKey);
  };
}

export function openModuleHelp(id) {
  // Close any existing overlay first.
  const existing = document.querySelector(".mhelp-overlay");
  if (existing) {
    existing._mhelpCleanup?.();
    existing.remove();
  }

  const help = getModuleHelp(id);
  if (!help) {
    console.warn(`[mhelp] no help content for module id=${id}`);
    return;
  }

  // Pull module metadata from the catalog card if present.
  const card = document.querySelector(
    `.modules-card[data-module-id="${CSS.escape(id)}"]`
  );
  const moduleMeta = {
    id,
    name: card?.querySelector(".modules-card-name")?.textContent || id,
    icon: card?.querySelector(".modules-icon")?.textContent || "❔",
    version:
      card?.querySelector(".modules-version")?.textContent?.replace(/^v/, "") ||
      "?",
    publisher:
      card?.querySelector(".modules-card-publisher")?.textContent?.replace(
        /^by\s+/,
        ""
      ) || "",
  };

  const overlay = buildOverlay(id, help, moduleMeta);

  const close = () => {
    overlay._mhelpCleanup?.();
    overlay.remove();
  };

  attachHandlers(overlay, close);
  document.body.appendChild(overlay);

  // Focus the close button so Esc/Tab work naturally.
  overlay.querySelector(".mhelp-close")?.focus();
}

export function closeModuleHelp() {
  const existing = document.querySelector(".mhelp-overlay");
  if (existing) {
    existing._mhelpCleanup?.();
    existing.remove();
  }
}