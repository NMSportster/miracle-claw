// pages/dashboard_launchpad.js — rc56.0 launchpad landing page.
//
// Public API: { mount(root, ctx), unmount() }
//
// Render shape (5 sections, locked in mc-module-driven-surfaces-2026-09.md):
//   1. Topbar: 🦞 Miracle Claw | greeting | user menu | settings
//   2. Tier row: [Free ████░░░░ 12,400/50K]  [Plans & Pricing →]
//   3. Container heading: "Good {morning|afternoon|evening}, *David*."
//   4. Launch tiles (3): Windows / Terminal / Commands
//   5. Your Tools: grid of installed modules + [+ Add a tool]
//   6. Recent: last 3 module invocations
//
// Data sources (all proxied through Rust Tauri commands, never direct HTTP):
//   - mc_list_categories         → 16 categories
//   - mc_list_marketplace        → browse marketplace
//   - mc_list_installed_modules  → user's installed modules + client_manifest
//   - mc_install_module(id)      → install + return manifest
//   - mc_uninstall_module(id)    → uninstall
//
// Mount behavior:
//   - Fetches tier + quota + installed modules in parallel.
//   - Renders the 5 sections in order.
//   - Wires launch tiles to navigate('chat' | 'terminal' | ?).
//   - Wires module tiles to a modal that renders form_schema from the
//     module's client_manifest.
//
// Unmount behavior:
//   - Closes any open modal.
//   - Aborts any in-flight fetches via AbortController.

import { invoke } from "@tauri-apps/api/core";
import { navigate } from "../navigation.js";
import { toast } from "../toast.js";

// --- helpers -------------------------------------------------------------

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

// Week 7.3 (2026-09-10, David): friendly error mapping.
//
// The Rust side now surfaces MAIC's actual response body in the
// error string (e.g. `HTTP 401: {"detail":"Session expired or revoked"}`).
// This helper turns that into a short user-facing headline plus
// optional action. The full technical detail stays in the
// `result.dataset.rawError` so power users can copy-paste it
// into a bug report; the headline is what they read first.
//
// Returns { headline, detail, action } where action is a
// `{ label, callback }` or null.
function mapMclawError(err) {
  const raw = String(err?.message || err || "");

  // Parse the Rust-side `HTTP <code>: <body>` shape.
  const m = raw.match(/^HTTP\s+(\d{3}):\s*(.*)$/s);
  if (!m) {
    // Non-HTTP error (network, timeout, etc.). Show as-is.
    return { headline: raw || "Something went wrong.", detail: null, action: null };
  }
  const status = parseInt(m[1], 10);
  const body = m[2].trim();

  // Try to extract the MAIC `detail` field for the detail line.
  let detailText = body;
  try {
    const parsed = JSON.parse(body);
    if (parsed && typeof parsed.detail === "string") {
      detailText = parsed.detail;
    }
  } catch {
    // body wasn't JSON; use it as-is.
  }

  switch (status) {
    case 401:
      return {
        headline: "Your session expired. Please sign in again.",
        detail: detailText,
        action: { label: "Sign in", callback: () => navigate("login") },
      };
    case 403:
      return {
        headline: "You don't have access to this module.",
        detail: detailText,
        action: null,
      };
    case 404:
      return {
        headline: "This module isn't installed. Try installing it again.",
        detail: detailText,
        action: { label: "Install", callback: null /* caller fills in */ },
      };
    case 409:
      return {
        headline: "This module is already installed.",
        detail: detailText,
        action: null,
      };
    case 422:
      return {
        headline: "Invalid input. Check the form fields and try again.",
        detail: detailText,
        action: null,
      };
    case 429:
      return {
        headline: "Too many requests. Wait a moment and try again.",
        detail: detailText,
        action: null,
      };
    case 502:
    case 503:
    case 504:
      return {
        headline: "MAIC is temporarily unavailable. Try again in a moment.",
        detail: detailText,
        action: null,
      };
    default:
      return {
        headline: `MAIC returned ${status}.`,
        detail: detailText,
        action: null,
      };
  }
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
  return n.toLocaleString();
}

function greet() {
  const h = new Date().getHours();
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

function firstName(displayName) {
  if (!displayName) return "friend";
  return displayName.split(/\s+/)[0];
}

// --- state ---------------------------------------------------------------

let _abortController = null;
let _openModal = null;
let _currentRoot = null;
let _currentCtx = null;

function _closeModal() {
  if (_openModal) {
    _openModal.remove();
    _openModal = null;
  }
}

function unmount() {
  if (_abortController) _abortController.abort();
  _abortController = null;
  _closeModal();
  _currentRoot = null;
  _currentCtx = null;
}

// --- section renderers ---------------------------------------------------

function renderTopbar(displayName) {
  return `
    <header class="mc-topbar">
      <div class="mc-topbar-left">
        <span class="mc-brand">🦞 Miracle Claw</span>
      </div>
      <div class="mc-topbar-right">
        <button class="mc-user" data-action="user-menu">
          <span class="mc-user-name">${escapeHtml(displayName || "Account")}</span>
          <span class="mc-user-caret">▾</span>
        </button>
        <button class="mc-iconbtn" data-action="settings" title="Settings">⚙</button>
      </div>
    </header>
  `;
}

function renderTierRow(tier, quota) {
  const tierLabel = formatTierLabel(tier);
  const used = quota?.used ?? 0;
  const limit = quota?.limit ?? 0;
  const pct = limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  const bar = limit > 0
    ? `<div class="mc-bar"><div class="mc-bar-fill" style="width:${pct}%"></div></div>`
    : `<div class="mc-bar-empty">No quota loaded</div>`;
  return `
    <div class="mc-tier-row">
      <div class="mc-tier-badge" data-tier="${escapeHtml(tier)}">
        <span class="mc-tier-label">${escapeHtml(tierLabel)}</span>
        ${bar}
        <span class="mc-tier-numbers">${formatNumber(used)}/${formatNumber(limit)}</span>
      </div>
      <button class="mc-link" data-action="open-plans">Plans and Pricing →</button>
    </div>
  `;
}

function renderGreeting(displayName) {
  return `
    <h1 class="mc-greeting">${escapeHtml(greet())}, <em>${escapeHtml(firstName(displayName))}</em>.</h1>
  `;
}

function renderLaunchTiles() {
  return `
    <section class="mc-launch">
      <div class="mc-launch-grid">
        <button class="mc-launch-tile" data-action="launch-chat">
          <div class="mc-launch-icon">🦞</div>
          <div class="mc-launch-title">Miracle Claw · Windows</div>
          <div class="mc-launch-sub">Chat with the local model</div>
        </button>
        <button class="mc-launch-tile" data-action="launch-terminal">
          <div class="mc-launch-icon">⌨️</div>
          <div class="mc-launch-title">Miracle Claw · Terminal</div>
          <div class="mc-launch-sub">Run mc-terminal in a shell</div>
        </button>
        <button class="mc-launch-tile" data-action="launch-commands">
          <div class="mc-launch-icon">🖥️</div>
          <div class="mc-launch-title">Miracle Claw · Commands</div>
          <div class="mc-launch-sub">One-shot <code>mclaw run …</code> invocations</div>
        </button>
      </div>
    </section>
  `;
}

function renderYourTools(installed) {
  const count = installed.length;
  const tiles = installed.map((m) => {
    const emoji = m.icon || "📦";
    const tools = m.tools || [];
    const firstTool = tools[0]?.name;
    return `
      <div class="mc-tool-tile" data-module-id="${escapeHtml(m.module_id)}">
        <div class="mc-tool-emoji">${emoji}</div>
        <div class="mc-tool-name">${escapeHtml(m.name)}</div>
        <div class="mc-tool-desc">${escapeHtml(m.description || "")}</div>
        ${firstTool
          ? `<button class="mc-tool-run" data-action="run-tool">Run</button>`
          : `<button class="mc-tool-run" disabled>No tools</button>`}
      </div>
    `;
  }).join("");

  const addTile = `
    <button class="mc-tool-tile mc-tool-add" data-action="add-tool">
      <div class="mc-tool-emoji">＋</div>
      <div class="mc-tool-name">Add a tool</div>
      <div class="mc-tool-desc">Browse the marketplace</div>
    </button>
  `;

  return `
    <section class="mc-tools">
      <div class="mc-tools-header">
        <h2>Your Tools</h2>
        <span class="mc-tools-count">${count} installed</span>
        <button class="mc-link" data-action="add-tool">+ Add a tool</button>
      </div>
      <div class="mc-tools-grid">
        ${tiles}
        ${addTile}
      </div>
    </section>
  `;
}

// Week 7.1 (2026-09-10, David): empty state for new users.
//
// When the user has no installed modules, the normal "Your Tools" grid
// is just one "+ Add a tool" tile — not very welcoming. Replace it
// with a curated set of 3-4 starter modules from the marketplace so
// the first-run user has a guided path to "I ran a tool" in 30s.
//
// The curated list (STARTER_MODULE_IDS) is ordered by usefulness for
// the typical first-run user: data analysis (universally applicable),
// marketing copy (saves time for small businesses like David's auto
// repair shop), brand assets (visible output), web research
// (fetch_url is the single most demoable module). Order matters: this
// is the order the user sees them, and it's also the order we try to
// fall back from if a starter is missing.
const STARTER_MODULE_IDS = [
  "data-analyzer",
  "marketing-suite",
  "brand-asset-kit",
  "firecrawl",
];

// Render the empty state. `marketplaceModules` is the (possibly
// empty) list returned by mc_list_marketplace. We pick starters that
// are present in the marketplace, then top up with whatever's at the
// top of the marketplace list, until we have 3-4 cards. Worst case
// (marketplace is unreachable), we render 0 starter tiles plus the
// persistent "+ Browse the marketplace" tile so the user always has a
// way forward.
function renderStarters(marketplaceModules) {
  const byId = new Map(
    (marketplaceModules || []).map((m) => [m.module_id || m.id, m])
  );

  // 1. Curated starters, in order, that exist in the marketplace.
  const curated = STARTER_MODULE_IDS
    .map((id) => byId.get(id))
    .filter(Boolean);

  // 2. Top up with the first non-curated marketplace entries, up to
  //    a total of 4 starters.
  const curatedIds = new Set(curated.map((m) => m.module_id || m.id));
  const topups = (marketplaceModules || [])
    .filter((m) => !curatedIds.has(m.module_id || m.id));
  const starters = [...curated, ...topups].slice(0, 4);

  const tiles = starters.map((m) => {
    const id = m.module_id || m.id;
    const emoji = m.icon || "🧩";
    const name = m.name || m.title || id;
    const desc = m.description || m.summary || "";
    return `
      <div class="mc-tool-tile mc-starter-tile" data-module-id="${escapeHtml(id)}">
        <div class="mc-tool-emoji">${emoji}</div>
        <div class="mc-tool-name">${escapeHtml(name)}</div>
        <div class="mc-tool-desc">${escapeHtml(desc)}</div>
        <button class="mc-tool-run" data-action="install-starter">Install</button>
      </div>
    `;
  }).join("");

  const browseAll = `
    <button class="mc-tool-tile mc-tool-add" data-action="add-tool">
      <div class="mc-tool-emoji">＋</div>
      <div class="mc-tool-name">Browse all</div>
      <div class="mc-tool-desc">See the full marketplace</div>
    </button>
  `;

  return `
    <section class="mc-tools">
      <div class="mc-tools-header">
        <h2>Get started in 30 seconds</h2>
        <span class="mc-tools-count">${tiles ? "" : ""}</span>
        <button class="mc-link" data-action="add-tool">+ Browse the marketplace</button>
      </div>
      <p class="mc-tools-blurb">Pick one of these popular tools. You can change it any time.</p>
      <div class="mc-tools-grid">
        ${tiles}
        ${browseAll}
      </div>
    </section>
  `;
}

function renderRecent(runs) {
  const items = (runs || []).slice(0, 3);
  if (items.length === 0) {
    return `
      <section class="mc-recent">
        <h2>Recent</h2>
        <div class="mc-recent-empty">No invocations yet — install a tool above to get started.</div>
      </section>
    `;
  }
  const rows = items.map((r) => `
    <div class="mc-recent-row">
      <span class="mc-recent-when">${escapeHtml(r.relative_time || r.created_at || "")}</span>
      <span class="mc-recent-name">${escapeHtml(r.module_id || "")}.${escapeHtml(r.tool_name || "")}</span>
      <span class="mc-recent-status" data-status="${escapeHtml(r.status || "ok")}">${escapeHtml(r.status || "ok")}</span>
    </div>
  `).join("");
  return `
    <section class="mc-recent">
      <h2>Recent</h2>
      <div class="mc-recent-list">${rows}</div>
    </section>
  `;
}

// --- modal: module form --------------------------------------------------

function openModuleModal(module) {
  _closeModal();
  const tools = module.tools || [];
  if (tools.length === 0) {
    toast(`${module.name} has no tools to run.`);
    return;
  }
  const firstTool = tools[0];
  // Week 3 (2026-09-09): MAIC's form_schema is the JSON Schema subset
  // (type/required/properties), NOT the simpler {fields:[{name,label,type}]}
  // shape the old code expected. We use the same parsing rules as the
  // shared Rust `form_schema` module (see src-tauri/src/form_schema.rs)
  // so TUI + launchpad + mclaw CLI render the SAME fields.
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
      _closeModal();
    }
  });

  const form = modal.querySelector("form.mc-modal-form");
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();

    // Week 7.2 (2026-09-10, David): pre-submit validation.
    //
    // Before we round-trip to MAIC, run the browser's native
    // constraint check (HTML5 `required`, `min`, `max`,
    // `minlength`, `maxlength`, `pattern`) so the user sees the
    // error inline next to the bad field rather than as a
    // 30-second "MAIC error: ..." toast. `reportValidity()` shows
    // the bubble, focuses the first invalid field, and returns
    // false on failure.
    if (!form.reportValidity()) return;

    // After HTML5 passes, check for type-coercion failures that
    // HTML5 doesn't catch: an empty number field that wasn't
    // `required` (e.g. optional with min=0) where the user typed
    // something non-numeric. We can't do this until after the
    // form's input values are read.
    const fd = new FormData(form);
    const inputs = {};
    for (const [k, v] of fd.entries()) {
      // For checkboxes (Boolean) the absence means false; the form
      // includes a hidden "" value first so we skip those empty
      // entries. For number inputs the string is parsed below.
      if (typeof v === "string" && v === "" && isCheckboxField(form, k)) continue;
      inputs[k] = v;
    }
    // Coerce number fields from string → number so MAIC gets the
    // typed value (the Rust side validates `integer`/`number` strictly).
    for (const f of fields) {
      if ((f.type === "number" || f.type === "integer") && k in inputs) {
        const n = Number(inputs[k]);
        if (!Number.isNaN(n)) inputs[k] = n;
      }
    }
    // Apply defaults for fields the user left blank.
    for (const f of fields) {
      if (!(f.name in inputs) && f.default !== undefined) inputs[f.name] = f.default;
    }
    // Coerce booleans from checkbox presence.
    for (const f of fields) {
      if (f.type === "boolean") {
        inputs[f.name] = !!form.querySelector(`input[name="${cssEscape(f.name)}"]:checked`);
      }
    }

    // Re-check for number fields whose empty string failed the
    // HTML5 check (because they weren't required) but would still
    // produce a NaN when coerced. Treat this as "missing optional
    // number" — the apply-defaults step above already filled in
    // defaults, so any number field that survived to here is
    // either valid (Number.isFinite) or genuinely broken. We
    // surface the latter as a toast.
    const broken = [];
    for (const f of fields) {
      if ((f.type === "number" || f.type === "integer") && inputs[f.name] !== undefined) {
        if (typeof inputs[f.name] === "string" && inputs[f.name].trim() === "") {
          // Empty string on a number field after HTML5 passed;
          // strip the bad value so MAIC gets a clean absence.
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
    try {
      // Week 3 (2026-09-09): wire to the new Rust command. The Rust
      // side does the auth, MAIC POST, and error mapping. Returns
      // { outputs, duration_ms, run_id }.
      const run = await invoke("mc_run_module", {
        moduleId: module.module_id,
        tool: firstTool.name,
        inputs,
        surface: "tauri",
      });
      // `outputs` is a model-generated string; the model may have
      // wrapped it in markdown ```json fences. Strip those for the
      // preview, but keep the original as a copy-to-clipboard action.
      const out = String(run.outputs ?? "");
      const stripped = out.replace(/^```(?:json)?\n/, "").replace(/\n```$/, "").trim();
      result.textContent = stripped || out || "(no output)";
      result.dataset.rawOutput = out;
    } catch (err) {
      // Week 7.3: friendly error mapping. Show a short headline
      // with the raw technical detail collapsed below.
      const result = modal.querySelector(".mc-modal-result");
      const mapped = mapMclawError(err);
      result.hidden = false;
      result.classList.add("mc-modal-result-error");
      result.textContent = mapped.headline;
      result.dataset.rawError = String(err?.message || err);
      // Render the detail in a `<details>` block if present.
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
      // Wire the action button if present.
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

// ─────────────────────────────────────────────────────────────────────
// Week 3 (2026-09-09): form_schema → UI fields.
// Mirrors src-tauri/src/form_schema.rs::parse_form_schema. Each surface
// (launchpad, TUI, mclaw) calls its own renderer against the same
// parsed structure. If you change this, change the Rust parser too.

// Parse a MAIC form_schema (JSON Schema subset) into a list of fields
// the launchpad's HTML renderer knows how to draw.
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
      name,
      label,
      type,
      required: isRequired,
      default: prop.default,
      displayOrder: order++,
    };
    if (type === "select" && Array.isArray(prop.enum)) field.options = prop.enum;
    if (type === "number" || type === "integer") {
      if (prop.minimum !== undefined) field.min = prop.minimum;
      if (prop.maximum !== undefined) field.max = prop.maximum;
      if (prop.minLength !== undefined) field.minLength = prop.minLength;
      if (prop.maxLength !== undefined) field.maxLength = prop.maxLength;
    }
    if (type === "text" || type === "textarea") {
      if (prop.minLength !== undefined) field.minLength = prop.minLength;
      if (prop.maxLength !== undefined) field.maxLength = prop.maxLength;
    }
    out.push(field);
  }
  return out;
}

// Map a JSON Schema property type → launchpad UI type tag. The Rust
// `FieldKind::type_tag` uses the same strings so JS and Rust agree.
function propJsonSchemaTypeToUiType(prop) {
  if (prop.type === "boolean") return "boolean";
  if (prop.type === "integer" || prop.type === "number") return prop.type;
  if (prop.type === "string") {
    if (Array.isArray(prop.enum) && prop.enum.length > 0) return "select";
    if (prop.format === "long-text" || prop.format === "textarea") return "textarea";
    return "text";
  }
  return "text"; // fallback: treat unknown as plain text
}

// Render one parsed field as an HTML label + input pair. Mirrors the
// snippet in openModuleModal but extracted for testability.
function renderField(f) {
  const id = `f-${f.name}`;
  const requiredMark = f.required ? " *" : "";
  const label = `<label for="${id}">${escapeHtml(f.label)}${requiredMark}</label>`;
  let input;
  if (f.type === "textarea") {
    input = `<textarea id="${id}" name="${cssEscape(f.name)}"${f.required ? " required" : ""}${f.minLength !== undefined ? ` minlength="${f.minLength}"` : ""}${f.maxLength !== undefined ? ` maxlength="${f.maxLength}"` : ""}>${escapeHtml(f.default ?? "")}</textarea>`;
  } else if (f.type === "select") {
    const opts = (f.options || []).map((o) =>
      `<option value="${escapeHtml(String(o))}"${o === f.default ? " selected" : ""}>${escapeHtml(String(o))}</option>`).join("");
    input = `<select id="${id}" name="${cssEscape(f.name)}"${f.required ? " required" : ""}>${opts}</select>`;
  } else if (f.type === "number" || f.type === "integer") {
    const step = f.type === "integer" ? "1" : "any";
    const minAttr = f.min !== undefined ? ` min="${f.min}"` : "";
    const maxAttr = f.max !== undefined ? ` max="${f.max}"` : "";
    const def = f.default !== undefined ? ` value="${escapeHtml(String(f.default))}"` : "";
    input = `<input id="${id}" name="${cssEscape(f.name)}" type="number" step="${step}"${minAttr}${maxAttr}${f.required ? " required" : ""}${def} />`;
  } else if (f.type === "boolean") {
    const checked = f.default === true ? " checked" : "";
    // Checkbox: the browser only submits when checked, so we also
    // include a hidden "" before it so the FormData always has the
    // key (the submit handler detects "blank = unchecked").
    input = `<input type="hidden" name="${cssEscape(f.name)}" value="" /><label class="mc-checkbox-row"><input id="${id}" name="${cssEscape(f.name)}" type="checkbox" value="true"${checked} /> ${escapeHtml(f.label)}</label>`;
    return `<div class="mc-field mc-field-bool">${input}</div>`;
  } else {
    // text (default)
    const def = f.default !== undefined ? ` value="${escapeHtml(String(f.default))}"` : "";
    const minAttr = f.minLength !== undefined ? ` minlength="${f.minLength}"` : "";
    const maxAttr = f.maxLength !== undefined ? ` maxlength="${f.maxLength}"` : "";
    input = `<input id="${id}" name="${cssEscape(f.name)}" type="text"${minAttr}${maxAttr}${f.required ? " required" : ""}${def} />`;
  }
  return `<div class="mc-field">${label}${input}</div>`;
}

// Detect whether a field is a checkbox (the form has a hidden "" before
// the real checkbox; if we see two inputs with the same name, the
// second is the checkbox and absence means unchecked).
function isCheckboxField(form, name) {
  const els = form.querySelectorAll(`input[name="${cssEscape(name)}"]`);
  return els.length >= 2 && els[1].type === "checkbox";
}

// Minimal CSS.escape polyfill for older Tauri webviews.
function cssEscape(s) {
  if (typeof CSS !== "undefined" && CSS.escape) return CSS.escape(s);
  return String(s).replace(/[^a-zA-Z0-9_-]/g, (c) => `\\${c}`);
}

// --- mount ---------------------------------------------------------------

async function mount(root, ctx) {
  _abortController = new AbortController();
  const signal = _abortController.signal;
  _currentRoot = root;
  _currentCtx = ctx;

  // Initial shell — render before we know the data, so the user sees
  // something immediately.
  root.innerHTML = `
    <div class="mc-launchpad">
      <div class="mc-loading">Loading your dashboard…</div>
    </div>
  `;

  // Fetch in parallel. Use settled so one failure doesn't break the rest.
  //
  // Week 7.1: when installed is empty we ALSO fetch a slice of the
  // marketplace (no category filter) so the empty state can show
  // curated starters. This is one extra round-trip on the first
  // launchpad visit only; subsequent visits with installed modules
  // skip the marketplace fetch entirely.
  const marketplacePromise = invoke("mc_list_marketplace", { category: null })
    .catch(() => ({ modules: [] }));
  const [tierRes, quotaRes, installedRes, userRes, marketplaceRes] = await Promise.allSettled([
    invoke("mc_refresh_tier").catch(() => null),
    invoke("mc_get_quota").catch(() => null),
    invoke("mc_list_installed_modules").catch(() => ({ modules: [], grouped_by_category: {} })),
    invoke("mc_get_user_info").catch(() => null),
    marketplacePromise,
  ]);

  if (signal.aborted) return;

  const tier = tierRes.status === "fulfilled" ? (tierRes.value?.tier || "free") : "free";
  const quota = quotaRes.status === "fulfilled" ? quotaRes.value : null;
  const installed = installedRes.status === "fulfilled"
    ? (installedRes.value?.modules || [])
    : [];
  const marketplace = marketplaceRes.status === "fulfilled"
    ? (Array.isArray(marketplaceRes.value) ? marketplaceRes.value : (marketplaceRes.value?.modules || []))
    : [];
  const user = userRes.status === "fulfilled" ? userRes.value : null;
  const displayName = user?.name || user?.email || ctx?.user?.name || "";

  // Recent invocations: not wired in Week 2 yet (needs module_runs table
  // populated by mclaw CLI in Week 5). Show empty state for now.
  const recent = [];

  // Week 7.1: choose between the normal "Your Tools" grid and the
  // curated empty state based on whether the user has any installed
  // modules. The empty state also depends on the marketplace list, so
  // it's a function of BOTH the installed count and what we got back
  // from mc_list_marketplace.
  const toolsSection = installed.length === 0
    ? renderStarters(marketplace)
    : renderYourTools(installed);

  root.innerHTML = `
    <div class="mc-launchpad">
      ${renderTopbar(displayName)}
      ${renderTierRow(tier, quota)}
      <main class="mc-container">
        ${renderGreeting(displayName)}
        ${renderLaunchTiles()}
        ${toolsSection}
        ${renderRecent(recent)}
      </main>
    </div>
  `;

  // Wire interactions.
  root.addEventListener("click", _onClick);
}

function _onClick(ev) {
  const target = ev.target.closest("[data-action]");
  if (!target) return;
  const action = target.dataset.action;
  switch (action) {
    case "settings":
      navigate("settings");
      break;
    case "user-menu":
      // Lightweight stub — could expand to a popover later. For now, just
      // surface a hint that account menu is forthcoming.
      toast("Account menu — coming soon.");
      break;
    case "open-plans":
      // rc55.25: plans page is registered as "pricing". Once the bucket 1
      // rename lands (mc-launchpad-A), we add an alias or rename.
      navigate("pricing");
      break;
    case "launch-chat":
      // rc55.25: chat lives inside the main dashboard view. A dedicated
      // "chat" page is on the Week 3-4 plan; for now route to dashboard
      // which renders the chat tab.
      navigate("dashboard");
      break;
    case "launch-terminal":
      navigate("terminal");
      break;
    case "launch-commands":
      // For Week 2, route to extras hub; replace with `mclaw run` UI in Week 5.
      navigate("extras");
      break;
    case "add-tool":
      // Marketplace is the live MAIC catalog (rc56.x). Backed by 5 mc_*
      // commands in src-tauri/src/lib.rs; UI lives at src/pages/marketplace.js.
      navigate("marketplace");
      break;
    case "run-tool": {
      const installedCard = target.closest("[data-module-id]");
      if (!installedCard) return;
      const moduleIdFromCard = installedCard.dataset.moduleId;
      // We need the full client_manifest to render the form. Re-fetch is
      // wasteful — but for Week 2 we keep it simple by re-listing installed.
      invoke("mc_list_installed_modules").then((res) => {
        const mod = (res.modules || []).find((m) => m.module_id === moduleIdFromCard);
        if (mod) openModuleModal(mod);
      }).catch((err) => toast(`Failed to load module: ${err}`));
      break;
    }
    case "install-starter": {
      // Week 7.1: empty-state "Install" button. Same Rust command as
      // the marketplace's install button, but instead of navigating
      // away we re-mount the page so the empty state is replaced
      // with the normal "Your Tools" grid (and the just-installed
      // module shows up).
      const card = target.closest("[data-module-id]");
      if (!card) return;
      const moduleId = card.dataset.moduleId;
      const button = card.querySelector("button");
      if (button) {
        button.disabled = true;
        button.textContent = "Installing…";
      }
      invoke("mc_install_module", { moduleId })
        .then(() => {
          toast(`Installed ${moduleId}.`);
          // Re-mount the page. mount() handles its own abort via
          // _abortController, so we can safely call it again — the
          // previous AbortController is replaced.
          if (_currentRoot && _currentCtx) {
            // Clear the existing click listener before re-rendering.
            _currentRoot.removeEventListener("click", _onClick);
            // Re-mount (sets a fresh AbortController, re-fetches data,
            // re-renders the shell).
            mount(_currentRoot, _currentCtx);
          }
        })
        .catch((err) => {
          toast(`Install failed: ${err}`);
          if (button) {
            button.disabled = false;
            button.textContent = "Install";
          }
        });
      break;
    }
    default:
      break;
  }
}

export const dashboardLaunchpadPage = { mount, unmount, requiresAuth: true };

// Test-only namespace. Mirrors the workflow page pattern (Lesson 833):
// expose pure helpers so node:test can drive them without needing a
// DOM. Not part of the public page surface.
export const _formSchemaInternals = {
  formFieldsFromSchema,
  propJsonSchemaTypeToUiType,
  renderField,
  // Week 7.3: error mapper.
  mapMclawError,
};
