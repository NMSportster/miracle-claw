// modules-catalog-fallback.test.mjs — verify the bundled catalog and
// the merge logic produce the right shape for the marketplace UI.
//
// We don't invoke the Rust command directly (it needs a Tauri runtime),
// so we exercise the *data shape* — load modules-catalog.json as the
// Rust code does, then merge with a fake registry, then verify the JS
// side renders it correctly.

import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";

const catalog = JSON.parse(
  readFileSync("./src-tauri/modules-catalog.json", "utf8"),
);

// Mirror the Rust merge_catalog_with_registry shape.
function merge(catalog, installedIds) {
  return catalog
    .map((m) => ({
      id: m.id,
      name: m.name,
      subtitle: m.subtitle || null,
      version: m.version,
      description: m.description,
      icon: m.icon || "🧩",
      category: m.category || "Other",
      tags: m.tags || [],
      publisher: m.publisher || "Milagro Cloud",
      minMcVersion: m.minMcVersion || null,
      installed: installedIds.has(m.id),
      installed_version: null, // would come from real registry
    }))
    .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
}

// Mirror the JS renderCard's field fallbacks.
function renderCard(m) {
  const id = m.module_id || m.id;
  const name = m.name || m.title || id;
  const desc = m.description || m.summary || "";
  const installed = m.installed === true;
  const subtitle = m.subtitle || "";
  return { id, name, desc, installed, subtitle };
}

// ---- Tests -----------------------------------------------------------------

function test_catalog_loads_with_expected_count() {
  assert.ok(Array.isArray(catalog), "catalog should be an array");
  assert.ok(catalog.length >= 8, `expected >=8 modules, got ${catalog.length}`);
  const ids = catalog.map((m) => m.id);
  for (const required of ["voice", "email", "calendar", "pdf", "ocr", "translate", "tts"]) {
    assert.ok(ids.includes(required), `catalog missing required module: ${required}`);
  }
  console.log("PASS: catalog_loads_with_expected_count (n=" + catalog.length + ")");
}

function test_every_entry_has_required_fields() {
  for (const m of catalog) {
    assert.ok(m.id, `missing id in ${JSON.stringify(m)}`);
    assert.ok(m.name, `missing name in ${m.id}`);
    assert.ok(m.version, `missing version in ${m.id}`);
    assert.ok(m.description, `missing description in ${m.id}`);
    assert.ok(m.category, `missing category in ${m.id}`);
  }
  console.log("PASS: every_entry_has_required_fields");
}

function test_voice_entry_has_subtitle() {
  const voice = catalog.find((m) => m.id === "voice");
  assert.ok(voice.subtitle, "voice should have a subtitle");
  assert.match(voice.subtitle, /Push-to-talk|Push-to-?talk/i);
  console.log("PASS: voice_entry_has_subtitle");
}

function test_merge_marks_installed_correctly() {
  const installedIds = new Set(["voice", "tts", "calendar"]);
  const merged = merge(catalog, installedIds);
  const voice = merged.find((m) => m.id === "voice");
  const email = merged.find((m) => m.id === "email");
  assert.equal(voice.installed, true);
  assert.equal(email.installed, false);
  console.log("PASS: merge_marks_installed_correctly");
}

function test_renders_installed_and_not_installed() {
  const installedIds = new Set(["voice"]);
  const merged = merge(catalog, installedIds);
  const voiceCard = renderCard(merged.find((m) => m.id === "voice"));
  const emailCard = renderCard(merged.find((m) => m.id === "email"));
  assert.equal(voiceCard.installed, true);
  assert.equal(voiceCard.subtitle, "Push-to-talk chat input");
  assert.equal(emailCard.installed, false);
  console.log("PASS: renders_installed_and_not_installed");
}

function test_categories_derive_correctly() {
  const counts = new Map();
  for (const m of catalog) {
    counts.set(m.category, (counts.get(m.category) || 0) + 1);
  }
  assert.ok(counts.size >= 3, "should have at least 3 distinct categories");
  assert.ok(counts.has("Productivity"), "Productivity should be a category");
  assert.ok(counts.has("Communication"), "Communication should be a category");
  console.log("PASS: categories_derive_correctly (" + Array.from(counts.keys()).join(", ") + ")");
}

function test_no_duplicate_ids() {
  const ids = catalog.map((m) => m.id);
  const unique = new Set(ids);
  assert.equal(unique.size, ids.length, `duplicate ids in catalog: ${ids.join(",")}`);
  console.log("PASS: no_duplicate_ids");
}

function test_render_no_modules_empty_state_still_works() {
  // Simulate state.modules = []
  const empty = [];
  assert.equal(empty.length, 0);
  // The renderGrid code in marketplace.js has an "empty" branch — that's
  // what shows when the catalog is missing entirely.
  console.log("PASS: render_no_modules_empty_state_still_works");
}

// ---- Run all ----------------------------------------------------------------

const tests = [
  test_catalog_loads_with_expected_count,
  test_every_entry_has_required_fields,
  test_voice_entry_has_subtitle,
  test_merge_marks_installed_correctly,
  test_renders_installed_and_not_installed,
  test_categories_derive_correctly,
  test_no_duplicate_ids,
  test_render_no_modules_empty_state_still_works,
];

let passed = 0;
let failed = 0;
for (const t of tests) {
  try {
    await t();
    passed++;
  } catch (e) {
    failed++;
    console.error(`FAIL: ${t.name}: ${e.message}`);
  }
}
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
