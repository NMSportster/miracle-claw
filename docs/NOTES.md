# Miracle Claw — Architecture Notes

This file holds long-lived architectural facts about Miracle Claw (MC) that don't
fit cleanly into CHANGELOG entries or AGENTS.md. If you're new to MC, read this
before debugging picker / chat / terminal issues.

---

## MC embeds OpenClaw. OpenClaw runs BOTH Terminal and Web Chat.

**Miracle Claw is a Tauri shell around an embedded OpenClaw runtime.** MC's
`dist/` (built by Vite) is JUST the MC dashboard / splash / module UI. It does
NOT contain any model picker code, any chat backend code, or any terminal code.

The chat dropdown, the chat input, the agent picker, the `/models` TUI command,
and all chat/terminal logic come from OpenClaw's vendored bundle.

### What runs where

| Surface          | Rendered by                  | How MC exposes it                           |
|------------------|------------------------------|---------------------------------------------|
| MC Dashboard     | MC's `dist/index.html`       | Tauri main webview (`lib.rs:3546`)          |
| Module Settings  | MC's `dist/index.html`       | Same webview, internal routing              |
| **Chat**         | **OpenClaw's webchat**       | **Webview navigates to `http://127.0.0.1:28789/`** |
| **Terminal**     | **OpenClaw's TUI**           | **`node openclaw.mjs tui --local` in xterm** |

### The embedded gateway

MC starts `node openclaw.mjs` as a sidecar process (lib.rs:4156
`start_gateway_after_login`) and exposes it on `http://127.0.0.1:28789/`. The
embedded OpenClaw gateway serves BOTH the webchat (at `/`) and the TUI (when
launched via `tui --local` from MC-Terminal).

**Two key env vars** (set in `lib.rs:5957-5968`):
- `OPENCLAW_CONFIG_PATH` → `<HOME>/.miracle-claw/openclaw.json` (Linux) or
  `<APPDATA>/MiracleClaw/openclaw.json` (Windows)
- `OPENCLAW_STATE_DIR` → `<HOME>/.miracle-claw` (Linux) or
  `<APPDATA>/MiracleClaw` (Windows)

These tell the embedded OpenClaw to read MC's config and write to MC's state
directory instead of its own defaults.

### Agent directory layout

```
<STATE_DIR>/
  openclaw.json                # main config
  extensions/
    maic/                       # MAIC plugin (LIVE source-of-truth)
      index.js
      openclaw.plugin.json
  agents/
    main/
      agent/                    # default agent
        models.json             # configured catalog
        <encoded_plugin_id>.models.json   # plugin sidecars
      last_session.json
      sessions/
```

The `encoded_plugin_id` for MAIC is base64-ish encoding of the plugin id
(`encodeURIComponent(btoa(pluginId))`, see
`plugin-model-catalog-C26wDCJp.js:43`).

### TUI vs Web Chat — same engine, different frontends

Both UIs hit the same OpenClaw runtime:

- **Web Chat** (`http://127.0.0.1:28789/`): browser-side webchat from
  `resources/dist/webchat-*.js`. Picker dropdown comes from
  `model-picker-B-Hnysol.js`.
- **Terminal** (`node openclaw.mjs tui --local`): TUI in xterm, rendered by
  `tui-ttOZNpsl.js`. For `--local` mode, uses `EmbeddedTuiBackend`
  (`embedded-backend-DungnrvS.js`) which reads `loadEmbeddedTuiModelCatalog`
  → `loadGatewayModelCatalog`. `/models` command opens the picker
  (`openModelSelector` at `tui-ttOZNpsl.js:2043`).

Both backends call into `loadModelCatalog`
(`model-catalog-BfvH9gPq.js:326`) → `ModelRegistry.create(authStorage, agentDir)`
→ reads `<agentDir>/models.json` + plugin sidecars. **Same catalog source.**

### Editing rules

- **For picker / catalog / chat dropdown bugs**: edit `resources/dist/`
  (OpenClaw's vendored bundle). `npx vite build` alone won't help — that's
  MC's dashboard only.
- **For dashboard / module / splash bugs**: edit `src/`, run `npx vite build`
  to update `dist/`. Then `npm run tauri build` to bake into the installer.
- **For Rust sidecar / Tauri commands**: edit `src-tauri/src/`, run
  `npm run tauri build`.
- **For MAIC plugin logic (chat backend, model routing)**: edit
  `$HOME/.openclaw/extensions/maic/` (LIVE), then
  `bash scripts/bundle-runtime.sh --force` to refresh depot + resources.
  **Verify** `diff -q depot/maic-plugin $HOME/.openclaw/extensions/maic` is
  empty BEFORE building the installer (Lesson 815).

### Stale-cache gotcha (Lesson 820)

If the embedded OpenClaw gateway is already running from a previous install
and the user installs a new version, the running gateway process keeps the
**stale catalog**. The picker may show the old model list until the gateway
restarts. Force-restart MC (close the app and reopen, or
`taskkill /F /IM node.exe && reopen MC` on Windows) to pick up changes.

---

## agents.defaults schema contract (Lesson 829, rc55.15)

OpenClaw's `AgentDefaultsSchema` (in `resources/dist/zod-schema-O9ml_nmo.js`,
line 120+) defines which fields are valid at `agents.defaults`. The schema is
**`.strict()`** — any unknown keys cause validation to fail with
`InvalidConfigError: agents.defaults: Invalid input`, and the gateway refuses
to boot.

Allowed keys (verified 2026-08-30): `params`, `model`, `utilityModel`,
`imageModel`, `imageGenerationModel`, `videoGenerationModel`,
`musicGenerationModel`, `voiceModel`, `mediaGenerationAutoProviderFallback`,
`pdfModel`, `pdfMaxBytesMb`, `pdfMaxPages`, `models`, `workspace`, `skills`,
`silentReply`, `repoRoot`, `promptOverlays`, `skipBootstrap`,
`skipOptionalBootstrapFiles`, `contextInjection`, `bootstrapMaxChars`,
`bootstrapTotalMaxChars`, `experimental`, `bootstrapPromptTruncationWarning`,
`userTimezone`, `startupContext`, `subagents`, `humanDelay`, `timeoutSeconds`,
`mediaMaxMb`, `imageMaxDimensionPx`, `imageQuality`, `typingIntervalSeconds`,
`typingMode`, `heartbeat`, `maxConcurrent`, `tts`, `contextLimits`,
`contextTokens`, `message`, `tools`, `toolProgressDetail`, `reasoningDefault`,
`fastModeDefault`, `skillsLimits`, `verboseDefault`, `thinkingDefault`,
`blockStreamingDefault`, `blockStreamingBreak`, `blockStreamingChunk`,
`blockStreamingCoalesce`, `runs`, `sandbox`, `identity`, `groupChat`,
`runRetries`, `embeddedAgent`.

**NOT in the schema**: `agents.defaults.fallbacks` (top-level). The
fallback list MUST live inside `model.fallbacks[]`:
```json
{
  "agents": {"defaults": {
    "model": {
      "primary": "maic/milagro-oc-glm",
      "fallbacks": ["maic/milagro-oc-minimax", "maic/milagro-oc-kimi", "maic/milagro-m1-t3"]
    }
  }}
}
```

Lesson 800 (rc55.13) attempted to migrate to a flat schema with top-level
fallbacks — that was wrong and was reversed in Lesson 829 (rc55.15). The
writer now: (a) seeds the OBJECT form, (b) rewrites bare ids in `model.primary`
+ `model.fallbacks[]` with the `maic/` prefix (Lesson 824), (c) lifts any
stray top-level `fallbacks` into `model.fallbacks[]` (auto-repair of
rc55.14-on-disk files) and strips the invalid top-level key.

---

## Cross-references

- `CHANGELOG.md` — version-by-version release notes (RC55.x fixes live here)
- `docs/2026-08-30-litellm-to-maic-migration.md` — routing fix from RC55.10
- `docs/BRANDING.md` — naming, voice, packaging decisions
- `depot/openclaw-patches/MANIFEST.md` — patches against vendored OpenClaw
- `../MEMORY.md` Lessons 816-821 — picker schema, env wiring, model fallback chain