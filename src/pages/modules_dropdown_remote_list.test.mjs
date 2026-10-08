// Lesson 975 (2026-09-11 17:09 MDT, David): regression test that pins
// the modules-menu to query the REMOTE module list (`mc_list_installed_modules`)
// in addition to the local list. Before this fix, modules installed via
// MAIC's `/v1/modules/install` (Firecrawl, etc.) were tracked remotely
// but never registered in the local Tauri registry, so the toolbar
// Modules dropdown showed "No modules installed yet" even though the
// launchpad's "Your Tools" grid rendered them correctly.
//
// Pins:
//   1. mountModulesMenu uses `invoke("mc_list_installed_modules")` to
//      build the dropdown list (not just `listInstalledModules()`).
//   2. The remote list is merged with the local list. Remote wins for
//      overlap; local-only items keep their richer hooks info.
//   3. If the remote call fails, we fall back to the local-only list
//      (graceful degradation, no crash).
//   4. Remote items are normalized to {id, name, version, hooks, icon}
//      where `id` comes from `module_id` (not `id`) — same shape as
//      local items so defaultRenderItem can pick `m.icon`.
//   5. defaultRenderItem uses m.icon when present, so Firecrawl etc.
//      render with their custom icon.
//
// Run via: `node --test src/pages/modules_dropdown_remote_list.test.mjs`

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const src = await readFile(
  new URL("../modules-menu.js", import.meta.url),
  "utf8"
);

test("mountModulesMenu calls invoke('mc_list_installed_modules')", () => {
  // The fix: query the REMOTE list, not just the local Map.
  // The toolbar dropdown should reflect MAIC-installed modules too.
  assert.match(
    src,
    /invoke\(\s*["']mc_list_installed_modules["']\s*\)/,
    "mountModulesMenu should query mc_list_installed_modules (Lesson 975)"
  );
});

test("mountModulesMenu merges remote + local lists (remote wins)", () => {
  // We should have both lists combined. Remote wins for overlap so the
  // more recent install state from MAIC takes precedence.
  assert.match(
    src,
    /listInstalledModules\(\)/,
    "should still call listInstalledModules() for the local list"
  );
  // Lesson 1136 (2026-10-08, David): the merge must use the
  // canonical id (not raw .id) so MAIC's `email-campaigns-pro` and
  // the local registry's `email` collapse to a single record. The
  // previous `!remoteIds.has(m.id)` filter was a strict id match
  // and counted both — which is how David ended up seeing 18
  // modules in the Terminal badge.
  assert.match(
    src,
    /canonicalId\(m\.id\)/,
    "should dedup by canonical id (MAIC + local ids may differ)"
  );
  // Remote items come first (they win for overlap):
  assert.match(
    src,
    /\.\.\.remoteItems[\s\S]*localItems/,
    "remote items should come before local-only items in the merged list"
  );
});

test("mountModulesMenu has graceful fallback if remote call fails", () => {
  // The `try { ... } catch { fall back to local only }` pattern.
  // Pin both the try block (remote fetch) and the catch fallback
  // (local-only list) so we don't regress to "remote fails → empty
  // list → user can't open the menu at all".
  assert.match(
    src,
    /try\s*\{[\s\S]*mc_list_installed_modules[\s\S]*\}\s*catch\s*\([\s\S]*listInstalledModules\(\)/,
    "should have try/catch with local-only fallback"
  );
  // Console.warn so users can debug in DevTools
  assert.match(
    src,
    /console\.warn\(\s*["']\[modules-menu\][^"']*["']/,
    "should log a console.warn when the remote call fails"
  );
});

test("remote items are normalized to {id, name, version, hooks, icon}", () => {
  // MAIC's /v1/modules/installed returns `module_id`, not `id`. The
  // launcher / dashboard Modules dropdown expects `id`. Pin the
  // normalization so future refactors don't drop it.
  assert.match(
    src,
    /id:\s*m\.module_id/,
    "should normalize module_id → id for remote items"
  );
  assert.match(
    src,
    /name:\s*m\.name/,
    "should use m.name for remote items"
  );
  // client_manifest.version if present, else ""
  assert.match(
    src,
    /version:\s*cm\.version\s*\|\|\s*["']["']/,
    "should default version to '' when client_manifest.version is missing"
  );
  // icon: prefer m.icon (top-level), fall back to client_manifest.icon
  assert.match(
    src,
    /icon:\s*m\.icon\s*\|\|\s*cm\.icon/,
    "should use m.icon || cm.icon for remote items"
  );
});

test("defaultRenderItem uses m.icon when present", () => {
  // The render-item shape already accepts `icon`. Pin that the
  // default renderer passes it through, so Firecrawl/etc. get their
  // custom icon (🔥 for firecrawl, etc.) instead of the generic 🧩.
  assert.match(
    src,
    /icon:\s*m\.icon\s*\|\|\s*cm\.icon\s*\|\|\s*["']🧩["']/,
    "defaultRenderItem should use m.icon || cm.icon || '🧩' (Lesson 1137 — matches the Windows toolbar so Firecrawl/etc. get their custom icon)"
  );
});
