# Miracle Claw Branding — ADeal Auto Repair (deferred to v1.0.1+)

This document is a **placeholder / spec** for the v1.0.1 branding pass. v1.0.0 ships with generic Tauri icons so we don't block the release-gate on cosmetic work.

## What "branded" means for MC

The goal: anyone who sees the app, the installer, or the desktop shortcut should immediately recognize **ADeal Auto Repair**. Not "a generic Tauri app", not "OpenClaw WebChat in a window".

Three places branding shows up:

1. **Pre-install** (NSIS installer wizard + desktop shortcut icon)
2. **Post-install** (Tauri window chrome: title bar, taskbar icon, system tray)
3. **In-app** (loading splash, About screen, default theme accent)

## ADeal brand assets we need

| Asset | Spec | Use |
|---|---|---|
| Primary icon | 256x256 PNG transparent, ADeal green + lobster | Desktop shortcut, taskbar |
| Installer icon | 256x256 .ico multi-resolution (16, 32, 48, 64, 256) | NSIS installer + uninstaller |
| macOS icon | 512x512 .icns | `bundle.icon.icns` |
| Splash image | 512x512 PNG | Tauri splash window |
| Brand color | TBD — likely ADeal Auto Repair logo green hex | Theme accent |

**TODO before v1.0.1**: David supplies the ADeal Auto Repair logo files (PNG, ICO, ICNS). If David has an existing brand kit, point me at it. If not, I can stub from the existing Tauri default icons and recolor to ADeal green.

## Code changes (when branding happens)

1. `tauri.conf.json`:
   - `bundle.icon` → replace default Tauri icons with ADeal ICO/PNG/ICNS
   - `bundle.shortDescription` → keep "Miracle Claw desktop chat — talk to MAIC models in a native window" but add ADeal footer
   - `bundle.publisher` → already "ADeal Auto Repair" ✅
   - `bundle.homepage` → already "https://adealauto.com" ✅
2. `src-tauri/tauri.conf.json` → `app.windows[0].title` → already "MiracleClaw" ✅; consider adding ADeal logo as window icon
3. `src-tauri/icons/` → replace 32x32, 128x128, 128x128@2x with ADeal versions
4. `src-tauri/icons/icon.ico` → replace
5. `src-tauri/icons/icon.icns` → replace
6. `index.html` → currently plain HTML h1; could add inline SVG logo + ADeal green accent
7. Optional: system tray icon (deferred to v1.0.2 — requires additional tauri-plugin-tray crate)

## Why deferred from v1.0.0

- The release-gate (`docs/CLEAN-WINDOWS-INSTALL-TEST.md`) verifies **functional** behavior: install runs, app boots, chat works, state dir isolated, MAIC plugin loaded, uninstall clean.
- Branding is cosmetic — doesn't change any of the 7 pass criteria.
- David's no-space convention (`MiracleClaw`) is already applied to productName, window title, h1, installer filename. That's the only "branding"-flavored change we shipped.
- Real branding assets (ADeal logos, ADeal green hex) require David's input. Not invented by us.

## Open questions for David (when ready)

1. **What's the exact ADeal green hex?** (If you don't have it: pick a forest green, ~#0F5132 or similar — let me know if you have a brand spec)
2. **Logo as-is, or simplified monochrome for 32x32?** (Some logos don't downscale well)
3. **Want a lobster on it?** (The 🦞 Lobster in IDENTITY.md is personal to me as the agent; the user-facing brand should be ADeal's)
4. **NSIS installer dialog header art?** (NSIS supports a bitmap header — ADeal logo would go there)
5. **Taskbar grouping?** (Should the app group under "MiracleClaw" or "ADeal Auto Repair" in the Windows taskbar?)

## When to do this

After v1.0.0 ships AND David has the ADeal brand assets ready. Estimate: 1-2 hours of work if assets are pre-made, half a day if we have to source/create them.

**Tracking**: When David's ready, say "let's do the branding" and I'll create a `feat: ADeal branding` branch, work the changes, build a new installer (`MiracleClaw_1.0.1_x64-setup.exe`), run the install test again, tag v1.0.1.

## Lessons applied (pre-emptively)

- **Lesson 351**: All three version strings stay in sync (Cargo.toml, tauri.conf.json, package.json). Bumping to 1.0.1 means updating all three.
- **Lesson 352**: `git add .` is forbidden. Stage only branding-specific files.
- **Lesson 419-422**: Re-use the cross-compile build infra. Rebuild should be ~5 min with sccache warm.
- **Lesson 213**: Code signing deferred. If David's ready to sign at the same time as branding, that's a good combined pass.