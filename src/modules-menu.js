// src/modules-menu.js — shared module picker + run modal for the
// dashboard and terminal toolbars. Replaces the per-page openModuleModal
// (which was private to dashboard_launchpad.js) and adds the
// Copy / Send-to-Chat actions that bridge scrape-style tools into the
// agent chat.
//
// Public API:
//   - mountModulesMenu(buttonEl, { surface })  attach dropdown behavior
//   - openModuleModal(module, { surface })     open form, run, show result
//   - closeModuleModal()                       close any open module modal
//   - sendResultToChat(text)                   use voice's fallback chain
//
// The component is inline in the page (real <button>/<div> in the DOM),
// not a floating hover popup. The dropdown is a sibling of the trigger
// button so the page layout absorbs it naturally (Lesson 968 sibling
// lesson applies here too: both elements share the same parent and both
// need their own positioning rules).
//
// Wiring: callers provide a trigger button and the menu is mounted
// next to it. The trigger button's text/icon is the caller's choice —
// we just inject the .mc-modules-menu element and wire behavior.

import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { listInstalledModules, isModuleInstalled, invokeModule } from "./modules-runtime.js";
import { toast } from "./toast.js";

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function cssEscape(s) {
  if (window.CSS && CSS.escape) return CSS.escape(s);
  return String(s).replace(/[^a-zA-Z0-9_-]/g, (c) => `\\${c}`);
}

// ─────────────────────────────────────────────────────────────────────
// send-to-chat bridge
//   Same lookup chain the voice modal uses (src/pages/dashboard.js:859-
//   870, Lesson 574). Tries the page's chat composer first; falls back
//   to the OS clipboard so the user can paste manually.
// ─────────────────────────────────────────────────────────────────────

export async function sendResultToChat(text) {
  if (!text) return false;
  const chatInput =
    document.querySelector("#dashboard-chat-input") ||
    document.querySelector("textarea[name='message']");
  if (chatInput) {
    if (chatInput.value && !chatInput.value.endsWith("\n")) {
      chatInput.value = chatInput.value.trimEnd() + "\n";
    }
    chatInput.value = chatInput.value + text;
    chatInput.focus();
    toast("Added to chat input", { kind: "success" });
    return true;
  }
  try {
    await writeText(text);
    toast("Copied to clipboard — Ctrl+V into chat", { kind: "info" });
    return true;
  } catch (e) {
    toast(`Failed to copy: ${e}`, { kind: "error" });
    return false;
  }
}

// ─────────────────────────────────────────────────────────────────────
// form_schema parsing — same rules as src/pages/dashboard_launchpad.js
// and src-tauri/src/form_schema.rs. If you change this, change them.
// ─────────────────────────────────────────────────────────────────────

function propJsonSchemaTypeToUiType(prop) {
  if (!prop || typeof prop !== "object") return "string";
  if (Array.isArray(prop.type)) {
    if (prop.type.includes("boolean")) return "boolean";
    if (prop.type.includes("number") || prop.type.includes("integer")) return "number";
    if (prop.type.includes("string")) return "string";
  }
  switch (prop.type) {
    case "boolean": return "boolean";
    case "integer":
    case "number":  return "number";
    case "array":   return "array";
    case "object":  return "object";
    default:        return "string";
  }
}

function formFieldsFromSchema(schema) {
  if (!schema || typeof schema !== "object") return [];
  if (schema.type !== "object") return [];
  const properties = schema.properties || {};
  const required = Array.isArray(schema.required) ? schema.required : [];
  const out = [];
  let order = 0;
  for (const [name, prop] of Object.entries(properties)) {
    if (!prop || typeof prop !== "object") continue;
    const label = prop.title || name;
    const isRequired = required.includes(name);
    const type = propJsonSchemaTypeToUiType(prop);
    const field = {
      name, label, type, required: isRequired, order: order++,
      description: prop.description || "",
      default: prop.default,
      minimum: typeof prop.minimum === "number" ? prop.minimum : null,
      maximum: typeof prop.maximum === "number" ? prop.maximum : null,
      enum: Array.isArray(prop.enum) ? prop.enum : null,
      pattern: typeof prop.pattern === "string" ? prop.pattern : null,
    };
    out.push(field);
  }
  out.sort((a, b) => a.order - b.order);
  return out;
}

function renderField(f) {
  const id = `mc-modal-field-${cssEscape(f.name)}`;
  const req = f.required ? " required" : "";
  const descHtml = f.description
    ? `<small class="mc-modal-field-desc">${escapeHtml(f.description)}</small>`
    : "";
  if (f.type === "boolean") {
    // Hidden "" sent first so unchecked state appears in FormData.
    return `
      <div class="mc-modal-field mc-modal-field-bool">
        <label class="mc-modal-checkbox">
          <input type="hidden" name="${escapeHtml(f.name)}" value="" />
          <input type="checkbox" id="${id}" name="${escapeHtml(f.name)}" value="true"${req} />
          <span>${escapeHtml(f.label)}</span>
        </label>
        ${descHtml}
      </div>
    `;
  }
  if (f.type === "number" || f.type === "integer") {
    const min = f.minimum != null ? ` min="${f.minimum}"` : "";
    const max = f.maximum != null ? ` max="${f.maximum}"` : "";
    const def = f.default != null ? ` value="${escapeHtml(f.default)}"` : "";
    return `
      <div class="mc-modal-field">
        <label for="${id}">${escapeHtml(f.label)}${f.required ? ' <span class="mc-modal-req">*</span>' : ""}</label>
        <input type="number" id="${id}" name="${escapeHtml(f.name)}"${req}${min}${max}${def} step="any" />
        ${descHtml}
      </div>
    `;
  }
  if (f.enum && f.enum.length > 0) {
    const opts = f.enum
      .map((v) => `<option value="${escapeHtml(v)}">${escapeHtml(v)}</option>`)
      .join("");
    return `
      <div class="mc-modal-field">
        <label for="${id}">${escapeHtml(f.label)}${f.required ? ' <span class="mc-modal-req">*</span>' : ""}</label>
        <select id="${id}" name="${escapeHtml(f.name)}"${req}>
          <option value="">— select —</option>
          ${opts}
        </select>
        ${descHtml}
      </div>
    `;
  }
  // Default: text input
  const def = f.default != null ? ` value="${escapeHtml(f.default)}"` : "";
  return `
    <div class="mc-modal-field">
      <label for="${id}">${escapeHtml(f.label)}${f.required ? ' <span class="mc-modal-req">*</span>' : ""}</label>
      <input type="text" id="${id}" name="${escapeHtml(f.name)}"${req}${def} />
      ${descHtml}
    </div>
  `;
}

function isCheckboxField(form, name) {
  const el = form.querySelector(`input[type="checkbox"][name="${cssEscape(name)}"]`);
  return !!el;
}

// ─────────────────────────────────────────────────────────────────────
// Result formatting — strips markdown ```json fences from the model's
// outputs so the preview is clean, but keeps the original string on a
// data attribute so Copy / Send-to-Chat can use the raw output.
// ─────────────────────────────────────────────────────────────────────

function stripFences(s) {
  return String(s ?? "")
    .replace(/^```(?:json)?\n/, "")
    .replace(/\n```$/, "")
    .trim();
}

// ─────────────────────────────────────────────────────────────────────
// Module modal
// ─────────────────────────────────────────────────────────────────────

let _openModal = null;

export function closeModuleModal() {
  if (_openModal && _openModal.parentNode) {
    _openModal.parentNode.removeChild(_openModal);
  }
  _openModal = null;
}

export function openModuleModal(module, opts = {}) {
  closeModuleModal();
  const tools = (module.client_manifest && module.client_manifest.tools) || [];
  if (tools.length === 0) {
    toast(`${module.name} has no tools to run.`);
    return;
  }
  const firstTool = tools[0];
  const fields = formFieldsFromSchema(firstTool.form_schema);
  const fieldsHtml = fields.map(renderField).join("");

  const modal = document.createElement("div");
  modal.className = "mc-modal";
  modal.innerHTML = `
    <div class="mc-modal-backdrop" data-action="modal-close"></div>
    <div class="mc-modal-panel">
      <header class="mc-modal-header">
        <h3>${escapeHtml(module.name)} · ${escapeHtml(firstTool.name)}</h3>
        <button class="mc-iconbtn" data-action="modal-close" title="Close">×</button>
      </header>
      <form class="mc-modal-form" data-module="${escapeHtml(module.module_id)}" data-tool="${escapeHtml(firstTool.name)}">
        ${fieldsHtml || `<p class="mc-modal-empty">${escapeHtml(firstTool.description || "No inputs needed.")}</p>`}
        <footer class="mc-modal-footer">
          <button type="button" data-action="modal-close">Cancel</button>
          <button type="submit" class="mc-primary">Run</button>
        </footer>
      </form>
      <div class="mc-modal-result" hidden></div>
    </div>
  `;
  document.body.appendChild(modal);
  _openModal = modal;

  modal.addEventListener("click", (ev) => {
    const target = ev.target.closest("[data-action]");
    if (!target) return;
    if (target.dataset.action === "modal-close") {
      closeModuleModal();
    }
  });

  const form = modal.querySelector("form.mc-modal-form");
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (!form.reportValidity()) return;
    const fd = new FormData(form);
    const inputs = {};
    for (const [k, v] of fd.entries()) {
      if (typeof v === "string" && v === "" && isCheckboxField(form, k)) continue;
      inputs[k] = v;
    }
    for (const f of fields) {
      if ((f.type === "number" || f.type === "integer") && k in inputs) {
        const n = Number(inputs[k]);
        if (!Number.isNaN(n)) inputs[k] = n;
      }
    }
    for (const f of fields) {
      if (!(f.name in inputs) && f.default !== undefined) inputs[f.name] = f.default;
    }
    for (const f of fields) {
      if (f.type === "boolean") {
        inputs[f.name] = !!form.querySelector(`input[name="${cssEscape(f.name)}"]:checked`);
      }
    }
    const broken = [];
    for (const f of fields) {
      if ((f.type === "number" || f.type === "integer") && inputs[f.name] !== undefined) {
        if (typeof inputs[f.name] === "string" && inputs[f.name].trim() === "") {
          delete inputs[f.name];
        } else if (typeof inputs[f.name] === "number" && !Number.isFinite(inputs[f.name])) {
          broken.push(f);
        }
      }
    }
    if (broken.length > 0) {
      const result = modal.querySelector(".mc-modal-result");
      result.hidden = false;
      result.textContent = `Invalid number: ${broken.map((f) => f.label).join(", ")}. Use a numeric value (e.g. 42, 3.14).`;
      return;
    }
    const result = modal.querySelector(".mc-modal-result");
    result.hidden = false;
    result.textContent = "Running…";
    result.classList.remove("mc-modal-result-error");
    try {
      const run = await invoke("mc_run_module", {
        moduleId: module.module_id,
        tool: firstTool.name,
        inputs,
        surface: opts.surface || "tauri",
      });
      const out = String(run.outputs ?? "");
      const stripped = stripFences(out);
      result.textContent = stripped || out || "(no output)";
      result.dataset.rawOutput = out;
      renderResultActions(result, out, opts);
    } catch (err) {
      const mapped = mapMclawError(err);
      result.hidden = false;
      result.classList.add("mc-modal-result-error");
      result.textContent = mapped.headline;
      result.dataset.rawError = String(err?.message || err);
      if (mapped.detail && mapped.detail !== mapped.headline) {
        const details = document.createElement("details");
        details.className = "mc-modal-result-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Show details";
        details.appendChild(summary);
        const pre = document.createElement("pre");
        pre.textContent = mapped.detail;
        details.appendChild(pre);
        result.appendChild(details);
      }
      if (mapped.action && mapped.action.callback) {
        const btn = document.createElement("button");
        btn.className = "mc-link mc-modal-action";
        btn.textContent = mapped.action.label;
        btn.addEventListener("click", mapped.action.callback);
        result.appendChild(btn);
      }
    }
  });
}

// Append the Copy / Send-to-Chat / Close actions to the result area.
// These let the user bridge scraped content into the agent chat.
function renderResultActions(resultEl, rawOutput, opts) {
  // Remove any prior actions
  const prior = resultEl.parentNode.querySelector(".mc-modal-result-actions");
  if (prior) prior.remove();

  const actions = document.createElement("div");
  actions.className = "mc-modal-result-actions";
  actions.innerHTML = `
    <button type="button" class="mc-primary" data-result-action="send-chat">📤 Send to Chat</button>
    <button type="button" data-result-action="copy">📋 Copy</button>
    <button type="button" data-result-action="close">✕ Close</button>
  `;
  resultEl.parentNode.appendChild(actions);

  const text = stripFences(rawOutput) || rawOutput || "";
  actions.addEventListener("click", async (ev) => {
    const btn = ev.target.closest("[data-result-action]");
    if (!btn) return;
    const action = btn.dataset.resultAction;
    if (action === "copy") {
      try {
        await writeText(text);
        toast("Copied to clipboard", { kind: "success" });
      } catch (e) {
        toast(`Copy failed: ${e}`, { kind: "error" });
      }
    } else if (action === "send-chat") {
      const ok = await sendResultToChat(text);
      if (ok && opts && opts.closeAfterSend !== false) {
        closeModuleModal();
      }
    } else if (action === "close") {
      closeModuleModal();
    }
  });
}

// Friendly error mapper — same surface as dashboard_launchpad.js so
// users see consistent messages across surfaces.
function mapMclawError(err) {
  const msg = String(err?.message || err || "");
  if (/401|session.*expired|not logged in/i.test(msg)) {
    return {
      headline: "Your MAIC session expired. Sign in again and retry.",
      detail: msg,
      action: null,
    };
  }
  if (/429|quota|rate.?limit/i.test(msg)) {
    return {
      headline: "MAIC rate-limited this run. Wait a moment and try again.",
      detail: msg,
      action: null,
    };
  }
  if (/network|fetch|timeout/i.test(msg)) {
    return {
      headline: "Network error talking to MAIC. Check your connection.",
      detail: msg,
      action: null,
    };
  }
  return { headline: msg || "Unknown error", detail: msg, action: null };
}

// ─────────────────────────────────────────────────────────────────────
// Modules dropdown
// ─────────────────────────────────────────────────────────────────────

let _openMenu = null;

function closeMenu() {
  if (_openMenu && _openMenu.parentNode) {
    _openMenu.parentNode.removeChild(_openMenu);
  }
  _openMenu = null;
  document.removeEventListener("click", onDocClickCloseMenu, true);
  document.removeEventListener("keydown", onEscCloseMenu, true);
  window.removeEventListener("scroll", onScrollCloseMenu, true);
  window.removeEventListener("resize", onScrollCloseMenu, true);
}

function onScrollCloseMenu() {
  // The menu is anchored to a position captured at click time. If
  // the page scrolls or resizes, the menu will be in the wrong
  // place — close rather than drift.
  closeMenu();
}

function onDocClickCloseMenu(ev) {
  if (!_openMenu) return;
  if (_openMenu.contains(ev.target)) return;
  if (ev.target.closest("[data-modules-menu-trigger]")) return;
  closeMenu();
}

function onEscCloseMenu(ev) {
  if (ev.key === "Escape") closeMenu();
}

/**
 * Mount a Modules dropdown anchored to `buttonEl`.
 *
 * @param {HTMLElement} buttonEl  trigger button. Caller decides label/icon.
 * @param {object}      opts
 *   - surface: "dashboard" | "terminal" | "tauri" — passed to mc_run_module
 *   - renderItem: (mod) => { icon, name, subtitle } — UI customization
 */
export function mountModulesMenu(buttonEl, opts = {}) {
  if (!buttonEl) return;
  buttonEl.setAttribute("data-modules-menu-trigger", "1");
  buttonEl.addEventListener("click", async (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
    if (_openMenu) {
      closeMenu();
      return;
    }
    const items = listInstalledModules();
    if (items.length === 0) {
      toast("No modules installed yet. Visit Settings → Modules.", { kind: "info" });
      return;
    }
    // Build dropdown — sibling of the button so layout absorbs it
    // naturally. The menu is positioned absolutely below the button.
    const menu = document.createElement("div");
    menu.className = "mc-modules-menu";
    menu.setAttribute("role", "menu");
    menu.innerHTML = items
      .map((m) => {
        const r = (opts.renderItem || defaultRenderItem)(m);
        return `
          <button class="mc-modules-menu-item" type="button" role="menuitem"
                  data-module-id="${escapeHtml(m.id)}">
            <span class="mc-modules-menu-icon">${escapeHtml(r.icon || "🧩")}</span>
            <span class="mc-modules-menu-text">
              <span class="mc-modules-menu-name">${escapeHtml(r.name)}</span>
              ${r.subtitle ? `<span class="mc-modules-menu-sub">${escapeHtml(r.subtitle)}</span>` : ""}
            </span>
          </button>
        `;
      })
      .join("");
    // Place the menu right after the button in DOM (sibling).
    buttonEl.parentNode.insertBefore(menu, buttonEl.nextSibling);
    // Position: directly below the button.
    const rect = buttonEl.getBoundingClientRect();
    menu.style.top = `${rect.bottom + 4}px`;
    menu.style.left = `${rect.left}px`;
    _openMenu = menu;

    menu.addEventListener("click", async (itemEv) => {
      const item = itemEv.target.closest("[data-module-id]");
      if (!item) return;
      const moduleId = item.dataset.moduleId;
      closeMenu();
      // Fetch full module manifest (with client_manifest.tools) and open
      // the run modal.
      try {
        const list = await invoke("mc_list_installed_modules");
        const mod = (list.modules || []).find((m) => m.module_id === moduleId);
        if (!mod) {
          toast(`Module ${moduleId} not found.`, { kind: "error" });
          return;
        }
        openModuleModal(mod, { surface: opts.surface || "tauri" });
      } catch (e) {
        toast(`Failed to open module: ${e}`, { kind: "error" });
      }
    });

    setTimeout(() => {
      document.addEventListener("click", onDocClickCloseMenu, true);
      document.addEventListener("keydown", onEscCloseMenu, true);
      window.addEventListener("scroll", onScrollCloseMenu, true);
      window.addEventListener("resize", onScrollCloseMenu, true);
    }, 0);
  });
}

function defaultRenderItem(m) {
  return { icon: "🧩", name: m.name || m.id, subtitle: m.version ? `v${m.version}` : "" };
}

// Re-export so callers don't need a second import.
export { isModuleInstalled, invokeModule };
