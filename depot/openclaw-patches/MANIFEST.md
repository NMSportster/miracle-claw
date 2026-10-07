# OpenClaw dist patches

Each file in this directory (except MANIFEST.md) is applied by
`scripts/patch-openclaw-dist.sh` to the freshly-extracted openclaw dist
in `src-tauri/resources/dist/`. bundle-runtime.sh blows those files
away every time it re-extracts the openclaw tarball, so this is the
only durable home for our edits to vendored openclaw code.

## How to add a new patch

1. Place the patched file under `dist/<subpath>/<filename>`.
   `scripts/patch-openclaw-dist.sh` copies it over
   `src-tauri/resources/<subpath>/<filename>` (full overwrite, content
   compared for idempotency).
2. Document it here with the Lesson number, rc version, and a brief
   rationale.
3. Test the patch with `bash scripts/patch-openclaw-dist.sh --dry-run`.
4. Commit the patch file (and MANIFEST entry). The patch files live in
   `depot/`, which is .gitignored at the project root, but the
   patches ARE tracked. If you find they aren't, fix `.gitignore`
   exception rules — the patches are part of the source tree.

## Index

| Patch | Lesson | rc | Description |
|---|---|---|---|
| `dist/control-ui/mc-back-button.js` | 502/511 | rc53.4 | Adds a "← Dashboard" button to the top-left of the chat UI when hosted in MC's Tauri webview. Detection: `window.location.host === '127.0.0.1:28789'` (Lesson 511 IPC-independent gate). Click: `window.location.href = 'tauri://localhost/index.html'`. Previously inlined as a script block in `index.html` (append-patch); migrated to a separate file in Lesson 978 so the patcher can also wipe it on `bundle-runtime.sh --force` and the `<script src=...>` reference works. |
| `dist/control-ui/mc-voice-native.js` | 1001/1095 | rc57.5 | Chat voice button pivot from SAPI 5 (`mc_voice_native_capture`, dead on this Windows install) to `mc_module_call('mc_voice_transcribe', ...)` -> whisper.cpp sidecar. Auto-checks model, auto-downloads ggml-tiny.en.bin (~75 MB) on first click via friendly toast. Returns {text, duration_ms, transcribe_ms, rms, diagnostics} to JS via dispatcher (envelope unwrapped). Error mapping: NotInstalled|sidecar not found|module 'voice' regex -> "Voice module not installed. Open Settings -> Modules to install it." toast. Lesson 1095 (2026-10-01, David): replaced vague 'No speech detected' overlay with diagnostics-aware copy (peak amplitude + VAD stats disambiguate mic-silent vs VAD-too-strict vs whisper-rejected), classified download errors (HTTP/TLS/DNS/SHA/fs/sidecar), reset voiceReadyCache on failure so next click retries. Replaces Lesson 706 (rc54.3).
| `dist/control-ui/mc-toolbar.js` | 1001/1109 | rc57.7 | Top-right floating toolbar in the OpenClaw chat UI. Attach / Secrets / Modules Run (from Lesson 978). Voice button (was hardcoded to call broken SAPI5 path in Lesson 978) now uses the same `mc_module_call('mc_voice_transcribe', ...)` pivot as mc-voice-native.js above (Lesson 1001), sharing a `ensureVoiceReady()` cached helper. Self-contained IIFE; gates on `location.host === '127.0.0.1:28789'`. Emoji rendered as text ([attach] [key] [mic] [modules]) so the file stays ASCII for the Lesson-998 patch-tool byte-match. Lesson 1109 (2026-10-02, David): fetchInstalledModulesMerged() now also calls mc_module_list (in-memory registry) when MAIC fails or returns empty, so the dropdown never goes blank. Local-only items get an (offline copy) tag in the meta line. Replaces Lesson 975.
| `dist/control-ui/mc-toolbar.css` | 978 | rc56.8 | Styles for the chat toolbar. All classes prefixed `mc-oc-` to scope under our toolbar. |
| `dist/control-ui/index.html.insert` | 978 | rc56.8 | `<script>` + `<link>` tags for the toolbar and the existing `mc-back-button.js`, inserted before `</body>`. Migrated the back-button script tag from append to insert so all control-ui additions share one idempotent path. |
| `dist/stream-BcRkg2P0.js` | 802 | rc55.13 | When neither caller nor model configures a `requestTimeoutMs`, fall back to 60s in `resolveModelRequestTimeoutMs` instead of returning `undefined`. Stops the cosmetic `timeoutMs=undefined` log spam on every chat. |

| Patch | Lesson | rc | Description |
|---|---|---|---|
| `dist/selection-JInn13lc.js.replace` | 1127 | rc57.10 | Extend `OLLAMA_INCOMPLETE_TURN_PROVIDER_ID_PATTERN` from `/^ollama(?:-|$)/` to `/^(?:ollama\|maic)(?:-|$)/` so the embedded-agent retry guards (resolveReasoningOnlyRetryInstruction, resolveEmptyResponseRetryInstruction, isIncompleteTurnRecoverySupportedProviderModel) fire for `maic/*` providers when `stopReason=length` happens. Required `scripts/patch-openclaw-dist.sh` to learn a new `.replace` mode (one-line sed-equivalent via awk: sentinel+replacement, marker-based idempotency). Used by RC-7's MAIC provider; without this, every `stopReason=length` falls through to the "⚠️ Agent couldn't generate a response" error path with zero retries. Replaces Lesson 509 (rc54) on paths the MAIC provider exercises. |
