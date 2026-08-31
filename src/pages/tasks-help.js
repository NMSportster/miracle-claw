// src/pages/tasks-help.js — Tasks page help overlay (rc55.18, 2026-08-30)
//
// David 2026-08-30 18:08 MDT: "create a help file in Tasks and tutorial
// for users that might want help using that page."
//
// Renders a full-screen overlay with plain-language framing for the
// Tasks page. Content comes from src/data/tasks-help.js.
//
// Modeled after src/pages/module-help.js (Add-On Module help) so the
// visual language matches — same sections (whoFor, whyUseIt, whatItDoes,
// windowsUI, examples, chat, troubleshooting), same overlay structure,
// same keyboard handlers (Escape closes, click backdrop closes).
//
// Why a separate file from module-help.js:
//   - module-help.js pulls metadata from `.modules-card[data-module-id=…]`
//     (only present on the Modules page). Tasks isn't a module card.
//   - Tasks help needs its own meta (name="Tasks", icon="✅", no card lookup).
//   - Splitting keeps module-help.js's regression risk = zero.
//
// Usage:
//   import { openTasksHelp } from "./tasks-help.js";
//   openTasksHelp();
//
// The Tasks page header has a ❔ button (rendered in src/pages/tasks.js)
// that calls openTasksHelp().
//
// CSS:
//   - Reuses the .mhelp-* selectors from src/styles.css (41 selectors).
//     No new CSS needed; same visual identity as Module Help.
//
// Schema (from src/data/tasks-help.js):
//   whoFor, whyUseIt, whatItDoes, windowsUI, examples, chat,
//   troubleshooting. See that file for details.
//
// All content is plain text — no markdown, no HTML. The renderer
// escapes it for safety. Keep examples concrete (real commands, real
// expected outputs) so the user can copy-paste-test.

import { TASKS_HELP } from "../data/tasks-help.js";

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

  // Plain-language framing for newer / non-CLI users (David 2026-08-26
  // 16:33 MDT on module-help: "have to do a good job of explaining
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

  // Using-it-in-MiracleClaw: per-surface walkthroughs. Tasks has its
  // own UI surfaces (Dashboard → Tasks page, header buttons, date
  // groupings, syncing with MAIC) so the help walks through each.
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

  // Example things to ask MAIC — most important section for newer
  // users (David 2026-08-26: "for the newer type user"). Shows the
  // natural-language prompts that drive the chat-side task tools.
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

  // Slash-command reference for chat. Tasks's chat-side surface is
  // MAIC's `mc_task_*` tools (list_tasks, add_task, update_task,
  // complete_task, delete_task). Listed as natural-language examples
  // in the "examples" section; this table gives the canonical verbs
  // for power users.
  if (help.chat && help.chat.length) {
    blocks.push(`
      <section class="mhelp-section">
        <h2 class="mhelp-h2">💬 Talking to MAIC <span class="mhelp-tag">natural language</span></h2>
        <table class="mhelp-cmd-table">
          <thead><tr><th>What to say</th><th>What MAIC does</th></tr></thead>
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

  // Troubleshooting — common gotchas, in plain English. No jargon.
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

function buildOverlay(help, pageMeta) {
  const overlay = document.createElement("div");
  overlay.className = "mhelp-overlay";
  overlay.setAttribute("role", "dialog");
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("aria-label", `Help for ${pageMeta.name}`);
  overlay.innerHTML = `
    <div class="mhelp-backdrop" data-close="1"></div>
    <div class="mhelp-panel">
      <header class="mhelp-header">
        <span class="mhelp-icon" aria-hidden="true">${escapeHtml(pageMeta.icon || "❔")}</span>
        <div class="mhelp-titles">
          <h1 class="mhelp-title">${escapeHtml(pageMeta.name)}</h1>
          <span class="mhelp-subtitle">${escapeHtml(help.whatItDoes)}</span>
        </div>
        <button class="mhelp-close" type="button" aria-label="Close help" data-close="1">✕</button>
      </header>
      <div class="mhelp-body">
        ${renderSections(help)}
      </div>
      <footer class="mhelp-footer">
        <span class="mhelp-meta muted small">
          v${escapeHtml(pageMeta.version)} · ${escapeHtml(pageMeta.publisher || "")}
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
  overlay._tasksHelpCleanup = () => {
    document.removeEventListener("keydown", onKey);
  };
}

/**
 * Open the Tasks help overlay. No-op if already open.
 * Safe to call multiple times — each call closes the previous
 * overlay before opening a fresh one.
 */
export function openTasksHelp() {
  // Close any existing overlay first (Tasks help OR leftover Module
  // help — both use .mhelp-overlay so we collapse them together).
  const existing = document.querySelector(".mhelp-overlay");
  if (existing) {
    existing._tasksHelpCleanup?.();
    existing._mhelpCleanup?.();
    existing.remove();
  }

  const help = TASKS_HELP;

  // Tasks page is meta — no DOM lookup needed (Tasks isn't a card in
  // a card grid). Pulled directly from the data file.
  const pageMeta = {
    name: help.name || "Tasks",
    icon: help.icon || "✅",
    version: help.version || "?",
    publisher: help.publisher || "",
  };

  const overlay = buildOverlay(help, pageMeta);

  const close = () => {
    overlay._tasksHelpCleanup?.();
    overlay.remove();
  };

  attachHandlers(overlay, close);
  document.body.appendChild(overlay);

  // Focus the close button so Esc/Tab work naturally.
  overlay.querySelector(".mhelp-close")?.focus();
}

/**
 * Close the Tasks help overlay if it's open.
 * Useful for keyboard shortcuts / "?" keypress handlers elsewhere.
 */
export function closeTasksHelp() {
  const existing = document.querySelector(".mhelp-overlay");
  if (existing) {
    existing._tasksHelpCleanup?.();
    existing._mhelpCleanup?.();
    existing.remove();
  }
}