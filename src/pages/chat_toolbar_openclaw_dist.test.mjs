// Lesson 978 (2026-09-11 19:00 MDT, David): regression tests for the
// chat-toolbar openclaw-dist patch.
//
// The toolbar lives in the OpenClaw chat UI (served at
// 127.0.0.1:28789 by the bundled miracle-claw-gateway.mjs), NOT in
// the MC source tree. It is wired in via depot/openclaw-patches/ and
// applied by scripts/patch-openclaw-dist.sh. These tests pin:
//
//   1. All three patch files exist (mc-toolbar.js, mc-toolbar.css,
//      index.html.insert).
//   2. The JS is self-contained (no `import`/`export` — the openclaw
//      dist's JS world can't resolve MC's modules).
//   3. The JS gates on `location.host === '127.0.0.1:28789'` (Lesson
//      511 — IPC-independent gate, same as mc-back-button.js).
//   4. The JS uses `.mc-oc-` prefix on all its DOM classes (so we
//      can't collide with the openclaw host page).
//   5. The JS handles all four buttons (Attach, Secrets, Voice,
//      Modules) and calls the matching Tauri command for each
//      (mc_list_installed_modules, mc_run_module, mc_voice_native_capture,
//      mc_list_tools).
//   6. The modules dropdown merges with mc_list_installed_modules
//      (Lesson 975) and shows graceful fallback to "No modules
//      installed" if the call fails.
//   7. The CSS rules are all `.mc-oc-`-prefixed (no global resets
//      that would bleed into openclaw's host page).
//   8. The HTML insert patch loads both mc-toolbar.js AND
//      mc-back-button.js, with two MC-PATCH markers for idempotency.
//   9. The patcher dry-run reports a clean apply for these three
//      patches.
//  10. The MANIFEST.md indexes all three new patches.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "..", "..");
const PATCHES_DIR = resolve(REPO_ROOT, "depot/openclaw-patches");
const RESOURCES_DIR = resolve(
  REPO_ROOT,
  "src-tauri/resources/dist/control-ui"
);

const jsPath = resolve(PATCHES_DIR, "dist/control-ui/mc-toolbar.js");
const cssPath = resolve(PATCHES_DIR, "dist/control-ui/mc-toolbar.css");
const insertPath = resolve(PATCHES_DIR, "dist/control-ui/index.html.insert");
const backButtonPath = resolve(PATCHES_DIR, "dist/control-ui/mc-back-button.js");
const manifestPath = resolve(PATCHES_DIR, "MANIFEST.md");

const jsSrc = await readFile(jsPath, "utf8");
const cssSrc = await readFile(cssPath, "utf8");
const insertSrc = await readFile(insertPath, "utf8");
const backButtonSrc = await readFile(backButtonPath, "utf8");
const manifestSrc = await readFile(manifestPath, "utf8");

test("all three toolbar patch files exist and are non-empty", async () => {
  for (const p of [jsPath, cssPath, insertPath]) {
    const s = await stat(p);
    assert.ok(s.isFile(), `${p} should be a regular file`);
    assert.ok(s.size > 100, `${p} should be non-trivial (>100 bytes)`);
  }
});

test("mc-back-button.js patch also exists and gates on 127.0.0.1:28789", async () => {
  // Lesson 978: the back-button used to be inlined as a script block
  // in index.html (append-patch). We migrated it to its own file so
  // the patcher can wipe it on `bundle-runtime.sh --force` and the
  // <script src="./mc-back-button.js"> reference in the insert
  // actually resolves. If mc-back-button.js is missing, the chat UI
  // logs a 404 but the toolbar still mounts — not catastrophic, just
  // noisy. Pin that the file is present + uses the same gate.
  const s = await stat(backButtonPath);
  assert.ok(s.isFile(), `${backButtonPath} should be a regular file`);
  assert.ok(s.size > 100, `${backButtonPath} should be non-trivial (>100 bytes)`);
  assert.match(
    backButtonSrc,
    /127\.0\.0\.1:28789/,
    "back-button should also gate on 127.0.0.1:28789 (Lesson 511)"
  );
});

test("mc-toolbar.js is self-contained (no import / export / require)", () => {
  // The openclaw dist's JS world can't resolve MC's source modules
  // (`src/modules-menu.js`, etc.), so the toolbar must be a single
  // IIFE. It can still `await` and call `tauriInvoke()`, but it
  // cannot import anything.
  assert.doesNotMatch(jsSrc, /^\s*import\b/m, "no top-level import statements");
  assert.doesNotMatch(jsSrc, /^\s*export\b/m, "no top-level export statements");
  assert.doesNotMatch(
    jsSrc,
    /require\s*\(\s*["'][^"']*\.js["']\s*\)/,
    "no CommonJS require() of .js files"
  );
  assert.match(jsSrc, /\(function\s*\(\s*\)\s*\{[\s\S]*?\}\s*\)\s*\(\s*\)\s*;?/, "IIFE wrapper");
});

test("mc-toolbar.js gates on 127.0.0.1:28789 (Lesson 511 IPC-independent)", () => {
  assert.match(
    jsSrc,
    /window\.location\.host\s*===\s*['"]127\.0\.0\.1:28789['"]/,
    "should gate on the chat page host"
  );
  assert.doesNotMatch(
    jsSrc,
    /__openclawHostBridge/,
    "should NOT depend on the bridge pill globals (Lesson 511 lesson)"
  );
});

test("mc-toolbar.js uses mc-oc- prefix on its own DOM classes", () => {
  // Tauri webview still shares the document with the openclaw host
  // page, so we prefix every class we create. Pin that the toolbar
  // root, dropdown, modal, toast, and buttons all use the prefix.
  for (const cls of [
    "mc-oc-toolbar",
    "mc-oc-icon-link",
    "mc-oc-icon-link-glyph",
    "mc-oc-icon-link-count",
    "mc-oc-dropdown",
    "mc-oc-dropdown-item",
    "mc-oc-modal-backdrop",
    "mc-oc-modal-panel",
    "mc-oc-attach-dropzone",
    "mc-oc-toast",
  ]) {
    assert.ok(
      jsSrc.includes(cls) || cssSrc.includes(`.${cls}`),
      `${cls} should appear in the toolbar source or styles`
    );
  }
  // Should NOT use any plain class names that collide with openclaw
  // (e.g. `dropdown` alone, `modal` alone).
  assert.doesNotMatch(jsSrc, /className\s*=\s*['"]dropdown['"]/, "no unprefixed 'dropdown' className");
  assert.doesNotMatch(jsSrc, /className\s*=\s*['"]modal['"]/, "no unprefixed 'modal' className");
});

test("mc-toolbar.js wires all four buttons + matching Tauri commands", () => {
  // Each button id should be passed to makeButton() in the source.
  for (const id of [
    "mc-oc-attach-btn",
    "mc-oc-secrets-btn",
    "mc-oc-voice-btn",
    "mc-oc-modules-btn",
  ]) {
    assert.ok(
      new RegExp(`makeButton\\(\\s*['"]${id}['"]`).test(jsSrc),
      `should call makeButton('${id}', ...)`
    );
  }
  // Matching Tauri commands.
  assert.match(jsSrc, /tauriInvoke\(\s*['"]mc_voice_native_capture['"]/, "voice uses mc_voice_native_capture");
  assert.match(jsSrc, /tauriInvoke\(\s*['"]mc_list_installed_modules['"]/, "modules uses mc_list_installed_modules");
  assert.match(jsSrc, /tauriInvoke\(\s*['"]mc_run_module['"]/, "modules Run uses mc_run_module");
  assert.match(jsSrc, /tauriInvoke\(\s*['"]mc_list_tools['"]/, "secrets uses mc_list_tools");
});

test("mc-toolbar.js merges modules list with graceful fallback (Lesson 975)", () => {
  // Lesson 975: modules menu reads from mc_list_installed_modules
  // (the remote MAIC source). Lesson 976: the count badge and the
  // dropdown use the SAME data so they agree.
  assert.match(
    jsSrc,
    /fetchInstalledModulesMerged[\s\S]*?mc_list_installed_modules/,
    "should have a fetchInstalledModulesMerged() helper that calls mc_list_installed_modules"
  );
  // Graceful fallback: if the call throws, return [].
  assert.match(
    jsSrc,
    /try\s*\{[\s\S]*?mc_list_installed_modules[\s\S]*?catch\s*\(\s*\w+\s*\)\s*\{[\s\S]*?return\s*\[\s*\]/,
    "should fall back to [] on error"
  );
  // No-tools fallback (Lesson 976): when a module has empty tools,
  // show an info modal, not a silent toast bail.
  assert.match(
    jsSrc,
    /openModuleInfoModal|module info modal/i,
    "should have a no-tools info modal fallback"
  );
  // Voice "idle" CSS state (Lesson 974).
  assert.match(jsSrc, /setAttribute\(\s*['"]data-state['"]\s*,\s*['"]idle['"]/, "voice btn should be set to data-state='idle'");
});

test("mc-toolbar.css is fully scoped under .mc-oc- (no global resets)", () => {
  // No top-level type selectors that could collide.
  assert.doesNotMatch(
    cssSrc,
    /^\s*body\s*\{/m,
    "no global `body {` rule"
  );
  assert.doesNotMatch(
    cssSrc,
    /^\s*html\s*\{/m,
    "no global `html {` rule"
  );
  assert.doesNotMatch(
    cssSrc,
    /^\s*\*\s*\{/m,
    "no global `* {` reset"
  );
  // Walk the file as a sequence of tokens: when we see a `{` we
  // push the preceding selector onto the stack; when we see `}` we
  // pop. We allow two exceptions:
  //   - @media / @keyframes wrappers (selectors starting with @)
  //   - selectors INSIDE a @keyframes block (e.g. `0%, 100% { ... }`)
  const stripped = cssSrc.replace(/\/\*[\s\S]*?\*\//g, "");
  // Token-walk approach: scan char by char. We open a "block" on
  // each `{`; the block's "selector" is the trimmed run of non-`{`
  // chars that came immediately before it. We close on `}`.
  const stack = []; // each frame: { selector, kind: 'rule'|'at', atKeyframes: bool }
  let buf = "";
  let realRules = 0;
  for (let i = 0; i < stripped.length; i++) {
    const c = stripped[i];
    if (c === "{") {
      const sel = buf.trim();
      buf = "";
      if (!sel) continue;
      const isAt = sel.startsWith("@");
      const isKeyframes = /^@keyframes\b/.test(sel);
      stack.push({ selector: sel, atKeyframes: isKeyframes });
      if (isAt && !isKeyframes) {
        // @media etc. — selector doesn't have to start with .mc-oc-
        // because the inner rules will be checked when we recurse.
        // (Our CSS doesn't actually use @media, so this is a no-op.)
        continue;
      }
      if (isKeyframes) {
        // Don't check keyframe wrapper.
        continue;
      }
      // If we're inside a keyframes block, skip.
      if (stack.some((f) => f.atKeyframes)) continue;
      // Top-level rule check.
      realRules++;
      assert.ok(
        /^\.mc-oc-/.test(sel),
        `CSS rule "${sel}" must start with .mc-oc-`
      );
    } else if (c === "}") {
      buf = ""; // reset so the next selector doesn't include this rule's body
      stack.pop();
    } else {
      buf += c;
    }
  }
  assert.ok(realRules >= 8, `should have at least 8 real CSS rules, found ${realRules}`);
});

test("index.html.insert loads mc-toolbar.js + mc-back-button.js with two markers", () => {
  // Two MC-PATCH markers for idempotency (Lesson 706).
  const markers = insertSrc.match(/<!--\s*MC-PATCH:[^>]+-->/g) || [];
  assert.equal(markers.length, 2, "should have exactly two MC-PATCH markers");
  // The script tag for the toolbar.
  assert.match(insertSrc, /<script[^>]+src=["']\.\/mc-toolbar\.js["']/, "should load mc-toolbar.js");
  // The CSS link.
  assert.match(insertSrc, /<link[^>]+href=["']\.\/mc-toolbar\.css["']/, "should load mc-toolbar.css");
  // The back-button script (migrated here from old append patch).
  assert.match(insertSrc, /<script[^>]+src=["']\.\/mc-back-button\.js["']/, "should load mc-back-button.js");
});

test("patch-openclaw-dist.sh --dry-run reports a clean apply for the new patches", () => {
  // Run the patcher in dry-run against the current resources dir.
  // If a previous run already applied the patches, the dry-run will
  // report SKIP, which is the right outcome. The key check is that
  // the patcher recognizes all four files.
  const out = execSync("bash scripts/patch-openclaw-dist.sh --dry-run", {
    cwd: REPO_ROOT,
    encoding: "utf8",
  });
  assert.match(out, /dist\/control-ui\/mc-toolbar\.js/, "patcher should reference mc-toolbar.js");
  assert.match(out, /dist\/control-ui\/mc-toolbar\.css/, "patcher should reference mc-toolbar.css");
  assert.match(out, /dist\/control-ui\/mc-back-button\.js/, "patcher should reference mc-back-button.js");
  assert.match(out, /dist\/control-ui\/index\.html/, "patcher should reference index.html");
});

test("MANIFEST.md indexes all three new toolbar patches (Lesson 978, rc56.8)", () => {
  // Each patch should appear on a row in the table. The row format is
  // `| patch | lesson | rc | description |` so we just need a regex
  // that matches the patch filename, the number 978, and rc56.8
  // appearing in the same row.
  for (const patchFile of [
    "mc-toolbar.js",
    "mc-toolbar.css",
    "index.html.insert",
  ]) {
    const re = new RegExp(
      `\\|\\s+.*${patchFile.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\\\$&")}.*\\|\\s+978\\s+\\|\\s+rc56\\.8\\s+\\|`
    );
    assert.ok(
      re.test(manifestSrc),
      `MANIFEST should list ${patchFile} as Lesson 978 / rc56.8`
    );
  }
});

test("MANIFEST.md also indexes the migrated mc-back-button.js patch (Lesson 502/511)", () => {
  // The back-button migration to its own file is part of Lesson 978.
  // Pin that MANIFEST.md documents it.
  assert.match(
    manifestSrc,
    /mc-back-button\.js.*rc53\.4/,
    "MANIFEST should reference mc-back-button.js as Lesson 502/511 / rc53.4"
  );
});

// Lesson 985 (2026-09-16 07:15 MDT, David): regression tests for the
// 4 chat-toolbar bugs reported after installing 2 more modules:
//   Bug 1 — Secrets button: sent snake_case `overlay_key` to
//           `mc_open_overlay`, Tauri 2 IPC rejected with
//           "missing required key overlayKey". Fix: camelCase `overlayKey`.
//   Bug 2 — Modules Run from chat: `mc_run_module` invocations passed
//           snake_case keys (`module_id`, `tool`) where Tauri 2 IPC
//           expects camelCase. Fix: ensure camelCase `moduleId`/`tool`/
//           `inputs`/`surface` consistently across the toolbar.
//   Bug 3 — Voice button: passed `timeoutMs` correctly, but the
//           MC native voice path returns code 5 when the user starts
//           speaking too late. We add a listener-side `listening` state
//           that the pill can react to + an explicit user-preflight
//           hint (the rc56.9 message is the actual SAPI reply).
//   Bug 4 — Terminal Modules Run does nothing: caused by an early
//           `return` in the menu click handler when `moduleId` did not
//           match the cache key. Fixed in Lesson 980 by using
//           `(m.id || m.module_id)`. Pin the structure here.

test("Lesson 985 Bug 1: Secrets button sends camelCase overlayKey (not snake_case overlay_key)", () => {
  // The Rust fn signature is `fn mc_open_overlay(overlay_key: String, ...)`
  // but Tauri 2's IPC requires JS callers to send camelCase keys; the
  // IPC layer converts camelCase → snake_case when matching Rust params.
  // Sending snake_case `overlay_key` causes a double-conversion bug
  // where Tauri normalizes the JS key to camelCase first, then can't
  // find the matching Rust param (which is snake_case). Error: "missing
  // required key overlayKey".
  assert.match(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_open_overlay['"]\s*,\s*\{\s*overlayKey\s*:\s*['"]secrets['"]/,
    "Secrets button should send { overlayKey: 'secrets' } (camelCase)"
  );
  assert.doesNotMatch(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_open_overlay['"]\s*,\s*\{\s*overlay_key\s*:/,
    "Secrets button must NOT send snake_case overlay_key"
  );
});

test("Lesson 985 Bug 2: chat Modules Run sends camelCase moduleId/tool/inputs/surface", () => {
  // The chat toolbar's Run handler must pass camelCase keys to
  // mc_run_module. (Same Tauri 2 IPC convention as Bug 1.)
  // Without camelCase, Tauri 2 silently drops the snake_case key
  // and the Rust side gets an empty moduleId → "missing required key
  // module_id".
  assert.match(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_run_module['"]\s*,\s*\{\s*moduleId\s*:/,
    "chat Run must pass moduleId (camelCase)"
  );
  assert.match(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_run_module['"]\s*,\s*\{[\s\S]*?moduleId\s*:\s*\w+\.\w+[\s\S]*?tool\s*:/,
    "chat Run must pass tool (camelCase) along with moduleId"
  );
  assert.match(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_run_module['"]\s*,\s*\{[\s\S]*?surface\s*:\s*['"]openclaw_chat['"]/,
    "chat Run must pass surface: 'openclaw_chat' (camelCase)"
  );
  assert.doesNotMatch(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_run_module['"]\s*,\s*\{\s*module_id\s*:/,
    "chat Run must NOT send snake_case module_id"
  );
});

test("Lesson 985 Bug 2b: chat Modules Run uses tool.name (MAIC schema), not tool.id", () => {
  // Lesson 985 (2026-09-16 07:15 MDT, David): the chat toolbar's
  // Run handler sent `tool: tool.id` (undefined, since MAIC tools
  // have `name` not `id`). Rust's `mc_run_module` rejected the
  // empty string with "tool is required" and the user saw "Run
  // failed". The launchpad (`src/pages/dashboard_launchpad.js:548`)
  // correctly uses `tool: firstTool.name`. The chat toolbar must
  // match that convention: prefer tool.name, fall back to tool.id.
  assert.match(
    jsSrc,
    /tool:\s*tool\.name\s*\|\|\s*tool\.id/,
    "chat Run must send `tool: tool.name || tool.id` (MAIC tools have .name, not .id)"
  );
});

test("Lesson 985 Bug 3: chat voice button uses camelCase timeoutMs (consistent with mc-voice-native.js)", () => {
  // The standalone mc-voice-native.js patch uses { timeoutMs: 15000 }
  // and works. The chat toolbar should match that convention.
  assert.match(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_voice_native_capture['"]\s*,\s*\{\s*timeoutMs\s*:/,
    "voice button must pass timeoutMs (camelCase)"
  );
  assert.doesNotMatch(
    jsSrc,
    /tauriInvoke\(\s*['"]mc_voice_native_capture['"]\s*,[^)]*timeout_ms\s*:/,
    "voice button must NOT pass snake_case timeout_ms"
  );
});

test("Lesson 985 Bug 4: terminal Modules Run click uses (m.id || m.module_id) cache lookup (Lesson 980)", async () => {
  // Bug: terminal Modules Run does nothing. Lesson 980 pinned the fix
  // by introducing a `(m.id || m.module_id)` cache lookup and the
  // _raw unwrap. Re-pin the relevant shape from src/modules-menu.js
  // (which terminal.js imports).
  const menuSrc = await readFile(
    resolve(REPO_ROOT, "src/modules-menu.js"),
    "utf8"
  );
  assert.match(
    menuSrc,
    /\(m\.id\s*\|\|\s*m\.module_id\)\s*===\s*moduleId/,
    "modules-menu click handler must use (m.id || m.module_id) === moduleId cache lookup"
  );
  assert.match(
    menuSrc,
    /menu\._items/,
    "modules-menu click handler must read cached items from menu._items"
  );
  assert.match(
    menuSrc,
    /cached\._raw/,
    "modules-menu click handler must unwrap cached._raw to MAIC shape"
  );
});

// ---------------------------------------------------------------------
// Lesson 985 follow-up (2026-09-16, ~08:30 MDT, David user retest):
// the chat toolbar used to render a JSON-textarea modal for module
// Run, which felt different from the Launchpad's schema-form modal
// and pasted into an unresolvable chat-input selector. The toolbar
// must now render the same schema-driven form fields and show the
// result IN the modal with Send-to-Chat / Copy / Close buttons.
// ---------------------------------------------------------------------

test("Lesson 985 follow-up Bug 3: chat toolbar renders form_schema fields (not just JSON textarea)", () => {
  // User feedback 2026-09-16 08:28 MDT: "different popup from
  // Dashboard, none of them work". Dashboard uses dashboard_launchpad.js
  // / modules-menu openModuleModal which render schema fields. Toolbar
  // must match — read tool.form_schema.properties and render proper
  // <input>/<select>/<textarea> controls, not just one JSON textarea.
  assert.match(
    jsSrc,
    /schema\s*=\s*tool\.form_schema/,
    "showRunForm must read tool.form_schema"
  );
  assert.match(
    jsSrc,
    /schema\.properties/,
    "showRunForm must iterate schema.properties"
  );
  assert.match(
    jsSrc,
    /data-field-type/,
    "showRunForm must tag rendered inputs with data-field-type for collection"
  );
});

test("Lesson 985 follow-up Bug 3: chat toolbar Run result stays in modal (not silent paste-to-chat)", () => {
  // Old behaviour: pasteIntoChatInput ran automatically after Run,
  // but the OpenClaw chat composer selector chain often failed
  // (placeholder is a translated i18n string, not literal "essage").
  // New behaviour: the modal's own result area + Copy/Send-to-Chat
  // buttons always work; pasting only happens on Send-to-Chat click.
  assert.match(
    jsSrc,
    /result\.textContent\s*=\s*outText/,
    "showRunForm must set result.textContent = outText"
  );
  assert.match(
    jsSrc,
    /📤 Send to Chat|Send to Chat/,
    "showRunForm must offer a Send-to-Chat button"
  );
  assert.match(
    jsSrc,
    /📋 Copy|Copy/,
    "showRunForm must offer a Copy button"
  );
});

test("Lesson 985 follow-up Bug 4: modules-menu logs click + openModuleModal calls (sidecar diagnostic)", () => {
  // 08:28 MDT retest: terminal Modules Run still does nothing. The
  // static code path is correct (Lesson 980) but the sidecar capture
  // doesn't tell us whether the click reached the handler. Pin the
  // console.log lines so future builds land in MC-openclaw info.txt.
  // Console prefix `[miracle-claw]` is the sidecar's scan key.
  assert.match(
    readFileSync(resolve(REPO_ROOT, "src/modules-menu.js"), "utf8"),
    /\[miracle-claw\]\s+modules-menu click/,
    "modules-menu click handler must console.log with [miracle-claw] prefix"
  );
  assert.match(
    readFileSync(resolve(REPO_ROOT, "src/modules-menu.js"), "utf8"),
    /\[miracle-claw\]\s+modules-menu lookup/,
    "modules-menu click handler must console.log the lookup result"
  );
  assert.match(
    readFileSync(resolve(REPO_ROOT, "src/modules-menu.js"), "utf8"),
    /\[miracle-claw\]\s+openModuleModal/,
    "openModuleModal must console.log its inputs at entry"
  );
});
