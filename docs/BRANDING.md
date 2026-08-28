# Miracle Claw Branding — Milagro Distribution Corp

This document describes the visual identity applied across Miracle Claw's installer, in-app UI, About screen, and external surfaces (GitHub social preview, favicon).

## Brand owner

**Milagro Distribution Corp** is the legal entity and brand owner for Miracle Claw. All product, marketing, and support surfaces reference Milagro. See [`LICENSE`](../../LICENSE) for copyright details.

For support inquiries: **support@milagrocloud.com**
For product info: <https://milagrocloud.com>

## Brand assets

| Asset | Spec | Use |
|---|---|---|
| Primary icon | 256x256 PNG, transparent background, Milagro green + lobster mark | Desktop shortcut, taskbar, GitHub social preview |
| Installer icon | 256x256 `.ico` multi-resolution (16, 32, 48, 64, 256) | NSIS installer + uninstaller |
| macOS icon | 512x512 `.icns` | `bundle.icon.icns` |
| Splash image | 512x512 PNG | Tauri splash window |
| Brand color | `#22c55e` (Milagro green) | Theme accent, splash background |

Master SVG sources live under `brand/`. To regenerate the platform icon set:

```bash
python3 brand/build_tauri_icons.py
```

This rebuilds `src-tauri/icons/icon.png`, `icon.ico`, `icon.icns`, and the various PNG sizes from `brand/variants2/H-mcle-tight-icon.svg`.

## Code touchpoints (where branding lives)

1. **`src-tauri/tauri.conf.json`**
   - `bundle.publisher` → `"Milagro Distribution Corp"`
   - `bundle.homepage` → `"https://milagrocloud.com"`
   - `bundle.icon` → icon set in `src-tauri/icons/`
   - `app.windows[0].title` → `"MiracleClaw"`
2. **`src-tauri/Cargo.toml`** — `description` and `authors` reference Milagro.
3. **`package.json`** — `description` references Milagro.
4. **`src/styles.css`** — `--accent: #22c55e` (Milagro green).
5. **`index.html`** — splash + title bar text.
6. **`README.md`** — top-level description + license copyright.

## Lessons applied

- **Lesson 351**: All three version strings stay in sync (Cargo.toml, tauri.conf.json, package.json). Bumping to 1.0.2 means updating all three.
- **Lesson 352**: `git add .` is forbidden. Stage only branding-specific files.
- **Lesson 419-422**: Re-use the cross-compile build infra. Rebuild should be ~5 min with sccache warm.

## Updating brand assets

When Milagro releases a refreshed brand kit (new logo, new color, new splash):

1. Drop replacement PNGs/ICO/ICNS into `src-tauri/icons/`
2. Drop replacement SVG into `brand/variants2/H-mcle-tight-icon.svg`
3. Update `--accent` in `src/styles.css` if the color changed
4. Run `python3 brand/build_tauri_icons.py` to regenerate derived sizes
5. Run a full installer build (`scripts/build-windows-docker.sh`)
6. Smoke-test the installer on a clean Windows VM
7. Tag a new release (`v1.x.y`) and ship via the auto-updater
