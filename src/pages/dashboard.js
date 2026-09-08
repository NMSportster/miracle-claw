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
// Lesson 571 (2026-08-25 00:37 MDT, David): MC Module Framework —
// dashboard FAB calls the voice module via the JS-side runtime.
// Button starts greyed out (data-module-voice-installed="false").
import { isModuleInstalled, invokeModule, ensureVoiceModel } from "../modules-runtime.js";
import { toast } from "../toast.js";

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

// Lesson 564 (2026-08-24 17:28 MDT, David): previously we rendered a
// 6-card pricing grid on the dashboard. David didn't want the
// dashboard cluttered — just a small button below the token line that
// opens the real /pricing page in the OS browser. The in-app upgrade
// card is removed; `mc_list_plans` is no longer called. The
// `mc_open_checkout_url` command stays (Lesson 561) so the /pricing
// page itself can hand a user straight to Stripe when they click
// "Choose <Plan>" there. We just stopped preloading that into the
// dashboard.
// Lesson 574 (2026-08-25 08:19 MDT, David): put Add-On Modules to the
// right of Plans & Pricing on the same line, not stacked above. Cleaner.
function renderLinkButtons() {
  return `
    <div class="dashboard-link-row">
      ${renderPlansButton()}
      ${renderModulesButton()}
    </div>`;
}
function renderPlansButton() {
  return `
    <button type="button"
            class="pricing-link-btn"
            id="pricing-link-btn"
            title="Open pricing page in your browser">
      Plans &amp; Pricing
      <span class="pricing-link-arrow" aria-hidden="true">↗</span>
    </button>`;
}

// Lesson 573 (2026-08-25 07:22 MDT, David): Add-On Modules catalog hub.
// Mirrors the pricing button styling + arrow so the two read as a button
// pair. Routes to the in-app modules catalog page (not the OS browser).
function renderModulesButton() {
  return `
    <button type="button"
            class="modules-link-btn"
            id="modules-link-btn"
            title="Browse the Add-On Modules catalog">
      Add-On Modules
      <span class="modules-link-arrow" aria-hidden="true">↗</span>
    </button>`;
}

// Lesson 833 Phase 5: workflow-run banner. When the user kicks off a
// workflow and navigates back to the dashboard, we show a small
// banner at the top of the tiles area that reads
//   - "Workflow 'Code Review' is running" (status: running), or
//   - "Workflow 'Code Review' finished" (status: complete / killed / error).
// Clicking the banner navigates to the workflows page where the run
// panel is already on screen.
//
// We read the same sessionStorage key the workflows page writes to
// (mc.workflows.active_run.v1). sessionStorage is per-tab, so
// closing the tab or finishing + clearing the run makes the banner
// disappear without manual cleanup.
function renderWorkflowRunBanner() {
  let active = null;
  try {
    const raw = sessionStorage.getItem("mc.workflows.active_run.v1");
    if (!raw) return "";
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed.sessionId !== "string") return "";
    active = parsed;
  } catch (e) { return ""; }
  if (!active) return "";

  const status = active.status || "running";
  const name = active.workflowName || "Workflow";
  const isFinished = status === "complete" || status === "killed" || status === "error";
  const statusLabel = isFinished
    ? status === "complete"
      ? "finished"
      : status === "killed"
      ? "was stopped"
      : "had a problem"
    : "is running";
  const icon = isFinished
    ? status === "complete"
      ? "✅"
      : status === "killed"
      ? "⏹"
      : "⚠"
    : "✨";
  const cls = `workflow-banner workflow-banner-${isFinished ? "finished" : "running"} workflow-banner-status-${status}`;

  return `
    <button type="button"
            class="${cls}"
            id="workflow-run-banner"
            title="Open the workflow run">
      <span class="workflow-banner-icon">${icon}</span>
      <span class="workflow-banner-text">
        <strong>${escapeHtml(name)}</strong>
        <span class="workflow-banner-status">${statusLabel}</span>
      </span>
      <span class="workflow-banner-cta">View run →</span>
    </button>
  `;
}

// =============================================================================
// Voice transcription preview modal (rc53.29, David 2026-08-26 16:33 MDT)
//
// Lesson 596: every sidecar returning text needs a visible UI surface,
// not just a clipboard fallback. The dashboard has no chat input, so
// the old code copied the transcript to clipboard and toa "Copied to
// clipboard". David reported "hears the speech but doesn't write to
// the input area" — he assumed it failed because there was no
// visible result.
//
// This modal shows:
//   - The captured transcript text in an editable textarea
//   - Copy / Try again / Send to OpenClaw buttons
//   - A collapsible Diagnostics section with the v0.1.8 fields so
//     we can debug "No Speech Detected" failures
// =============================================================================

let voiceModalEl = null;

function buildDiagnosticsHtml(result) {
  if (!result || !result.diagnostics) return "";
  const d = result.diagnostics;
  const vadStats = d.vadStats || {};
  const rows = [
    ["Peak amplitude", typeof d.peakAmplitude === "number" ? d.peakAmplitude.toFixed(4) : "—"],
    ["RMS", typeof d.rms === "number" ? d.rms.toFixed(4) : "—"],
    ["VAD enabled", d.vadEnabled ? "yes" : "no"],
    ["Total frames", vadStats.totalFrames ?? "—"],
    ["Speech frames", vadStats.speechFrames ?? "—"],
    ["Speech ratio", typeof vadStats.speechRatio === "number" ? vadStats.speechRatio.toFixed(2) : "—"],
    ["Silence tripped", vadStats.silenceTripped ? "yes" : "no"],
    ["Duration (ms)", result.duration_ms ?? "—"],
    ["Transcribe (ms)", result.transcribe_ms ?? "—"],
  ];
  return `
    <dl class="voice-modal-diag">
      ${rows.map(([k, v]) => `<dt>${escapeHtml(k)}</dt><dd>${escapeHtml(String(v))}</dd>`).join("")}
    </dl>
  `;
}

function interpretDiagnostics(result) {
  // Lesson 592 (rc53.28, voice v0.1.8): turn raw diagnostics into a
  // plain-language hint about WHY transcription might have failed.
  const d = result?.diagnostics;
  if (!d) return "";
  const peak = d.peakAmplitude ?? 0;
  const speechFrames = d.vadStats?.speechFrames ?? 0;
  if (result.text && result.text.trim()) return "";
  if (peak < 0.001) {
    return "Your microphone looks silent — peak amplitude is near zero. Check that the mic is plugged in, not muted, and that the right input device is selected in your system sound settings.";
  }
  if (d.vadEnabled && speechFrames === 0) {
    return "Sound is reaching the mic, but the Voice Activity Detector didn't classify any of it as speech. Try speaking louder, getting closer to the mic, or disabling VAD in Settings → Modules → Voice.";
  }
  if (result.warning) return result.warning;
  return "Whisper heard audio but didn't transcribe any words. Try speaking more clearly, or check that you're on the latest voice module (v0.1.8).";
}

function openVoiceResultModal(result, opts = {}) {
  // Close any existing voice modal first.
  if (voiceModalEl) {
    voiceModalEl._cleanup?.();
    voiceModalEl.remove();
    voiceModalEl = null;
  }
  const text = (result && result.text ? String(result.text) : "").trim();
  const hasText = !!text;
  const warning = (result && result.warning) || "";
  const hint = interpretDiagnostics(result);

  const overlay = document.createElement("div");
  overlay.className = "voice-modal-overlay";
  overlay.setAttribute("role", "dialog");
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("aria-label", "Voice transcription result");
  overlay.innerHTML = `
    <div class="voice-modal-backdrop" data-close="1"></div>
    <div class="voice-modal-panel">
      <header class="voice-modal-header">
        <span class="voice-modal-icon" aria-hidden="true">🎙</span>
        <div class="voice-modal-titles">
          <h1 class="voice-modal-title">${hasText ? "Voice transcript" : (warning || hint ? "No transcript captured" : "Voice transcript")}</h1>
          <span class="voice-modal-subtitle">${hasText ? "Edit if needed, then choose where to send it." : "What the mic heard."}</span>
        </div>
        <button class="voice-modal-close" type="button" aria-label="Close" data-close="1">✕</button>
      </header>
      <div class="voice-modal-body">
        ${hasText ? `
          <textarea class="voice-modal-text"
                    id="voice-modal-text"
                    rows="6"
                    spellcheck="true"
                    aria-label="Voice transcript (editable)">${escapeHtml(text)}</textarea>
        ` : `
          <div class="voice-modal-empty">
            <div class="voice-modal-empty-msg">
              ${escapeHtml(hint || warning || "The mic didn't capture any speech this time.")}
            </div>
            <button type="button" class="voice-modal-cta primary" data-action="retry">
              🎙 Try again
            </button>
          </div>
        `}
        ${hint && hasText ? `
          <div class="voice-modal-hint muted small">${escapeHtml(hint)}</div>
        ` : ""}
        <details class="voice-modal-diag-details">
          <summary>Diagnostics</summary>
          ${buildDiagnosticsHtml(result)}
        </details>
      </div>
      ${hasText ? `
        <footer class="voice-modal-footer">
          <button type="button" class="voice-modal-cta secondary" data-action="copy">
            📋 Copy
          </button>
          <button type="button" class="voice-modal-cta secondary" data-action="retry">
            🎙 Record again
          </button>
          <button type="button" class="voice-modal-cta primary" data-action="send-chat">
            💬 Send to OpenClaw chat
          </button>
        </footer>
      ` : ""}
    </div>
  `;

  // Escape key + backdrop close.
  const onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      closeVoiceResultModal();
    }
  };
  overlay.addEventListener("click", (e) => {
    if (e.target.dataset?.close === "1") {
      closeVoiceResultModal();
    } else if (e.target.dataset?.action === "copy") {
      const ta = overlay.querySelector("#voice-modal-text");
      const value = ta ? ta.value : text;
      navigator.clipboard
        .writeText(value)
        .then(() => toast("Transcript copied to clipboard", { kind: "success" }))
        .catch(() => writeText(value).then(() => toast("Transcript copied to clipboard", { kind: "success" })));
    } else if (e.target.dataset?.action === "retry") {
      closeVoiceResultModal();
      opts.onRetry?.();
    } else if (e.target.dataset?.action === "send-chat") {
      const ta = overlay.querySelector("#voice-modal-text");
      const value = ta ? ta.value : text;
      Promise.resolve()
        .then(async () => {
          try {
            await writeText(value);
          } catch {
            // ignore — we'll still open the window
          }
          try {
            await invoke("openclaw_open_window");
            toast("OpenClaw chat opened — press Ctrl+V in the chat input to paste.", { kind: "info", duration: 6000 });
          } catch (err) {
            toast(`Could not open OpenClaw chat: ${escapeHtml(String(err))}`, { kind: "error" });
          }
        });
    }
  });
  document.addEventListener("keydown", onKey);
  overlay._cleanup = () => {
    document.removeEventListener("keydown", onKey);
  };

  document.body.appendChild(overlay);
  voiceModalEl = overlay;

  // Focus the textarea (or retry button) for natural keyboard flow.
  const ta = overlay.querySelector("#voice-modal-text");
  if (ta) {
    ta.focus();
    ta.setSelectionRange(ta.value.length, ta.value.length);
  } else {
    overlay.querySelector("[data-action='retry']")?.focus();
  }
}

function closeVoiceResultModal() {
  if (!voiceModalEl) return;
  voiceModalEl._cleanup?.();
  voiceModalEl.remove();
  voiceModalEl = null;
}

export const dashboardPage = {
  label: "Dashboard",
  icon: null,
  requiresAuth: true,

  mount(root, ctx = {}) {
    const { onNeedsLogin, onOpenSettings, onOpenTerminal, onOpenOpenClawTerminal, onOpenFiles, onOpenNotebook, onOpenExtras, onOpenPricing, onOpenModules, onOpenProviderKeys, onOpenTasks, onOpenWorkflows } = ctx;

    // Lesson 704 (2026-08-27 10:05 MDT, David): wrap the whole mount
    // body in try/catch so a single ReferenceError or template-literal
    // evaluation failure can't leave the dashboard stuck on the
    // "Loading dashboard" button text forever. We render an error
    // card with the message instead, plus a "Reload" button that
    // re-mounts. This is the belt-and-suspenders complement to
    // fixing the voiceStatus variable-name typo.
    (async () => {
      try {
        await this._doMount(root, ctx, { onNeedsLogin, onOpenSettings, onOpenTerminal, onOpenOpenClawTerminal, onOpenFiles, onOpenNotebook, onOpenExtras, onOpenPricing, onOpenModules, onOpenProviderKeys, onOpenTasks });
      } catch (err) {
        console.error("[dashboard.mount] failed:", err);
        try {
          root.innerHTML = `
            <div class="dashboard">
              <header class="dashboard-header">
                <h1 class="logo">MiracleClaw</h1>
              </header>
              <div class="dashboard-error" role="alert">
                <strong>Dashboard failed to load.</strong>
                <pre class="dashboard-error-detail">${escapeHtml(String(err && err.stack || err))}</pre>
                <button type="button" class="primary" id="dashboard-reload">Reload dashboard</button>
              </div>
            </div>
          `;
          const reloadBtn = document.getElementById("dashboard-reload");
          if (reloadBtn) {
            reloadBtn.addEventListener("click", () => this.mount(root, ctx));
          }
        } catch (innerErr) {
          console.error("[dashboard.mount] error-card render failed:", innerErr);
          root.textContent = `Dashboard failed to load: ${String(err)}`;
        }
      }
    })();
  },

  async _doMount(root, ctx, { onNeedsLogin, onOpenSettings, onOpenTerminal, onOpenOpenClawTerminal, onOpenFiles, onOpenNotebook, onOpenExtras, onOpenPricing, onOpenModules, onOpenProviderKeys, onOpenTasks, onOpenWorkflows }) {

      // Lesson 561 (2026-08-24 16:08 MDT, David): the dashboard's usage
      // line used to render only an em-dash below threshold because
      // `mc_get_nudge` returns null-ish when the user hasn't hit a
      // 500/1000/cap/80/95/100% trigger. We now fetch the RAW quota
      // from `mc_get_quota` and use it to ALWAYS render
      // "X / Y tokens this period" + a progress bar. `mc_get_nudge`
      // stays in the request so the existing nudge modal still fires.
      const [tierResult, nudgeResult, quotaResult, voiceStatusResult] = await Promise.allSettled([
        invoke("mc_get_tier"),
        invoke("mc_get_nudge"),
        invoke("mc_get_quota"),
        // Lesson TBD (2026-08-27): cheap registry read, returns whether
        // the installer set up the Windows voice stack. Drives the
        // dashboard banner that nudges users to install voice FODs.
        invoke("first_run_report"),
      ]);

      // If the tier fetch failed with "not logged in", bounce back to login.
      if (tierResult.status === "rejected" &&
          String(tierResult.reason).includes("not logged in")) {
        if (onNeedsLogin) onNeedsLogin();
        return;
      }

      const tier = tierResult.status === "fulfilled" ? tierResult.value : null;
      const nudge = nudgeResult.status === "fulfilled" ? nudgeResult.value : null;
      const quota = quotaResult.status === "fulfilled" ? quotaResult.value : null;

      const tierLabel = tier ? formatTierLabel(tier.tier) : "Unknown";

      // Always show a real number, even when below any nudge threshold.
      // Falls back to em-dash only when MAIC is unreachable (network
      // error / quota endpoint down). That's the "dead line" David's
      // been seeing — previously we hit this on every successful load.
      let usageText;
      let usageFraction;
      if (quota && quota.limit > 0) {
        usageText = `${formatNumber(quota.used)} / ${formatNumber(quota.limit)} tokens this period`;
        usageFraction = Math.min(quota.used / quota.limit, 1);
      } else {
        usageText = "—";
        usageFraction = 0;
      }

      root.innerHTML = `
        <div class="dashboard">
          <header class="dashboard-header">
            <h1 class="logo">MiracleClaw</h1>
            <div class="dashboard-header-actions">
              <button
                type="button"
                class="icon-link"
                id="mc-voice-fab"
                title="Voice input (requires Voice for MiracleClaw module)"
                aria-label="Voice input"
                data-module-voice-installed="false"
              >🎙</button>
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
            <div class="usage-progress" aria-hidden="true">
              <div class="usage-progress-fill" style="width: ${Math.round(usageFraction * 100)}%"></div>
            </div>
            ${nudge && nudge.text && nudge.kind !== "none"
              ? `<button type="button" class="usage-cta" id="usage-cta">${escapeHtml(nudge.text.split('.')[0])} → Upgrade</button>`
              : ""}
          </div>

          ${(() => {
            // Lesson TBD (2026-08-27): voice-setup nudge banner. Only
            // shows on Windows when the installer's voice stack flag is
            // missing. Quietly dismissed via localStorage so it doesn't
            // nag returning users.
            //
            // Lesson 704 (2026-08-27 10:05 MDT, David): the IIFE
            // originally referenced `voiceStatus` (without the
            // `Result` suffix), which threw ReferenceError at runtime
            // and prevented root.innerHTML from being assigned — the
            // dashboard got stuck on "Loading dashboard" forever. The
            // variable from Promise.allSettled is `voiceStatusResult`.
            if (!voiceStatusResult || voiceStatusResult.status !== "fulfilled") return "";
            const voiceReport = voiceStatusResult.value;
            if (!voiceReport || voiceReport.voice_stack_installed) return "";
            const dismissedKey = "mc-voice-banner-dismissed-v1";
            try {
              if (window.localStorage.getItem(dismissedKey) === "1") return "";
            } catch {}
            return `
              <div class="voice-banner" id="voice-banner" role="status">
                <span class="voice-banner-icon" aria-hidden="true">🎙</span>
                <div class="voice-banner-body">
                  <strong>Get the most out of voice input.</strong>
                  <p>MC's installer can set up Windows offline speech recognition for faster, more accurate dictation. Open Settings → Voice to verify your setup.</p>
                </div>
                <div class="voice-banner-actions">
                  <button type="button" class="link-button" id="voice-banner-open">Open Voice settings</button>
                  <button type="button" class="link-button" id="voice-banner-dismiss" aria-label="Dismiss">✕</button>
                </div>
              </div>
            `;
          })()}

          ${renderLinkButtons()}

          ${renderWorkflowRunBanner()}

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
            <!-- rc53.8 (feature/extras-hub): tile linking to the
                 Extras hub page. Lists all mc-* commands with one-click
                 "Run in Terminal" actions. -->
            <button class="tile" id="extras-tile" type="button">
              <div class="tile-icon">🛠️</div>
              <div class="tile-body">
                <div class="tile-title">Extras</div>
                <div class="tile-description">
                  Companion CLI tools for Miracle Claw: diagnostics,
                  stats, cost analytics, snapshot sharing, project
                  templates. Each card launches the tool live in the
                  Terminal.
                </div>
                <div class="tile-cta">Browse extras →</div>
              </div>
            </button>
            <!-- Lesson 713 (2026-08-28, David): tile linking to the
                 Provider Keys page. Lets users bring their own
                 OpenAI / Anthropic / Ollama Cloud / etc. API keys,
                 stored encrypted in the vault + injected as env vars
                 for the openclaw runtime. -->
            <button class="tile" id="provider-keys-tile" type="button">
              <div class="tile-icon">🔑</div>
              <div class="tile-body">
                <div class="tile-title">Provider Keys</div>
                <div class="tile-description">
                  Bring your own API key for OpenAI, Anthropic, Ollama Cloud,
                  Mistral, Cohere, OpenRouter, Groq, xAI, and more. Keys are
                  stored AES-256-GCM in the local vault and never sent to MAIC.
                  Use your own quota instead of MAIC's billing.
                </div>
                <div class="tile-cta">Manage keys →</div>
              </div>
            </button>
            <!-- Lesson 725 (2026-08-28 21:30 MDT, David): MC Tasks
                 tile — persistent task list that syncs to MAIC. The
                 Miracle Bot use case (David called it "the foundation
                 of the agent platform"): the model can read/write the
                 user's task list across sessions. Paid-tier-gated; free
                 users see the tile but get an upgrade CTA on click. -->
            <button class="tile" id="tasks-tile" type="button">
              <div class="tile-icon">✅</div>
              <div class="tile-body">
                <div class="tile-title">Tasks</div>
                <div class="tile-description">
                  Persistent task list — adds, edits, and syncs to MAIC so your
                  agent remembers follow-ups across sessions. Tasks stay on
                  this machine; MAIC gets the latest copy for cross-device
                  access. Paid tier only.
                </div>
                <div class="tile-cta">Open tasks →</div>
              </div>
            </button>
            <!-- Lesson 833 (NEW 2026-09-08, David): MC Workflows tile —
                 subagents / parallel specialists. User-facing copy:
                 "Workflows" not ".prose programs", "specialists" not
                 "agents". Six built-in workflows ship out of the box.
                 Free-tier friendly: the tile is visible to everyone, and
                 each workflow runs against the local MAIC model (no
                 external API spend). -->
            <button class="tile" id="workflows-tile" type="button">
              <div class="tile-icon">✨</div>
              <div class="tile-body">
                <div class="tile-title">Workflows</div>
                <div class="tile-description">
                  Pick a workflow and watch a team of specialists work in parallel.
                  Six built-in recipes: explore a codebase, run a code review with
                  three specialists at once, plan a project, and more.
                </div>
                <div class="tile-cta">Open workflows →</div>
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
      // rc53.8 (feature/extras-hub): dashboard tile for the Extras
      // hub page. Wired on its own — do NOT nest under onOpenNotebook
      // (rc53.11 bugfix: this used to be inside that block, so the
      // Extras handler silently died if onOpenNotebook was ever
      // missing).
      if (onOpenExtras) {
        const extrasTile = document.getElementById("extras-tile");
        if (extrasTile) {
          extrasTile.addEventListener("click", () => onOpenExtras());
        }
      }
      // Lesson 713 (2026-08-28, David): dashboard tile for the
      // Provider Keys page. Same defensive pattern as Extras — own
      // if-block so missing ctx doesn't kill the handler.
      if (onOpenProviderKeys) {
        const pkTile = document.getElementById("provider-keys-tile");
        if (pkTile) {
          pkTile.addEventListener("click", () => onOpenProviderKeys());
        }
      }
      // Lesson 725 (2026-08-28 21:30 MDT, David): Tasks tile handler.
      // Same defensive pattern as Provider Keys above.
      if (onOpenTasks) {
        const tasksTile = document.getElementById("tasks-tile");
        if (tasksTile) {
          tasksTile.addEventListener("click", () => onOpenTasks());
        }
      }
      // Lesson 833 (NEW 2026-09-08, David): Workflows tile handler.
// Same defensive pattern as Tasks above.
      if (onOpenWorkflows) {
        const workflowsTile = document.getElementById("workflows-tile");
        if (workflowsTile) {
          workflowsTile.addEventListener("click", () => onOpenWorkflows());
        }
        // Lesson 833 Phase 5: workflow-run banner (above the tiles)
        // navigates to the workflows page so the user can rejoin the
        // active run. The banner is rendered conditionally based on
        // sessionStorage, so it may not exist on every dashboard
        // mount — guard the lookup.
        const banner = document.getElementById("workflow-run-banner");
        if (banner) {
          banner.addEventListener("click", () => onOpenWorkflows());
        }
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

      // Lesson TBD (2026-08-27): voice-setup banner dismiss + open.
      // Dismiss is sticky via localStorage so it doesn't nag returning
      // users. Open routes to Settings → Voice section.
      const voiceBannerDismiss = document.getElementById("voice-banner-dismiss");
      if (voiceBannerDismiss) {
        voiceBannerDismiss.addEventListener("click", () => {
          try { window.localStorage.setItem("mc-voice-banner-dismissed-v1", "1"); } catch {}
          const banner = document.getElementById("voice-banner");
          if (banner) banner.remove();
        });
      }
      const voiceBannerOpen = document.getElementById("voice-banner-open");
      if (voiceBannerOpen && onOpenSettings) {
        voiceBannerOpen.addEventListener("click", () => onOpenSettings());
      }

      // Lesson 571: dashboard voice FAB. Capture + transcribe + show
      // the result in a preview modal (rc53.29, David 2026-08-26
      // 16:33 MDT: "hears the speech but doesn't write to the input
      // area" — clipboard fallback was invisible to him). The modal
      // shows the transcript, has Copy / Re-record / Send-to-chat
      // buttons, and surfaces v0.1.8 diagnostics so we can debug
      // "No Speech Detected" failures (Lesson 592).
      const voiceFab = document.getElementById("mc-voice-fab");
      const runVoiceFab = async () => {
        if (!isModuleInstalled("voice")) {
          toast("Voice module not installed. Install via Settings → Modules.", { kind: "warn" });
          return;
        }
        voiceFab.disabled = true;
        const orig = voiceFab.textContent;
        voiceFab.textContent = "🎙…";
        try {
          // Lesson 581 (2026-08-25 16:25 MDT, David): ensure whisper
          // model is present before transcribing. If missing,
          // auto-download (~75MB) so the user gets a working mic
          // without having to dig into the binary's env vars.
          const ready = await ensureVoiceModel((status) => {
            voiceFab.textContent = status.startsWith("Downloading") ? "📥…" : "🎙…";
          });
          if (!ready) {
            toast(
              "🎙 whisper model missing. Run Settings → Modules → Voice → \"Download model\".",
              { kind: "error", duration: 10000 }
            );
            return;
          }
          const result = await invokeModule("mc_voice_transcribe", {
            seconds: 30,
            vad_enabled: true,
            silence_ms: 1500,
          });
          // rc53.29: always show the preview modal so David can SEE
          // what was captured. The textarea is editable; Copy / Re-
          // record / Send-to-chat buttons give him control over
          // where it goes. (Lesson 596)
          openVoiceResultModal(result || { text: "", diagnostics: null }, {
            onRetry: runVoiceFab,
          });
        } catch (e) {
          const msg = String(e);
          if (/no whisper model|MILAGRO_VOICE_MODEL/i.test(msg)) {
            toast(
              "🎙 whisper model missing. Run Settings → Modules → Voice → \"Download model\".",
              { kind: "error", duration: 10000 }
            );
          } else {
            // Surface the error in the modal so it's visible — not
            // buried in a 3-second toast.
            openVoiceResultModal({
              text: "",
              warning: `transcription failed: ${msg}`,
              diagnostics: null,
            }, { onRetry: runVoiceFab });
          }
        } finally {
          voiceFab.disabled = false;
          voiceFab.textContent = orig;
        }
      };
      if (voiceFab) {
        voiceFab.addEventListener("click", runVoiceFab);
      }

      // Lesson 574 (2026-08-25 08:19 MDT, David): Files page "Add to
      // Chat" pushes the file path into the dashboard chat input.
      // Same lookup chain as voice — #dashboard-chat-input first,
      // textarea[name='message'] as the generic fallback. If neither
      // exists (Terminal page drives mc-openclaw via clipboard), stage
      // the path to the clipboard instead — same bridge the existing
      // attach-zone uses for files dropped into the chat-input-less
      // OpenClaw window.
      const filesHandler = async (ev) => {
        const p = ev && ev.detail && ev.detail.path;
        if (!p) return;
        const chatInput =
          document.querySelector("#dashboard-chat-input") ||
          document.querySelector("textarea[name='message']");
        if (chatInput) {
          // Insert as a token the model can recognize. Same format
          // voice uses for transcripts; keeps it on-brand simple.
          const mention = `@file: ${p}`;
          if (chatInput.value && !chatInput.value.endsWith(" ")) {
            chatInput.value = chatInput.value.trimEnd() + "\n";
          }
          chatInput.value = chatInput.value + mention;
          chatInput.focus();
          toast(`Added file path to chat input`, { kind: "success" });
        } else {
          try {
            await writeText(`@file: ${p}`);
            toast(`Copied file path to clipboard — Ctrl+V into chat`, {
              kind: "info",
            });
          } catch (e) {
            toast(`File: ${p}`, { kind: "info", sticky: true });
          }
        }
      };
      document.addEventListener("mc:files:add-to-chat", filesHandler);

      // Lesson 564 (2026-08-24 17:30 MDT, David): the in-app plans card
      // is gone — the dashboard no longer renders pricing tiers inline
      // to avoid clutter. The pricing-link button below the usage bar
      // navigates in-app to the dedicated pricing page, which renders
      // the 6 plans + per-plan Stripe Checkout buttons.
      const pricingBtn = document.getElementById("pricing-link-btn");
      if (pricingBtn && onOpenPricing) {
        pricingBtn.addEventListener("click", () => {
          onOpenPricing();
        });
      }

      // Lesson 573 (2026-08-25 07:22 MDT, David): the Add-On Modules
      // button routes to the in-app catalog hub. Mirror of the pricing
      // wire above — kept on its own (not nested under onOpenPricing)
      // so an onOpenPricing regression can't take this down with it
      // (rc53.11 lesson: don't share failure modes between siblings).
      const modulesBtn = document.getElementById("modules-link-btn");
      if (modulesBtn && onOpenModules) {
        modulesBtn.addEventListener("click", () => {
          onOpenModules();
        });
      }

      // Wire the usage-bar CTA (the "Upgrade" button that appears
      // when a nudge fires). Routes the user to the same in-app
      // pricing page as the dashboard button.
      const usageCta = document.getElementById("usage-cta");
      if (usageCta && onOpenPricing) {
        usageCta.addEventListener("click", () => onOpenPricing());
      }

      // Wire the drag-and-drop attachment zone. Dragover/drop are
      // attached to the zone element so dropping anywhere on the page
      // doesn't accidentally trigger file navigation. The zone is also
      // focusable + click-to-open, but the clickable fallback is
      // intentionally minimal — newbies will discover drag on their own.
      wireAttachZone(root);

      if (tier?.tier_changed) showTierChangedModal(tier);
      if (nudge && nudge.text && nudge.text.length > 0) showNudgeModal(nudge);
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
    // rc53.29: also tear down the voice preview modal (uses .voice-
    // modal-overlay, not .modal-overlay — keeps its own scoped class).
    closeVoiceResultModal();
  },
};

// --- helpers below are not part of the page API ---

// Re-export the attach zone + helpers so terminal.js (and other pages)
// can render the same drop-to-chat zone as an overlay without re-creating
// the component. See pages/terminal.js:attachOverlayPage.
//
// feature/drag-drop: attachment staging zone. The zone is a self-contained
// subcomponent on the dashboard — it manages its own queue state, drag
// listeners, and the "Send to chat" handoff. Wire once per dashboard mount.
export { wireAttachZone, iconFor, formatBytes };
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
