# IDEAS-BACKLOG.md — Future Improvements Track

**Created**: 2026-08-23 (rc49 shipped)
**Status**: Living document. Append new ideas at the bottom; promote to
a `feature/*` branch when ready to ship.
**Source**: Originally captured during conversation about the
"10 Must-have CLIs for your AI Agents in 2026" article (Prosper
Otemuyiwa, Medium, Apr 1 2026 — https://medium.com/@unicodeveloper/10-must-have-clis-for-your-ai-agents-in-2026-51ba0d0881df).
David asked us to save this track so it doesn't get forgotten.

---

## Why this file exists

We split MC's roadmap into **audience buckets** while discussing the
public-product reframe. The buckets are:

- **Bucket A** — *entry-level polish* (newbie-friendly). Shipped into
  v1.0.0 (image preview, drag-drop) + rc49 (palette, OS clipboard).
  Remaining: first-run wizard, file-search inside cmd-k, magic-byte
  sniffing for richer previews.
- **Bucket B** — *power-user unlocks* (coder-friendly). Shipped in
  rc49 (cmd-k palette). Remaining: `mc` CLI companion on PATH,
  `tauri-plugin-updater` for auto-updates, Mac .dmg + Linux .deb/.AppImage
  installers, syntax highlighting.
- **Bucket C** — *chat-is-the-workspace* (the long-arc vision). Not
  started. Inline file previews in chat, model writes notes inline,
  terminal output shown in chat, dashboard tiles become chat-invokable
  tools.

Items below aren't sorted by bucket — they're sorted by how recently
they came up so the most recent ideas stay at the bottom (where you'll
see them when adding more).

---

## Already shipped (don't redo)

- ✅ v1.0.0 — image preview, PDF/Office open-externally, binary detect
- ✅ rc45 — 5-tile dashboard split (Terminal TUI, Local Terminal, Files, Notebook, OpenClaw Windows)
- ✅ rc47 — files ACL fix, notebook Save & New
- ✅ rc48 — drag-and-drop file attachment staging (`mc_stage_attachment`, etc.)
- ✅ rc49 — Cmd-K palette, OS clipboard via `tauri-plugin-clipboard-manager`

---

## Backlog (sorted: newest at bottom)

### Theme: discoverability + onboarding

- **First-run wizard** *(rc50?)* — When `first_run_report.needs_maic_login`
  is true, show a 3-screen walk-through *before* the login form:
  "What is MC? What can it do? What do you need?" Big buttons, no
  jargon, no config. Newbies currently land on a login form with no
  context. Bucket A.
- **Cmd-K file search** *(rc50 or rc51?)* — Walk the 4 allowed roots
  on palette open, dedupe file names, index for fuzzy search. ~50 LOC.
  Pairs naturally with palette already shipped in rc49.
- **Tooltips on dashboard tiles** *(rc51?)* — Hover any tile → see a
  one-line "what is this for?" pop. Newbie-friendly. Avoids the
  "what does each tile mean?" question David had during development.
- **Welcome banner on dashboard** *(rc51?)* — If user has < 3 sessions,
  show a dismissible "👋 new here? Try the Cmd-K palette (Ctrl+K) or
  drop a file below to chat with it." Auto-hides after dismissal.

### Theme: smarter previews

- **Magic-byte sniffing in `mc_ui_read_file`** *(rc5x?)* — Detect PDF,
  ZIP, image formats by their first 4 bytes, not just by extension.
  Lets users with misnamed files still get the right preview.
- **PDF page-1 thumbnail** *(rc5x?)* — `pdfium-render` crate renders
  page 1 as a PNG; show alongside the "Open externally" button.
  Adds ~12 MB to the binary; David's call. Not v1.x blocking.
- **Syntax highlighting** *(rc5x?)* — Prism.js or highlight.js in the
  text preview. ~150 KB JS in the bundle, well within budget. Adds
  real readability to code files. Defer to a `feature/syntax-highlight`
  branch.
- **Diff view for text files** *(future)* — When two files of the same
  name exist in different dirs, show a diff. Power-user feature.

### Theme: power-user CLI / scriptability

- **`mc` CLI companion on PATH** *(rc5x?)* — A `mc` binary that
  installs alongside the .exe and exposes:
    - `mc chat --attach <path> [--message "<msg>"]` (drag-drop from terminal)
    - `mc open <file>` (open in default app via the OS shell)
    - `mc model set <model-id>` (switch default model)
    - `mc exec "<cmd>"` (run a shell command with model context)
    - `mc status` (show connected tier, model, version)
  Mirrors OpenAI's `codex`, Anthropic's `claude`. Big unlock for
  power users.
- **`tauri-plugin-updater`** *(rc5x?)* — Auto-update check on launch,
  "New version available" banner, one-click download + install. Drops
  the manual installer dance for most users. Trivial Rust wiring
  (Tauri's own plugin); main work is the GitHub releases plumbing.
- **Headless `mc-server` mode** *(future)* — Run MC without a window
  for embedded use (Raspberry Pi kiosk, CI runner).

### Theme: platform coverage

- **Mac .dmg installer** *(v1.2.0?)* — `cargo tauri build` on a Mac
  host produces the .dmg. Need to confirm with Tauri docs whether
  xwin-style cross-compile works for Mac or if we need a Mac runner.
- **Linux .deb + .AppImage** *(v1.2.0?)* — Same question. .deb for
  Debian/Ubuntu, .AppImage for the rest. Probably easier than .dmg.
- **Windows ARM64 installer** *(v1.3.0?)* — Snapdragon X laptops are
  becoming real. Separate target, separate build script.

### Theme: chat-as-workspace (Bucket C, the long arc)

- **Inline file previews in chat panel** *(future)* — When the model
  references a file by path, the chat renders a 1-line preview inline
  instead of just showing the path.
- **Model writes notes inline** *(future)* — "save this to my notes"
  → model calls the notebook write command → result shows up in
  notebook page immediately. No copy-paste round-trip.
- **Terminal output in chat** *(future)* — "run npm test" → terminal
  runs → last 100 lines appear in chat. Tiles become chat-invokable.
- **Dashboard tiles become chat-invokable tools** *(future)* — User
  says "open my files" → MC navigates to Files page automatically.
- **Multi-model threads** *(future)* — Same chat, switch models mid-thread
  to compare answers. Saves the "let me try this with GPT" round-trip.

### Theme: integrations

- **VS Code extension** *(future)* — Right-click a file → "Send to
  MC chat". Mirrors Cursor's "send to composer" pattern.
- **Browser extension** *(future)* — Highlight text on a webpage →
  right-click → "Send to MC chat".
- **Raycast / Alfred extension** *(future)* — `mc chat <query>` from
  any Mac app.

### Theme: monetization (when MAIC has paying customers)

- **Pro tier features** *(v1.5+)* — Multi-device sync, longer context,
  faster cascade, custom system prompts, model fine-tuning, team
  workspaces. All gated behind MAIC's tier system.

### Theme: security / hardening

- **Code signing for Windows installer** *(v1.1.x blocker for public
  release)* — Currently unsigned, so SmartScreen warns "unknown
  publisher". Need an EV code-signing cert (~USD 300/yr) before any
  non-David user installs.
- **Auto-update with signed payloads** *(rc5x?)* — Tauri updater
  supports signed manifests; pair with the EV cert.
- **CSP tightening** *(rc5x?)* — `connect-src` currently allows any
  HTTPS (so MAIC works from any subdomain); narrow to `maicserver.com`
  + `*.milagro.cloud`. Defense in depth.

---

## How to use this file

When you have a new idea, **append at the bottom** under whichever
theme fits (or start a new theme). When ready to ship something, copy
it into a `feature/<slug>` branch's commit message and a PR/issue,
then **strike it through here** (`~~like this~~`).

When you decide NOT to ship something, move it to a "Considered,
declined" section at the bottom with a one-line reason. We delete
those after 90 days unless David re-asks.