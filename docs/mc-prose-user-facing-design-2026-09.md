# MC Subagents — User-facing design (2026-09-08)

**Constraint from David**: easy for general users to understand (not just techies); competitive visual bar with Cursor et al; MC's own touch.

This doc answers the three questions David landed on at 08:15 MDT:
1. **Easy for everyone, not just techies** — what does that actually look like in UI/copy?
2. **Built-in `.prose` library — my recommendation (Option A + 2 new ideas)**
3. **Competitive visual style — what MC's own touch is**

---

## 1. Easy for general users — concrete UX rules

**The single hard rule**: never show the user a `.prose` file. They never edit one, never see "this is a Prose VM program", never see YAML-ish syntax. They pick a name and a goal.

### What's hidden
- `.prose` file extension
- `agent name:` blocks
- `parallel:` / `session:` / `do:` syntax
- The word "subagent" outside of Settings → "About"
- The fork terminology ("agents run in parallel" becomes "Three specialists run at the same time")

### What's shown
| Generic term | User-facing name | Why |
|---|---|---|
| `.prose` program | **Workflow** | Tells the user: this is a recipe, not code |
| `agent name:` | **Specialist** | Specialists = people you hire; closer to user mental model than "agent" |
| `parallel:` block | **"Three specialists run at the same time"** | Literal English |
| `session: <agent>` | (invisible) | Just routing under the hood |
| Workers window | **Workflow Center** | Center = where the action happens |
| Workers tile badge | "3 specialists running" | Not "3 workers" |

### Plain English copy examples
- Chat empty-state: "**Try a workflow** — pick a goal, get a team of specialists on it" (not "Run an OpenProse program")
- Workers page header: "**Your team is working on this**" (not "Active worker branches")
- Tooltip on a paused branch: "Waiting for a tool result" (not "blocked-on-tool")
- Error chip on a failed branch: "Couldn't find the file you mentioned" (not "I/O error: ENOENT")

### One-tap entry points
- Dashboard has 4 big tiles: "Explore", "Code review", "Write tests", "Plan a project" — each = one workflow. Click → it runs. No "pick a model" dialog first.
- Chat input has a `/` button next to send that opens a "**Workflows**" menu, plain English descriptions, one click to insert as a chat message: `"Run the Code Review workflow on /src-tauri/src/lib.rs"`.

---

## 2. Built-in `.prose` library — 6 workflows

I'm recommending Option A (curated built-ins) plus two new ideas specific to MC's positioning.

### The 6 curated built-ins

User-facing names; real `.prose` lives in `src-tauri/resources/prose-library/`.

| # | User-facing name | `.prose` file | What it does (plain English) |
|---|---|---|---|
| 1 | **Explore Codebase** | `explore.prose` | One specialist reads a directory and answers a question. Like asking a coworker "what does this folder do?" |
| 2 | **Code Review** | `code-review.prose` | Three specialists look at the same files in parallel — one for bugs, one for performance, one for clarity. You get one combined report. |
| 3 | **Fix Tests** | `fix-tests.prose` | One specialist writes a failing test, another makes it pass. Loop until green. |
| 4 | **Plan a Project** | `multi-module.prose` | Five specialists map out a new project: features, risks, schedule, dependencies, questions. |
| 5 | **Pair Debug** | `pair-debug.prose` | Three specialists on a bug: one reproduces, one hypothesizes, one verifies. |
| 6 | **Docs From Code** | `docs-from-code.prose` | One specialist reads, three write in parallel: README, CHANGELOG, API reference. |

### Idea #1 — "Specialists" library (MC's own differentiator)

This is what I want to push hard on. **MC's specialist library is curated, opinionated, and visible to users** — they pick by name, not by `.prose` file.

Examples (4-6 specialists, all visible in dashboard's "Workflows" menu):

- **`@explorer`** — Read-only codebase research. No write tool. Safe for untrusted code.
- **`@reviewer`** — Reads one file, returns a checklist of issues. Three of them in parallel give you a triple-perspective review.
- **`@tester`** — Reads source, writes failing tests, then makes them pass.
- **`@docwriter`** — Reads, writes markdown. Three parallel = README / CHANGELOG / API ref in one shot.
- **`@planner`** — Reads requirements, writes a structured plan with questions.
- **`@debugger`** — Three parallel: repro / hypothesize / verify on a single bug.

Users pick `@reviewer` from a dropdown when running **Code Review**. Behind the scenes, that's `agent name: reviewer, model: milagro-dev`. But to the user it's a menu choice.

This is **competitive differentiation**. Cursor doesn't ship this — Cursor ships "agent mode" but it's a generic opaque model. Aider has specialized agents but they're CLI flags, not named specialists. We're shipping **named specialists** with one-click presets.

### Idea #2 — "Recipes" tab in Workflow Center

The Workers page (internal) doubles as a **Recipes** tab (user-facing). Read-only, curated, no editing. Lists:
- **Built-in**: the 6 above
- **Specialists**: the 6 specialist definitions
- **Examples**: 6 of the upstream 48 most-asked-for (gas-town for parallel pattern, captain's-chair for human-in-the-loop, etc.) shown with "**Advanced — pattern reference only**" badge. Click → runs as a workflow but with copy like "You can run gas-town to set up a 7-worker role coordination pattern. This is for advanced use. Most users won't need this." — Sets the bar that the built-ins cover 95% of needs.

That copy is how we keep "easy for general users" front and center while still shipping the advanced primitives to power users.

---

## 3. Competitive visual style — MC's own touch

The competitive bar: Cursor's UI is dark-on-light, blue accents, lots of cards. Aider is CLI-first, minimal. Claude Code TUI is plain text with a tree view. OpenCode is web-first, modern. Codex CLI is bare terminal.

### What we steal from competitors (and openly credit)

| Pattern | From | Why it works |
|---|---|---|
| Dark theme as default | Cursor | Matches developer preference; easier on the eyes for long sessions |
| Sidebar with status pills | Cursor / Linear | At-a-glance for "is something running?" |
| Cmd-K palette | Linear / Raycast | Power-user quick navigation |
| Branch tree view | GitLens / Sourcetree | Visual model for parallel work |
| Live-streaming text | Claude.ai | Removes the "is it working?" anxiety |

### MC's own touch — what makes us different

**Touch 1: Honest accessibility**

Cursor uses "New chat" / "Reply" / "Code" / "Apply" — fine for devs. MC uses **plain English** the way Cursor's marketing copy does, not the UI:

- "**Pick a specialist**" instead of "Pick an agent"
- "**Three of them, in parallel**" instead of "Fan-out"
- "**Working…**" instead of "running"
- "**Couldn't find the file**" instead of "ENOENT"

**Non-techies should be able to use MC without watching a tutorial.** A junior dev who's never used an IDE could sit down and pick "Explore Codebase" → enter a folder → get an answer.

**Touch 2: Calm progress indication**

Cursor's busy state is "...". Claude.ai uses animated dots. Both feel chatty.

MC's progress state is **silent until there's something to show**. Workers page updates without animations unless something changes. Status pills don't pulse — they just sit there with their color. The whole app feels **calm and quiet** during long agent runs.

**Touch 3: One light theme, one dark theme, automatic**

Cursor forces dark. Visual Studio Code requires manual toggle. Claude.ai is always light.

MC follows the OS. macOS users get light by default, Windows/Linux get the system color. Single button in Settings to override. This is what "our own touch for ease of use" means — we choose for the user where it's obvious, we let them choose where it's not.

**Touch 4: Built-in Specialists as a UX primitive**

Power tools feel approachable when they have named pre-sets. Just like Photoshop has "Cinematic" / "Vintage" / "Vivid" filters in one dropdown:

```
Workflow: Code Review ✓
Specialist: @reviewer ✓
Quantity: 3 (recommended for diverse perspective) ▼
   - 1 (fast)
   - 3 (recommended)
   - 5 (exhaustive)
Run Workflow ▶
```

Each preset has a one-line description in plain English. Hover for more details. **No required fields beyond "what do you want reviewed?"**.

**Touch 5: Direct edits trump chat**

Cursor's killer UX move: Cmd+K in the editor lets you edit code inline without leaving the editor.

MC's parallel: **Cmd+K in chat suggests workflows inline** based on what you've pasted. Paste a Python file → Cmd+K suggests "Run the **Code Review** workflow on this file" as the top hit. Paste a directory path → suggests "Run **Explore Codebase**". This saves the click of going to the dashboard.

**Touch 6: Receipts, not alerts**

AI tools usually show warnings ("This model might be slower"). MC shows **receipts** — at the end of a workflow run, you see exactly what was spent:

> "**Code Review** finished. 3 specialists ran in parallel. 4,200 tokens used. Took 38 seconds. **Open in Workflow Center**."

No alerts, no surprise dialogs, no "this might be wrong" anxiety. The Workflow Center is the single place to inspect anything that went wrong.

---

## Putting it together — landing page for `/workflows`

A new dashboard tile: **Workflows**.

Click → `Workflows` page (`src/pages/workflows.js`, registers as `"workflows"` — note: internally workers, externally workflows).

Header: "**Pick what you want done. Specialists handle it.**"

Grid of 6 tiles (Explore / Review / Fix Tests / Plan / Debug / Docs). Each tile:
- Big icon
- One-line description
- "1 specialist", "3 in parallel", etc.
- Click → a modal: "What should [Specialist] look at?" + Run button.

Below the grid: "**Or pick a specialist from the dropdown**" — advanced entry. Power users go here.

Top-right corner: status badge showing "3 specialists running" when active. Click → jumps to a side-by-side view (light users) or the full Workers dashboard (power users set this in Settings).

---

## Settings → "Workflows" tab

For the user-facing framing:

- **Default specialist model**: dropdown (cheap / balanced / smart)
- **Show Workflow Center after a workflow runs?** toggle (on by default — they see receipts)
- **Use plain English descriptions?** toggle (on by default; turns off → power-user copy)
- **Maximum parallel specialists**: slider (1-10, default 3, capped by tier)

Five settings total. None require explanation.

---

## Phase 1 deliverable (3-4 days, revised with this design)

1. **Lesson 535 applied to open-prose**: stamp `plugins.entries.open-prose.enabled = true`, add `open-prose` to `plugins.allow`. Now `/prose` works in chat.
2. **`prose_host.rs` Tauri commands**: `prose_run(file_or_slug)`, `prose_compile(file)`, `prose_poll(id)`, `prose_kill(id)`.
3. **Specialists library** in `src-tauri/resources/prose-library/specialists/` — 6 named specialists as `.prose` files.
4. **6 built-in workflows** in `src-tauri/resources/prose-library/workflows/`.
5. **Workflow Center page** (`src/pages/workflows.js`) — MVP: list workflows + run. Live streaming TBD in Phase 2.
6. **Dashboard tile**: "Workflows" — points to the page.
7. **Cmd-K palette entry**: "Workflows".
8. **Onboarding tooltip** (one-time): "✨ New: Pick from 6 ready-made workflows — specialists handle the work."

That's the easy-for-everyone baseline. Polished (gradient hero, status pill animations, receipts system) is Phase 4.

---

## Risks added by this user-facing framing

| Risk | Mitigation |
|---|---|
| "Specialists" jargon still feels techie to non-devs | User-tested copy; fall back to "**Your team**" if tests fail. David reads the test scripts before publication. |
| 6 workflows is too few — power users want more from day 1 | Recipes tab exposes the 48 upstream examples with "Advanced" badge. Don't pre-promote. |
| "Use plain English" setting off-by-default for power users is contradictory | Off-by-default is wrong. ON by default. Power users can disable in Settings — but the default experience must be inclusive. |
| Cmd-K inline workflow suggestions take 2-3 weeks to do well | Phase 4 deliverable. Phase 1 ships the simpler "go to Workflows page" pattern; Phase 4 layers the smart suggestions on top. |
| Receipts UI may leak tier info ("free users got 400 tokens") that upsets paid users | Receipts show tokens-as-USD equivalent and tier limits, not absolute numbers. Tier limits already public on /pricing page. |

---

## Changelog

- **2026-09-08 08:15 MDT**: Added user-facing framing requirements, 6 named specialists, competitive-style touch list. Phases 1-2 deliverable list confirmed.
