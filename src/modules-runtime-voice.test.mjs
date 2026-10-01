// modules-runtime-voice.test.mjs — Lesson 1095 (2026-10-01, David)
//
// Unit tests for ensureVoiceModel + voiceModelFailureToast.
// We stub window/localStorage and a fake invokeModule so the
// tests run in plain Node (no Tauri runtime required).
//
// Run: node src/modules-runtime-voice.test.mjs

import assert from "node:assert/strict";

// ---- Test harness -----------------------------------------------------------

// Minimal window stub. ensureVoiceModel doesn't touch window directly
// but we set one up so importing the module doesn't crash on the
// `typeof window === "undefined"` check.
globalThis.window = { localStorage: { getItem: () => null, setItem: () => {} } };
globalThis.window.__TAURI__ = {
  core: {
    invoke: async () => ({ ok: true }),
  },
  event: {
    listen: async () => () => {},
  },
};

// We need to import AFTER setting up globals, and we need to mock
// the module side-effects (Tauri dispatch) since we don't want the
// real invokeModule here.
//
// `invokeModule(command, params)` calls `mc_module_call(command, params)`
// under the hood — so the mock intercepts at that boundary.
let invokeModuleImpl = async () => ({ ok: true, model_loaded: false });

globalThis.window.__TAURI__.core.invoke = async (cmd, args) => {
  if (cmd === "mc_module_call") {
    return invokeModuleImpl(args?.command, args?.params);
  }
  // events etc.
  return {};
};

// Now import the module under test.
const { ensureVoiceModel, voiceModelFailureToast } = await import("./modules-runtime.js");

// ---- Helpers ---------------------------------------------------------------

function makeHealthOk() {
  return {
    model_loaded: true,
    model_path: "C:\\Users\\Adeal\\.cache\\whisper.cpp\\ggml-tiny.en.bin",
  };
}
function makeHealthMissing() {
  return { model_loaded: false, model_path: null };
}

// ---- Tests -----------------------------------------------------------------

async function test_already_loaded_skips_download() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthOk();
    throw new Error("download_model should NOT be called when model is loaded");
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, true, "should return ok when model is loaded");
  assert.equal(result.model_loaded, true);
  assert.equal(result.model_path.includes("ggml-tiny.en.bin"), true);
  console.log("PASS: already_loaded_skips_download");
}

async function test_download_succeeds_returns_ok() {
  let checkCount = 0;
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") {
      checkCount++;
      // First call: missing. Second call (post-download): loaded.
      return checkCount === 1 ? makeHealthMissing() : makeHealthOk();
    }
    if (cmd === "mc_voice_download_model") return { ok: true };
    throw new Error("unexpected cmd: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, true, "should return ok after successful download");
  console.log("PASS: download_succeeds_returns_ok");
}

async function test_http_error_classified_as_http() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("HTTP error: 403 Forbidden");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "http");
  assert.match(result.message, /HTTP/);
  console.log("PASS: http_error_classified_as_http");
}

async function test_tls_error_classified_as_tls() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("TLS handshake failed: certificate verify failed");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "tls");
  assert.match(result.message, /TLS/);
  console.log("PASS: tls_error_classified_as_tls");
}

async function test_dns_error_classified_as_dns() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("DNS error: could not resolve host milagrocloud.com");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "dns");
  console.log("PASS: dns_error_classified_as_dns");
}

async function test_sha_error_classified_as_sha() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("sha256 mismatch: expected 921e4cf..., got deadbeef...");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "sha");
  console.log("PASS: sha_error_classified_as_sha");
}

async function test_sidecar_missing_classified_as_sidecar() {
  // Both the health check AND the download must fail with the same
  // sidecar-missing error: health check reports not-loaded, download
  // attempt reveals the sidecar isn't installed.
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("sidecar not found at C:\\missing\\voice.exe");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "sidecar");
  assert.match(result.message, /Settings.*Modules/);
  console.log("PASS: sidecar_missing_classified_as_sidecar");
}

async function test_fs_error_classified_as_fs() {
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") {
      throw new Error("permission denied writing to C:\\Users\\Adeal\\.cache");
    }
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "fs");
  console.log("PASS: fs_error_classified_as_fs");
}

async function test_download_but_model_still_missing() {
  let checkCount = 0;
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") {
      checkCount++;
      return checkCount === 1 ? makeHealthMissing() : makeHealthMissing();
    }
    if (cmd === "mc_voice_download_model") return { ok: true };
    throw new Error("unexpected: " + cmd);
  };
  const result = await ensureVoiceModel(() => {});
  assert.equal(result.ok, false);
  assert.equal(result.reason, "missing");
  assert.match(result.message, /not on disk/i);
  console.log("PASS: download_but_model_still_missing");
}

async function test_status_callback_invoked() {
  const statusCalls = [];
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") return makeHealthMissing();
    if (cmd === "mc_voice_download_model") return { ok: true };
    throw new Error("unexpected: " + cmd);
  };
  let checkCount = 0;
  invokeModuleImpl = async (cmd) => {
    if (cmd === "mc_voice_check") {
      checkCount++;
      return checkCount === 1 ? makeHealthMissing() : makeHealthOk();
    }
    if (cmd === "mc_voice_download_model") return { ok: true };
    throw new Error("unexpected: " + cmd);
  };
  await ensureVoiceModel((s) => statusCalls.push(s));
  assert.ok(statusCalls.length >= 2, "expected multiple status callbacks");
  assert.match(statusCalls[0], /Checking whisper model/);
  assert.ok(
    statusCalls.some((s) => /Whisper model ready/.test(s)),
    "expected a 'Whisper model ready' status"
  );
  // Size label should reference 75 MB (tiny.en), NOT 141 MB (base.en).
  assert.ok(
    statusCalls.some((s) => /75 MB/.test(s)),
    "expected size label to say ~75 MB (tiny.en), got: " + JSON.stringify(statusCalls)
  );
  assert.ok(
    !statusCalls.some((s) => /141 MB/.test(s)),
    "size label should NOT say ~141 MB (legacy base.en)"
  );
  console.log("PASS: status_callback_invoked");
}

async function test_voiceModelFailureToast_returns_null_for_ok() {
  const t = voiceModelFailureToast({ ok: true });
  assert.equal(t, null);
  console.log("PASS: voiceModelFailureToast_returns_null_for_ok");
}

async function test_voiceModelFailureToast_http_has_retry_action() {
  const t = voiceModelFailureToast({ ok: false, reason: "http", message: "..." });
  assert.equal(t.actionLabel, "Retry download");
  assert.equal(t.kind, "error");
  console.log("PASS: voiceModelFailureToast_http_has_retry_action");
}

async function test_voiceModelFailureToast_sidecar_has_open_modules_action() {
  const t = voiceModelFailureToast({ ok: false, reason: "sidecar", message: "..." });
  assert.equal(t.actionLabel, "Open Settings → Modules");
  assert.equal(t.kind, "warn");
  console.log("PASS: voiceModelFailureToast_sidecar_has_open_modules_action");
}

async function test_voiceModelFailureToast_unknown_falls_through() {
  const t = voiceModelFailureToast({ ok: false, reason: "weird_new_thing", message: "..." });
  assert.equal(t.actionLabel, "Retry download");
  console.log("PASS: voiceModelFailureToast_unknown_falls_through");
}

// ---- Run all ---------------------------------------------------------------

const tests = [
  test_already_loaded_skips_download,
  test_download_succeeds_returns_ok,
  test_http_error_classified_as_http,
  test_tls_error_classified_as_tls,
  test_dns_error_classified_as_dns,
  test_sha_error_classified_as_sha,
  test_sidecar_missing_classified_as_sidecar,
  test_fs_error_classified_as_fs,
  test_download_but_model_still_missing,
  test_status_callback_invoked,
  test_voiceModelFailureToast_returns_null_for_ok,
  test_voiceModelFailureToast_http_has_retry_action,
  test_voiceModelFailureToast_sidecar_has_open_modules_action,
  test_voiceModelFailureToast_unknown_falls_through,
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
    if (e.stack) console.error(e.stack);
  }
}
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
