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
