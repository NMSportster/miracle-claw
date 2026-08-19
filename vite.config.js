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
    emptyOutDir: true,
  },
});
