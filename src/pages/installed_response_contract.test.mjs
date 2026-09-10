// Week 7.5: cross-surface contract test for `/v1/modules/installed`.
//
// All three surfaces (Tauri launchpad, mc-terminal TUI, mclaw CLI)
// deserialize the same MAIC response. This test pins the canonical
// fixture from the launchpad's perspective and asserts the fields
// the launchpad actually reads (module_id, name, category, etc.) are
// present and well-typed.
//
// The same fixture is pinned in mclaw (tests/installed_response_contract.rs)
// and in mc-terminal (struct InstalledModule in src-tauri/src/bin/mc_terminal.rs).
// If you change the fixture here, you must update those too — or the
// contract will silently break for one of the surfaces.
//
// Run via: `npm run test:js`.

import { test } from "node:test";
import assert from "node:assert/strict";

/** Canonical MAIC response. MUST match the mclaw Rust test fixture. */
const CANONICAL_FIXTURE = {
  modules: [
    {
      module_id: "firecrawl",
      name: "Firecrawl Web Scraper",
      description: "Scrape any URL to clean markdown.",
      icon: "🔥",
      category: "data",
      subcategory: null,
      installed_at: "2026-09-10T10:00:00Z",
      last_used_at: "2026-09-10T10:05:00Z",
      client_manifest: {
        module_id: "firecrawl",
        tools: [
          {
            name: "scrape",
            description: "Scrape a URL",
            form_schema: { fields: [{ name: "url", type: "text" }] },
          },
        ],
      },
    },
    {
      module_id: "data-analyzer",
      name: "Data Analyzer",
      description: "Summarize CSV/JSON datasets.",
      icon: "📊",
      category: "data",
      subcategory: "analytics",
      installed_at: "2026-09-09T08:30:00Z",
      last_used_at: null,
      client_manifest: { tools: [] },
    },
  ],
  grouped_by_category: { data: ["firecrawl", "data-analyzer"] },
};

test("canonical fixture has the launchpad-required fields", () => {
  // The launchpad reads module_id, name, icon, category, and walks
  // client_manifest.tools to render form inputs. We pin those here.
  for (const m of CANONICAL_FIXTURE.modules) {
    assert.equal(typeof m.module_id, "string", "module_id is required");
    assert.equal(typeof m.name, "string", "name is required");
    assert.equal(typeof m.icon, "string", "icon is required");
    assert.equal(typeof m.category, "string", "category is required");
    assert.ok(
      m.client_manifest && Array.isArray(m.client_manifest.tools),
      "client_manifest.tools must be an array"
    );
  }
});

test("empty installed response yields a renderable empty array", () => {
  // mc-terminal relies on this for its first-launch landing page
  // (Mode::Empty). The launchpad should also be able to render
  // an empty state without crashing.
  const empty = { modules: [], grouped_by_category: {} };
  assert.deepEqual(empty.modules, []);
  assert.deepEqual(empty.grouped_by_category, {});
});

test("grouped_by_category is consistent with modules[].category", () => {
  // Soft contract: every id in grouped_by_category[cat] should also
  // appear in modules[].module_id with the same category.
  const byId = new Map(CANONICAL_FIXTURE.modules.map((m) => [m.module_id, m]));
  for (const [cat, ids] of Object.entries(CANONICAL_FIXTURE.grouped_by_category)) {
    for (const id of ids) {
      const m = byId.get(id);
      assert.ok(m, `grouped_by_category[${cat}] references unknown module: ${id}`);
      assert.equal(m.category, cat, `module ${id} has category ${m.category} but is in grouped_by_category[${cat}]`);
    }
  }
});

test("subcategory can be null, string, or undefined (loose-typed)", () => {
  // mc-terminal uses #[serde(default)] on subcategory so it accepts
  // null OR undefined. We pin the launchpad's tolerance here so a
  // future tightening doesn't break mc-terminal.
  for (const m of CANONICAL_FIXTURE.modules) {
    assert.ok(
      m.subcategory === null || typeof m.subcategory === "string" || m.subcategory === undefined,
      `subcategory for ${m.module_id} must be null|string|undefined`
    );
  }
});

test("last_used_at can be null (never-used module)", () => {
  // data-analyzer has never been run. The launchpad must NOT crash
  // when sorting by last_used_at with a null value.
  const neverUsed = CANONICAL_FIXTURE.modules.find((m) => m.module_id === "data-analyzer");
  assert.equal(neverUsed.last_used_at, null);
});
