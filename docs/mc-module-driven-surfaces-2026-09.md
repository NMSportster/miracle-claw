# Module-Driven Surfaces (Miracle Claw rc56.0)

**Status:** Approved visual direction, draft spec
**Date:** 2026-09-09
**Author:** Claw (with David's input)
**Ships as:** Miracle Claw v1.2.0-rc56.0

---

## 1. Problem

The current Miracle Claw Tauri app has a feature-catalog dashboard: Tasks, Workflows, Files, Notebook, Voice, Provider Keys, Modules, Extras, plus three terminal-style tiles. That's a lot of tiles for a launchpad. Most of those are *features* that could be *modules* — apps the user installs from a marketplace.

The wedge for MilagroCloud isn't the Tauri app's feature surface. It's the **module system** that runs across **all three client surfaces**: the Tauri app, the mc-openclaw TUI, and the `mclaw` CLI. Same modules, same auth, same backend, three UIs.

This spec describes the module system and the new dashboard launchpad. The TUI and CLI module surfaces are follow-on work in the same release.

---

## 2. Decisions locked

### 2.1 Brand alignment (NOT a rebrand)

Miracle Claw has always been Miracle Claw. This is brand alignment, not a rename. The lobster, the name, and the tagline get adopted consistently across all surfaces — but the product name doesn't change.

**Phase out "OpenClaw" the name** in user-visible code. See §6 for the rename buckets.

### 2.2 Dashboard = launchpad, not catalog

Most current dashboard tiles become modules. The dashboard becomes a thin launchpad with:

1. **Topbar** (thin): logo, "Settings" button, "David ▾" dropdown
2. **Tier row**: Free/Pro/Team/Enterprise badge, usage bar, "Plans and Pricing" button
3. **Greeting** + "Pick a tool below, or just ask Miracle Claw anything"
4. **Launch tiles** (3): Windows, Terminal, Commands
5. **Your Tools** section: installed module buttons + "+ Add a tool" button
6. **Recent**: last 3 module invocations

### 2.3 Modules are free

Every module is free. No price labels, no "Free" labels, no "Free only" filter. Marketplace footer: "Every module is free. Become a module author →"

The tier system (Free/Pro/Team/Enterprise) is unchanged and gates **chat tokens**, not modules.

### 2.4 Three launch tiles

- **🦞 Miracle Claw · Windows** — full chat webview UI
- **⌨️ Miracle Claw · Terminal** — keyboard-driven TUI
- **🖥️ Miracle Claw · Commands** — one-shot `mclaw run ...` invocations

The third tile is the "headless" / "CLI" path. It launches a quick command runner, not a persistent terminal session.

### 2.5 Three client surfaces, one module system

The same `client_manifest` is rendered as:
- **Tauri app** → button grid on dashboard + form widgets in modal
- **mc-openclaw TUI** → toolbar (F1-F7 installed) + form mode
- **mclaw CLI** → `mclaw run <module>.<tool> <args>` + `mclaw tools` listing

A module installed on one surface is available on all three (after sync).

---

## 3. Module system

### 3.1 What is a module?

A **module** is a server-side MAIC component that exposes a `client_manifest` describing its tools. Each tool has:
- `name` (e.g., `draft_email`)
- `description` (one-line summary)
- `form_schema` (JSON Schema for inputs)
- `category` (controlled vocabulary, see §3.3)
- `subcategory` (freeform string)
- `tier_required` (Free/Pro/Team/Enterprise — defaults to Free)

### 3.2 `client_manifest` shape

```json
{
  "module_id": "firecrawl",
  "name": "Firecrawl",
  "description": "Web page content extraction",
  "icon": "🌐",
  "category": "research",
  "subcategory": "web-scraping",
  "tier_required": "free",
  "tools": [
    {
      "name": "fetch_url",
      "description": "Pull clean content from a URL",
      "form_schema": {
        "type": "object",
        "properties": {
          "url": { "type": "string", "title": "URL", "minLength": 1 }
        },
        "required": ["url"]
      }
    }
  ]
}
```

### 3.3 Category vocabulary (v1)

16 categories, controlled vocabulary (server-side validation):

| Category | Emoji |
|---|---|
| `productivity` | ✏️ |
| `communication` | ✉️ |
| `research` | 🔍 |
| `data-analysis` | 📊 |
| `creative` | 🎨 |
| `marketing` | 📣 |
| `sales` | 💼 |
| `finance` | 💰 |
| `legal` | ⚖️ |
| `engineering` | 🛠️ |
| `hr` | 👥 |
| `scheduling` | 📅 |
| `files` | 📄 |
| `media` | 🎬 |
| `voice` | 🎙️ |
| `other` | 📦 |

`subcategory` is freeform (e.g., "web-scraping", "newsletter", "tts").

### 3.4 New MAIC endpoints

```
GET  /v1/categories
  → { categories: [{id, label, emoji, count}] }

GET  /v1/marketplace/modules?category=X&sort=top_rated|recent|alphabetical
  → { modules: [{module_id, name, description, icon, category, subcategory, rating, install_count, author}] }

GET  /v1/modules/installed
  → { modules: [{module_id, name, description, icon, category, subcategory, installed_at, last_used_at, client_manifest}] }

POST /v1/modules/install { module_id }
  → { success, client_manifest }

POST /v1/modules/uninstall { module_id }
  → { success }
```

### 3.5 Module install flow

1. User on dashboard, clicks "+ Add a tool" in "Your Tools" section header
2. Lands on marketplace page (`/marketplace`)
3. Browses categories (sidebar) or searches
4. Clicks "Add to MC Claw" on a module card
5. `POST /v1/modules/install` → server stores, returns `client_manifest`
6. Client refreshes "Your Tools" grid (new tile appears)
7. Optional: client_manifest propagates to TUI and CLI on next launch

---

## 4. Dashboard launchpad layout

```
┌──────────────────────────────────────────────────────┐
│  🦞 Miracle Claw          David ▾   ⚙ Settings      │  ← topbar
│  Made for YOU, not for everyone                       │
├──────────────────────────────────────────────────────┤
│  [Free tier]    ████░░░░  12,400 / 50,000 tokens   │  ← tier row
│                                  [Plans and Pricing] │
├──────────────────────────────────────────────────────┤
│  Good afternoon, David.                              │
│  Pick a tool below, or just ask Miracle Claw anything.│
│                                                       │
│  LAUNCH                                               │
│  [🦞 Windows]   [⌨️ Terminal]   [🖥️ Commands]      │
│                                                       │
│  YOUR TOOLS · 7 installed        [+ Add a tool]      │
│  [✉️ Draft] [📄 Summarize] [🌐 Fetch] [🔍 Search]    │
│  [📅 Schedule] [🎨 Create] [📊 Analyze] [➕]         │
│                                                       │
│  RECENT                                               │
│  • Client follow-up email · 2h                       │
│  • Q3 invoice summary · yesterday                    │
└──────────────────────────────────────────────────────┘
```

### 4.1 Empty state

3-4 curated starter modules pre-installed for new users (Marketing Suite, Office Tools, Firecrawl, Calendar Companion). "Add all recommended" button on first-run. Avoids empty-feeling first-run.

### 4.2 Visual style

- **Background:** `#fffaf0` (warm cream)
- **Cards:** white with 1.5px `#f0e3c0` border
- **Primary accent:** `#e8542f` (lobster orange)
- **Secondary accents:** `#f4b41a` (sunny yellow), `#2c5e4f` (forest green)
- **Headlines:** serif stack (Iowan Old Style / Charter / Georgia)
- **Body:** system sans-serif
- **Voice:** warm, friendly, slightly playful. "Good afternoon, *David*."

See `Pictures\Miracle Claw\UX\` for canonical mockups:
- `dashboard-home-launchpad.png` — new launchpad layout (approved)
- `dashboard-home-launchpad-minimal.png` — trimmed version (no launch tiles)
- `dashboard-home-launchpad-forest.png` — forest green palette variant
- `marketplace-module-discovery.png` — marketplace page (no prices)
- `modal-form-and-result-miracle-claw.png` — form modal pattern
- `tui-toolbar-form-result-miracle-claw.png` — TUI module surface
- `mclaw-cli-miracle-claw.png` — CLI module surface

---

## 5. Three client surfaces, same module system

### 5.1 Tauri app (Week 2)

- Dashboard launchpad (above)
- Marketplace page (sidebar categories + search + module cards)
- Modal form (renders `form_schema` as inputs)
- Result card (shows module output)

### 5.2 mc-openclaw TUI (Weeks 3-4)

- Greeting with lobster ASCII art + tagline (brand)
- Toolbar: F1-F7 = installed module shortcuts, F12 = "Add to MC Claw"
- Form mode: prompts for each `form_schema` field
- Result mode: formatted output

### 5.3 mclaw CLI (Week 5)

- `mclaw run <module>.<tool> [args]` — one-shot invocation
- `mclaw tools` — list installed modules grouped by category
- `mclaw marketplace` — browse + install
- `mclaw --version` — lobster ASCII + version
- `mclaw login` — branded welcome

The TUI and CLI are follow-on work in the same rc56.0 release. The Tauri launchpad ships first.

---

## 6. Phase out "OpenClaw" the name

### 6.1 Bucket 1: User-visible labels (rename in rc56.0)

- Dashboard tiles: "OpenClaw · Windows" → "Miracle Claw · Windows", "OpenClaw · Terminal" → "Miracle Claw · Terminal"
- Command palette entries
- Terminal page shell dropdown labels
- Toast messages, error messages, settings copy
- Module help text
- Provider keys page tooltip

Files touched:
- `src/pages/dashboard.js`
- `src/cmd_k_palette.js`
- `src/pages/terminal.js`
- `src/pages/settings.js`
- `src/pages/modules.js`
- `src/pages/provider_keys.js`
- `src/data/module-help.js`

### 6.2 Bucket 2: Internal Rust API (rename in rc56.0 with migration)

- Tauri webview window name `"openclaw"` → `"miracle-claw-chat"`
- Capability file `openclaw.json` → `chat.json`
- Bundled resource `openclaw.mjs` → `miracle-claw-gateway.mjs`
- Config file path `~/.openclaw/openclaw.json` → `~/.openclaw/chat.json`

**Migration:** read old, write new, delete old on first launch of rc56.0. All four renames must move together; partial rename breaks the install.

### 6.3 Bucket 3: Internal-only (leave alone)

- Function names (`onOpenOpenClawTerminal`, etc.)
- `mc-openclaw` binary name → defer to rc56.0 follow-up: rename to `mc-terminal`
- `defaultShell: "mc-openclaw"` setting
- `__MC_OPENCLAW__` env var
- Test asserts (`assert_eq!(mc.0, "openclaw")`)

### 6.4 TUI binary name

Default for rc56.0: rename `mc-openclaw` → `mc-terminal`. Matches UI label, short, no "openclaw" anywhere in user-launchable binary names.

**File changes:**
- `mc-openclaw/Cargo.toml` line 2: `name = "mc-terminal"`
- `mc-openclaw/src/main.rs` `[[bin]]` `name = "mc-terminal"`
- `src/pages/terminal.js` shell dropdown: "Miracle Claw · Terminal (mc-terminal)"
- `src/pages/dashboard.js` launch command: `mc-terminal`
- `mc-openclaw/dist/` → `mc-terminal/dist/`

---

## 7. Build plan

### Week 1 — MAIC server
- `client_manifest` schema
- `category` + `subcategory` fields on modules
- `GET /v1/categories`
- `GET /v1/marketplace/modules?category=X`
- `GET /v1/modules/installed` (grouped by category)
- `POST /v1/modules/install` / `POST /v1/modules/uninstall`
- Seed 8 starter modules across categories

### Week 2 — Tauri app
- Dashboard launchpad layout
- "Your Tools" grid
- Marketplace page
- Modal form (renders `form_schema`)
- OpenClaw name phase-out (Bucket 1)
- Dashboard tile rename (Windows / Terminal / Commands)

### Weeks 3-4 — mc-openclaw TUI
- Toolbar (F1-F7 installed + F12 add)
- Form mode
- Result mode
- Lobster ASCII greeting (brand)
- TUI binary rename `mc-openclaw` → `mc-terminal` (Bucket 2)

### Week 5 — mclaw CLI
- `mclaw run <module>.<tool>` one-shot
- `mclaw tools` listing
- `mclaw marketplace` browse + install
- Branded welcome text on login/version/help

### Week 6 — Migration
- Bucket 2 renames (window name, capability, resource, config path)
- Read-old-write-new-delete-old on first launch
- Rollback safety: if migration fails, fall back to old config

### Weeks 7-8 — Polish
- Empty state for new users (3-4 curated starter modules)
- Form validation
- Error states
- Smoke tests across all three surfaces
- Documentation

---

## 8. Out of scope for rc56.0

- Module marketplace uploads from Pro users (curated only for v1)
- Module versioning / auto-update
- Module search across installed + marketplace
- Module analytics dashboard
- Faceted browsing (defer until 50+ modules)
- Subcategory control (defer until 50+ modules; freeform for v1)
- Cross-surface live sync (TUI refreshes `client_manifest` on launch only; no WebSocket in v1)
- Reimplement `mclaw usage` (wait for stable MAIC quota endpoint)
- Scheduled tasks
- `mclaw chat` interactive REPL polish

---

## 9. Open questions (parked)

- Tauri OpenClaw: sidecar vs same-process for the chat webview (recommend sidecar)
- WebSocket for live module updates (defer to v2; polling on focus for v1)
- Custom module uploads for free tier (no — curated only; Pro+ can upload)
- "Your Tools" location: dashboard section (recommended) vs separate `/tools` page
- Vocabulary size: 8 starter vs 16 full (default: 16 full, easier to extend)
- Subcategory control: freeform vs controlled (default: freeform, revisit at 50+ modules)
- Marketplace location: separate `/marketplace` page (recommended) vs tab in existing Modules page
- Dashboard tile rename for rc55.25.x patch or rc56.0 only? (default: rc56.0 only — too risky to patch)

---

## 10. Visual references

`Pictures\Miracle Claw\UX\`:
- `dashboard-home-launchpad.png` — canonical launchpad (this spec's target)
- `dashboard-home-launchpad-minimal.png` — trimmed alternative
- `dashboard-home-launchpad-forest.png` — forest green palette
- `dashboard-home-miracle-claw-branded.png` — alternative branded version
- `marketplace-module-discovery.png` — marketplace page (no prices)
- `modal-form-and-result-miracle-claw.png` — modal form pattern
- `tui-toolbar-form-result-miracle-claw.png` — TUI module surface
- `mclaw-cli-miracle-claw.png` — CLI module surface

---

## 11. Acceptance criteria

- [ ] Dashboard is a launchpad (5 sections, 3 launch tiles, "Your Tools" grid)
- [ ] Modules are free, no price labels anywhere
- [ ] Marketplace page is reachable from "+ Add a tool" button in "Your Tools" header
- [ ] `client_manifest` endpoint returns all installed modules
- [ ] `category` field on modules, server-side validation against 16-vocabulary
- [ ] `GET /v1/categories` returns count per category
- [ ] `GET /v1/marketplace/modules?category=X` filters by category
- [ ] `POST /v1/modules/install` returns `client_manifest` and updates dashboard
- [ ] Tauri dashboard tile names use "Miracle Claw" (not "OpenClaw")
- [ ] Command palette entries use "Miracle Claw" (not "OpenClaw")
- [ ] Terminal page shell dropdown uses "Miracle Claw" labels
- [ ] mc-openclaw binary renamed to `mc-terminal` (or kept with UI label only)
- [ ] 3 curated starter modules pre-installed for new users
- [ ] All 3 client surfaces (Tauri, TUI, mclaw) can install + run a module
- [ ] Bucket 2 renames (window name, capability, resource, config path) migrated safely
- [ ] Visual style matches `dashboard-home-launchpad.png` mockup
- [ ] Brand voice: warm, friendly, slightly playful; lobster + tagline in topbar
- [ ] 350+ tests pass, no regressions

---

## 12. Anti-patterns to avoid

- **AP-877-A:** Build the wedge, not the moat. Don't pre-build for power users who don't exist.
- **AP-878-A:** Don't conflate ADeal Auto Repair with MilagroCloud. Different businesses, different users, different revenue.
- **AP-880-A:** Drop features on first pushback, not the second.
- **AP-881-A:** Ask "feature or platform?" before scoping.
- **AP-882-A:** Don't pre-build discovery infrastructure for marketplaces without inventory.
- **AP-883-A:** Don't rename existing user-facing terminology when adding new surfaces.
- **AP-launchpad-A:** Don't keep "Modules" as a nav entry. It duplicates the marketplace and clutters the launchpad.
- **AP-pricing-A:** Don't show prices on the marketplace. All modules are free.
- **AP-rebrand-A:** Don't call this a rebrand. Miracle Claw has always been Miracle Claw. This is brand alignment.
- **AP-unveil-A:** Don't deploy mid-build milestones to Hetzner just because they're "ready." Build-but-don't-unveil until David calls it.
