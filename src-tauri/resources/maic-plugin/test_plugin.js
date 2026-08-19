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

import { default as plugin } from "./index.js";

// Capture the provider object passed to registerProvider by calling register
// with a fake api. The provider's extraParamsForTransport hook is what we test.
let captured = null;
const fakeApi = {
  registerProvider(provider) {
    captured = provider;
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

console.log(`\n=== ${pass} passed, ${fail} failed ===`);
process.exit(fail > 0 ? 1 : 0);