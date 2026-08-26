# OpenClaw dist patches

MiracleClaw-specific patches applied to the openclaw dist that lives under
`src-tauri/resources/`. The dist is re-extracted from the cached npm tarball
on every `bundle-runtime.sh --force` run, so these patches MUST be applied
by `scripts/patch-openclaw-dist.sh` as part of `build-windows-docker.sh`.

## Patch inventory

### dist/control-ui/mc-back-button.js (WRITE)

- **Source**: `depot/openclaw-patches/dist/control-ui/mc-back-button.js`
- **Target**: `src-tauri/resources/dist/control-ui/mc-back-button.js`
- **Effect**: Adds a "← Dashboard" button to the chat UI only when
  `window.__openclawHostBridge.hosted === true` AND the page host is
  `127.0.0.1:28789` (the openclaw gateway). Click does
  `window.location.href = 'tauri://localhost/index.html'`.
- **Why**: Belt-and-suspenders to the bridge pill. If Tauri invoke silently
  fails on the cross-origin chat page (Lesson 491/495 root cause), this
  button uses plain DOM navigation, which WebView2 follows natively.
- **Idempotent**: skips if target file content already matches.

### dist/control-ui/mc-chat-toolbar.js (WRITE)

- **Source**: `depot/openclaw-patches/dist/control-ui/mc-chat-toolbar.js`
- **Target**: `src-tauri/resources/dist/control-ui/mc-chat-toolbar.js`
- **Effect**: Adds a 🔑 Secrets + 📎 Attach floating toolbar at the
  **top-right** of the chat UI when `window.location.host ===
  '127.0.0.1:28789'` (Lesson 511 — IPC-INDEPENDENT detection). Each
  click does
  `window.location.href = 'tauri://localhost/index.html#mcAutoOpen=<key>'`.
- **Why**: Lesson 243 (rc53.9). David asked 2026-08-23 to mirror the
  terminal toolbar's 🔑 + 📎 buttons onto the OpenClaw chat topbar's
  right side. The chat topbar is a vendored React bundle we don't
  fork, so we render a floating overlay at fixed top-right that LOOKS
  attached. Mirrors the IPC-INDEPENDENT navigation trick used by
  `mc-back-button.js`. The `#mcAutoOpen=<key>` URL hash carries the
  intent across the cross-origin boundary (127.0.0.1:28789 →
  tauri://localhost have separate localStorage, so URL hash is the
  only signal that survives); MC's main.js boot reads the hash,
  routes straight to Terminal with `autoOpenOverlay` extras; the
  Terminal mount consumes the hash, opens the matching overlay, and
  strips the hash from the URL.
- **Idempotent**: skips if target file content already matches.

### dist/control-ui/index.html (APPEND) — extended for rc53.9

- **Marker**: `<!-- MC-PATCH: mc-back-button -->`
- **Effect**: Appends two `<script type="module">` tags before
  `</body>`: `./mc-back-button.js` and `./mc-chat-toolbar.js`.
- **Why**: Lesson 243 — bundling the rc53.9 toolbar load alongside
  the existing back-button load keeps the patch surface area minimal
  (one insert file, one marker) and the runtime order matches the
  source file order.
- **Idempotent**: re-running the patcher skips because the marker is
  already present in the target file.

### dist/openai-transport-stream-B0WkSqXp.js (INSERT)

- **Source**: `depot/openclaw-patches/dist/openai-transport-stream-B0WkSqXp.js.insert`
- **Target**: `src-tauri/resources/dist/openai-transport-stream-B0WkSqXp.js`
  (last verified openclaw 2026.7.1-2)
- **Effect**: Inserts 7 lines (plus comments) right before the `return params;`
  that closes `buildOpenAICompletionsParams`. The inserted block reads
  `model.params.tool_execution` and, if it is `"client"` / `"server"` /
  `"client_only"`, copies it onto the request params body. Reinstates the
  steeler-era MAIC compat patch (openclaw 2026.6.8 had this; 2026.7.1
  dropped it during the `buildOpenAICompletionsParams` rewrite).
- **Why**: Lesson 534. The maic plugin's `extraParamsForTransport` hook
  returns `{tool_execution: "client", ...}`. This patch is correctly merged
  into `effectiveExtraParams` by `extra-params-cce1g0up.js`, but
  `createStreamFnWithExtraParams` whitelists only temperature / topP /
  maxTokens / responseFormat / etc. through to the underlying stream
  options — `tool_execution` is dropped on the floor before it reaches
  `buildOpenAICompletionsParams`. Without this patch, MAIC requests fire
  without the flag, MAIC defaults to `tool_execution: "server"`, and the
  model only sees the 4-5 MAIC server tools (weather, web_search, time,
  calculate, describe_image). With this patch, MAIC gets `client` mode and
  the model sees 11 tools (4 server + 7 client).
- **Idempotent**: skips if `// MC-PATCH: maic-tool-execution-compat` is
  already present in the target file. The patch file format is:
  - Line 1: `// MC-PATCH: <id>` (idempotency marker)
  - Line 2: `// INSERT-BEFORE: <exact sentinel line>` (target line)
  - Lines 3+: patch contents (inserted verbatim, including the `if` block)
- **Filename drift**: if openclaw bumps the file hash (B0WkSqXp → ABcdEFgh
  etc.), rename this patch file to match. The sentinel is structurally
  unique to the end of `buildOpenAICompletionsParams`, so the line text
  should survive most refactors — but the file hash itself is part of the
  filename (patcher targets by exact name).
- **Patch mode `*.js.insert`** is new in Lesson 534. Implemented in
  `scripts/patch-openclaw-dist.sh` lines ~232-300. Tests:
  - `grep -F` sentinel in target: must match exactly once (skips if zero
    matches = openclaw bundle restructured, or > 1 match = ambiguous).
  - First line must be `// MC-PATCH: <id>` (regex enforced).
  - Second line must be `// INSERT-BEFORE: <text>` (regex enforces single
    space separator so tabs in sentinel are preserved).

### dist/session-log-runtime-BUZsJRws.js (WRITE) — added for rc53.23

- **Source**: `depot/openclaw-patches/dist/session-log-runtime-BUZsJRws.js`
- **Target**: `src-tauri/resources/dist/session-log-runtime-BUZsJRws.js`
- **Effect**: Replaces the three `throw new Error(...)` messages inside
  `resolveConfiguredRealtimeVoiceProvider` (the function that the Discord
  realtime voice stack calls when connecting). Original messages are
  terse ("Realtime voice provider 'X' is not configured") and don't tell
  the user what subsystem failed or where to look. New messages:
  - `missing-configured-provider`: tells user the realtime voice stack
    (Discord/voice-call) needs `voice.realtime.provider` set to a
    registered id, and points at the MC voice module for push-to-talk.
  - `no-registered-provider` (default fallback): same MC voice module hint.
  - generic "not configured": tells user to fix the provider block in
    `voice.realtime.providers` or switch `voice.realtime.provider`.
- **Why**: Users (incl. David) saw the bare "Realtime voice provider 'openai'
  is not configured" toast and had no idea whether it was a config bug,
  a missing plugin, or a MC-specific issue. The new messages give them
  three actionable next steps. They also point at the MC voice module so
  users who wanted push-to-talk chat don't waste time debugging the
  Discord realtime-voice stack.
- **Idempotent**: skipped if `cmp -s` finds the target already matches
  the patch source (WRITE mode).
- **Bundle hash drift**: file hash `BUZsJRws` is part of the filename —
  if openclaw bumps it, rename this patch file to match the new target
  hash. The three throw lines are likely stable across refactors but if
  upstream changes them substantially, re-export and re-patch.
- **Scope note**: this is Phase 2a — friendlier error message only. It
  does NOT register the `openai` realtime provider or make MC voice
  work in Discord voice mode. MC voice remains a push-to-talk chat module
  (the 🎙 button), not a Discord realtime streaming provider.

## Adding a new patch

1. Drop the patch file under `depot/openclaw-patches/<subpath>/` matching
   where it should land under `src-tauri/resources/`.
2. For HTML appends: first line MUST be `<!-- MC-PATCH: <id> -->` so the
   patcher can detect already-applied state.
3. For JS/CSS writes: file is copied verbatim; idempotency via content
   comparison.
4. For JS inserts (Lesson 534): first line MUST be `// MC-PATCH: <id>`,
   second line MUST be `// INSERT-BEFORE: <exact target line>` (single space
   separator, sentinel itself is matched verbatim including leading tabs).
5. Update this MANIFEST with a description.

## Build hookup

`scripts/build-windows-docker.sh` calls `scripts/patch-openclaw-dist.sh`
AFTER `bundle-runtime.sh` and BEFORE the `docker run` that does cargo +
NSIS. Wired up in Lesson 502 / hardened in Lesson 534.