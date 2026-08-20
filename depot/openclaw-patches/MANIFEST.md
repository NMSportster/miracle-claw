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

## Adding a new patch

1. Drop the patch file under `depot/openclaw-patches/<subpath>/` matching
   where it should land under `src-tauri/resources/`.
2. For HTML appends: first line MUST be `<!-- MC-PATCH: <id> -->` so the
   patcher can detect already-applied state.
3. For JS/CSS writes: file is copied verbatim; idempotency via content
   comparison.
4. Update this MANIFEST with a description.

## Build hookup

`scripts/build-windows-docker.sh` must call `scripts/patch-openclaw-dist.sh`
AFTER `bundle-runtime.sh` and BEFORE the `docker run` that does cargo + NSIS.
Currently NOT hooked up (Lesson 501 follow-up).