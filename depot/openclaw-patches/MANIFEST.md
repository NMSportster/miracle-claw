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
| `dist/control-ui/mc-voice-native.js` | 706 | rc54.3 | Native Windows SAPI 5 voice capture (replaces OpenClaw's slow Realtime Talk cloud STT). |
| `dist/control-ui/mc-toolbar.js` | 978 | rc56.8 | Top-right floating toolbar in the OpenClaw chat UI: 📎 Attach (drop files, copy paths to clipboard), 🔑 Secrets (list local tool names — no values), 🎙 Voice (reuses SAPI5 path), 🧩 Modules (dropdown of installed modules + Run modal that calls `mc_run_module` and pastes the result into the chat input). Self-contained IIFE; gates on `location.host === '127.0.0.1:28789'`. |
| `dist/control-ui/mc-toolbar.css` | 978 | rc56.8 | Styles for the chat toolbar. All classes prefixed `mc-oc-` to scope under our toolbar. |
| `dist/control-ui/index.html.insert` | 978 | rc56.8 | `<script>` + `<link>` tags for the toolbar and the existing `mc-back-button.js`, inserted before `</body>`. Migrated the back-button script tag from append to insert so all control-ui additions share one idempotent path. |
| `dist/stream-BcRkg2P0.js` | 802 | rc55.13 | When neither caller nor model configures a `requestTimeoutMs`, fall back to 60s in `resolveModelRequestTimeoutMs` instead of returning `undefined`. Stops the cosmetic `timeoutMs=undefined` log spam on every chat. |