# OpenClaw dist patches

MiracleClaw-specific patches applied to the openclaw dist that lives under
`src-tauri/resources/`. The dist is re-extracted from the cached npm tarball
on every `bundle-runtime.sh --force` run, so these patches MUST be applied
by `scripts/patch-openclaw-dist.sh` as part of `build-windows-docker.sh`.

## Patch inventory

### dist/control-ui/index.html (APPEND)

- **Marker**: `<!-- MC-PATCH: mc-back-button -->`
- **Effect**: Appends a `<script type="module" src="./mc-back-button.js"></script>`
  tag to the chat UI's `index.html`. Runs after the chat UI's own scripts.
- **Why**: Lesson 502 — fallback back-button that uses `window.location.href`
  instead of Tauri IPC. Independent of the bridge pill (which uses invoke).
- **Idempotent**: re-running the patcher skips because the marker is already
  present in the target file.

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