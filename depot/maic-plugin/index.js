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
// Hook surface used (one of six available; see Lesson 295):
//   extraParamsForTransport(ctx) → { patch?: Record<string, unknown> }
//
// Lifecycle: scanned at startup from `~/.openclaw/extensions/maic/` per
// `roots-BmJakFIf.js::resolvePluginSourceRoots` (workspace + global dirs).
// Manifest: `openclaw.plugin.json` (`PLUGIN_MANIFEST_FILENAME` constant).
// No dependencies on OpenClaw internals beyond the `register()` API.

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
  const providerParams = readRecord(
    ctx.config?.models?.providers?.[PROVIDER_ID]?.params
  );
  const modelParams = readRecord(ctx.model?.params);

  if (!providerParams && !modelParams) return undefined;

  // Special-case tool_execution: if neither layer explicitly sets it,
  // default to "client" so MAIC's partition_tool_calls returns unresolved
  // tool_calls for our client tools (see MAIC chat.py). If the user
  // explicitly sets it (provider-level OR model-level), their value wins —
  // no forced override.
  const explicitToolExecution =
    (modelParams && modelParams.tool_execution) ??
    (providerParams && providerParams.tool_execution);

  return {
    patch: {
      ...providerParams,
      ...modelParams,
      ...(explicitToolExecution !== undefined
        ? { tool_execution: explicitToolExecution }
        : { tool_execution: "client" }),
    },
  };
}

export default {
  id: PROVIDER_ID,
  name: "MAIC Provider",
  description:
    "Minimal MAIC provider plugin: injects tool_execution='client' and per-model extraParams into outbound OpenAI-compatible chat completion requests.",
  version: "0.1.0",
  register(api) {
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
  },
};