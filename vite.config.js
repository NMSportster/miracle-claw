// Vite config for MC's first-run login page.
//
// Port 1420 matches Tauri's devUrl in tauri.conf.json. Strict port during
// dev so the Tauri webview can navigate to it predictably.

import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    // Lesson 836 (2026-09-09): scripts/prebuild-clean.mjs handles the
    // dist/ wipe before `vite build` runs. Set emptyOutDir=false here
    // so vite doesn't try to unlink files we don't own (which fails
    // with EACCES when dist/ was created by a previous root-owned
    // build — e.g. via sudo cargo tauri build). With emptyOutDir=false,
    // vite emits new files alongside the old ones; the prebuild-clean
    // step is the single source of truth for the dist/ lifecycle.
    emptyOutDir: false,
  },
});
