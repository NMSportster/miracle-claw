// Standalone smoke test for the MAIC plugin's extraParamsForTransport hook.
// Run: node test_plugin.js
//
// Verifies the function:
//   - Returns undefined when no params configured
//   - Reads provider-level params
//   - Reads per-model params
//   - Model-level overrides provider-level
//   - Always injects tool_execution: "client"
//   - Doesn't crash on malformed input

import { default as plugin, buildSandboxSystemContext } from "./index.js";

// Capture the provider object passed to registerProvider by calling register
// with a fake api. The provider's extraParamsForTransport hook is what we test.
let captured = null;
let beforePromptBuildFn = null;
const fakeApi = {
  registerProvider(provider) {
    captured = provider;
  },
  on(event, fn) {
    if (event === "before_prompt_build") beforePromptBuildFn = fn;
  },
};
plugin.register(fakeApi);
const fn = captured.extraParamsForTransport;

let pass = 0;
let fail = 0;
function check(label, cond, detail) {
  if (cond) {
    pass++;
    console.log(`✓ ${label}`);
  } else {
    fail++;
    console.error(`✗ ${label} — ${detail ?? ""}`);
  }
}

console.log("=== MAIC plugin extraParamsForTransport smoke tests ===\n");

// 1. No params anywhere → undefined
{
  const r = fn({ config: { models: { providers: {} } }, model: {} });
  check("returns undefined when no params configured", r === undefined, `got ${JSON.stringify(r)}`);
}

// 2. Provider-level params
{
  const r = fn({
    config: { models: { providers: { maic: { params: { foo: "bar" } } } } },
    model: {},
  });
  check(
    "reads provider-level params",
    r && r.patch && r.patch.foo === "bar",
    `got ${JSON.stringify(r)}`
  );
  check(
    "always injects tool_execution: 'client'",
    r && r.patch && r.patch.tool_execution === "client",
    `got ${r?.patch?.tool_execution}`
  );
}

// 3. Per-model params
{
  const r = fn({
    config: { models: { providers: {} } },
    model: { params: { baz: "qux" } },
  });
  check(
    "reads per-model params",
    r && r.patch && r.patch.baz === "qux",
    `got ${JSON.stringify(r)}`
  );
}

// 4. Model-level overrides provider-level
{
  const r = fn({
    config: { models: { providers: { maic: { params: { foo: "from-provider", shared: "p" } } } } },
    model: { params: { foo: "from-model", extra: "m" } },
  });
  check(
    "model-level overrides provider-level for same key",
    r && r.patch && r.patch.foo === "from-model",
    `got foo=${r?.patch?.foo}`
  );
  check(
    "unique provider-only keys still present",
    r && r.patch && r.patch.shared === "p",
    `got shared=${r?.patch?.shared}`
  );
  check(
    "unique model-only keys still present",
    r && r.patch && r.patch.extra === "m",
    `got extra=${r?.patch?.extra}`
  );
}

// 5. Malformed input — arrays, nulls, primitives
{
  const r1 = fn({ config: { models: { providers: { maic: { params: [1, 2, 3] } } } }, model: {} });
  check("ignores array params (treats as no provider params)", r1 === undefined || (r1?.patch && !Array.isArray(r1.patch)), `got ${JSON.stringify(r1)}`);

  const r2 = fn({ config: { models: { providers: { maic: { params: null } } } }, model: { params: null } });
  check("returns undefined when both params are null", r2 === undefined, `got ${JSON.stringify(r2)}`);

  const r3 = fn({ config: null, model: null });
  check("returns undefined when ctx is null/missing config & model", r3 === undefined, `got ${JSON.stringify(r3)}`);
}

// 6. Explicit tool_execution override at model level is preserved
{
  const r = fn({
    config: { models: { providers: { maic: { params: {} } } } },
    model: { params: { tool_execution: "client_only" } },
  });
  check(
    "model-level tool_execution override preserved",
    r && r.patch && r.patch.tool_execution === "client_only",
    `got tool_execution=${r?.patch?.tool_execution}`
  );
}

console.log("\n=== Lesson 516 sandbox system-context tests ===\n");

// 7. buildSandboxSystemContext returns a non-empty block that mentions all
//    four allowed roots by path-style, NOT by name only.
{
  const ctx = buildSandboxSystemContext();
  check("returns a non-empty string", typeof ctx === "string" && ctx.length > 200, `len=${ctx?.length}`);
  check("mentions Documents path", ctx.includes("Documents"), `ctx=${ctx?.slice(0, 120)}…`);
  check("mentions Desktop path",   ctx.includes("Desktop"),   `ctx=${ctx?.slice(0, 120)}…`);
  check("mentions Downloads path", ctx.includes("Downloads"), `ctx=${ctx?.slice(0, 120)}…`);
  check("mentions workspace dir",  ctx.includes("miracle-claw"), `ctx=${ctx?.slice(0, 120)}…`);
  check("warns about other Users dirs being blocked", ctx.includes("AppData") || ctx.includes("ProgramData"), `ctx=${ctx?.slice(0, 120)}…`);
  check("warns about bash cwd quirk",                  ctx.includes("bash") && ctx.includes("cwd"),          `ctx=${ctx?.slice(0, 120)}…`);
}

// 8. Plugin registers a before_prompt_build hook (api.on)
{
  check("register() wires up before_prompt_build hook", typeof beforePromptBuildFn === "function", `got ${typeof beforePromptBuildFn}`);
}

// 9. before_prompt_build hook returns { prependSystemContext }
{
  const r = await beforePromptBuildFn({ prompt: "hi", messages: [] }, {});
  check("returns object with prependSystemContext", r && typeof r.prependSystemContext === "string", `got ${JSON.stringify(r)}`);
  check("prependSystemContext mentions Documents",  r?.prependSystemContext?.includes("Documents"),  `ctx=${r?.prependSystemContext?.slice(0, 120)}…`);
}

console.log(`\n=== ${pass} passed, ${fail} failed ===`);
process.exit(fail > 0 ? 1 : 0);