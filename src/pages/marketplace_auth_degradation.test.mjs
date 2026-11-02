// src/pages/marketplace_auth_degradation.test.mjs
//
// Lesson 1147 (2026-11-02, David) regression coverage.
//
// Symptom on Windows rc57.36 (reported by David):
//   1. "All models failed (4): maic/milagro-m1-t1: 401 ... | maic/milagro-m1-t2:
//      401 ... | maic/milagro-m1-t3: 401 ... | maic/chat-nemotron-nano: 401 ..."
//   2. "none of the Modules are showing up when I click 'Browse the Marketplace'"
//
// Root causes found:
//   A. The MAIC JWT in chat.json was expired/revoked. ONE dead token was
//      retried against FOUR model names, which reads like "four models
//      are broken" and hides the real cause.
//   B. `mc_list_marketplace_modules` fell back to the bundled catalog
//      ONLY on 404. A dead key returns 401, so the fallback was never
//      reached and the Marketplace rendered EMPTY.
//   C. The launchpad called `invoke("mc_list_marketplace", ...)` — but the
//      registered Tauri command is `mc_list_marketplace_modules`. The
//      invoke failed with "command not found" every time and a bare
//      `.catch(() => ({ modules: [] }))` swallowed it.
//
// These tests assert the REAL shipped source (Rust + JS), not a
// reimplementation, per Lesson 245e.
//
// Run: node --test src/pages/marketplace_auth_degradation.test.mjs

import { strict as assert } from "node:assert";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";

const RUST = "./src-tauri/src/lib.rs";
const LAUNCHPAD = "./src/pages/dashboard_launchpad.js";
const MARKETPLACE = "./src/pages/marketplace.js";

const rustSrc = await readFile(RUST, "utf8");
const launchpadSrc = await readFile(LAUNCHPAD, "utf8");
const marketplaceSrc = await readFile(MARKETPLACE, "utf8");

// ---------------------------------------------------------------------------
// B. Marketplace must degrade to the bundled catalog on 401/403, not just 404.
// ---------------------------------------------------------------------------

test("mc_list_marketplace_modules falls back to the bundled catalog on 401", () => {
  // Isolate the command body so we don't match unrelated 401 arms.
  const start = rustSrc.indexOf("fn mc_list_marketplace_modules(");
  assert.ok(start > -1, "mc_list_marketplace_modules must exist");
  const body = rustSrc.slice(start, start + 6000);

  // The 404 arm (original behavior) must still be there.
  assert.match(
    body,
    /Err\(ureq::Error::Status\(404, _\)\)/,
    "the 404 → bundled-catalog fallback must be preserved",
  );

  // The NEW arm: 401 and 403 must also reach the bundled catalog.
  assert.match(
    body,
    /Err\(ureq::Error::Status\(401, _\)\)\s*\|\s*Err\(ureq::Error::Status\(403, _\)\)/,
    "a 401/403 must route to the bundled-catalog fallback, not the error arm",
  );
});

test("the 401/403 arm calls the shared bundled-catalog helper", () => {
  const start = rustSrc.indexOf("fn mc_list_marketplace_modules(");
  const body = rustSrc.slice(start, start + 6000);
  assert.match(
    body,
    /bundled_catalog_response\(&app, &state, category, sort\)/,
    "the 401/403 arm must use the shared bundled_catalog_response helper",
  );
  assert.match(
    rustSrc,
    /fn bundled_catalog_response\(/,
    "bundled_catalog_response must be defined",
  );
});

test("the 401 fallback flags auth_degraded so the UI can explain itself", () => {
  const start = rustSrc.indexOf("fn mc_list_marketplace_modules(");
  const body = rustSrc.slice(start, start + 6000);
  assert.match(
    body,
    /"auth_degraded"/,
    "the auth-degraded response must carry auth_degraded:true",
  );
  assert.match(
    body,
    /sign in to see your account/i,
    "the auth-degraded message must tell the user to sign in",
  );
});

// ---------------------------------------------------------------------------
// mc_list_installed_modules must degrade to the LOCAL registry on 401/403.
// ---------------------------------------------------------------------------

test("mc_list_installed_modules falls back to the local registry on 401", () => {
  const start = rustSrc.indexOf("fn mc_list_installed_modules(");
  assert.ok(start > -1, "mc_list_installed_modules must exist");
  const body = rustSrc.slice(start, start + 4000);

  assert.match(
    body,
    /Err\(ureq::Error::Status\(401, _\)\)\s*\|\s*Err\(ureq::Error::Status\(403, _\)\)/,
    "a 401/403 must serve the local module registry instead of erroring",
  );
  assert.match(
    body,
    /"local_registry"/,
    "the degraded response must be marked as coming from the local registry",
  );
});

test("mc_list_installed_modules no longer takes no arguments (it needs AppState)", () => {
  const start = rustSrc.indexOf("fn mc_list_installed_modules(");
  const sig = rustSrc.slice(start, start + 200);
  assert.match(
    sig,
    /state:\s*tauri::State<'_, AppState>/,
    "the command must take AppState so it can read the local module registry",
  );
});

// ---------------------------------------------------------------------------
// C. The launchpad must call the REAL registered command name.
// ---------------------------------------------------------------------------

test("launchpad calls mc_list_marketplace_modules, not the nonexistent mc_list_marketplace", () => {
  // The old, broken name must be gone from invoke() calls.
  assert.doesNotMatch(
    launchpadSrc,
    /invoke\(\s*"mc_list_marketplace"/,
    'invoke("mc_list_marketplace") must be gone — that command does not exist',
  );
  assert.match(
    launchpadSrc,
    /invoke\(\s*"mc_list_marketplace_modules"/,
    "the launchpad must call the registered mc_list_marketplace_modules",
  );
});

test("every mc_* command the launchpad invokes is actually registered in Rust", () => {
  // Catch the class of bug: a frontend invoking a command name that the
  // Rust invoke_handler never registers. The error is silent because
  // every call site wraps in .catch().
  const invoked = new Set();
  const re = /invoke\(\s*"(mc_[a-z0-9_]+)"/g;
  let m;
  while ((m = re.exec(launchpadSrc)) !== null) invoked.add(m[1]);

  assert.ok(invoked.size >= 5, `expected several mc_* invokes, got ${invoked.size}`);

  const missing = [];
  for (const cmd of invoked) {
    // Registered commands appear in the invoke_handler list as a bare
    // identifier: `            mc_list_marketplace_modules,`
    const registered = new RegExp(`\\b${cmd}\\s*,`).test(rustSrc);
    if (!registered) missing.push(cmd);
  }
  assert.deepEqual(
    missing,
    [],
    `launchpad invokes unregistered command(s): ${missing.join(", ")}`,
  );
});

test("the launchpad marketplace catch logs instead of silently swallowing", () => {
  assert.match(
    launchpadSrc,
    /\[launchpad\] marketplace fetch failed/,
    "a failed marketplace fetch must be logged, not silently dropped",
  );
});

// ---------------------------------------------------------------------------
// A. Startup must validate the MAIC key and say something useful.
// ---------------------------------------------------------------------------

test("startup validates the MAIC key before the gateway burns 401s", () => {
  assert.match(
    rustSrc,
    /fn validate_maic_key_at_startup\(\)/,
    "validate_maic_key_at_startup must exist",
  );
  assert.match(
    rustSrc,
    /validate_maic_key_at_startup\(\);/,
    "validate_maic_key_at_startup must be called during setup",
  );
});

test("the startup validation distinguishes an expired key from a network blip", () => {
  const start = rustSrc.indexOf("fn validate_maic_key_at_startup(");
  const body = rustSrc.slice(start, start + 4000);

  // 401/403 → the account really is broken → tell the user.
  assert.match(
    body,
    /Err\(ureq::Error::Status\(401, _\)\)\s*\|\s*Err\(ureq::Error::Status\(403, _\)\)/,
    "401/403 must be recognized as an expired/revoked key",
  );
  assert.match(
    body,
    /session has expired/i,
    "the user-facing message must say the session expired",
  );
  assert.match(
    body,
    /sign out and sign in again/i,
    "the user-facing message must state the actual remedy",
  );

  // Any other error (timeout, 5xx, DNS) must NOT accuse the account.
  assert.match(
    body,
    /inconclusive/,
    "a network/server error must be treated as inconclusive, not as a bad key",
  );
});

test("the startup validation hits /v1/auth/me with the bearer token", () => {
  const start = rustSrc.indexOf("fn validate_maic_key_at_startup(");
  const body = rustSrc.slice(start, start + 4000);
  assert.match(body, /\/v1\/auth\/me/, "must validate against /v1/auth/me");
  assert.match(
    body,
    /Authorization.*Bearer/,
    "must send the key as a Bearer token",
  );
});

// ---------------------------------------------------------------------------
// Guard: the bundled catalog must still ship in the installer resources.
// ---------------------------------------------------------------------------

test("modules-catalog.json is still declared as a bundle resource", () => {
  const conf = JSON.parse(readFileSync("./src-tauri/tauri.conf.json", "utf8"));
  const resources = conf.bundle.resources || [];
  assert.ok(
    resources.includes("modules-catalog.json"),
    "modules-catalog.json must remain in bundle.resources or the 401 " +
      "fallback has nothing to fall back TO",
  );
});

test("the bundled catalog on disk is non-empty", () => {
  const catalog = JSON.parse(
    readFileSync("./src-tauri/modules-catalog.json", "utf8"),
  );
  assert.ok(Array.isArray(catalog), "catalog must be an array");
  assert.ok(
    catalog.length >= 8,
    `bundled catalog must have modules to fall back to, got ${catalog.length}`,
  );
});

// ---------------------------------------------------------------------------
// The marketplace page must render the bundled/degraded notice.
// ---------------------------------------------------------------------------

test("the marketplace page surfaces the server's bundled-notice message", () => {
  assert.match(
    marketplaceSrc,
    /bundledNotice/,
    "the marketplace must keep the bundled-notice state",
  );
  assert.match(
    marketplaceSrc,
    /mc-mkt-notice-text/,
    "the marketplace must render a notice element for the degraded state",
  );
});
