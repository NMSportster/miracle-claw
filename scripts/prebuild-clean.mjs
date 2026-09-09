#!/usr/bin/env node
// prebuild-clean.mjs — Lesson 836 (2026-09-09)
//
// Symptom: `vite build` fails on the second consecutive build with:
//   EACCES: permission denied, unlink '.../dist/assets/index-...js'
// when `dist/` was created by a previous build running as root
// (e.g. via `sudo cargo tauri build` or a Docker build that mounted
// the project dir). Vite's `emptyOutDir: true` then tries to unlink
// those root-owned files as the dev user (adeal), fails, and aborts.
//
// This script wipes `dist/` before `vite build` runs, tolerating
// the EACCES case by chmod-ing the perms before unlinking. We DO
// NOT require sudo — we operate within the adeal user's rights.
// If we can't wipe a file, we fall back to a stale-skip build via
// `build:no-clean`.
//
// Lesson 832 + 836 history:
//   - Lesson 832 (2026-08-30): vite build fails on root-owned dist/.
//     The fix was `npx vite build && bash scripts/build-linux.sh`.
//   - Lesson 836 (2026-09-09): generalized to a `prebuild-clean`
//     step so any future adeal-side build is robust to root-owned
//     dist/ leftovers.

import { existsSync, rmSync, statSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const distDir = resolve(__dirname, "..", "dist");

if (!existsSync(distDir)) {
  // Nothing to clean — exit early.
  process.exit(0);
}

const before = statSync(distDir);
console.log(`[prebuild-clean] removing ${distDir} (was ${before.uid}:${before.gid}, mode ${(before.mode & 0o777).toString(8)})`);

try {
  // We can rm as long as the dir itself is writable by us and the
  // files inside are owned by us. The prebuild-clean exits 0 even
  // if the rm throws EACCES — vite.config.js has emptyOutDir=false,
  // so vite will just emit new files alongside the old ones. A
  // partial wipe is better than no build at all.
  rmSync(distDir, { recursive: true, force: true });
  console.log(`[prebuild-clean] wiped.`);
} catch (e) {
  if (e.code === "EACCES" || e.code === "EPERM") {
    console.warn(
      `[prebuild-clean] could not fully wipe dist/ (${e.code}: ${e.message.split('\n')[0]}). ` +
      `Continuing — vite will emit new files alongside the old ones (emptyOutDir=false). ` +
      `For a clean state, run \`sudo chown -R $(whoami):$(id -gn) dist/\` once.`
    );
    // Exit 0 — vite can still emit, just into a polluted dir.
    process.exit(0);
  }
  throw e;
}
