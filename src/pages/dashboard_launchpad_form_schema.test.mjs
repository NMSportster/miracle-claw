// Week 3 (2026-09-09): regression test for the launchpad's form_schema
// parser. Mirrors src-tauri/src/form_schema.rs (Lesson 885: shared
// parser across launchpad, TUI, mclaw CLI).
//
// Before the refactor (rc55.25): the launchpad expected the simpler
// `form_schema.fields[].{name,label,type,required,options}` shape that
// the in-house old dev used. MAIC actually emits the JSON Schema subset
// (type/required/properties). The modal would silently render
// "No inputs needed" for every installed module.
//
// After the refactor: the launchpad walks `properties` and renders the
// right inputs. This test pins that behavior.
//
// Run via: `npm run test:js` (added to package.json scripts).

import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { _formSchemaInternals } from "./dashboard_launchpad.js";

const { formFieldsFromSchema, propJsonSchemaTypeToUiType, renderField } =
  _formSchemaInternals;

test("parses a real firecrawl schema (URL field)", () => {
  // Exact shape from MAIC's first-party modules (see
  // docs/maic-migrations/035_client_manifest.sql).
  const schema = {
    type: "object",
    required: ["url"],
    properties: {
      url: { type: "string", title: "URL", minLength: 1 },
    },
  };
  const fields = formFieldsFromSchema(schema);
  assert.equal(fields.length, 1);
  const f = fields[0];
  assert.equal(f.name, "url");
  assert.equal(f.label, "URL");
  assert.equal(f.required, true);
  assert.equal(f.type, "text");
  assert.equal(f.minLength, 1);
  assert.equal(f.displayOrder, 0);
});

test("parses integer field with default and bounds", () => {
  const schema = {
    type: "object",
    properties: {
      max_pages: {
        type: "integer",
        title: "Max pages",
        default: 10,
        minimum: 1,
        maximum: 100,
      },
    },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.name, "max_pages");
  assert.equal(f.type, "integer");
  assert.equal(f.default, 10);
  assert.equal(f.min, 1);
  assert.equal(f.max, 100);
  assert.equal(f.required, false);
});

test("parses string+enum as select", () => {
  const schema = {
    type: "object",
    properties: {
      tone: {
        type: "string",
        title: "Tone",
        enum: ["friendly", "professional", "urgent", "playful"],
      },
    },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.type, "select");
  assert.deepEqual(f.options, ["friendly", "professional", "urgent", "playful"]);
});

test("preserves property declaration order", () => {
  // Tab navigation in the TUI + index 0/1/2 in the form depend on
  // declaration order being preserved. JSON object iteration order is
  // guaranteed by ES2015+ for string keys.
  const schema = {
    type: "object",
    properties: {
      first: { type: "string" },
      second: { type: "string" },
      third: { type: "string" },
    },
  };
  const fields = formFieldsFromSchema(schema);
  assert.deepEqual(
    fields.map((f) => f.name),
    ["first", "second", "third"]
  );
  assert.deepEqual(
    fields.map((f) => f.displayOrder),
    [0, 1, 2]
  );
});

test("returns empty list for missing/invalid schema (no crash)", () => {
  assert.deepEqual(formFieldsFromSchema(null), []);
  assert.deepEqual(formFieldsFromSchema(undefined), []);
  assert.deepEqual(formFieldsFromSchema({}), []);
  assert.deepEqual(formFieldsFromSchema({ type: "array" }), []);
  assert.deepEqual(formFieldsFromSchema({ type: "object" }), []); // no properties
});

test("falls back to property name when title is missing", () => {
  const schema = {
    type: "object",
    properties: { url: { type: "string", minLength: 1 } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.label, "url"); // not undefined, not "URL"
});

test("boolean field becomes a checkbox render", () => {
  const schema = {
    type: "object",
    properties: { active: { type: "boolean", title: "Active" } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.type, "boolean");
  const html = renderField(f);
  // The renderField for boolean is wrapped in mc-field-bool and
  // includes a hidden "" input so the form has the key on submit.
  assert.match(html, /mc-field-bool/);
  assert.match(html, /type="hidden"/);
  assert.match(html, /type="checkbox"/);
});

test("string with format long-text becomes textarea", () => {
  const schema = {
    type: "object",
    properties: { bio: { type: "string", title: "Bio", format: "long-text" } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.type, "textarea");
  const html = renderField(f);
  assert.match(html, /<textarea/);
});

test("type mapping covers all variants we use in MAIC", () => {
  assert.equal(propJsonSchemaTypeToUiType({ type: "string" }), "text");
  assert.equal(
    propJsonSchemaTypeToUiType({ type: "string", enum: ["a"] }),
    "select"
  );
  assert.equal(
    propJsonSchemaTypeToUiType({ type: "string", format: "long-text" }),
    "textarea"
  );
  assert.equal(propJsonSchemaTypeToUiType({ type: "integer" }), "integer");
  assert.equal(propJsonSchemaTypeToUiType({ type: "number" }), "number");
  assert.equal(propJsonSchemaTypeToUiType({ type: "boolean" }), "boolean");
  // Unknown type → text fallback (defensive)
  assert.equal(propJsonSchemaTypeToUiType({ type: "array" }), "text");
});

test("required flag mirrors the required array", () => {
  const schema = {
    type: "object",
    required: ["url", "max_pages"],
    properties: {
      url: { type: "string" },
      max_pages: { type: "integer" },
      optional: { type: "string" },
    },
  };
  const fields = formFieldsFromSchema(schema);
  const byName = Object.fromEntries(fields.map((f) => [f.name, f.required]));
  assert.equal(byName.url, true);
  assert.equal(byName.max_pages, true);
  assert.equal(byName.optional, false);
});

test("rendered HTML includes required attribute for required fields", () => {
  const schema = {
    type: "object",
    required: ["url"],
    properties: {
      url: { type: "string", title: "URL", minLength: 1 },
    },
  };
  const [f] = formFieldsFromSchema(schema);
  const html = renderField(f);
  assert.match(html, /required/);
  assert.match(html, /minlength="1"/);
});

// =========================================================================
// Week 7.1 (2026-09-10): empty state for new users.
//
// `renderStarters(marketplaceModules)` returns the "Get started in 30
// seconds" HTML when the user has no installed modules. It picks
// curated starters from the marketplace, tops up with top-of-list
// modules, caps at 4, and always renders a "Browse all" tile at the
// end. The exact set of installed modules is irrelevant — that's
// handled by the caller (mount) which calls renderStarters only when
// installed.length === 0.
// =========================================================================

// We need to access renderStarters, but it's a private helper. Test
// the curated list (which is module-scoped) by reading the source
// file. The actual renderStarters() function is verified manually in
// the live E2E smoke test (Lesson 198 pattern: cheap assertions
// inline, expensive DOM assertions via the integration test).
//
// Future cleanup: expose _emptyStateInternals = { renderStarters, STARTER_MODULE_IDS }
// if this test surface grows beyond constants.

function readStarterIds() {
  const src = readFileSync(
    new URL("./dashboard_launchpad.js", import.meta.url),
    "utf8"
  );
  const m = src.match(/const STARTER_MODULE_IDS = \[([\s\S]*?)\];/);
  if (!m) throw new Error("STARTER_MODULE_IDS must exist in dashboard_launchpad.js");
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

test("STARTER_MODULE_IDS are in the curated order we promised", () => {
  // Sanity check: the order of the curated list is the order the
  // user sees them. We documented this in the source comment, so
  // a future maintainer who reorders should also update the test.
  const ids = readStarterIds();
  assert.deepEqual(ids, [
    "data-analyzer",
    "marketing-suite",
    "brand-asset-kit",
    "firecrawl",
  ], "curated starter order must match docs/mc-module-driven-surfaces-2026-09.md §5.1 + Week 7.1 retro");
});

test("STARTER_MODULE_IDS has 4 entries (UX budget for the empty state)", () => {
  // We cap at 4 because the launchpad grid is minmax(180px, 1fr) and
  // 5+ tiles wrap to a second row, which the empty state isn't
  // designed for. If we add more, the layout breaks; update the grid
  // CSS at the same time.
  const ids = readStarterIds();
  assert.equal(ids.length, 4, "UX budget: 4 starters, not 3 (too few) or 5 (wraps)");
});

// =========================================================================
// Week 7.2 (2026-09-10): form validation.
//
// The launchpad modal uses `form.reportValidity()` (HTML5 native) for
// required / min / max / minlength / maxlength. After that passes we
// also catch number-coercion failures that HTML5 doesn't surface:
// an optional number field where the user typed "abc". This is hard
// to unit-test without a JSDOM, so we just verify the rendered HTML
// carries the right HTML5 attrs (so reportValidity() will work in the
// browser).
// =========================================================================

test("rendered number field carries min/max/required/step", () => {
  const f = {
    name: "max_pages",
    label: "Max pages",
    type: "integer",
    required: true,
    min: 1,
    max: 100,
    default: 10,
  };
  const html = renderField(f);
  assert.match(html, /type="number"/, "number input");
  assert.match(html, /step="1"/, "integer step");
  assert.match(html, /min="1"/, "min attr");
  assert.match(html, /max="100"/, "max attr");
  assert.match(html, /required/, "required attr");
  assert.match(html, /value="10"/, "default value");
});

test("rendered text field carries minlength/maxlength when present", () => {
  const f = {
    name: "api_key",
    label: "API Key",
    type: "text",
    required: true,
    minLength: 8,
    maxLength: 64,
  };
  const html = renderField(f);
  assert.match(html, /type="text"/);
  assert.match(html, /minlength="8"/);
  assert.match(html, /maxlength="64"/);
  assert.match(html, /required/);
});

test("rendered text field omits minlength/maxlength when not set", () => {
  const f = {
    name: "url",
    label: "URL",
    type: "text",
    required: true,
  };
  const html = renderField(f);
  assert.doesNotMatch(html, /minlength/);
  assert.doesNotMatch(html, /maxlength/);
});

test("rendered select field marks required and includes options", () => {
  const f = {
    name: "format",
    label: "Format",
    type: "select",
    required: true,
    options: ["json", "csv", "markdown"],
    default: "json",
  };
  const html = renderField(f);
  assert.match(html, /<select/);
  assert.match(html, /required/);
  assert.match(html, /<option value="json"/);
  assert.match(html, /<option value="csv"/);
  assert.match(html, /<option value="markdown"/);
  assert.match(html, /<option value="json" selected/, "default is selected");
});

// =========================================================================
// rc57.11 (2026-10-07, David): per-field help text + placeholder.
//
// The form_schema `description` field becomes a `mc-field-help` <p>
// under the input. `placeholder` becomes a real HTML placeholder attr
// on text/textarea inputs. Both are trimmed and capped so a runaway
// schema can't blow up the form.
// =========================================================================

test("formFieldsFromSchema picks up description as help", () => {
  const schema = {
    type: "object",
    properties: {
      goal: {
        type: "string",
        title: "Goal",
        description: "What you want to accomplish.",
      },
    },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.help, "What you want to accomplish.");
});

test("formFieldsFromSchema picks up placeholder", () => {
  const schema = {
    type: "object",
    properties: {
      goal: {
        type: "string",
        title: "Goal",
        placeholder: "win back inactive customers",
      },
    },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.placeholder, "win back inactive customers");
});

test("formFieldsFromSchema trims and caps long descriptions", () => {
  const long = "x".repeat(500);
  const schema = {
    type: "object",
    properties: { goal: { type: "string", title: "Goal", description: long } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.help.length, 240, "help text is capped at 240 chars");
});

test("formFieldsFromSchema trims and caps long placeholders", () => {
  const long = "x".repeat(300);
  const schema = {
    type: "object",
    properties: { goal: { type: "string", title: "Goal", placeholder: long } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.placeholder.length, 120, "placeholder is capped at 120 chars");
});

test("formFieldsFromSchema omits help when description missing", () => {
  const schema = {
    type: "object",
    properties: { goal: { type: "string", title: "Goal" } },
  };
  const [f] = formFieldsFromSchema(schema);
  assert.equal(f.help, undefined);
});

test("rendered text field includes help paragraph when help is set", () => {
  const f = {
    name: "goal",
    label: "Goal",
    type: "text",
    required: true,
    help: "What you want to accomplish. Be specific.",
  };
  const html = renderField(f);
  assert.match(html, /class="mc-field-help"/);
  assert.match(html, /What you want to accomplish/);
});

test("rendered text field omits help paragraph when help is unset", () => {
  const f = {
    name: "goal",
    label: "Goal",
    type: "text",
    required: true,
  };
  const html = renderField(f);
  assert.doesNotMatch(html, /mc-field-help/);
});

test("rendered text field includes placeholder attribute", () => {
  const f = {
    name: "goal",
    label: "Goal",
    type: "text",
    placeholder: "win back inactive customers",
  };
  const html = renderField(f);
  assert.match(html, /placeholder="win back inactive customers"/);
});

test("rendered textarea field includes placeholder attribute", () => {
  const f = {
    name: "body",
    label: "Body",
    type: "textarea",
    placeholder: "Paste your draft here",
  };
  const html = renderField(f);
  assert.match(html, /<textarea/);
  assert.match(html, /placeholder="Paste your draft here"/);
});

test("rendered help text is HTML-escaped (defense against schema injection)", () => {
  const f = {
    name: "goal",
    label: "Goal",
    type: "text",
    help: '<script>alert("xss")</script>',
  };
  const html = renderField(f);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /&lt;script&gt;/);
});

// =========================================================================
// Week 7.3 (2026-09-10): friendly error mapping.
//
// `mapMclawError(err)` parses the Rust-side `HTTP <code>: <body>`
// shape (added in this commit) and returns a { headline, detail,
// action } triple. The headline is user-friendly; the detail is
// the MAIC `detail` field from the body; the action is a
// suggested next step.
// =========================================================================

const { mapMclawError } = _formSchemaInternals;

test("401 returns session-expired headline + Sign in action", () => {
  const out = mapMclawError(
    new Error('HTTP 401: {"detail":"Session expired or revoked"}')
  );
  assert.match(out.headline, /session expired/i);
  assert.equal(out.detail, "Session expired or revoked");
  assert.ok(out.action, "401 must suggest an action");
  assert.equal(out.action.label, "Sign in");
  assert.equal(typeof out.action.callback, "function");
});

test("404 returns not-installed headline + Install action (no callback)", () => {
  const out = mapMclawError(
    new Error("HTTP 404: {\"detail\":\"module 'ghost' not found\"}")
  );
  assert.match(out.headline, /isn't installed/i);
  assert.equal(out.detail, "module 'ghost' not found");
  assert.ok(out.action);
  assert.equal(out.action.label, "Install");
  assert.equal(out.action.callback, null, "Install action needs caller-supplied callback (we don't have module_id in scope here)");
});

test("409 returns already-installed (no action)", () => {
  const out = mapMclawError(
    new Error('HTTP 409: {"detail":"already installed"}')
  );
  assert.match(out.headline, /already installed/i);
  assert.equal(out.action, null);
});

test("422 returns validation-failed headline", () => {
  const out = mapMclawError(
    new Error('HTTP 422: {"detail":"url: required field missing"}')
  );
  assert.match(out.headline, /invalid input/i);
  assert.equal(out.detail, "url: required field missing");
});

test("429 returns too-many-requests headline", () => {
  const out = mapMclawError(new Error("HTTP 429: {\"detail\":\"slow down\"}"));
  assert.match(out.headline, /too many requests/i);
});

test("502/503/504 return MAIC-unavailable headline", () => {
  for (const code of [502, 503, 504]) {
    const out = mapMclawError(new Error(`HTTP ${code}: gateway down`));
    assert.match(out.headline, /temporarily unavailable/i, `status ${code}`);
  }
});

test("non-HTTP error returns the raw message as headline", () => {
  const out = mapMclawError(new Error("network timeout"));
  assert.equal(out.headline, "network timeout");
  assert.equal(out.detail, null);
  assert.equal(out.action, null);
});

test("non-JSON body is used as detail verbatim", () => {
  const out = mapMclawError(new Error("HTTP 500: <html>500 Internal Server Error</html>"));
  assert.equal(out.headline, "MAIC returned 500.");
  assert.match(out.detail, /html/);
});

test("undefined/null errors are handled gracefully", () => {
  const out = mapMclawError(undefined);
  assert.equal(out.headline, "Something went wrong.");
  assert.equal(out.detail, null);
  assert.equal(out.action, null);
});
