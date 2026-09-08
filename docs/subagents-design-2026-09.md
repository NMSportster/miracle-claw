# Miracle Claw Subagents — Design (2026-09-08)

**Author:** ABQShop Claw 🦞
**Status:** Approved by David 2026-09-08 08:06 MDT — implementation starting
**User-facing framing:** David 2026-09-08 08:15 MDT — "make it easy for users to understand for the general user, not just techies"; visual bar = competitive with Cursor et al, but with MC's own touch for ease of use
**Goal:** Ship subagents / parallel agents / agent teams in MC, surfaced from every entry point (chat, Terminal tile, multi-window system), with a polished customer-facing UI.

---

## TL;DR

OpenProse — a complete `.prose` virtual machine for AI agent orchestration — ships as an **embedded OpenClaw extension** at `~/.openclaw/extensions/open-prose/`. It already provides Claude Code's subagent primitives: `agent name:`, `parallel:`, `session:`, `do:`, `try:`, `retry:`, `choice:`, all with 48 example programs including gas-town (7-worker role coordination), the forge (parallel browser tests), captain's chair (multi-session supervision), and PR review auto-fix.

**MC's job**: don't reinvent OpenProse. Surface it from chat + Terminal + windows with shared tool access, model routing, and a Workers UI.

---

## What MC already has

### Surfaces
- **Chat** (`src/main.js`) — primary chat surface, registers Cmd-K palette, slide-out sidebar, full CSS theme. 8 paid-tier local tools wired.
- **Terminal tile** (`src/pages/terminal.js`) — xterm.js + host shell picker (cmd / pwsh / wsl on Win; bash / sh / zsh on Nix). Runs `mc-openclaw` TUI by default.
- **Windows** — Tauri 2 multi-window system (`create_main_window`, `openclaw_open_window` for chat gateway, `openclaw_back_to_dashboard` nav). Module help, secrets, files, tasks, settings all live in their own windows/routes.

### Tool / model access
- All 15 pages resolve through `src/page_registry.js` (Lesson 713 contract: `mount`, `unmount`, `requiresAuth`).
- `src/navigation.js` (rc49) provides `navigate('page-id', extras)` and a single ctx builder map. Cmd-K palette + tiles + back buttons all use it.
- Tool catalog is shared globally — `modules-runtime.js` exposes `isModuleInstalled`/`invokeModule` for paid-tier modules (Voice, FireCrawl, LeadGen, OCR, etc.).
- MAIC routes all chat through `/v1/chat/completions`. `tool_execution: "client"` already proven (Lesson 169) for parent chat; same flag works for subagent sessions.

### What's missing
- OpenProse is **not yet bundled** with the MC Tauri installer (`ls src-tauri/resources/ | grep -i prose` returns nothing; extension lives only on David's dev box).
- No `/prose` slash command in MC chat.
- No Workers UI page in the page registry.
- No built-in `.prose` library marked "MC starter agents" — the 48 examples are upstream; we curate.

---

## Design — three surfaces, one runtime

The user's hard requirement: **chat, Terminal tile, and Windows all need to run prose / subagents with the same tools, same models, same polish**.

### Surface 1: Chat
- Add `/prose <script-or-slug>` slash command to chat input
- On invocation: route to MAIC chat with the prose script as a system-prompt instruction + model override to orchestrator-model (e.g. `milagro-dev`)
- Stream child sessions back to chat as a collapsible "Workers" panel
- Final synthesis message posted in main chat thread

### Surface 2: Terminal tile
- Add a shell picker option: `mc-openclaw` (existing), `bash`, `cmd`, **`prose`** (new)
- Selecting `prose` opens an inline REPL: `prose> ` prompt
- Commands: `run <file>`, `compile <file>`, `examples`, `agents` (list built-ins), `worker <id>` (attach to running worker stream)
- Streams the OpenProse VM's narration into xterm.js — exactly like a shell, but for prose programs

### Surface 3: Windows (Tauri multi-window system)
- Add a `Workers` page to `src/pages/workers.js` and register it in `src/page_registry.js`
- Open it in its own window via `openclaw_open_window` (or in-dashboard if rc55+ dashboard layout permits)
- Workers page: branch tree view, status pills (idle / running / done / error / cancelled), live transcript per branch, click-to-attach
- Reuses existing `mc-openclaw` TUI session lifecycle — same kill switch, same poll loop, just remapped for prose sessions

---

## Unified tool / model access contract

**Lesson 830 (must-have)**: a tool that works in chat must work identically in Terminal `prose>` REPL and in the Workers window. No surface-specific tool forks.

### Tool access — already proven
- Chat calls tools via MAIC's `tool_calls` channel with `tool_execution: "client"` (Lesson 169)
- Terminal `mc-openclaw` shell can already call `mc_terminal_*` Tauri commands
- Windows: each window inherits `__openclawHostBridge` (rc15) which exposes `window.__TAURI__.core.invoke` with multi-path fallback

### What needs unification
1. **Tool catalog source of truth**: extend `src-tauri/src/tools_main.rs` (or new `tools_registry.rs`) to publish the canonical tool list once. Both `chat.rs` (existing) AND a new `prose_host.rs` read from it.
2. **Model name mapper**: `.prose` files use OpenAI-style names (`model: sonnet`). MC needs to translate `sonnet → milagro-dev` (or fallback cascade). One mapper file: `src-tauri/src/prose_model_map.rs`.
3. **Session isolation**: child sessions from prose must have their own MAIC chat session ID (preserves context isolation). Reuse `maic::stream_chat` with a child-session-id UUID.
4. **Billing aggregation**: child sessions count toward the user's tier limits just like chat messages — no separate counter. MAIC already meters by session.

### Required Tauri commands (new)
| Command | Surface | Purpose |
|---|---|---|
| `prose_run(file_or_slug, ctx)` | All 3 | Compile + execute a `.prose` program, return session_id |
| `prose_compile(file)` | All 3 | Validate without running |
| `prose_poll(session_id, last_seq)` | All 3 | Stream new chunks for a running prose session |
| `prose_kill(session_id)` | All 3 | Cancel a running program |
| `prose_attach(session_id)` | All 3 | Attach to a worker (Terminal / Workers window) |
| `prose_examples(category?)` | All 3 | List bundled `.prose` library |
| `prose_list_agents()` | All 3 | List built-in + user-defined `agent name:` defs |
| `prose_install_extension(path)` | Settings | Install a user-extensions directory |

### State management
- Active prose sessions live in `AppState` (existing) keyed by `session_id` UUID
- State shape: `{ id, program, agents: Vec<AgentDef>, branches: Vec<BranchState>, status, started_at }`
- Each branch tracks: `{ id, agent, status, transcript: Vec<Msg>, tool_calls: Vec<ToolCall> }`
- Persisted to `~/.miracle-claw/prose-state/{session_id}.json` so Workers UI survives MC restart (Lesson 524: state durability)

---

## Built-in `.prose` library (Phase 3)

Bundle 6 starter programs in `src-tauri/resources/prose-library/`:

1. **explore.prose** — read-only codebase research; spawns one Explore agent per query, parallel style
2. **code-review.prose** — security / perf / style reviews in parallel (lifted from upstream example 16)
3. **fix-tests.prose** — write failing test + run + iterate; loop with retry
4. **multi-module.prose** — fan out FireCrawl + Translation + Voice for a content workflow
5. **pair-debug.prose** — three agents: reproduce / hypothesize / verify in parallel
6. **docs-from-code.prose** — read source, parallel write README + CHANGELOG + API docs

Plus: bundle the 48 upstream examples in `src-tauri/resources/prose-examples-upstream/` for "show me what's possible" demos (not user-facing until curated).

---

## UI polish contract

**Lesson 831 (must-have)**: customer-facing visuals.

### Sidebar rebrand
- Add a `Workers` tile to the dashboard
- Match existing tile style: same icon system, same hover, same Cmd-K integration
- Tile shows: count of active workers (badge), last completion latency, "kill all" button

### Workers page
- Branch tree at top, status pills (green=running, gray=idle, red=error, yellow=blocked-on-tool)
- Click branch → transcript pane on the right (collapsible per branch)
- Tool calls show inline with diffs/patch previews (reuse Lessons 805/848 patterns)
- Footer: kill / pause / export transcript / pin worker

### Chat integration
- `/prose <script>` result renders as a card with: program path, branch tree summary, total tokens used, total latency
- Inline "Open in Workers" button → jumps to Workers page with that session_id highlighted

### Terminal `prose>` REPL
- Tab completion for `run` / `compile` / `examples` / `agents` / `worker` (use xterm.js addon)
- Syntax-highlighted history (`up-arrow` recalls prior commands)
- Color-coded output: green for running, red for error, blue for prose VM narration, gray for tool calls
- Footer bar: `running: 3 / max: 10` (tier-aware cap)

---

## Phased ship plan (revised from 6 weeks → 1.5 weeks)

### Phase 1: Surface OpenProse from MC chat (3-4 days)
1. Add `prose_run` / `prose_compile` / `prose_poll` / `prose_kill` Tauri commands in `src-tauri/src/prose_host.rs`
2. Wire `/prose <script-or-slug>` slash command in `src/main.js` chat input handler
3. Model name mapper in `src-tauri/src/prose_model_map.rs`
4. Bundle the 6 starter `.prose` files in `src-tauri/resources/prose-library/`
5. Bundle the open-prose extension into the Tauri installer (copy `~/.openclaw/extensions/open-prose/` to `src-tauri/resources/extensions/open-prose/` and adjust `openclaw_extensions_dir()` resolution)

### Phase 2: Workers UI page (4-5 days)
1. New `src/pages/workers.js` exporting `mount(root, ctx)` / `unmount()`
2. Register in `src/page_registry.js`
3. Add `Workers` tile to dashboard
4. Wire up to the prose session state in `AppState`
5. Branch tree, status pills, transcript panes
6. Cmd-K palette entry

### Phase 3: Terminal `prose>` REPL (3 days)
1. Add `prose` to Terminal shell picker in `src/pages/terminal.js`
2. Wire input/polling to `prose_run` / `prose_poll` (reuses existing `mc_terminal_*` plumbing)
3. xterm.js tab completion addon
4. Color output routing (PROSE_NARRATION, BRANCH_STATUS, TOOL_CALL styles)
5. Docs in COMMAND_HELP dialog

### Phase 4: Polish + Windows (3 days)
1. Multi-window integration: open Workers page via `openclaw_open_window`
2. Tool catalog unification (Lesson 830 enforcement)
3. Visual polish pass: gradient hero on Workers page, animated status pills, transition on session completion
4. Onboarding tooltip: `✨ New: /prose runs agent teams`

---

## Risk register

| Risk | Mitigation |
|---|---|
| OpenProse skill assumes OpenClaw `sessions_spawn` works; need to verify it does in MC's embedded OpenClaw | Day 1 spike: write a 1-line `.prose` test, run from chat, confirm child session is spawned and returns |
| Child sessions don't see `tool_execution: "client"` flag — model won't see MC's tool schemas | Phase 1 day 1: write a stub subagent that calls `read_file`, confirm tool call comes back through MAIC |
| Bundling `open-prose` to Tauri resources breaks the dev-install path (extension lives in `~/.openclaw/extensions/`) | Phase 1 day 2: confirm `openclaw_extensions_dir()` resolution picks bundled first, dev fallback second |
| Model name `sonnet` doesn't map to anything in MAIC | Phase 1 day 1: model_map.rs covers `sonnet / opus / haiku / gpt-4* / gpt-5` → MAIC counterparts (`milagro-dev` is default) |
| User-defined `.prose` files in `~/.miracle-claw/prose-library/` could be malicious | Lesson 830 enforcement: only load on explicit install via Settings; never auto-execute remote `use` statements without approval (mirrors upstream `prose.md`'s "approve remote prose imports" gate) |
| Workers UI on Windows multi-window system can have stale state on window close | Persist session state to `~/.miracle-claw/prose-state/{session_id}.json`; reload on Workers page mount |

---

## Success metrics

| Metric | Target (30 days post-release) |
|---|---|
| User-invoked `/prose` runs | 100+ / week across installs |
| Built-in library activations | 50+ across `explore`, `code-review`, `fix-tests` |
| Terminal `prose>` REPL picks (vs shell picker total) | 15%+ |
| Avg subagent count per prose run | 3.2 (cross of 48 example patterns) |
| Crash-free session rate | >99.5% |
| Time-to-first-prose-success (after install) | <90 seconds |

---

## Changelog

- **2026-09-08 08:06 MDT**: Initial design. Approved by David. Phases 1-4 scoped. Lesson 830 (tool surface unification) + Lesson 831 (UI polish contract) drafted.
