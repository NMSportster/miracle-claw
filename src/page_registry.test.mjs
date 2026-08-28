// page_registry.test.mjs — unit tests for the page_registry module.
//
// Run: node src/page_registry.test.mjs
//
// Lesson 713 (2026-08-28, David): the auth gate is the load-bearing
// piece of the JWT requirement. These tests prove it works in isolation
// without needing the Tauri runtime. We mock window.__mc_mountLogin to
// avoid the Tauri-specific global.

import { register, mount, setAuthState, isAuthenticated } from "./page_registry.js";

let passed = 0;
let failed = 0;
const failures = [];

function assertEq(actual, expected, label) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    passed++;
  } else {
    failed++;
    failures.push(`${label}: expected ${e}, got ${a}`);
  }
}

function assert(cond, label) {
  if (cond) {
    passed++;
  } else {
    failed++;
    failures.push(`FAIL: ${label}`);
  }
}

// Reset between tests
function reset() {
  setAuthState(false);
  delete globalThis.window?.__mc_mountLogin;
}

// ---- Test 1: requiresAuth=true + unauthenticated → blocks mount ----
reset();
let bounceCount = 0;
globalThis.window = globalThis.window || {};
globalThis.window.__mc_mountLogin = () => { bounceCount++; };
register("test-protected", {
  label: "Test Protected",
  requiresAuth: true,
  mount: () => { throw new Error("should not be called"); },
  unmount: () => {},
});
const result1 = mount("test-protected", null);
assertEq(result1, false, "blocks unauthenticated mount of requiresAuth page");
assertEq(bounceCount, 1, "calls onNeedsLogin bounce exactly once");

// ---- Test 2: requiresAuth=true + authenticated → mounts ----
reset();
let mountCount = 0;
register("test-authed", {
  label: "Test Authed",
  requiresAuth: true,
  mount: () => { mountCount++; },
  unmount: () => {},
});
setAuthState(true);
const result2 = mount("test-authed", null);
assertEq(result2, true, "allows authenticated mount");
assertEq(mountCount, 1, "mount() called exactly once");

// ---- Test 3: requiresAuth=false (login page) → mounts without auth ----
reset();
let loginMountCount = 0;
register("test-login", {
  label: "Test Login",
  requiresAuth: false,
  mount: () => { loginMountCount++; },
  unmount: () => {},
});
const result3 = mount("test-login", null);
assertEq(result3, true, "login page mounts without auth");
assertEq(loginMountCount, 1, "login page mount() called");

// ---- Test 4: bounce also fires via ctx.onNeedsLogin override ----
reset();
let ctxBounceCount = 0;
register("test-ctx-bounce", {
  label: "Test Ctx Bounce",
  requiresAuth: true,
  mount: () => { throw new Error("should not be called"); },
  unmount: () => {},
});
const result4 = mount("test-ctx-bounce", null, {
  onNeedsLogin: () => { ctxBounceCount++; },
});
assertEq(result4, false, "ctx onNeedsLogin blocks mount");
assertEq(ctxBounceCount, 1, "ctx.onNeedsLogin called");

// ---- Test 5: setAuthState / isAuthenticated round-trip ----
reset();
assertEq(isAuthenticated(), false, "starts unauthenticated");
setAuthState(true);
assertEq(isAuthenticated(), true, "set true propagates");
setAuthState("yes"); // truthy string
assertEq(isAuthenticated(), true, "truthy coerces to true");
setAuthState(0);
assertEq(isAuthenticated(), false, "falsy coerces to false");

// ---- Test 6: unknown page returns false, doesn't bounce ----
reset();
let unknownBounceCount = 0;
globalThis.window.__mc_mountLogin = () => { unknownBounceCount++; };
const result6 = mount("does-not-exist", null);
assertEq(result6, false, "unknown page returns false");
assertEq(unknownBounceCount, 0, "unknown page does NOT trigger login bounce");

// ---- Summary ----
console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) {
  console.error("FAILURES:");
  for (const f of failures) console.error("  " + f);
  process.exit(1);
}
