// src/pages/tasks.js — MC Tasks feature (v1.1.0-rc54.6+)
//
// Lesson 725 (2026-08-28 21:30 MDT, David): Persistent tasks for the
// Miracle Bot use case. Paid-tier-gated. Free users see an upgrade CTA.
//
// Surface:
//   - Header: tier badge + sync status + "Sync now" button
//   - Add-task form (name, description, priority, date)
//   - Date-grouped task list (today's tasks first, then upcoming, then past)
//   - Per-row: checkbox (done), inline edit, delete
//   - Tooltip on each task with `local_id` (for debugging; hidden in prod)
//
// Storage: Tauri commands `mc_tasks_page` / `mc_task_add` / `mc_task_update`
// / `mc_task_done` / `mc_task_delete` / `mc_task_sync`. All commands are
// paid-tier-gated server-side; we still gate the UI for nicer UX.
//
// Tauri invocation pattern (Lesson 713 family): every command goes through
// the `__TAURI_INTERNALS__.invoke` shim. We catch the structured error
// response `{code: "paid_tier_required"}` and render the upgrade panel
// instead of a generic error toast.

const PRIORITY_LABEL = { low: "Low", medium: "Medium", high: "High" };
const PRIORITY_COLOR = { low: "#7aa2f7", medium: "#e0af68", high: "#f7768e" };

function todayIso() {
  const d = new Date();
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}`;
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

async function invoke(cmd, args) {
  if (!window.__TAURI_INTERNALS__?.invoke) {
    throw new Error("Tauri runtime not available");
  }
  return window.__TAURI_INTERNALS__.invoke(cmd, args);
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

export const tasksPage = {
  label: "Tasks",
  icon: "✅",
  requiresAuth: true,

  _state: null,

  async mount(root, ctx) {
    const state = (this._state = {
      paid: false,
      tier: "free",
      tasks: [],
      byDate: {},
      hasToken: false,
      lastSync: null,
      busy: false,
      autoSyncing: false,   // Lesson 736: silent sync on mount
      autoSyncDone: false,   // only auto-sync once per mount
      autoSyncError: null,  // shown subtly in header if MAIC unreachable
      editingId: null,
      draft: { name: "", description: "", priority: "medium", date: todayIso() },
      error: null,
    });

    // Stash header element so _rerenderHeader can replace it in-place.
    state._headerEl = this._renderHeader(state);
    root.innerHTML = "";
    root.appendChild(state._headerEl);
    root.appendChild(this._renderBody(state, ctx));
    await this._reload(state, ctx);
    // Lesson 736: auto-sync from MAIC on page mount so the user sees
    // any task changes the MAIC agent made in a chat turn. Fire-and-
    // forget from the UI's perspective — we don't block the page load,
    // and if MAIC is unreachable we keep showing local data. The
    // header shows a subtle "Syncing…" indicator while in flight.
    if (state.paid && !state.autoSyncDone) {
      this._onAutoSync(state, ctx);
    }
  },

  unmount() {
    this._state = null;
  },

  async _reload(state, ctx) {
    state.busy = true;
    state.error = null;
    this._rerenderBody(state, ctx);
    try {
      const page = await invoke("mc_tasks_page");
      state.paid = page.paid;
      state.tier = page.tier;
      state.tasks = page.tasks || [];
      state.byDate = page.by_date || {};
      state.hasToken = page.has_token;
      state.lastSync = page.last_sync;
    } catch (e) {
      state.error = String(e?.message || e);
    } finally {
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  _renderHeader(state) {
    const tierBadge = el("span", {
      class: `mc-tasks-tier-badge tier-${state.tier}`,
      title: `Your MAIC tier: ${state.tier}`,
    }, [state.tier]);

    const lastSync = state.lastSync
      ? `Last sync: ${new Date(state.lastSync).toLocaleString()}`
      : "Never synced";

    // Lesson 736: subtle auto-sync indicator. Shows during the silent
    // mount-time sync, otherwise silent.
    const autoSyncIndicator = state.autoSyncing
      ? el("span", { class: "mc-tasks-auto-sync", title: "Pulling latest tasks from MAIC" }, ["⟳ Syncing…"])
      : state.autoSyncError
        ? el("span", {
            class: "mc-tasks-auto-sync mc-tasks-auto-sync-error",
            title: `MAIC sync failed: ${state.autoSyncError}`,
          }, ["⚠ Stale"])
        : null;

    // Lesson 756 (2026-08-29 16:56 MDT, David): back-to-dashboard
    // button. The Tasks page is the deepest nav level — without this
    // the user has to close + relogin to get back. Mirror the
    // pattern from secrets.js / settings.js.
    const onBack = this._currentCtx().onBackToDashboard || (() => {});
    const backBtn = el("button", {
      class: "mc-tasks-back-btn",
      title: "Back to Dashboard",
      "aria-label": "Back to Dashboard",
      onClick: () => onBack(),
    }, ["← Dashboard"]);

    const syncBtn = el("button", {
      class: "mc-tasks-btn mc-tasks-btn-primary",
      onClick: () => this._onSync(state, this._currentCtx()),
      disabled: !state.paid || state.busy,
      title: "Push local changes and pull remote changes from MAIC",
    }, ["🔄 Sync now"]);

    const loginBtn = state.hasToken
      ? null
      : el("button", {
          class: "mc-tasks-btn mc-tasks-btn-secondary",
          onClick: () => this._onLogin(state, this._currentCtx()),
          disabled: !state.paid || state.busy,
          title: "Save a per-feature MAIC token (falls back to your session token if skipped)",
        }, ["🔑 Set token"]);

    const refreshBtn = el("button", {
      class: "mc-tasks-btn mc-tasks-btn-secondary",
      onClick: () => this._reload(state, this._currentCtx()),
      disabled: state.busy,
      title: "Reload from disk",
    }, ["↻ Refresh"]);

    // Lesson 756: two-row header. Row 1: title + tier + sync status
    // (so it has room to breathe). Row 2: back button (left) + actions
    // (right). Cleaner visual hierarchy than cramming everything into
    // a single flex row.
    return el("div", { class: "mc-tasks-header" }, [
      el("div", { class: "mc-tasks-header-row mc-tasks-header-row-1" }, [
        backBtn,
        el("div", { class: "mc-tasks-header-title" }, [
          el("h1", { class: "mc-tasks-title" }, ["✅ Tasks"]),
          tierBadge,
        ]),
        el("div", { class: "mc-tasks-header-meta" }, [
          el("span", { class: "mc-tasks-sync-info" }, [lastSync]),
          ...(autoSyncIndicator ? [autoSyncIndicator] : []),
        ]),
      ]),
      el("div", { class: "mc-tasks-header-row mc-tasks-header-row-2" }, [
        el("div", { class: "mc-tasks-header-spacer" }),
        el("div", { class: "mc-tasks-header-actions" }, [
          refreshBtn,
          ...(loginBtn ? [loginBtn] : []),
          syncBtn,
        ]),
      ]),
    ]);
  },

  _renderBody(state, ctx) {
    const body = el("div", { class: "mc-tasks-body" });
    state._bodyEl = body;
    this._rerenderBody(state, ctx);
    return body;
  },

  _rerenderHeader(state, ctx) {
    // Lesson 736: replace the header in-place to show auto-sync indicator
    // without rerendering the entire body (which would lose focus/scroll).
    const oldHeader = this._state?._headerEl;
    if (oldHeader && oldHeader.parentNode) {
      const newHeader = this._renderHeader(state);
      oldHeader.parentNode.replaceChild(newHeader, oldHeader);
      this._state._headerEl = newHeader;
    }
  },

  _rerenderBody(state, ctx) {
    const body = state._bodyEl;
    if (!body) return;
    body.innerHTML = "";

    if (state.error) {
      body.appendChild(el("div", { class: "mc-tasks-error" }, [state.error]));
      return;
    }

    if (state.busy && state.tasks.length === 0) {
      body.appendChild(el("div", { class: "mc-tasks-empty" }, ["Loading tasks…"]));
      return;
    }

    if (!state.paid) {
      body.appendChild(this._renderUpgrade(state, ctx));
      return;
    }

    body.appendChild(this._renderAddForm(state, ctx));
    body.appendChild(this._renderTaskList(state, ctx));
  },

  _renderUpgrade(state, ctx) {
    return el("div", { class: "mc-tasks-upgrade" }, [
      el("h2", {}, ["Tasks is a Pro feature"]),
      el("p", {}, [
        "Persistent tasks let you (and your MAIC agent) keep track of ",
        "recurring work, follow-ups, and ideas across sessions. ",
        "Free users can read this page but not save or sync tasks.",
      ]),
      el("button", {
        class: "mc-tasks-btn mc-tasks-btn-primary",
        onClick: () => {
          if (ctx?.onNavigate) ctx.onNavigate("pricing");
          else window.__mc_navigate?.("pricing");
        },
      }, ["See plans →"]),
    ]);
  },

  _renderAddForm(state, ctx) {
    const d = state.draft;
    const form = el("form", {
      class: "mc-tasks-add-form",
      onSubmit: (e) => {
        e.preventDefault();
        this._onAdd(state, ctx);
      },
    });

    form.appendChild(el("input", {
      class: "mc-tasks-input mc-tasks-input-name",
      placeholder: "Task name (e.g. Call customer about Civic brakes)",
      value: d.name,
      onInput: (e) => { d.name = e.target.value; },
      required: true,
    }));

    form.appendChild(el("textarea", {
      class: "mc-tasks-input mc-tasks-input-desc",
      placeholder: "Details (optional)",
      rows: 2,
      onInput: (e) => { d.description = e.target.value; },
    }, [d.description]));

    const prioSelect = el("select", {
      class: "mc-tasks-input mc-tasks-input-prio",
      onChange: (e) => { d.priority = e.target.value; },
    });
    for (const v of ["low", "medium", "high"]) {
      const opt = el("option", { value: v }, [PRIORITY_LABEL[v]]);
      if (d.priority === v) opt.selected = true;
      prioSelect.appendChild(opt);
    }

    const dateInput = el("input", {
      class: "mc-tasks-input mc-tasks-input-date",
      type: "date",
      value: d.date,
      onChange: (e) => { d.date = e.target.value; },
    });

    form.appendChild(el("div", { class: "mc-tasks-add-row" }, [
      prioSelect,
      dateInput,
      el("button", {
        type: "submit",
        class: "mc-tasks-btn mc-tasks-btn-primary",
        disabled: state.busy || !d.name.trim(),
      }, ["+ Add"]),
    ]));

    return form;
  },

  _renderTaskList(state, ctx) {
    if (state.tasks.length === 0) {
      return el("div", { class: "mc-tasks-empty" }, [
        "No tasks yet. Add one above — it'll be saved locally and pushed to MAIC on next sync.",
      ]);
    }

    const wrap = el("div", { class: "mc-tasks-list" });

    const dates = Object.keys(state.byDate).sort();
    for (const date of dates) {
      const header = el("div", { class: "mc-tasks-date-header" }, [
        this._dateLabel(date),
      ]);
      wrap.appendChild(header);

      for (const task of state.byDate[date]) {
        wrap.appendChild(this._renderTaskRow(state, ctx, task));
      }
    }
    return wrap;
  },

  _renderTaskRow(state, ctx, task) {
    const isEditing = state.editingId === task.local_id;
    const completed = task.completed;

    const row = el("div", {
      class: `mc-tasks-row ${completed ? "is-completed" : ""}`,
      title: task.local_id ? `local_id: ${task.local_id}` : "(no local_id — will be assigned on save)",
    });

    // Checkbox
    const checkbox = el("input", {
      type: "checkbox",
      class: "mc-tasks-checkbox",
      checked: completed ? "checked" : null,
      onChange: () => this._onToggle(state, ctx, task),
    });
    row.appendChild(checkbox);

    // Main content
    if (isEditing) {
      const editForm = el("form", {
        class: "mc-tasks-edit-form",
        onSubmit: (e) => {
          e.preventDefault();
          this._onSaveEdit(state, ctx, task);
        },
      });
      const nameInput = el("input", {
        class: "mc-tasks-input",
        value: task.name,
        onInput: (e) => { task._draftName = e.target.value; },
      });
      const descInput = el("textarea", {
        class: "mc-tasks-input",
        rows: 2,
        onInput: (e) => { task._draftDesc = e.target.value; },
      }, [task.description]);
      const prioSelect = el("select", {
        class: "mc-tasks-input",
        onChange: (e) => { task._draftPrio = e.target.value; },
      });
      for (const v of ["low", "medium", "high"]) {
        const opt = el("option", { value: v }, [PRIORITY_LABEL[v]]);
        if (task.priority === v) opt.selected = true;
        prioSelect.appendChild(opt);
      }
      const dateInput = el("input", {
        class: "mc-tasks-input",
        type: "date",
        value: task.date,
        onChange: (e) => { task._draftDate = e.target.value; },
      });
      editForm.appendChild(el("div", { class: "mc-tasks-edit-fields" }, [
        nameInput,
        descInput,
        el("div", { class: "mc-tasks-edit-row" }, [prioSelect, dateInput]),
        el("div", { class: "mc-tasks-edit-actions" }, [
          el("button", {
            type: "button",
            class: "mc-tasks-btn mc-tasks-btn-secondary",
            onClick: () => { state.editingId = null; this._rerenderBody(state, ctx); },
          }, ["Cancel"]),
          el("button", {
            type: "submit",
            class: "mc-tasks-btn mc-tasks-btn-primary",
          }, ["Save"]),
        ]),
      ]));
      row.appendChild(editForm);
      // Stash draft fields on the task object (avoid closure leaks).
      task._draftName = task.name;
      task._draftDesc = task.description;
      task._draftPrio = task.priority;
      task._draftDate = task.date;
    } else {
      const prioColor = PRIORITY_COLOR[task.priority] || "#888";
      const main = el("div", { class: "mc-tasks-row-main" }, [
        el("div", { class: "mc-tasks-row-name" }, [task.name]),
        task.description
          ? el("div", { class: "mc-tasks-row-desc" }, [task.description])
          : null,
        el("div", { class: "mc-tasks-row-meta" }, [
          el("span", { class: "mc-tasks-prio", style: `color: ${prioColor};` }, [PRIORITY_LABEL[task.priority] || task.priority]),
        ]),
      ]);
      row.appendChild(main);

      const actions = el("div", { class: "mc-tasks-row-actions" }, [
        el("button", {
          class: "mc-tasks-btn-icon",
          title: "Edit",
          onClick: () => {
            state.editingId = task.local_id;
            this._rerenderBody(state, ctx);
          },
        }, ["✎"]),
        el("button", {
          class: "mc-tasks-btn-icon mc-tasks-btn-icon-danger",
          title: "Delete",
          onClick: () => this._onDelete(state, ctx, task),
        }, ["🗑"]),
      ]);
      row.appendChild(actions);
    }
    return row;
  },

  _dateLabel(date) {
    const today = todayIso();
    if (date === today) return `Today · ${date}`;
    const d = new Date(date + "T00:00:00");
    const weekday = d.toLocaleDateString(undefined, { weekday: "long" });
    return `${weekday} · ${date}`;
  },

  // ── event handlers ────────────────────────────────────────

  _currentCtx() {
    // The registry passes ctx only to mount(); keep a reference for
    // rerender calls by stashing it on first mount.
    return this._state?._ctx || {};
  },

  async _onAdd(state, ctx) {
    const d = state.draft;
    if (!d.name.trim()) return;
    state.busy = true;
    this._rerenderBody(state, ctx);
    try {
      await invoke("mc_task_add", {
        args: {
          name: d.name.trim(),
          description: d.description.trim() || null,
          priority: d.priority,
          date: d.date,
        },
      });
      d.name = "";
      d.description = "";
      d.priority = "medium";
      d.date = todayIso();
      await this._reload(state, ctx);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  async _onToggle(state, ctx, task) {
    state.busy = true;
    this._rerenderBody(state, ctx);
    try {
      await invoke("mc_task_done", { localId: task.local_id, completed: !task.completed });
      await this._reload(state, ctx);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  async _onDelete(state, ctx, task) {
    if (!confirm(`Delete task "${task.name}"? This can't be undone.`)) return;
    state.busy = true;
    this._rerenderBody(state, ctx);
    try {
      await invoke("mc_task_delete", { localId: task.local_id });
      await this._reload(state, ctx);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  async _onSaveEdit(state, ctx, task) {
    state.busy = true;
    this._rerenderBody(state, ctx);
    try {
      await invoke("mc_task_update", {
        args: {
          local_id: task.local_id,
          name: task._draftName ?? task.name,
          description: task._draftDesc ?? task.description,
          priority: task._draftPrio ?? task.priority,
          date: task._draftDate ?? task.date,
          completed: null,
        },
      });
      state.editingId = null;
      await this._reload(state, ctx);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  async _onAutoSync(state, ctx) {
    // Lesson 736: fire-and-forget sync on page mount. Best-effort —
    // we silently fall back to local data if MAIC is unreachable.
    // Sets autoSyncDone so we never double-sync within one mount.
    state.autoSyncing = true;
    state.autoSyncError = null;
    state.autoSyncDone = true;
    this._rerenderHeader(state, ctx);
    try {
      // Reuse the same command as the manual button; pass `server: null`
      // to use the default MAIC URL from settings.
      await invoke("mc_task_sync", { server: null });
      // Reload from disk to surface the merged state.
      await this._reload(state, ctx);
    } catch (e) {
      // Silent failure — surface a tiny "⚠ Stale" badge in the header
      // so the user knows the local view might be out of date, but
      // don't block the page or show a scary error.
      state.autoSyncError = String(e?.message || e);
    } finally {
      state.autoSyncing = false;
      this._rerenderHeader(state, ctx);
    }
  },

  async _onSync(state, ctx) {
    if (!confirm("Sync tasks with MAIC? Local changes will be pushed; remote changes pulled. Conflicts resolve by latest `updated_at` (server wins on tie).")) return;
    state.busy = true;
    state.error = null;
    this._rerenderBody(state, ctx);
    try {
      const report = await invoke("mc_task_sync", { server: null });
      state._syncReport = report;
      // Clear any stale auto-sync indicator — manual sync succeeded.
      state.autoSyncError = null;
      await this._reload(state, ctx);
      alert(`MAIC sync complete:\n\n${report}`);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },

  async _onLogin(state, ctx) {
    const token = prompt(
      "Paste a MAIC API token (long-lived, not your session JWT).\n\n" +
      "Leave blank to cancel. You can also skip this — sync will use your active session token automatically."
    );
    if (!token) return;
    state.busy = true;
    this._rerenderBody(state, ctx);
    try {
      await invoke("mc_task_login", { token: token.trim() });
      await this._reload(state, ctx);
    } catch (e) {
      state.error = String(e?.message || e);
      state.busy = false;
      this._rerenderBody(state, ctx);
    }
  },
};

// Stash ctx for re-renders after mount() returns. The original mount()
// reassigns `this._state` to a fresh object, so we have to set _ctx
// AFTER it runs. The _currentCtx() helper reads from the state object.
const _origMount = tasksPage.mount.bind(tasksPage);
tasksPage.mount = async function (root, ctx) {
  const result = await _origMount(root, ctx);
  if (this._state) this._state._ctx = ctx;
  return result;
};

export function mount(root, ctx) {
  return tasksPage.mount(root, ctx);
}

export function unmount() {
  return tasksPage.unmount();
}

// Register with the page registry (Lesson 713 page_registry pattern).
import { register } from "../page_registry.js";
register("tasks", { mount, unmount, label: tasksPage.label, icon: tasksPage.icon, requiresAuth: true });
