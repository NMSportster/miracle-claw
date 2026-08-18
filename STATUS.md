# Miracle Claw v1.0.0 — Scaffold Status

**Scaffolded**: 2026-08-17 18:52 MDT (end of session, before pivoting from MC v1.7.28 era)
**Author**: David Adeal (with assistant scaffolding)
**Branch**: `master` (clean slate, no tags yet)

## What's done

✅ Clean directory at `/home/adeal/.openclaw/workspace/projects/miracle-claw/`
✅ Git repo initialized (master branch)
✅ `package.json` (npm manifest, vite + tauri deps)
✅ `src-tauri/Cargo.toml` (Rust manifest, lib + bin)
✅ `src-tauri/tauri.conf.json` (window config, webview URL = `http://localhost:28789/`)
✅ `src-tauri/src/main.rs` (Tauri entry, calls `miracle_claw_lib::run()`)
✅ `src-tauri/src/lib.rs` (Tauri builder + setup stub with TODO for child process spawn)
✅ `src-tauri/build.rs` (Tauri build hook)
✅ `src-tauri/icons/*` (copied from old MC repo, all formats)
✅ `scripts/build-windows-docker.sh` (Docker build script, copied + path-aware)
✅ `scripts/install-sccache.sh` (sccache installer, copied)
✅ `Dockerfile.build` (Windows cross-compile Docker image, copied)
✅ `index.html` (placeholder, "Loading OpenClaw WebChat…")
✅ `README.md` (architecture doc)

## What's NOT done (design phase tomorrow morning)

⏳ **lib.rs setup hook** — actually spawn `node openclaw gateway --port 28789` as a child process. Poll `http://localhost:28789/v1/models` until 200. Show webview. Kill child on exit.
⏳ **MAIC provider plugin** — verify it's loaded at OpenClaw startup (check `~/.openclaw/extensions/maic/openclaw.plugin.json` gets picked up). Currently sits in Main-Adeal's user dir; needs to ship with the installer so end users get it too.
⏳ **OpenClaw bundle** — decide: (a) require user has Node + openclaw globally installed, (b) bundle Node runtime + openclaw npm package into the installer, (c) use a system Node to launch a self-contained openclaw script shipped in resources.
⏳ **Branding pass** — ADeal green theme, splash screen, system tray icon (currently using old MC icons which are lobster/claw themed).
⏳ **Installer smoke test** — does `npm install && bash scripts/build-windows-docker.sh` actually produce `MiracleClaw_1.0.0_x64-setup.exe`?
⏳ **First-run UX** — when user double-clicks the .exe for the first time: install completes, app opens, child process spawns, OpenClaw WebChat loads, MAIC auth flow begins (or auto-logs-in via cached creds).

## Tomorrow morning checklist

1. Read this STATUS.md
2. Open the `miracle-claw` folder, NOT `old_mc_files/`
3. Decide the OpenClaw bundle strategy (3 options above) — call it before coding
4. Implement the child-process spawn in `lib.rs`
5. Try a smoke build: `cd projects/miracle-claw && npm install && bash scripts/build-windows-docker.sh --rust-only` first (10x faster feedback than full NSIS bundling)
6. If smoke build works, do the full NSIS bundle
7. Verify installer lands at `/mnt/c/Users/Adeal/Desktop/MiracleClaw_1.0.0_x64-setup.exe`
8. Double-click test on Windows side
9. Tag `v1.0.0` once it actually works

## Lessons to remember while building

- **Lesson 391**: "When iterate-on-existing fails 5 times in a day, rewrite from clean source." This whole repo IS that rewrite.
- **Lesson 395**: "When you find yourself repeating 'we need to add X' and X is already working in a different stack — adopt the working stack." OpenClaw WebChat is the working stack. We're wrapping it.
- **Lesson 351**: Bump all 3 version strings together (Cargo.toml, tauri.conf.json, package.json). They're all `1.0.0` right now.
- **Lesson 352**: NEVER `git add .` on the Tauri repo. Stage only what you actually touch.
- **Lesson 353**: Warm Docker builds = 1:30-4 min. Cold = 20+ min.

## OpenClaw connection notes (for tomorrow)

OpenClaw is already running on Main-Adeal at `http://localhost:18789/`. The MAIC plugin lives at `/home/adeal/.openclaw/extensions/maic/` and registers itself via `~/.openclaw/openclaw.json`. For the .exe to ship with MAIC wired, we need to:
- Either copy `extensions/maic/` into the installer resources and have the installer write it to user's `~/.openclaw/extensions/` on first run
- Or document that end users need to install the MAIC plugin themselves

The first option is the right answer — silent install, no user burden.

## Brand & product naming

- **Product name**: Miracle Claw
- **Bundle ID**: `com.adealauto.miracle-claw` (different from old `miracle-claw-ui` to avoid collision)
- **Installer file**: `MiracleClaw_1.0.0_x64-setup.exe`
- **Publisher**: ADeal Auto Repair
- **Homepage**: https://adealauto.com

## Git state

- Branch: `master`
- Working tree clean (nothing committed yet)
- First commit will be the scaffold
- Tag strategy: tag when actually shipped (e.g., `v1.0.0` after first installer works)