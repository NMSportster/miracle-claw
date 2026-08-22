// ~/.openclaw/extensions/maic/index.js
//
// Minimal MAIC provider plugin.
//
// Purpose: OpenClaw's request-body builder (`buildOpenAICompletionsParams` in
// `openai-transport-stream-D1R-kt0Q.js`) only knows how to inject fields its
// provider plugin explicitly registers. Without a plugin, config fields
// under `models.providers.maic.params` (or per-model `params`) sit in
// `openclaw.json` and never reach the wire.
//
// This plugin reads those `params` records and returns a `{ patch }` object
// that OpenClaw merges into the outbound request body. The MAIC backend
// honors `tool_execution: "client"` to keep client tools unexecuted and
// return them via `tool_calls` (see MAIC `milagro_handoff.assistant_message`
// wire format — Lesson 293).
//
// Hook surface used (Lesson 295 family + Lesson 516 NEW):
//   extraParamsForTransport(ctx) → { patch?: Record<string, unknown> }
//   before_prompt_build(event, ctx) → { prependSystemContext?: string }
//
// Lesson 516 (NEW, 2026-08-20): The agent had no clue what filesystem
// paths it could touch. Tool schema descriptions say "Documents/,
// Desktop/, Downloads/, or the workspace root" but never give the actual
// Windows paths. The model hallucinates ("only .openclaw/workspace")
// and refuses file tasks. Fix: prepend a sandbox-aware system context
// on every prompt build so the model knows exactly what it can/can't
// reach. Cached-friendly via prependSystemContext (vs prependContext).
//
// Plugin is loaded as ESM by OpenClaw's plugin loader — so use ESM
// `import` not CommonJS `require` (require() throws in pure ESM context,
// which silently breaks buildSandboxSystemContext). Lesson 516 sub-fix.
//
// Lifecycle: scanned at startup from `~/.openclaw/extensions/maic/` per
// `roots-BmJakFIf.js::resolvePluginSourceRoots` (workspace + global dirs).
// Manifest: `openclaw.plugin.json` (`PLUGIN_MANIFEST_FILENAME` constant).
// No dependencies on OpenClaw internals beyond the `register()` API.

console.log("[maic-plugin DEBUG] module top reached — file was loaded by Node, registering provider");

import os from "node:os";

const PROVIDER_ID = "maic";

function readRecord(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  return value;
}

/**
 * Read provider-level `params` and per-model `params` from openclaw.json
 * and return the union as a transport patch object.
 *
 * Precedence (matches OpenRouter's behavior):
 *   provider.params ⊕ model.params  (model-level overrides provider-level)
 *
 * Returns undefined if no params are configured (so OpenClaw skips the patch).
 */
function resolveMaicExtraParamsForTransport(ctx) {
  console.log("[maic-plugin DEBUG] extraParamsForTransport called", JSON.stringify({
    hasProviderParams: !!(ctx.config?.models?.providers?.[PROVIDER_ID]?.params),
    providerParamsKeys: Object.keys(ctx.config?.models?.providers?.[PROVIDER_ID]?.params ?? {}),
    hasModelParams: !!(ctx.model?.params),
    modelParamsKeys: Object.keys(ctx.model?.params ?? {}),
    modelId: ctx.modelId,
    configHasModels: !!ctx.config?.models,
    configHasProviders: !!ctx.config?.models?.providers,
    configHasMaic: !!ctx.config?.models?.providers?.[PROVIDER_ID]
  }));
  const providerParams = readRecord(
    ctx.config?.models?.providers?.[PROVIDER_ID]?.params
  );
  const modelParams = readRecord(ctx.model?.params);

  if (!providerParams && !modelParams) {
    console.log("[maic-plugin DEBUG] no params, returning undefined");
    return undefined;
  }

  // Special-case tool_execution: if neither layer explicitly sets it,
  // default to "client" so MAIC's partition_tool_calls returns unresolved
  // tool_calls for our client tools (see MAIC chat.py). If the user
  // explicitly sets it (provider-level OR model-level), their value wins —
  // no forced override.
  const explicitToolExecution =
    (modelParams && modelParams.tool_execution) ??
    (providerParams && providerParams.tool_execution);

  const patch = {
    ...providerParams,
    ...modelParams,
    ...(explicitToolExecution !== undefined
      ? { tool_execution: explicitToolExecution }
      : { tool_execution: "client" }),
  };
  console.log("[maic-plugin DEBUG] returning patch keys:", Object.keys(patch), "tool_execution=", patch.tool_execution, "tools count=", Array.isArray(patch.tools) ? patch.tools.length : "(not array)");
  return {
    patch,
  };
}

/**
 * Lesson 516: Build a sandbox-aware system context block that tells the
 * model exactly which filesystem paths it can reach and what it cannot.
 *
 * Why this exists:
 * - MC's Rust side enforces a path allowlist (Lesson 169 family):
 *   %USERPROFILE%\Documents, %USERPROFILE%\Desktop, %USERPROFILE%\Downloads,
 *   and MC-managed workspace dir under %LOCALAPPDATA%\miracle-claw\workspace.
 * - Tool schema descriptions say "under Documents/, Desktop/, Downloads/,
 *   or the workspace root" but never give concrete Windows paths.
 * - Without concrete paths, models hallucinate the sandbox scope (e.g.
 *   "I can only access .openclaw/workspace") and refuse valid file tasks.
 *
 * This block is injected via `prependSystemContext` (provider-cached, not
 * per-turn), so the model sees it on the first message and every turn
 * without re-tokenization cost.
 *
 * Kept static (no runtime env lookups) so the cache key stays stable
 * across requests — runs against MAIC's prompt-cache friendly.
 *
 * Returns empty string if we can't determine we're on Windows; the
 * plugin still emits the rest of its behavior.
 */
function buildSandboxSystemContext() {
  // Plugin is loaded as ESM (Lesson 516 sub-fix). Use top-level
  // `import os from "node:os"` (above) instead of `require("node:os")` —
  // the latter throws in pure ESM context and the try/catch below would
  // silently swallow it, returning empty paths and breaking the model.
  let home = "";
  let platform = "";
  let localAppData = "";
  try {
    platform = os.platform();
    home = os.homedir();
    if (platform === "win32") {
      localAppData = process.env.LOCALAPPDATA ||
        (home ? `${home}\\AppData\\Local` : "");
    }
  } catch (_e) {
    // os not available (e.g. test harness with mocked Node API) — fall
    // back to static paths so tests can still exercise the structure.
  }

  // Compute the MC workspace dir the same way Rust does in
  // src-tauri/src/tools/exec.rs::allowed_roots().
  let workspaceDir = "";
  if (platform === "win32" && localAppData) {
    workspaceDir = `${localAppData}\\miracle-claw\\workspace`;
  } else if (home) {
    workspaceDir = `${home}/.local/share/miracle-claw/workspace`;
  }

  const docs     = home ? (platform === "win32" ? `${home}\\Documents`     : `${home}/Documents`)     : "";
  const desktop  = home ? (platform === "win32" ? `${home}\\Desktop`       : `${home}/Desktop`)       : "";
  const downloads= home ? (platform === "win32" ? `${home}\\Downloads`     : `${home}/Downloads`)     : "";

  return `## MC Filesystem Sandbox (MiracleClaw 1.0.9-rc19+)

You are running inside the MiracleClaw desktop app. Filesystem access is
sandboxed by Rust at the tool layer (not by prompt convention). When a tool
call returns "path outside allowed paths", the sandbox rejected the path —
not the model. Pick a different path inside the allowlist.

### Allowed paths (read/write/execute)

- \`${docs}\`
- \`${desktop}\`
- \`${downloads}\`
- \`${workspaceDir}\`  ← MC-managed scratch space

### Not allowed (sandbox will reject)

- Other folders under \`C:\\Users\\\` (AppData, ProgramData, .ssh, .aws,
  Windows, Program Files, etc.)
- Other drives (\`D:\\\`, \`E:\\\`, network mounts)
- WSL native paths (\`/home/...\`, \`/etc/...\`)
- Relative paths (always pass absolute Windows paths)

### Path style

- Use Windows-style backslash paths: \`C:\\Users\\...\\Documents\\foo.txt\`
- WSL-style \`/mnt/c/Users/...\` is auto-converted to Windows form before
  the sandbox check, so either form works
- \`~\` is NOT expanded — pass the full path
- If a directory listing returns zero entries, the sandbox blocked the
  path; surface the error to the user rather than guessing

### Bash default CWD

- bash_run with no \`cwd\` runs in the process's current directory
  (NOT the workspace — this is a known quirk; pass \`cwd\` explicitly)
- Recommended: pass \`cwd: "${workspaceDir}"\` for shell commands`;
}

/**
 * Lesson 516: before_prompt_build hook. Returns the sandbox context to
 * prepend to the system prompt. Uses prependSystemContext (cached) not
 * prependContext (per-turn) so we don't pay token cost on every turn.
 */
async function onBeforePromptBuild(_event, _ctx) {
  return {
    prependSystemContext: buildSandboxSystemContext(),
  };
}

export default {
  id: PROVIDER_ID,
  name: "MAIC Provider",
  description:
    "Minimal MAIC provider plugin: injects tool_execution='client', per-model extraParams, and a filesystem-sandbox system context (Lesson 516) into outbound OpenAI-compatible chat completion requests.",
  version: "0.2.0",
  register(api) {
    console.log("[maic-plugin DEBUG] register() called — MAIC provider plugin v0.2.0 loading");
    api.registerProvider({
      id: PROVIDER_ID,
      label: "MAIC",
      docsPath: "/providers/models",
      envVars: [],
      // No auth method here — `models.providers.maic.apiKey` in
      // openclaw.json is sufficient (the default OpenAI-compat key path
      // picks it up via `Authorization: Bearer <key>`).
      extraParamsForTransport: resolveMaicExtraParamsForTransport,
    });
    // Lesson 516: inject sandbox-aware system context on every prompt.
    // Cheap, cached, no per-turn token cost.
    api.on("before_prompt_build", onBeforePromptBuild);
    console.log("[maic-plugin DEBUG] register() complete — provider + hook registered");
  },
};

// Exported for test_plugin.js so we can unit-test the sandbox builder
// without spinning up the full OpenClaw plugin host.
export { buildSandboxSystemContext };