# Plan: Wire all 8 modules + build user-friendly module cards

**Goal**: Every module in `/v1/modules/installed` should:
1. Actually work end-to-end when invoked (server-side `/v1/modules/{id}/run`)
2. Show a card in the MC launchpad/marketplace that exposes ALL its tools, not just the first
3. Have user-facing instructions on how to use it

**Trigger**: David tried "Build a 3-step email sequence Audience Auto Repair Goal Customer" in MC today and got `HTTP 404: {"detail":"Not Found"}` from `/v1/modules/email-campaigns-pro/run`. All 8 modules have this problem — the directory works, but the runner was never built (Lesson 1109 follow-on).

---

## Inventory: 8 modules, 12 tools total

| Module | Tools | Server capability needed |
|---|---|---|
| ad-performance | summarize | Stub: needs Google Ads API key (none configured). Mark as "coming soon" or build a demo data path. |
| brand-asset-kit | bio | LLM-generated brand bio (cheap, fast) |
| calendar-companion | find_slot | Stub: needs Google Calendar OAuth. Mark "coming soon". |
| data-analyzer | summarize | Fetch CSV from URL → LLM summary |
| email-campaigns-pro | sequence | LLM-generated email sequence |
| firecrawl | fetch_url, crawl_site | Client-side sidecar module; on server, return "runs client-side" with redirect |
| marketing-suite | draft_email, social_post | LLM-generated drafts |
| voice-memo | transcribe | Client-side whisper.cpp; on server, return "runs client-side" |

---

## Server: `POST /v1/modules/{id}/run`

Add to `/opt/maic/api/routes/modules.py`:

### Route signature
```python
@router.post("/v1/modules/{module_id}/run")
async def run_module(
    module_id: str,
    payload: RunRequest,
    request: Request,
    p: Principal = Depends(...),
) -> RunResponse:
    ...
```

### Request schema
```python
class RunRequest(BaseModel):
    tool: str  # matches client_manifest.tools[].name
    inputs: dict  # matches client_manifest.tools[].form_schema
    surface: str = "tauri"
```

### Response schema
```python
class RunResponse(BaseModel):
    outputs: dict
    duration_ms: int
    run_id: int  # PK in module_runs
```

### Dispatch table
```python
RUNNERS = {
    ("email-campaigns-pro", "sequence"): _run_email_sequence,
    ("marketing-suite", "draft_email"): _run_draft_email,
    ("marketing-suite", "social_post"): _run_social_post,
    ("brand-asset-kit", "bio"): _run_bio,
    ("data-analyzer", "summarize"): _run_csv_summary,
    ("firecrawl", "fetch_url"): _run_client_side_redirect,
    ("firecrawl", "crawl_site"): _run_client_side_redirect,
    ("voice-memo", "transcribe"): _run_client_side_redirect,
    ("ad-performance", "summarize"): _run_coming_soon,
    ("calendar-companion", "find_slot"): _run_coming_soon,
}
```

### LLM helpers (4 of them share a chat_completion pattern)

Each takes the inputs, builds a system+user message pair, calls `chat_completion()`, parses output. Reasonable temperature (0.7 for creative, 0.2 for analytical).

- `_run_email_sequence(goal, audience)` — system: "You are an email marketing strategist..." user: "Build a 3-step email sequence for {audience} with goal {goal}..." Output: `{sequence: [{step, subject_a, subject_b, body}], duration_ms}`
- `_run_draft_email(to_audience, key_points, tone)` — system: "You are an expert copywriter..." Output: `{subject, body, cta}`
- `_run_social_post(platform, topic)` — system: "You are a social media manager..." Output: `{text, hashtags: []}`
- `_run_bio(platform, keywords)` — system: "You write platform-native bios..." Output: `{bio, headline}`
- `_run_csv_summary(csv_url, focus)` — fetch CSV → first 5 rows → LLM summary. Output: `{summary, key_takeaways: [], anomalies: []}`
- `_run_client_side_redirect(...)` — return `{outputs: {client_side: true, message: "This module runs locally. Please use the [Your Tools] tile to invoke it."}}` with status 200 so the client knows it's intentional.
- `_run_coming_soon(...)` — return 501 with detail "This module is in private beta. Join the waitlist at...".

### Persistence
- INSERT into `module_runs` with `user_id`, `module_id`, `tool_name`, `inputs`, `outputs`, `duration_ms`, `surface`
- Return `run_id` to client for "recent runs" UI

### Auth / quota
- Same `_require_user` pattern as the other MAIC routes
- Per-user rate limit: 30/min on runs (configurable)
- Tier check via existing `tier_required` field on the module

---

## Client: Module cards in launchpad + marketplace

### Current state (rc57.8)
- `dashboard_launchpad.js` renders module tiles with the FIRST tool's name only
- `openModuleModal` only handles the first tool
- No "how to use this" guidance
- Marketplace page lists modules but no preview of tools

### What we'll change for rc57.9

**Launchpad (Your Tools section)**:
- Each module tile is now a "card" with:
  - Icon + name + description
  - "Show tools (N)" button → expands the card to list all tools with short descriptions
  - Each tool → "Use this tool" button → opens the form modal
  - Below tools: "💡 How to use this module" expandable hint with one-sentence guidance
- Bigger, more inviting card design

**Marketplace page** (modules not yet installed):
- Same card design
- "Tools (N)" preview (collapsed by default)
- Big "Install" button → after install, the card flips to "Open" with the same tools

**Modules dropdown in toolbar**:
- Now lists ALL tools per module, not just the module name
- Click a tool → opens the form for that tool

**Per-module instructions** (in `client_manifest.instructions`):
- Server adds an `instructions` field to each module's `client_manifest` with 1-3 sentences per module
- Client renders this in the modal "How to use" section

### Instructions content (server adds to client_manifest)

```
ad-performance: "Paste a date range like 'last 30 days' and your
  campaign ID (if you have one). We'll summarize clicks, conversions,
  and ROI. Need Google Ads linked? Open Settings → Integrations."

brand-asset-kit: "Pick your platform and add a few keywords that match
  your brand voice. We'll write a bio tuned for that platform's style —
  short and punchy for Twitter, professional for LinkedIn."

calendar-companion: "Tell us how long the meeting needs to be and
  who's attending. We'll find a free slot that works for everyone.
  Coming soon: Google Calendar sync."

data-analyzer: "Drop in a public CSV URL (or upload via [Add file])
  and tell us what to focus on. We'll summarize the data, pull out
  the most interesting rows, and flag any anomalies."

email-campaigns-pro: "Describe your goal in one or two sentences —
  'win back inactive customers', 'promote a brake special', 'follow
  up after a service'. Optionally tell us the audience. You'll get a
  3-email drip with A/B subject lines for each step."

firecrawl: "Paste a URL and we'll pull the clean text and metadata.
  Use 'crawl' to grab a whole site (up to N pages). Runs locally on
  your machine — nothing is sent to our servers."

marketing-suite: "Write a one-paragraph brief for your campaign.
  We'll generate matching emails, social posts, and ad copy in one
  go. Pick a tone to set the voice (friendly, professional, etc.)."

voice-memo: "Record a voice memo from the chat input (mic icon).
  We'll transcribe it locally on your machine and extract any action
  items. Audio never leaves your device."
```

---

## Build & ship

1. Implement server route + rebuild docker image (Lesson 195)
2. Add `instructions` field to all 8 modules' `client_manifest`
3. Implement client card redesign
4. Test end-to-end with 3-4 modules
5. Build rc57.9 Windows installer
6. Copy to David's Desktop

---

## Files we'll touch

### Server
- `/opt/maic/api/routes/modules.py` (add run endpoint + helpers)
- `module_installs`, `module_runs` (already exists)
- 8 modules' `client_manifest` (add `instructions` field)

### Client
- `src/pages/dashboard_launchpad.js` (card redesign, expand-tools)
- `src/pages/marketplace.js` (mirror card design)
- `src/modules-menu.js` (dropdown lists tools)
- `src-tauri/resources/dist/control-ui/mc-toolbar.js` (dist version, same change)
- `src/styles.css` (card styles)

### Build
- `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json` → 1.2.0-rc57.9

---

## Risk / known unknowns

- **Voice-memo + Firecrawl** return client-side redirects. The launchpad card UI for these needs to detect that and show "Use from chat input" instead of the form modal.
- **No API keys for ad-performance / calendar-companion**. Plan is to show "coming soon" — better than 404. If we want to wire these for real, that's a separate ticket (OAuth flow, Google Ads integration).
- **chat_completion() cost**: every LLM tool costs tokens. Make sure we route through user's quota. Existing `_check_quota` patterns in chat.py should work.
- **module_runs table growth**: with 8 modules × 30/min/user, could grow fast. Daily cleanup or partition is fine for now.
- **Module descriptions + instructions** are entered by us right now. For a customer-facing product this should be CMS-managed, but that's a separate ticket.

---

## Estimated scope

- Server route + 4 LLM tool impls + persistence + auth: ~250 lines Python
- Server `instructions` updates: SQL UPDATE statements
- Client card redesign + expand-collapse + tool selector: ~150 lines JS
- Marketplace mirror: ~100 lines
- Styles: ~80 lines CSS
- Test + installer: standard 30 min

Realistic total: 2-3 hours of focused work. I'll split between a sub-agent for server-side and direct work for client-side.

---

**Created**: 2026-10-02 by ABQShop Claw for David's MC v1.2.0-rc57.9 modules feature.