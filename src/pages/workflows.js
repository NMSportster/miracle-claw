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

async function mount(root, ctx = {}) {
  // Reset state on every mount (Lesson 524: idempotent remount).
  _state.activeSessionId = null;
  _state.activeWorkflowName = null;
  _state.lastSeq = 0;
  _state.pendingWorkflowHint = typeof ctx.workflowHint === "string" ? ctx.workflowHint : null;
  stopPolling();

  root.innerHTML = "";
  const wrap = el("div", { class: "workflows-wrap" });
  root.appendChild(wrap);

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

  // Lesson 833 Phase 4: gradient hero. We render this AFTER examples
  // load so the count pill ("6 workflows · 6 specialists") has the
  // real numbers. The Recipes button is inline so users can reach
  // saved workflows without scrolling past the grid.
  const header = el("div", { class: "workflows-hero" }, [
    el("h1", { class: "workflows-hero-title" }, ["Workflows"]),
    el("p", { class: "workflows-hero-subtitle" }, [
      "Pick a workflow, give it a goal, and watch your team work. ",
      "Each workflow runs a team of specialists in parallel.",
    ]),
    el("div", { class: "workflows-hero-actions" }, [
      el("span", { class: "workflows-hero-pill" }, [
        `${workflows.length} workflows · ${specialists.length} specialists`,
      ]),
      el(
        "button",
        {
          class: "workflows-hero-recipes-btn",
          id: "workflows-hero-recipes",
          title: "Browse your saved recipes (workflows you've customized)",
          onclick: () => openRecipesPanel(),
        },
        ["📚 Recipes"],
      ),
    ]),
  ]);
  wrap.appendChild(header);

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

  // Lesson 833 Phase 4: if Cmd-K sent us a workflowHint, scroll the
  // matching tile into view and open its run modal. We don't open the
  // modal automatically on *every* mount (would surprise users who
  // came from the dashboard) — only when a hint was explicitly passed.
  if (_state.pendingWorkflowHint) {
    const hint = _state.pendingWorkflowHint;
    _state.pendingWorkflowHint = null;
    const target = workflows.find((w) => w.file && w.file.includes(hint))
      || specialists.find((s) => s.file && s.file.includes(hint));
    if (target) {
      // Defer one tick so the DOM has settled before we scroll.
      setTimeout(() => {
        const tileEl = wrap.querySelector(`[data-file="${CSS.escape(target.file)}"]`);
        if (tileEl) tileEl.scrollIntoView({ behavior: "smooth", block: "center" });
        openRunModal(target);
      }, 50);
    }
  }

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
  // Lesson 833 Phase 4: if the user has a recipe for this workflow,
  // surface a "Last used 2h ago · open it" hint inline so they can
  // re-run with one click without opening the Recipes panel first.
  const recipe = loadRecipes()[wf.file];
  const recentHint = recipe
    ? el("div", { class: "workflow-tile-recent", title: recipe.goal }, [
        el("span", { class: "workflow-tile-recent-label" }, [
          `Last used ${formatTimeAgo(recipe.lastUsed)}`,
        ]),
        el(
          "button",
          {
            class: "workflow-tile-recent-btn",
            onclick: () => openRunModal(wf, recipe.goal),
          },
          ["Re-run"],
        ),
      ])
    : null;
  return el("div", { class: "workflow-tile", "data-file": wf.file }, [
    el("div", { class: "workflow-tile-name" }, [wf.name]),
    el("div", { class: "workflow-tile-desc" }, [wf.description]),
    el("div", { class: "workflow-tile-meta" }, [
      el("span", { class: "workflow-tile-parallel" }, [parallelHint]),
    ]),
    recentHint,
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
let _recipesPanelRoot = null;

// ----- Recipes (Phase 4) --------------------------------------------------
//
// A "recipe" is a saved goal + last-used timestamp for a workflow.
// We persist them to localStorage so users don't have to re-type
// their common goals. Recipes are surfaced in two places:
//   1. The 📚 button in the gradient hero opens a full Recipes panel.
//   2. Each workflow tile shows a "Recent goal" hint when it has one.
// This is intentionally light — no edit forms, no settings, no
// server sync. Power users who want more can extend later.

const RECIPES_KEY = "mc.workflows.recipes.v1";

function loadRecipes() {
  try {
    const raw = localStorage.getItem(RECIPES_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch (e) {
    return {};
  }
}

function saveRecipes(recipes) {
  try {
    localStorage.setItem(RECIPES_KEY, JSON.stringify(recipes));
  } catch (e) {
    // Quota exceeded or storage disabled — ignore. The user just won't
    // see this recipe persist; they can always re-enter it.
  }
}

function recordRecipeUse(workflow, goal) {
  const recipes = loadRecipes();
  recipes[workflow.file] = {
    name: workflow.name,
    goal,
    lastUsed: Date.now(),
  };
  saveRecipes(recipes);
}

function openRecipesPanel() {
  if (_recipesPanelRoot) {
    // Already open — toggle closed.
    closeRecipesPanel();
    return;
  }
  const recipes = loadRecipes();
  const entries = Object.entries(recipes).sort(
    (a, b) => (b[1].lastUsed || 0) - (a[1].lastUsed || 0),
  );
  _recipesPanelRoot = el("div", { class: "workflow-recipes-backdrop" });
  const panel = el("div", { class: "workflow-recipes" }, [
    el("div", { class: "workflow-recipes-header" }, [
      el("h2", {}, ["📚 Your saved recipes"]),
      el(
        "button",
        {
          class: "workflow-recipes-close",
          title: "Close",
          "aria-label": "Close recipes panel",
          onclick: () => closeRecipesPanel(),
        },
        ["×"],
      ),
    ]),
  ]);

  if (entries.length === 0) {
    panel.appendChild(
      el("div", { class: "workflow-recipes-empty" }, [
        "No recipes yet. Run a workflow and your goal will be saved here so you can re-run it with one click.",
      ]),
    );
  } else {
    panel.appendChild(
      el("p", { class: "workflow-recipes-hint" }, [
        `You have ${entries.length} saved recipe${entries.length === 1 ? "" : "s"}. Click any one to re-run it.`,
      ]),
    );
    const list = el("div", { class: "workflow-recipes-list" });
    for (const [file, recipe] of entries) {
      const snippet =
        recipe.goal.length > 100
          ? recipe.goal.slice(0, 100).trimEnd() + "…"
          : recipe.goal;
      const lastUsedDate = new Date(recipe.lastUsed || 0);
      const agoText = formatTimeAgo(recipe.lastUsed || 0);
      list.appendChild(
        el("div", { class: "workflow-recipe-card", "data-file": file }, [
          el("div", { class: "workflow-recipe-name" }, [recipe.name || file]),
          el("div", { class: "workflow-recipe-snippet" }, [snippet]),
          el("div", { class: "workflow-recipe-meta" }, [
            el("span", { class: "workflow-recipe-time" }, [
              agoText,
              lastUsedDate.getFullYear() > 1970
                ? ` (${lastUsedDate.toLocaleDateString()})`
                : "",
            ]),
            el(
              "button",
              {
                class: "workflow-recipe-run",
                title: "Re-run this workflow with this goal",
                onclick: () => {
                  closeRecipesPanel();
                  // Look up the workflow from the loaded examples.
                  const wf = (_state.examples || []).find((e) => e.file === file);
                  if (wf) {
                    // Pre-fill the modal with the saved goal.
                    openRunModal(wf, recipe.goal);
                  } else {
                    showActivePanelError(file, "That workflow isn't available anymore.");
                  }
                },
              },
              ["Re-run"],
            ),
            el(
              "button",
              {
                class: "workflow-recipe-delete",
                title: "Forget this recipe",
                onclick: () => {
                  const all = loadRecipes();
                  delete all[file];
                  saveRecipes(all);
                  closeRecipesPanel();
                  openRecipesPanel();
                },
              },
              ["Forget"],
            ),
          ]),
        ]),
      );
    }
    panel.appendChild(list);
  }

  _recipesPanelRoot.appendChild(panel);
  document.body.appendChild(_recipesPanelRoot);
}

function closeRecipesPanel() {
  if (_recipesPanelRoot && _recipesPanelRoot.parentNode) {
    _recipesPanelRoot.parentNode.removeChild(_recipesPanelRoot);
  }
  _recipesPanelRoot = null;
}

function formatTimeAgo(ts) {
  if (!ts) return "unknown";
  const diff = Date.now() - ts;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)}d ago`;
  return `${Math.floor(diff / (7 * 86_400_000))}w ago`;
}

function openRunModal(workflow, prefillGoal = "") {
  closeModal();
  _modalRoot = el("div", { class: "workflow-modal-backdrop" });
  // Lesson 833 Phase 4: prefill the textarea when called from a Recipe.
  // Empty by default — users typing fresh goals see a blank canvas.
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
      ta.value = prefillGoal;
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
  // Lesson 833 Phase 4: save the goal as a recipe so the user can
  // re-run with one click from the Recipes panel. We persist the
  // trimmed goal text + a timestamp keyed on the workflow file.
  // Errors here are silently swallowed (Lesson 831: never block a
  // successful run on a save failure).
  try { recordRecipeUse(workflow, goal); } catch (e) { /* non-fatal */ }
  closeModal();
  showActivePanelRunning(workflow);
  startPolling();
}

// ----- Active run panel ---------------------------------------------------

function showActivePanelRunning(workflow) {
  if (!_state.activePanel) return;
  _state.activePanel.innerHTML = "";
  _state.activePanel.style.display = "";
  // Lesson 833 Phase 4: mark running so the CSS knows which border
  // color to fade FROM when the run finishes.
  _state.activePanel.classList.remove("workflows-active-finished");
  _state.activePanel.classList.add("workflows-active-running");
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

    // Lesson 833 Phase 4: receipt card. We summarize the run with
    // counts the user can actually verify (output lines, errors, and
    // a plain-English takeaway). No internal codes, no jargon — same
    // Lesson 832 contract.
    const stats = computeReceiptStats(chunks);
    const receipt = buildReceiptCard(status, stats);
    out.appendChild(receipt);
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
  // Lesson 833 Phase 4: animated completion transition. Add a class
  // that fades the panel border from running-blue to done-green and
  // slides the receipt into view. The CSS handles the actual easing.
  _state.activePanel.classList.remove("workflows-active-running");
  _state.activePanel.classList.add("workflows-active-finished");
}

// Compute summary stats for the receipt. Excludes the placeholder
// "Starting up…" line and system messages so the numbers reflect the
// actual model output, not housekeeping.
function computeReceiptStats(chunks) {
  let lines = 0;
  let stderr = 0;
  let system = 0;
  let firstTs = null;
  let lastTs = null;
  for (const c of chunks) {
    if (c.stream === "system") {
      system += 1;
      continue;
    }
    if (c.stream === "stderr") stderr += 1;
    lines += 1;
    if (firstTs === null) firstTs = c.seq;
    lastTs = c.seq;
  }
  return {
    lines,
    stderr,
    system,
    firstTs,
    lastTs,
  };
}

function buildReceiptCard(status, stats) {
  const statusLabel = STATUS_LABEL[status] || "Finished";
  const isError = status === "error";
  const isKilled = status === "killed";
  const isComplete = status === "complete";

  let takeaway;
  if (isComplete) {
    takeaway = stats.stderr > 0
      ? `Finished, but ${stats.stderr} line${stats.stderr === 1 ? "" : "s"} looked unusual — worth a quick look.`
      : `All clear. The team delivered ${stats.lines} line${stats.lines === 1 ? "" : "s"} of output.`;
  } else if (isKilled) {
    takeaway = "You stopped this run. Nothing was saved.";
  } else if (isError) {
    takeaway = "Something went wrong before the team could finish.";
  } else {
    takeaway = "Finished.";
  }

  const rows = [];
  rows.push(
    el("div", { class: "workflows-receipt-row" }, [
      el("span", { class: "workflows-receipt-label" }, ["Status"]),
      el("span", { class: `workflows-receipt-value workflows-receipt-status-${status}` }, [statusLabel]),
    ]),
  );
  rows.push(
    el("div", { class: "workflows-receipt-row" }, [
      el("span", { class: "workflows-receipt-label" }, ["Output"]),
      el("span", { class: "workflows-receipt-value" }, [
        `${stats.lines} line${stats.lines === 1 ? "" : "s"}`,
      ]),
    ]),
  );
  if (stats.stderr > 0) {
    rows.push(
      el("div", { class: "workflows-receipt-row" }, [
        el("span", { class: "workflows-receipt-label" }, ["Unusual"]),
        el("span", { class: "workflows-receipt-value workflows-receipt-warn" }, [
          `${stats.stderr} line${stats.stderr === 1 ? "" : "s"}`,
        ]),
      ]),
    );
  }

  return el(
    "div",
    {
      class: "workflows-active-receipt",
      role: "status",
      "aria-live": "polite",
    },
    [
      el("div", { class: "workflows-receipt-takeaway" }, [takeaway]),
      el("div", { class: "workflows-receipt-stats" }, rows),
    ],
  );
}

function clearActivePanel() {
  if (!_state.activePanel) return;
  _state.activePanel.style.display = "none";
  _state.activePanel.innerHTML = "";
  // Lesson 833 Phase 4: drop the running/finished classes so the next
  // run starts from the same baseline as the first one.
  _state.activePanel.classList.remove("workflows-active-running", "workflows-active-finished");
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
