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
// Lesson 1138 (2026-10-08, David): the client-side result panel
// (renderClientSideResult) navigates to the launchpad when the
// module runs locally. Static import keeps the dependency graph
// obvious; modules-menu.dom.test.mjs can still mock navigation if
// it needs to.
import { navigate } from "./navigation.js";
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

// Module ID aliases (Lesson 1136, 2026-10-08, David). MAIC tracks
// modules under hyphenated product names (e.g. `email-campaigns-pro`,
// `voice-memo`) while the bundled installer registry on disk uses
// shorter generic names (`email`, `voice`). They're the same
// underlying product, but a naive merge by exact id counted both
// and David saw 18 "modules installed" in the Terminal toolbar
// badge on 2026-10-08 when the truth is 8.
//
// Direction: MAIC always wins on overlap because it has live install
// state + the rich client_manifest (tools, form_schema, description,
// category). The local registry is a fallback / source for modules
// that exist on disk but haven't been claimed by MAIC (e.g. standalone
// sidecar installs that pre-date MAIC enrollment).
//
// When adding a new MAIC module, also add its local-bundle id here
// (or its MAIC id) so the dropdown and badge count it once.
const MODULE_ID_ALIASES = {
  // MAIC id             -> canonical (local-bundle id)
  "email-campaigns-pro": "email",
  "calendar-companion":  "calendar",
  "voice-memo":          "voice",
  "brand-asset-kit":     "brand",
  "ad-performance":      "ads",
  "data-analyzer":       "data",
  "marketing-suite":     "marketing",
};
function canonicalId(id) {
  if (!id) return id;
  if (Object.prototype.hasOwnProperty.call(MODULE_ID_ALIASES, id)) {
    return MODULE_ID_ALIASES[id];
  }
  // Reverse: if `id` is the local side of an alias, return the MAIC
  // id so the merged Set's dedup key matches what MAIC contributes.
  for (const [maic, local] of Object.entries(MODULE_ID_ALIASES)) {
    if (local === id) return maic;
  }
  return id;
}

// Lesson 976 (2026-09-11 18:05 MDT, David): shared helper that returns
// the merged installed-modules list (local + remote). The count badge
// on the Modules button and the dropdown both need to agree on the
// same set — if they disagree, the user sees "0" on the badge but
// FireCrawl and Marketing Suite in the dropdown, which is confusing
// and looks broken. Both surfaces should call this same helper.
//
// Local modules come from `listInstalledModules()` (populated from the
// local Rust `mc_module_list` registry at boot). Remote modules come
// from `invoke("mc_list_installed_modules")` (MAIC's
// /v1/modules/installed). Remote wins for overlap (more recent
// install state). Local-only items keep their richer hooks info.
//
// If the remote call fails (offline, JWT expired, MAIC down), fall
// back to the local list so the UI still works.
export async function getInstalledModulesMerged() {
  const localItems = listInstalledModules();
  try {
    const remote = await invoke("mc_list_installed_modules");
    const remoteItems = (remote?.modules || []).map(normalizeRemoteModule);
    // Lesson 1136 (2026-10-08, David): dedup by canonical id so
    // `email-campaigns-pro` (MAIC) and `email` (local) collapse to a
    // single entry. MAIC wins on overlap. Local-only modules (no MAIC
    // counterpart) keep their (offline copy) tag.
    const remoteCanonicals = new Set(remoteItems.map((m) => canonicalId(m.id)));
    const merged = [...remoteItems];
    for (const m of localItems) {
      if (remoteCanonicals.has(canonicalId(m.id))) continue;
      merged.push({ ...m, _offline: true });
    }
    return merged;
  } catch (e) {
    console.warn("[modules-menu] remote list failed, using local only:", e);
    return localItems;
  }
}

// Map a MAIC /v1/modules/installed entry to our internal {id, name,
// version, hooks, icon} shape (same as the local list).
function normalizeRemoteModule(m) {
  const cm = m.client_manifest || {};
  return {
    id: m.module_id,
    name: m.name,
    version: cm.version || "",
    hooks: cm.ui_hooks || [],
    icon: m.icon || cm.icon,
    _remote: true,
    // Lesson 980: keep the raw MAIC shape so the dropdown click
    // handler can open the run modal without re-fetching (which
    // used to fail silently if the second mc_list_installed_modules
    // call returned a slightly different shape or blipped).
    _raw: m,
  };
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
  // rc57.20: switch from <small class="mc-modal-field-desc"> (un-styled,
  // no CSS rule exists for .mc-modal-field-desc) to <p class="mc-field-help">
  // so per-field help text on the toolbar modal matches the launchpad's
  // dashboard_launchpad.js renderField() (line 749). Capped at 240 chars
  // + HTML-escaped to match Lesson 1132 rules in mc-toolbar.js. The
  // existing .mc-field-help CSS rule (styles.css:6717) gives us
  // font-size:12px, color:var(--fg-muted), line-height:1.4, margin:0
  // — the right shape for a small muted line under an input.
  const descText = (f.description || "").trim().slice(0, 240);
  const descHtml = descText
    ? `<p class="mc-field-help">${escapeHtml(descText)}</p>`
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

// Show a simple "module info" modal for modules that have no runnable
// tools in their client_manifest. Lesson 976 (2026-09-11 18:05 MDT,
// David): when a user clicks a module from the toolbar dropdown and
// nothing visible happens, they think the click is broken. A modal
// with the module's description, version, and category tells them
// what they clicked on, and a "Run from launchpad" button points
// them at the right place to use the module.
function openModuleInfoModal(module) {
  closeModuleModal();
  const cm = module.client_manifest || {};
  const version = cm.version || "";
  const description = module.description || cm.description || "No description provided.";
  const category = module.category || "";
  const icon = module.icon || cm.icon || "🧩";

  const modal = document.createElement("div");
  modal.className = "mc-modal";
  modal.innerHTML = `
    <div class="mc-modal-backdrop" data-action="modal-close"></div>
    <div class="mc-modal-panel">
      <header class="mc-modal-header">
        <h3><span class="mc-modules-menu-icon">${escapeHtml(icon)}</span> ${escapeHtml(module.name)}${version ? ` <span class="mc-modules-menu-sub">v${escapeHtml(version)}</span>` : ""}</h3>
        <button class="mc-iconbtn" data-action="modal-close" title="Close">×</button>
      </header>
      <div class="mc-modal-info">
        ${category ? `<div class="mc-modal-info-row"><span class="mc-modal-info-label">Category</span> ${escapeHtml(category)}</div>` : ""}
        <p class="mc-modal-info-desc">${escapeHtml(description)}</p>
        <p class="mc-modal-info-hint">This module has no runnable tools registered. Use the <strong>launchpad</strong> → "Your Tools" → <strong>Run</strong> button to use it, or visit Settings → Modules to manage installed modules.</p>
      </div>
      <footer class="mc-modal-footer">
        <button type="button" data-action="modal-close" class="mc-primary">OK</button>
      </footer>
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
}

export function openModuleModal(module, opts = {}) {
  // Lesson 985 follow-up (2026-09-16): diagnostic log so we can see
  // in the sidecar capture whether openModuleModal was reached and
  // how many tools the module has.
  console.log(
    `[miracle-claw] openModuleModal: moduleId=${module && (module.module_id || module.id)}, ` +
    `tools=${(module && module.client_manifest && module.client_manifest.tools || []).length}, ` +
    `surface=${opts.surface || "tauri"}`
  );
  closeModuleModal();
  const tools = (module.client_manifest && module.client_manifest.tools) || [];
  if (tools.length === 0) {
    // Lesson 976 (2026-09-11 18:05 MDT, David): clicking a module with
    // no tools used to just toast a 1-line "has no tools to run" and
    // bail. That felt like nothing happened. Show a useful info modal
    // instead: module description, version, and a hint to use the
    // launchpad's "Run" button.
    openModuleInfoModal(module);
    return;
  }
  const firstTool = tools[0];
  const fields = formFieldsFromSchema(firstTool.form_schema);
  const fieldsHtml = fields.map(renderField).join("");
  // rc57.20: surface module-level "How to use" instructions + tool
  // description in the modal, matching the launchpad's openModuleModal
  // (dashboard_launchpad.js:511-525). Before this, the toolbar dropdown
  // modal only showed a one-line fallback in <p class="mc-modal-empty">
  // when there were no fields, so users with form fields never saw the
  // tool's description or the expandable instructions panel that
  // launchpad users got in rc57.11. The data is already in
  // `module.client_manifest.instructions` (the click handler at line
  // ~781 unwraps `_raw` which carries the full client_manifest), so we
  // just need to render it.
  const instructions =
    (module.client_manifest && module.client_manifest.instructions) || "";

  const modal = document.createElement("div");
  modal.className = "mc-modal";
  modal.innerHTML = `
    <div class="mc-modal-backdrop" data-action="modal-close"></div>
    <div class="mc-modal-panel">
      <header class="mc-modal-header">
        <h3>${escapeHtml(module.name)} · ${escapeHtml(firstTool.name)}</h3>
        <button class="mc-iconbtn" data-action="modal-close" title="Close">×</button>
      </header>
      <p class="mc-modal-tool-desc">${escapeHtml(firstTool.description || "")}</p>
      ${instructions ? `
        <details class="mc-modal-instructions">
          <summary>How to use ${escapeHtml(module.name)}</summary>
          <p>${escapeHtml(instructions)}</p>
        </details>
      ` : ""}
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
    // Coerce number/integer inputs to JS numbers before sending. Use
    // the FIELD's name (f.name), not the FormData key from the outer
    // loop — the original had a stray `k` here that mostly worked
    // because the type guard short-circuited, but it was wrong (Lesson
    // 970, 2026-09-11 09:51 MDT).
    for (const f of fields) {
      if ((f.type === "number" || f.type === "integer") && f.name in inputs) {
        const n = Number(inputs[f.name]);
        if (!Number.isNaN(n)) inputs[f.name] = n;
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
      // Lesson 1138 (2026-10-08, David): MAIC returns
      // `{outputs: {client_side: true, message: "..."}}` for modules
      // that run locally (firecrawl, voice-memo, calendar-companion,
      // etc. — anything that has a sidecar on disk instead of a
      // server-side handler). The old code just JSON.stringified
      // outputs and showed it as "the result", which David read as
      // "errors out" because the only text was "This module runs
      // locally. Please use the Your Tools tile in the launchpad to
      // invoke it." Now we detect client_side, show a clear "Open in
      // launchpad" CTA, and skip the raw-output / Send-to-Chat
      // actions (they'd be useless without an actual result).
      if (
        run &&
        run.outputs &&
        typeof run.outputs === "object" &&
        run.outputs.client_side === true
      ) {
        const msg =
          (typeof run.outputs.message === "string" && run.outputs.message) ||
          "This module runs locally on your machine. Open the launchpad " +
            "and use the Your Tools tile to invoke it.";
        renderClientSideResult(result, msg, module, opts);
        return;
      }
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

// Lesson 1138 (2026-10-08, David): show a "this module runs
// locally — open the launchpad" panel when MAIC returns
// outputs.client_side === true. The previous behavior was to dump
// the raw MAIC message string into the result area, which David
// read as "the tool errored out" (it didn't — the run was a stub
// pointer to the local sidecar). A clear CTA + a single "Open in
// launchpad" button is the fix.
function renderClientSideResult(resultEl, message, module, opts) {
  resultEl.classList.remove("mc-modal-result-error");
  resultEl.classList.add("mc-modal-result-client-side");
  resultEl.textContent = "";
  const heading = document.createElement("div");
  heading.className = "mc-modal-result-client-side-heading";
  heading.textContent = "🖥  Runs locally on this machine";
  const body = document.createElement("p");
  body.className = "mc-modal-result-client-side-msg";
  body.textContent = message;
  const actions = document.createElement("div");
  actions.className = "mc-modal-result-actions";

  // Open in launchpad button (navigate.js exports `navigate`; if
  // the navigation system isn't installed yet the call returns
  // false and we surface the error in a toast).
  const openBtn = document.createElement("button");
  openBtn.type = "button";
  openBtn.className = "mc-primary";
  openBtn.textContent = "Open in launchpad →";
  openBtn.addEventListener("click", () => {
    try {
      closeModuleModal();
      const ok = navigate("dashboard-launchpad");
      if (!ok) {
        toast("Could not open the launchpad from this page.", { kind: "info" });
      }
    } catch (e) {
      toast(`Could not open launchpad: ${e}`, { kind: "error" });
    }
  });
  actions.appendChild(openBtn);

  const closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.textContent = "Close";
  closeBtn.addEventListener("click", () => closeModuleModal());
  actions.appendChild(closeBtn);

  resultEl.appendChild(heading);
  resultEl.appendChild(body);
  resultEl.appendChild(actions);
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
    // Lesson 975 (2026-09-11 17:09 MDT, David): the toolbar Modules
    // dropdown was reading `listInstalledModules()` which only knows
    // about LOCAL modules (populated from `mc_module_list` at boot).
    // Modules installed via MAIC's `/v1/modules/install` (Firecrawl,
    // etc.) are tracked remotely but never registered locally, so the
    // dropdown showed "No modules installed yet" even though the
    // launchpad's "Your Tools" grid showed them. Fix: query the
    // remote list (`mc_list_installed_modules`) for the dropdown and
    // merge it with the local list. Both sources together cover the
    // full installed set.
    //
    // Lesson 976: use the shared `getInstalledModulesMerged()` helper
    // so the count badge and the dropdown always agree on the same
    // set of installed modules.
    const items = await getInstalledModulesMerged();
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
              ${r.meta ? `<span class="mc-modules-menu-meta">${escapeHtml(r.meta)}</span>` : ""}
            </span>
          </button>
        `;
      })
      .join("");
    // Place the menu right after the button in DOM (sibling).
    buttonEl.parentNode.insertBefore(menu, buttonEl.nextSibling);
    // Position: directly below the button. Lesson 976 (2026-09-11
    // 18:05 MDT, David): if the button is close to the right edge,
    // the menu extends past the viewport and gets clipped. Measure
    // after attaching so we know the actual menu width, then right-
    // align to the button's right edge if it would clip.
    const rect = buttonEl.getBoundingClientRect();
    menu.style.top = `${rect.bottom + 4}px`;
    menu.style.left = `${rect.left}px`;
    menu.style.right = "auto";
    // Force a layout pass so we can measure.
    const menuWidth = menu.offsetWidth;
    const margin = 8;
    if (rect.left + menuWidth > window.innerWidth - margin) {
      // Right-align: menu's right edge at button's right edge.
      menu.style.left = "auto";
      menu.style.right = `${window.innerWidth - rect.right}px`;
    }
    // Also clip to bottom: if the menu would extend past the
    // viewport bottom, set max-height so it scrolls.
    const maxMenuHeight = window.innerHeight - rect.bottom - margin;
    if (maxMenuHeight < 200) {
      // Not enough room below — flip above the button.
      const neededHeight = 240; // approx. 4 items
      if (rect.top > neededHeight) {
        menu.style.top = `${rect.top - neededHeight - 4}px`;
      } else {
        menu.style.maxHeight = `${Math.max(120, window.innerHeight - margin * 2)}px`;
      }
    }
    _openMenu = menu;

    // Store the full module objects on the menu so the click handler
    // doesn't need to re-fetch (Lesson 980: re-fetching was fragile —
    // if the second mc_list_installed_modules call returned a slightly
    // different shape or the network blipped, the lookup silently
    // returned undefined and nothing opened).
    menu._items = items;
    menu.addEventListener("click", async (itemEv) => {
      const item = itemEv.target.closest("[data-module-id]");
      if (!item) return;
      const moduleId = item.dataset.moduleId;
      // Lesson 985 follow-up (2026-09-16): diagnostic logging so the
      // next sidecar capture will tell us whether the click reached the
      // handler, what moduleId was resolved, and whether mod was found.
      // Console-log prefix "[miracle-claw]" is picked up by the
      // launcher-stderr capture so it lands in MC-openclaw info.txt.
      console.log(`[miracle-claw] modules-menu click: moduleId=${moduleId}`);
      closeMenu();
      const cached = (menu._items || []).find((m) => (m.id || m.module_id) === moduleId);
      let mod = null;
      if (cached) {
        // The cached object from getInstalledModulesMerged() may be the
        // normalised {id, name, version, icon, _remote, _raw} shape, not
        // the raw {module_id, client_manifest} shape that openModuleModal
        // expects. Unwrap to the raw MAIC shape if we have it.
        if (cached._raw) mod = cached._raw;
        else if (cached.client_manifest || cached.module_id) mod = cached;
        else mod = Object.assign({}, cached, {
          module_id: cached.id,
          name: cached.name,
          icon: cached.icon,
          client_manifest: {
            name: cached.name,
            version: cached.version,
            tools: cached.tools || [],
          },
        });
      }
      console.log(
        `[miracle-claw] modules-menu lookup: cached=${cached ? "yes" : "no"}, ` +
        `mod=${mod ? "yes" : "no"}, tools=${(mod && mod.client_manifest && mod.client_manifest.tools || []).length}`
      );
      // Fallback: re-fetch from the backend. If we still don't have the
      // module after this, surface a clear error to the user.
      if (!mod) {
        try {
          const list = await invoke("mc_list_installed_modules");
          const arr = list.modules || list.installed || [];
          mod = arr.find((m) => m.module_id === moduleId);
        } catch (e) {
          toast(`Failed to open module ${moduleId}: ${e}`, { kind: "error" });
          return;
        }
      }
      if (!mod) {
        toast(`Module ${moduleId} not found in installed list.`, { kind: "error" });
        return;
      }
      openModuleModal(mod, { surface: opts.surface || "tauri" });
    });

    setTimeout(() => {
      document.addEventListener("click", onDocClickCloseMenu, true);
      document.addEventListener("keydown", onEscCloseMenu, true);
      window.addEventListener("scroll", onScrollCloseMenu, true);
      window.addEventListener("resize", onScrollCloseMenu, true);
    }, 0);
  });
}

// Lesson 1137 (2026-10-08, David): the Terminal dropdown used to
// show only `icon + name + "vX"` per row, which looked sparse next
// to the Windows chat toolbar dropdown (which shows description +
// tool list under each row). David saw the difference on the
// FireCrawl + Marketing Suite rows and flagged it as a parity bug.
// Match the Windows shape: name + one-line description + meta line
// with version, tool count, and tool names (same cap-and-ellipsis
// rules as mc-toolbar.js so the rows stay scannable).
function defaultRenderItem(m) {
  const cm = m.client_manifest || (m._raw && m._raw.client_manifest) || {};
  // Description: top-level on the merged item (rc57.13 added this
  // to the Windows normalizer), then MAIC row description, then
  // client_manifest.description.
  const raw = m._raw || {};
  const description =
    (typeof m.description === "string" ? m.description : "") ||
    (typeof raw.description === "string" ? raw.description : "") ||
    (typeof cm.description === "string" ? cm.description : "") ||
    "";
  const descTrimmed = description.length > 100
    ? description.slice(0, 97) + "…"
    : description;

  // Tools: prefer the merged `tools` (already normalized by the
  // Windows normalizer), then client_manifest.tools, then commands.
  const tools = Array.isArray(m.tools)
    ? m.tools
    : Array.isArray(cm.tools)
    ? cm.tools
    : Array.isArray(m.commands)
    ? m.commands
    : [];
  const toolCount = tools.length;
  let toolLabel;
  if (toolCount === 0) {
    toolLabel = "no tools — open in launchpad";
  } else if (toolCount === 1) {
    toolLabel = `1 tool: ${tools[0].name || tools[0].id || "?"}`;
  } else {
    const names = tools.map((t) => t.name || t.id || "?").join(", ");
    toolLabel = `${toolCount} tools: ${names.length > 80 ? names.slice(0, 77) + "…" : names}`;
  }

  const metaParts = [];
  if (m.version) metaParts.push(`v${m.version}`);
  metaParts.push(toolLabel);
  if (m._offline) metaParts.push("(offline copy)");

  return {
    icon: m.icon || cm.icon || "🧩",
    name: m.name || cm.name || m.id,
    subtitle: descTrimmed || metaParts.join(" · "),
    // New: third line shows the meta. The Terminal's
    // .mc-modules-menu-sub only renders one subtitle, so we put the
    // description in subtitle and append the meta as a separate
    // property that the dropdown template below renders as a second
    // line. If the description is empty, the template falls back to
    // meta as the sole subtitle (matching the old behavior).
    meta: descTrimmed ? metaParts.join(" · ") : "",
  };
}

// Re-export so callers don't need a second import.
export { isModuleInstalled, invokeModule };
