// src/pages/workflows.js — MC Workflow Center (Phase 1 MVP)
//
// Lesson 833 (NEW 2026-09-08, David): Phase 1 of MC subagents. Surface
// the bundled open-prose plugin from a dedicated page so users can
// pick a workflow, give it a goal, and watch it run.
//
// Lesson 832: every user-facing string follows the contract.
//   - "Workflow" not ".prose program"
//   - "Specialist" not "agent" / "subagent"
//   - "Your team is working on this" not "subagent team running"
//   - Errors are translated to plain English (no ENOENT, etc.)
//   - User never sees a `.prose` filename — they pick by description.
//
// Phase 1 surface (this file):
//   - 6 built-in workflows (tile grid)
//   - 6 named specialists (collapsible section)
//   - Run Workflow modal: pick workflow + write goal + Run button
//   - Live output stream (calls prose_poll every 1s)
//   - Kill button + final receipt ("X specialists ran in N seconds")
//
// Phase 2 will add:
//   - Workers tree view (parent → child specialist list)
//   - Status pills with animation
//   - Cmd-K smart workflow suggestions
//
// Phase 3 will add:
//   - Terminal `prose>` REPL integration
//
// Phase 4 will add:
//   - Polished gradient hero
//   - Receipt animations
//   - Multi-window: `openclaw_open_window` for Workers page

import { register } from "../page_registry.js";

async function invoke(cmd, args) {
  if (!window.__TAURI_INTERNALS__?.invoke) {
    throw new Error("Tauri runtime not available");
  }
  return window.__TAURI_INTERNALS__.invoke(cmd, args);
}

// Lesson 833 Phase 2: real-time streaming via Tauri events. The Rust
// prose_host emits `prose:chunk` and `prose:status` for each line
// written to the buffer, so we can update the live transcript without
// waiting for the 1s poll cycle. Polling is retained as a safety net
// (e.g. if the listener misses a chunk during a navigation event).
async function listen(event, handler) {
  // Tauri 2 exposes listen on the global window.__TAURI__.event object
  // when the @tauri-apps/api/event module is bundled in. We try the
  // dynamic import first; fall back to the internals if needed.
  try {
    const ev = await import("@tauri-apps/api/event");
    return await ev.listen(event, handler);
  } catch (e) {
    // Older build: use the legacy window.__TAURI__.event.listen
    const legacy = window.__TAURI__?.event;
    if (legacy && typeof legacy.listen === "function") {
      return await legacy.listen(event, handler);
    }
    throw new Error("Tauri event API not available: " + e);
  }
}

function el(tag, attrs = {}, children = []) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k === "style") e.style.cssText = v;
    else if (k.startsWith("on") && typeof v === "function") {
      e.addEventListener(k.slice(2).toLowerCase(), v);
    } else if (k === "html") {
      e.innerHTML = v;
    } else if (v !== null && v !== undefined) {
      e.setAttribute(k, v);
    }
  }
  for (const c of children) {
    if (c == null) continue;
    if (typeof c === "string") e.appendChild(document.createTextNode(c));
    else e.appendChild(c);
  }
  return e;
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

// Status pill colors. Mirrors Lesson 831: green=running, blue=complete,
// yellow=blocked-on-tool, red=error, gray=queued/killed.
const STATUS_COLOR = {
  running: "#7aa2f7",
  complete: "#9ece6a",
  error: "#f7768e",
  killed: "#565f89",
  queued: "#565f89",
};

const STATUS_LABEL = {
  running: "Running",
  complete: "Done",
  error: "Stopped with a problem",
  killed: "Cancelled",
  queued: "Queued",
};

export const workflowsPage = {
  label: "Workflows",
  icon: "✨",
  requiresAuth: true,
};

// Page-local state. We keep it on the module object so unmount can
// stop the poll loop cleanly (Lesson 524: never leave background timers
// running after a page leaves the DOM).
const _state = {
  examples: [], // populated on mount via prose_examples
  activeSessionId: null,
  activeWorkflowName: null,
  pollTimer: null,
  lastSeq: 0,
  // Lesson 833 Phase 2: unlisten handles for Tauri events. Set when
  // mount() registers listeners, cleared on unmount().
  unlistenChunk: null,
  unlistenStatus: null,
};

async function mount(root) {
  // Reset state on every mount (Lesson 524: idempotent remount).
  _state.activeSessionId = null;
  _state.activeWorkflowName = null;
  _state.lastSeq = 0;
  stopPolling();

  root.innerHTML = "";
  const wrap = el("div", { class: "workflows-wrap" });
  root.appendChild(wrap);

  // Header — Lesson 832: plain English, no internal jargon.
  const header = el("div", { class: "workflows-header" }, [
    el("h1", {}, ["Workflows"]),
    el("p", { class: "workflows-subtitle" }, [
      "Pick a workflow, give it a goal, and watch your team work. ",
      "Each workflow runs a team of specialists in parallel.",
    ]),
  ]);
  wrap.appendChild(header);

  // Load examples (Lesson 833: prose_examples returns 6 workflows + 6 specialists).
  let examples = [];
  try {
    examples = await invoke("prose_examples", {});
  } catch (e) {
    wrap.appendChild(
      el("div", { class: "workflows-error" }, [
        "Could not load the workflow library. Try restarting MiracleClaw.",
      ]),
    );
    return;
  }
  _state.examples = examples;

  const workflows = examples.filter((e) => e.kind === "workflow");
  const specialists = examples.filter((e) => e.kind === "specialist");

  // Workflow tiles grid.
  const grid = el("div", { class: "workflows-grid" });
  for (const wf of workflows) {
    grid.appendChild(renderWorkflowTile(wf));
  }
  wrap.appendChild(grid);

  // Specialists section — collapsible, hidden by default (Lesson 832:
  // most users only ever pick a workflow, not a bare specialist).
  const specSection = el("details", { class: "workflows-specialists" }, [
    el("summary", {}, [`Named specialists (${specialists.length})`]),
    el(
      "p",
      { class: "workflows-specialists-hint" },
      [
        "Specialists are the building blocks workflows use. ",
        "Pick one when you want a single, focused assistant instead of a team.",
      ],
    ),
  ]);
  const specGrid = el("div", { class: "workflows-grid workflows-grid-specialists" });
  for (const sp of specialists) {
    specGrid.appendChild(renderSpecialistTile(sp));
  }
  specSection.appendChild(specGrid);
  wrap.appendChild(specSection);

  // Active run panel — only visible when a session is running or just finished.
  _state.activePanel = el("div", { class: "workflows-active", style: "display: none;" });
  wrap.appendChild(_state.activePanel);

  // Lesson 833 Phase 2: register Tauri event listeners for real-time
  // streaming. The Rust prose_host emits `prose:chunk` and `prose:status`
  // for each line written to the buffer. Polling stays as a safety net
  // (1s interval) — events drive the visible update; the poll catches
  // anything that was missed while the page was hidden.
  //
  // We register ONCE per mount (not per run) and filter by session_id
  // so we don't leak listeners across remounts. The unlisten functions
  // are stored on _state and called in unmount().
  try {
    _state.unlistenChunk = await listen("prose:chunk", (event) => {
      const payload = event?.payload;
      if (!payload) return;
      if (payload.session_id !== _state.activeSessionId) return; // not ours
      onProseChunk(payload.chunk);
    });
    _state.unlistenStatus = await listen("prose:status", (event) => {
      const payload = event?.payload;
      if (!payload) return;
      if (payload.session_id !== _state.activeSessionId) return;
      onProseStatus(payload.status);
    });
  } catch (e) {
    // No Tauri runtime (e.g. unit test) — fall back to polling only.
    // The active panel still works via the existing pollOnce path.
    _state.unlistenChunk = null;
    _state.unlistenStatus = null;
  }
}

function renderWorkflowTile(wf) {
  const parallelHint = wf.parallel_count > 1
    ? `${wf.parallel_count} specialists in parallel`
    : "1 specialist";
  return el("div", { class: "workflow-tile", "data-file": wf.file }, [
    el("div", { class: "workflow-tile-name" }, [wf.name]),
    el("div", { class: "workflow-tile-desc" }, [wf.description]),
    el("div", { class: "workflow-tile-meta" }, [
      el("span", { class: "workflow-tile-parallel" }, [parallelHint]),
    ]),
    el(
      "button",
      {
        class: "workflow-tile-run",
        onclick: () => openRunModal(wf),
      },
      ["Run"],
    ),
  ]);
}

function renderSpecialistTile(sp) {
  return el("div", { class: "specialist-tile", "data-file": sp.file }, [
    el("div", { class: "specialist-tile-name" }, [sp.name]),
    el("div", { class: "specialist-tile-desc" }, [sp.description]),
    el(
      "button",
      {
        class: "workflow-tile-run",
        onclick: () => openRunModal({ ...sp, parallel_count: 1, name: sp.name }),
      },
      ["Use"],
    ),
  ]);
}

// ----- Run modal ----------------------------------------------------------
//
// Lesson 832 contract: one text field ("What should this workflow look at?")
// + Run button. No jargon, no advanced options surfaced. Power users can
// extend later via the Recipes tab (Phase 4).

let _modalRoot = null;

function openRunModal(workflow) {
  closeModal();
  _modalRoot = el("div", { class: "workflow-modal-backdrop" });
  const modal = el("div", { class: "workflow-modal" }, [
    el("h2", {}, [`Run: ${workflow.name}`]),
    el("p", { class: "workflow-modal-desc" }, [workflow.description]),
    el("label", { class: "workflow-modal-label" }, [
      `What should ${workflow.name} look at?`,
    ]),
    (() => {
      const ta = el("textarea", {
        class: "workflow-modal-input",
        rows: "4",
        placeholder: placeholderFor(workflow),
      });
      ta.id = "workflow-modal-goal";
      _modalRoot._inputEl = ta;
      return ta;
    })(),
    el("div", { class: "workflow-modal-actions" }, [
      el(
        "button",
        {
          class: "workflow-modal-cancel",
          onclick: () => closeModal(),
        },
        ["Cancel"],
      ),
      el(
        "button",
        {
          class: "workflow-modal-run",
          onclick: () => submitRun(workflow),
        },
        ["Run"],
      ),
    ]),
  ]);
  _modalRoot.appendChild(modal);
  document.body.appendChild(_modalRoot);
  // Focus the textarea so the user can type immediately.
  setTimeout(() => _modalRoot?._inputEl?.focus(), 50);
}

function placeholderFor(workflow) {
  switch (workflow.name) {
    case "Explore Codebase":
      return "e.g. the src/ folder, or just '.'";
    case "Code Review":
      return "e.g. src/pages/tasks.js, or src-tauri/src/*.rs";
    case "Fix Tests":
      return "e.g. the test for the login form is failing — fix it";
    case "Plan a Project":
      return "e.g. a CLI for tracking gardening tasks";
    case "Pair Debug":
      return "e.g. /v1/auth/me returns 401 for valid JWTs";
    case "Docs From Code":
      return "e.g. src-tauri/src/maic/";
    default:
      return workflow.description;
  }
}

function closeModal() {
  if (_modalRoot && _modalRoot.parentNode) {
    _modalRoot.parentNode.removeChild(_modalRoot);
  }
  _modalRoot = null;
}

async function submitRun(workflow) {
  const goal = _modalRoot?._inputEl?.value?.trim();
  if (!goal) {
    // Inline hint rather than an alert — Lesson 832: no system alerts
    // for things the user can fix by typing.
    const hint = el("div", { class: "workflow-modal-hint" }, [
      "Tell the workflow what to look at first.",
    ]);
    _modalRoot.querySelector(".workflow-modal-input")?.parentNode?.insertBefore(
      hint,
      _modalRoot.querySelector(".workflow-modal-actions"),
    );
    return;
  }

  // Disable the Run button to prevent double-submits.
  const runBtn = _modalRoot.querySelector(".workflow-modal-run");
  if (runBtn) {
    runBtn.disabled = true;
    runBtn.textContent = "Starting…";
  }

  let result;
  try {
    result = await invoke("prose_run", {
      fileOrSlug: workflow.file,
      // Lesson 833: the goal text from the modal is injected into the
      // rewritten `.prose` file as `input goal: "..."` by the Rust
      // command (see prose_host.rs + prose_model_map.rs). OpenProse
      // then binds the variable `goal` so any session/parallel block
      // that references it gets the user's text.
      userInput: goal,
    });
  } catch (e) {
    closeModal();
    showActivePanelError(workflow, String(e));
    return;
  }

  _state.activeSessionId = result.session_id;
  _state.activeWorkflowName = workflow.name;
  _state.lastSeq = 0;
  closeModal();
  showActivePanelRunning(workflow);
  startPolling();
}

// ----- Active run panel ---------------------------------------------------

function showActivePanelRunning(workflow) {
  if (!_state.activePanel) return;
  _state.activePanel.innerHTML = "";
  _state.activePanel.style.display = "";
  _state.activePanel.appendChild(
    el("div", { class: "workflows-active-header" }, [
      el("span", {
        class: "workflows-active-status",
        style: `color: ${STATUS_COLOR.running};`,
      }, [STATUS_LABEL.running]),
      el("h3", {}, [_state.activeWorkflowName]),
    ]),
  );
  _state.activePanel.appendChild(
    el("div", { class: "workflows-active-output", id: "workflows-active-output" }, [
      el("div", { class: "workflows-active-placeholder" }, [
        "Starting up…",
      ]),
    ]),
  );
  _state.activePanel.appendChild(
    el("div", { class: "workflows-active-actions" }, [
      el(
        "button",
        {
          class: "workflows-active-kill",
          onclick: () => killActive(),
        },
        ["Cancel"],
      ),
    ]),
  );
}

function showActivePanelError(workflow, msg) {
  if (!_state.activePanel) return;
  _state.activePanel.innerHTML = "";
  _state.activePanel.style.display = "";
  _state.activePanel.appendChild(
    el("div", { class: "workflows-active-header" }, [
      el("span", {
        class: "workflows-active-status",
        style: `color: ${STATUS_COLOR.error};`,
      }, ["Couldn't start"]),
    ]),
  );
  _state.activePanel.appendChild(
    el("div", { class: "workflows-active-output" }, [
      el("div", { class: "workflows-active-placeholder" }, [msg]),
    ]),
  );
}

function showActivePanelComplete(status, chunks) {
  if (!_state.activePanel) return;
  const out = _state.activePanel.querySelector("#workflows-active-output");
  if (out) {
    // Lesson 833 Phase 2: events have already appended live chunks up
    // to lastSeq. The final poll may include any chunks emitted after
    // the last seen event (system receipt line, race window). Dedupe
    // by seq so the same line never appears twice.
    const seen = new Set();
    out.querySelectorAll(".workflows-active-line").forEach((node) => {
      const seqAttr = node.getAttribute("data-seq");
      if (seqAttr) seen.add(Number(seqAttr));
    });
    for (const c of chunks) {
      if (c.seq <= _state.lastSeq && seen.has(c.seq)) continue;
      out.appendChild(
        el(
          "div",
          {
            class: `workflows-active-line stream-${c.stream}`,
            "data-seq": String(c.seq),
          },
          [c.data],
        ),
      );
    }
    // Receipt at the bottom — Lesson 832: tell the user what just happened
    // in plain English with numbers they can verify.
    const systemChunks = chunks.filter((c) => c.stream === "system");
    const lastSystem = systemChunks[systemChunks.length - 1];
    out.appendChild(
      el("div", { class: "workflows-active-receipt" }, [
        lastSystem ? lastSystem.data : STATUS_LABEL[status] || "Finished",
      ]),
    );
  }
  // Replace the Cancel button with a Done button.
  const actions = _state.activePanel.querySelector(".workflows-active-actions");
  if (actions) {
    actions.innerHTML = "";
    actions.appendChild(
      el(
        "button",
        {
          class: "workflows-active-done",
          onclick: () => clearActivePanel(),
        },
        ["Close"],
      ),
    );
  }
  const headerStatus = _state.activePanel.querySelector(".workflows-active-status");
  if (headerStatus) {
    headerStatus.textContent = STATUS_LABEL[status] || "Finished";
    headerStatus.style.color = STATUS_COLOR[status] || STATUS_COLOR.complete;
  }
}

function clearActivePanel() {
  if (!_state.activePanel) return;
  _state.activePanel.style.display = "none";
  _state.activePanel.innerHTML = "";
  _state.activeSessionId = null;
  _state.activeWorkflowName = null;
  _state.lastSeq = 0;
}

async function killActive() {
  if (!_state.activeSessionId) return;
  try {
    await invoke("prose_kill", { sessionId: _state.activeSessionId });
  } catch (e) {
    // Idempotent — kill on dead child is a no-op in Rust.
  }
}

// ----- Polling ------------------------------------------------------------

// Lesson 833 Phase 2: real-time streaming via Tauri events. Each chunk
// arrives as soon as the Rust reader thread writes it to the buffer —
// no 1s poll lag. The poll loop stays as a safety net (see startPolling)
// but the user-visible updates come through these handlers first.
function onProseChunk(chunk) {
  if (!chunk) return;
  const out = _state.activePanel?.querySelector("#workflows-active-output");
  if (!out) return; // panel not visible yet — pollOnce will pick it up
  // Skip system lines in the live view; we show them in the receipt.
  if (chunk.stream === "system") {
    _state.lastSeq = chunk.seq;
    return;
  }
  const placeholder = out.querySelector(".workflows-active-placeholder");
  if (placeholder) placeholder.remove();
  out.appendChild(
    el(
      "div",
      {
        class: `workflows-active-line stream-${chunk.stream}`,
        "data-seq": String(chunk.seq),
      },
      [chunk.data],
    ),
  );
  // Auto-scroll if the user is already at the bottom (don't yank them
  // away from reading earlier output). Lesson 831: calm UX, no fighting
  // the user.
  const nearBottom = out.scrollHeight - out.scrollTop - out.clientHeight < 80;
  if (nearBottom) {
    out.scrollTop = out.scrollHeight;
  }
  _state.lastSeq = chunk.seq;
}

function onProseStatus(status) {
  const headerStatus = _state.activePanel?.querySelector(".workflows-active-status");
  if (!headerStatus) return;
  headerStatus.textContent = STATUS_LABEL[status] || status;
  headerStatus.style.color = STATUS_COLOR[status] || STATUS_COLOR.complete;
  // Animate on transition (Lesson 831: pulse only when changing).
  headerStatus.classList.remove("workflows-active-status-flash");
  // Force reflow so the animation re-triggers even on identical status.
  void headerStatus.offsetWidth;
  headerStatus.classList.add("workflows-active-status-flash");
}

function startPolling() {
  stopPolling();
  _state.pollTimer = setInterval(pollOnce, 1000);
  // Run one poll immediately so the user sees output without a 1s gap.
  pollOnce();
}

function stopPolling() {
  if (_state.pollTimer) {
    clearInterval(_state.pollTimer);
    _state.pollTimer = null;
  }
}

async function pollOnce() {
  if (!_state.activeSessionId) {
    stopPolling();
    return;
  }
  let result;
  try {
    result = await invoke("prose_poll", {
      sessionId: _state.activeSessionId,
      lastSeq: _state.lastSeq,
    });
  } catch (e) {
    showActivePanelError(_state.activeWorkflowName, String(e));
    stopPolling();
    return;
  }

  const out = _state.activePanel?.querySelector("#workflows-active-output");
  if (out && result.chunks && result.chunks.length > 0) {
    // Lesson 833 Phase 2: events drive live appending. The poll here
    // is a safety net — only render chunks with seq > lastSeq, since
    // anything <= lastSeq was already shown via the prose:chunk event.
    for (const c of result.chunks) {
      if (c.seq <= _state.lastSeq) continue;
      // Skip system lines in the live view; we show them in the receipt.
      if (c.stream === "system") {
        _state.lastSeq = c.seq;
        continue;
      }
      const placeholder = out.querySelector(".workflows-active-placeholder");
      if (placeholder) placeholder.remove();
      out.appendChild(
        el(
          "div",
          {
            class: `workflows-active-line stream-${c.stream}`,
            "data-seq": String(c.seq),
          },
          [c.data],
        ),
      );
      _state.lastSeq = c.seq;
    }
  }

  // Update header status pill live.
  const headerStatus = _state.activePanel?.querySelector(".workflows-active-status");
  if (headerStatus && result.status) {
    headerStatus.textContent = STATUS_LABEL[result.status] || result.status;
    headerStatus.style.color = STATUS_COLOR[result.status] || STATUS_COLOR.complete;
  }

  // If finished, render the final receipt and stop polling.
  if (
    result.status === "complete" ||
    result.status === "killed" ||
    result.status === "error"
  ) {
    stopPolling();
    // One more poll so we capture the trailing system chunk ("Workflow finished.")
    // that the wait/reap thread emits.
    let finalResult = result;
    try {
      finalResult = await invoke("prose_poll", {
        sessionId: _state.activeSessionId,
        lastSeq: _state.lastSeq,
      });
    } catch (e) {
      // ignore — we already have result
    }
    const allChunks = (result.chunks || []).concat(finalResult.chunks || []);
    showActivePanelComplete(result.status, allChunks);
  }
}

function unmount() {
  stopPolling();
  closeModal();
  clearActivePanel();
  // Lesson 833 Phase 2: detach the Tauri event listeners we registered
  // in mount(). If we don't, Tauri keeps invoking the handler with
  // stale closures after the page is gone, leaking memory.
  if (_state.unlistenChunk) {
    try { _state.unlistenChunk(); } catch (e) { /* idempotent */ }
    _state.unlistenChunk = null;
  }
  if (_state.unlistenStatus) {
    try { _state.unlistenStatus(); } catch (e) { /* idempotent */ }
    _state.unlistenStatus = null;
  }
}

register("workflows", {
  mount,
  unmount,
  label: workflowsPage.label,
  icon: workflowsPage.icon,
  requiresAuth: true,
});
