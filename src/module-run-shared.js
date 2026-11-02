// ─────────────────────────────────────────────────────────────────────────────
// Shared module-run helpers.
//
// Lesson 1146 (2026-11-02, David): the module-run logic was copy-pasted
// across three surfaces and the copies had DIVERGED:
//
//   A. OpenClaw chat webview  -- depot/openclaw-patches/dist/control-ui/mc-toolbar.js
//      (self-contained patch; cannot import from src/, so it keeps its own
//      copy. Its embedded help block is GENERATED from src/data/ by
//      scripts/sync-toolbar-module-help.mjs so it can't drift in content.)
//
//   B. MC Terminal page       -- src/modules-menu.js
//
//   C. MC Dashboard launchpad -- src/pages/dashboard_launchpad.js
//
// The divergence was real, not theoretical:
//
//   - formFieldsFromSchema existed in TWO copies with DIFFERENT field
//     shapes (B emitted `order`/`enum`/`minimum`; C emitted
//     `displayOrder`/`options`/`min`) and DIFFERENT type tags
//     (B: string/number/array/object; C: text/select/textarea/number).
//     A `format: "long-text"` string became a plain one-line text input
//     in the Terminal, so long URLs / notes got a cramped input there.
//
//   - The error mapper only handled 401/429/network. A 404 "Unknown
//     module/tool", a 422 validation rejection, and a 501 COMING_SOON
//     (calendar-companion.find_slot) all leaked raw ureq text.
//
//   - Only the OpenClaw toolbar had a tool PICKER. The Terminal
//     hardcoded `tools[0]`, so of Calendar's 15 tools the Terminal could
//     only ever reach `search_events`.
//
//   - Only the Modules settings page read the canonical help catalog
//     (src/data/module-help.js). The Terminal and launchpad had no
//     module-level Help affordance at all.
//
// This module is the single source of truth for B and C. It deliberately
// does NOT cover A (the OpenClaw patch is self-contained by design).
//
// If you change anything here, mirror the change into the openclaw patch
// and keep the generated help block in sync:
//   node scripts/sync-toolbar-module-help.mjs
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────
// JSON Schema -> UI fields
// ─────────────────────────────────────────────────────────────────────
//
// Mirrors src-tauri/src/form_schema.rs::parse_form_schema. MAIC's
// form_schema is a JSON Schema subset (type/required/properties), NOT
// the old simpler {fields:[{name,label,type}]} shape.
//
// The field shape returned here is the UNION of what the two old copies
// produced, so neither renderer loses information:
//
//   name, label, type, required, order, displayOrder,   // identity
//   description, help, placeholder, default,             // text
//   minimum, maximum, min, max, minLength, maxLength,    // numeric/length
//   enum, options,                                      // choice lists
//
// `type` uses the launchpad's richer tag set (text/select/textarea/
// number/integer/boolean) because it distinguishes cases the Terminal
// previously collapsed.

export function propJsonSchemaTypeToUiType(prop) {
  if (!prop || typeof prop !== "object") return "text";
  const raw = prop.type;
  if (Array.isArray(raw)) {
    if (raw.includes("boolean")) return "boolean";
    if (raw.includes("integer") || raw.includes("number")) return "number";
    if (raw.includes("string")) return "text";
  }
  if (raw === "boolean") return "boolean";
  if (raw === "integer") return "integer";
  if (raw === "number") return "number";
  if (raw === "string") {
    if (Array.isArray(prop.enum) && prop.enum.length > 0) return "select";
    if (prop.format === "long-text" || prop.format === "textarea") return "textarea";
    return "text";
  }
  if (raw === "array" || raw === "object") return "text";
  return "text"; // fallback: treat unknown as plain text
}

/**
 * Parse a MAIC form_schema (JSON Schema subset) into a list of fields
 * the HTML renderers know how to draw.
 */
export function formFieldsFromSchema(schema) {
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
      // `order` and `displayOrder` are both emitted so the old renderers
      // that read either one keep working.
      order: order,
      displayOrder: order++,
      description: prop.description || "",
      default: prop.default,
      minimum: typeof prop.minimum === "number" ? prop.minimum : null,
      maximum: typeof prop.maximum === "number" ? prop.maximum : null,
      min: typeof prop.minimum === "number" ? prop.minimum : undefined,
      max: typeof prop.maximum === "number" ? prop.maximum : undefined,
      minLength: typeof prop.minLength === "number" ? prop.minLength : undefined,
      maxLength: typeof prop.maxLength === "number" ? prop.maxLength : undefined,
      pattern: typeof prop.pattern === "string" ? prop.pattern : null,
    };
    if (Array.isArray(prop.enum) && prop.enum.length > 0) {
      field.enum = prop.enum;
      field.options = prop.enum;
    }
    // Per-field help text, capped so a runaway schema can't blow up the
    // form.
    if (typeof prop.description === "string" && prop.description.trim().length > 0) {
      field.help = prop.description.trim().slice(0, 240);
    }
    if (typeof prop.placeholder === "string" && prop.placeholder.trim().length > 0) {
      field.placeholder = prop.placeholder.trim().slice(0, 120);
    }
    out.push(field);
  }
  return out;
}

// ─────────────────────────────────────────────────────────────────────
// Error mapping
// ─────────────────────────────────────────────────────────────────────

/**
 * Turn a raw run failure into a user-facing headline, the server's own
 * detail line, and an optional suggested action.
 *
 * Returns `{ headline, detail, action, status, serverDetail, raw }`
 * where `action` is `{ label, callback }` or null.
 *
 * Covers the codes the MAIC module runner actually returns:
 *   400 bad request        401 expired session
 *   403 forbidden          404 unknown module/tool
 *   409 already installed  422 validation
 *   429 rate limited       501 coming soon
 *   5xx server error       network / timeout
 *
 * Lesson 1146: three per-surface copies of this function had drifted.
 * The launchpad's was the most complete (it had the `HTTP <code>:
 * <body>` shape and action buttons) but was missing 501; the
 * Terminal's knew only 401/429/network. This is the merged version --
 * it keeps the launchpad's exact contract (its existing tests pin the
 * wording) and adds the codes the others were missing.
 *
 * `opts.actions` lets a caller inject navigation callbacks:
 *   signIn  -- offered on 401
 *   install -- offered on 404
 */
export function mapMclawError(err, opts = {}) {
  const raw = String((err && err.message) || err || "");
  const actions = opts.actions || {};

  // Parse the Rust-side `HTTP <code>: <body>` shape.
  const m = raw.match(/^HTTP\s+(\d{3}):\s*(.*)$/s);
  if (!m) {
    // Non-HTTP error (network, timeout, etc.).
    //
    // The launchpad's original contract returned the raw message as
    // the headline with `detail: null`. Keep that, but add a friendlier
    // headline for the network cases the Terminal used to catch
    // (its mapper had a dedicated network branch).
    const network = /network|fetch|timeout|econnrefused|enotfound|error sending request/i.test(raw);
    return {
      headline: network
        ? "Could not reach the MAIC server. Check your connection and retry."
        : raw || "Something went wrong.",
      detail: null,
      action: null,
      status: null,
      serverDetail: null,
      raw,
    };
  }

  const status = parseInt(m[1], 10);
  const body = m[2].trim();

  // Try to extract the MAIC `detail` field for the detail line.
  let detailText = body;
  let serverDetail = null;
  try {
    const parsed = JSON.parse(body);
    if (parsed && typeof parsed.detail === "string") {
      detailText = parsed.detail;
      serverDetail = parsed.detail;
    }
  } catch {
    // body wasn't JSON; use it as-is.
  }

  const mk = (headline, action = null) => ({
    headline,
    detail: detailText,
    action,
    status,
    serverDetail,
    raw,
  });

  switch (status) {
    case 400:
      return mk("The module rejected that request. Check the values and try again.");
    case 401:
      return mk(
        "Your session expired. Please sign in again.",
        actions.signIn || { label: "Sign in", callback: null }
      );
    case 403:
      return mk("You don't have access to this module.");
    case 404:
      return mk(
        "This module isn't installed. Try installing it again.",
        actions.install || { label: "Install", callback: null }
      );
    case 409:
      return mk("This module is already installed.");
    case 422:
      return mk("Invalid input. Check the form fields and try again.");
    case 429:
      return mk("Too many requests. Wait a moment and try again.");
    case 501:
      // Lesson 1146: the launchpad's old mapper had no 501 case, so
      // `calendar-companion.find_slot` -- a deliberate COMING_SOON --
      // rendered as the flat "MAIC returned 501." Explain it instead.
      return mk(
        "That tool is not available yet -- it is in private beta. The waitlist " +
          "link is in the details below."
      );
    case 502:
    case 503:
    case 504:
      return mk("MAIC is temporarily unavailable. Try again in a moment.");
    default:
      if (status >= 500) {
        return mk("MAIC is temporarily unavailable. Try again in a moment.");
      }
      return mk(`MAIC returned ${status}.`);
  }
}

/** Pull the HTTP status code out of an ureq-style error string. */
export function extractHttpStatus(msg) {
  const m = String(msg || "").match(/\bHTTP\s+(\d{3})\b/i);
  if (m) return parseInt(m[1], 10);
  const m2 = String(msg || "").match(/status(?:_code)?\D{0,3}(\d{3})\b/i);
  return m2 ? parseInt(m2[1], 10) : null;
}

/**
 * Pull the server's JSON `detail` field out of an ureq-style error, so
 * the headline can name the offending field ("q is required").
 */
export function extractServerDetail(msg) {
  const s = String(msg || "");
  const m = s.match(/\{.*\}/s);
  if (!m) return null;
  try {
    const parsed = JSON.parse(m[0]);
    if (parsed && typeof parsed.detail === "string") return parsed.detail;
    if (parsed && typeof parsed.message === "string") return parsed.message;
  } catch {
    /* not JSON -- fall through */
  }
  return null;
}

// ─────────────────────────────────────────────────────────────────────
// Run helpers
// ─────────────────────────────────────────────────────────────────────

/**
 * Coerce raw FormData values into the shape MAIC expects.
 *
 * - empty checkbox values are dropped (the hidden "" input)
 * - number/integer fields become JS numbers
 * - missing fields fall back to the schema default
 * - booleans become real booleans
 *
 * Returns `{ inputs, broken }` where `broken` lists fields whose value
 * could not be coerced (so the caller can show a friendly message
 * instead of sending garbage to the server).
 */
export function collectRunInputs(form, fields) {
  const fd = new FormData(form);
  const inputs = {};
  for (const [k, v] of fd.entries()) {
    if (typeof v === "string" && v === "" && isCheckboxField(form, k)) continue;
    inputs[k] = v;
  }
  // Coerce number fields from string -> number so MAIC gets the typed
  // value (the Rust side validates `integer`/`number` strictly).
  //
  // Lesson 1146: skip EMPTY optional numbers. `Number("")` is 0, so a
  // blank optional integer used to be sent as `0` rather than being
  // omitted -- e.g. Calendar's `list_events.calendar_id` came back as
  // `calendar_id: 0` when the user left it blank, which MAIC reads as
  // "calendar id zero" rather than "no filter". A blank number field
  // is an ABSENCE, not a zero.
  for (const f of fields) {
    if ((f.type === "number" || f.type === "integer") && f.name in inputs) {
      const raw = inputs[f.name];
      if (typeof raw === "string" && raw.trim() === "") {
        delete inputs[f.name];
        continue;
      }
      const n = Number(raw);
      if (!Number.isNaN(n)) inputs[f.name] = n;
    }
  }
  for (const f of fields) {
    if (!(f.name in inputs) && f.default !== undefined) inputs[f.name] = f.default;
  }
  for (const f of fields) {
    if (f.type === "boolean") {
      inputs[f.name] = !!form.querySelector(
        `input[type="checkbox"][name="${cssEscape(f.name)}"]:checked`
      );
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
  return { inputs, broken };
}

export function isCheckboxField(form, name) {
  const el = form.querySelector(`input[type="checkbox"][name="${cssEscape(name)}"]`);
  return !!el;
}

/**
 * Minimal CSS.escape polyfill. Chromium/WebView2 have the real thing,
 * but the test harness (jsdom) may not, and field names can contain
 * characters that break a naive querySelector.
 */
export function cssEscape(s) {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") return CSS.escape(s);
  return String(s).replace(/[^a-zA-Z0-9_-]/g, (c) => "\\" + c);
}

/**
 * The list of runnable tools on a module, from the live client_manifest.
 * Accepts both the raw MAIC shape and the normalized local shape.
 */
export function moduleTools(module) {
  if (!module) return [];
  const cm = module.client_manifest || (module._raw && module._raw.client_manifest) || {};
  if (Array.isArray(cm.tools) && cm.tools.length) return cm.tools;
  if (Array.isArray(module.tools) && module.tools.length) return module.tools;
  if (Array.isArray(module.commands) && module.commands.length) return module.commands;
  return [];
}

/**
 * Pick the tool to open. An explicit `toolName` wins; otherwise fall
 * back to the first tool (legacy callers).
 *
 * Lesson 1146: the Terminal used to hardcode `tools[0]` with no way to
 * choose, so of Calendar's 15 tools only `search_events` was reachable.
 */
export function selectTool(module, toolName) {
  const tools = moduleTools(module);
  if (!tools.length) return null;
  if (toolName) {
    const found = tools.find((t) => t && (t.name === toolName || t.id === toolName));
    if (found) return found;
  }
  return tools[0];
}

/**
 * Strip markdown ``` fences from a model's output.
 */
export function stripFences(out) {
  const s = String(out ?? "");
  const m = s.match(/^\s*```(?:json|javascript|js|text)?\s*\n([\s\S]*?)\n?\s*```\s*$/);
  return m ? m[1] : s;
}
