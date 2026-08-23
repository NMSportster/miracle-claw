// pages/notebook.js — MC Notebook tab (v1.0.9-rc46).
//
// Public API: { mount(root, ctx), unmount() }
//
// Surface:
//   - List of saved notes on the left.
//   - "New note" button at the top.
//   - Editor on the right: title input + Markdown textarea.
//   - Save / Delete buttons at the bottom of the editor.
//   - Auto-saves on blur (the user is unlikely to hit Ctrl+S).
//
// Storage: localStorage for v1, under the key `mc.notebook.v1`. Each
// note is { id, title, body, createdAt, updatedAt }. We keep at most
// `MAX_NOTES` and surface a count in the header.
//
// Notes are per-install (per localStorage origin) — not synced to MAIC
// in v1. A future iteration can hook `mc_save_note` / `mc_list_notes`
// against the MAIC user_notes table if the user signs in.

const STORAGE_KEY = "mc.notebook.v1";
const MAX_NOTES = 200;

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
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
    /* private mode or quota — ignore */
  }
}

function loadNotes() {
  const raw = safeLocalGet(STORAGE_KEY);
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed;
  } catch {
    return [];
  }
}

function saveNotes(notes) {
  safeLocalSet(STORAGE_KEY, JSON.stringify(notes));
}

function newId() {
  return `n_${Date.now().toString(36)}_${Math.random()
    .toString(36)
    .slice(2, 8)}`;
}

function formatTime(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  if (isNaN(d.getTime())) return "";
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(
    d.getDate()
  )} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export const notebookPage = {
  label: "Notebook",
  icon: "📓",
  requiresAuth: true,

  mount(root, ctx = {}) {
    const { onBackToDashboard } = ctx;
    let notes = loadNotes();
    let activeId = notes.length ? notes[0].id : null;
    let dirty = false;

    const render = () => {
      const active = notes.find((n) => n.id === activeId) || null;
      const sorted = [...notes].sort((a, b) => b.updatedAt - a.updatedAt);

      const listHtml = sorted.length
        ? sorted
            .map((n) => {
              const preview = (n.body || "").slice(0, 80).replace(/\n/g, " ");
              return `
                <div class="note-row ${n.id === activeId ? "active" : ""}" data-id="${escapeHtml(
                n.id
              )}">
                  <div class="note-row-title">${escapeHtml(n.title || "(untitled)")}</div>
                  <div class="note-row-preview muted small">${escapeHtml(preview)}</div>
                  <div class="note-row-time muted small">${escapeHtml(
                    formatTime(n.updatedAt)
                  )}</div>
                </div>`;
            })
            .join("")
        : `<div class="muted small">No notes yet. Click <strong>New note</strong> to start.</div>`;

      root.innerHTML = `
        <div class="notebook-page">
          <header class="notebook-header">
            <button class="link-button" id="nb-back">← Dashboard</button>
            <div class="notebook-title">Notebook <span class="muted small">(${
              notes.length
            }/${MAX_NOTES})</span></div>
            <button class="primary" id="nb-new">+ New note</button>
          </header>
          <div class="notebook-body">
            <div class="notebook-list" id="nb-list">${listHtml}</div>
            <div class="notebook-editor" id="nb-editor">
              ${
                active
                  ? `
                <input class="nb-title-input" id="nb-title" type="text"
                  placeholder="Title" value="${escapeHtml(active.title)}" />
                <textarea class="nb-body-input" id="nb-body" placeholder="Write a note… (Markdown)">${escapeHtml(
                  active.body
                )}</textarea>
                <div class="nb-actions">
                  <span class="muted small" id="nb-status">${
                    dirty ? "Unsaved changes" : `Saved ${formatTime(active.updatedAt)}`
                  }</span>
                  <button class="link-button" id="nb-delete">Delete</button>
                  <button class="primary" id="nb-save">Save</button>
                  <button class="primary" id="nb-save-new">Save &amp; New</button>
                </div>
              `
                  : `<div class="muted small">Create a note to start writing.</div>`
              }
            </div>
          </div>
        </div>
      `;

      // Wire list rows
      root.querySelectorAll(".note-row").forEach((el) => {
        el.addEventListener("click", () => {
          if (dirty) {
            if (!confirm("Discard unsaved changes?")) return;
          }
          activeId = el.dataset.id;
          dirty = false;
          render();
        });
      });

      // Header buttons
      const back = root.querySelector("#nb-back");
      if (back && onBackToDashboard) back.addEventListener("click", onBackToDashboard);

      const newBtn = root.querySelector("#nb-new");
      if (newBtn) {
        newBtn.addEventListener("click", () => {
          if (notes.length >= MAX_NOTES) {
            alert(`Notebook is full (${MAX_NOTES} notes). Delete some first.`);
            return;
          }
          // Auto-commit any unsaved typing into the currently-active note
          // BEFORE creating the new one. Without this, a user typing into
          // a fresh note + clicking +New would silently lose their text.
          if (dirty) {
            const cur = notes.find((n) => n.id === activeId);
            if (cur && titleInput && bodyInput) {
              cur.title = titleInput.value;
              cur.body = bodyInput.value;
              cur.updatedAt = Date.now();
            }
          }
          const note = {
            id: newId(),
            title: "",
            body: "",
            createdAt: Date.now(),
            updatedAt: Date.now(),
          };
          notes = [note, ...notes];
          activeId = note.id;
          dirty = false;
          saveNotes(notes);
          render();
        });
      }

      if (!active) return;

      // Editor wiring
      const titleInput = root.querySelector("#nb-title");
      const bodyInput = root.querySelector("#nb-body");
      const status = root.querySelector("#nb-status");
      const saveBtn = root.querySelector("#nb-save");
      const deleteBtn = root.querySelector("#nb-delete");

      const markDirty = () => {
        dirty = true;
        if (status) status.textContent = "Unsaved changes";
      };

      if (titleInput) titleInput.addEventListener("input", markDirty);
      if (bodyInput) bodyInput.addEventListener("input", markDirty);

      const persist = () => {
        const target = notes.find((n) => n.id === activeId);
        if (!target) return;
        target.title = titleInput.value;
        target.body = bodyInput.value;
        target.updatedAt = Date.now();
        dirty = false;
        saveNotes(notes);
        render();
      };

      if (saveBtn) saveBtn.addEventListener("click", persist);
      const saveNewBtn = root.querySelector("#nb-save-new");
      if (saveNewBtn) {
        saveNewBtn.addEventListener("click", () => {
          // Save current first.
          persist();
          if (notes.length >= MAX_NOTES) {
            alert(`Notebook is full (${MAX_NOTES} notes). Delete some first.`);
            return;
          }
          // Then create a fresh blank note and select it (without an
          // extra dirty check \u2014 we just persisted).
          const note = {
            id: newId(),
            title: "",
            body: "",
            createdAt: Date.now(),
            updatedAt: Date.now(),
          };
          notes = [note, ...notes];
          activeId = note.id;
          dirty = false;
          saveNotes(notes);
          render();
        });
      }
      if (titleInput) titleInput.addEventListener("blur", () => { if (dirty) persist(); });
      if (bodyInput) bodyInput.addEventListener("blur", () => { if (dirty) persist(); });

      if (deleteBtn) {
        deleteBtn.addEventListener("click", () => {
          const current = notes.find((n) => n.id === activeId);
          if (!confirm(`Delete "${current?.title || "this note"}"?`)) return;
          notes = notes.filter((n) => n.id !== activeId);
          activeId = notes.length ? notes[0].id : null;
          dirty = false;
          saveNotes(notes);
          render();
        });
      }
    };

    render();
  },

  unmount() {
    // No timers, no async work in flight.
  },
};
