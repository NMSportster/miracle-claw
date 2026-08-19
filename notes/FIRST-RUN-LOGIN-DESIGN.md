# First-Run Login UI — Design Question (2026-08-18 19:35 MDT)

## What we have

- MC v1.0.0 `index.html` is a 17-line placeholder ("Loading chat...")
- The actual chat UI the user sees is **openclaw's bundled WebChat** at `http://localhost:28789/`
- MC's Rust side already has `tauri::command fn maic_login(email, password)` fully wired with HTTP POST to `/v1/auth/login` — backed by `ureq` sync HTTPS
- The login command writes the JWT into `openclaw.json` and re-bootstraps the MAIC provider

## What David asked for (verbatim from 19:25 MDT)

> "Yes we should have the first run login UI with the first run, we are going to have to do it. Make sure it works from the start."

## The architectural choice

Where does the first-run login UI live? Three options:

### Option A — Build a real MC frontend (replace 17-line placeholder)
Build a real MC Tauri webview UI that:
1. On startup, calls `invoke('first_run_report')` to check if MAIC is configured
2. If not, shows login form (email + password) → calls `invoke('maic_login')`
3. After login, redirects/replaces webview with `http://localhost:28789/` for the openclaw chat UI
4. If MAIC is configured, just go straight to the openclaw chat UI

**Pros**: proper UX, customer-facing polished, MC owns the user experience
**Cons**: significant UI work, Vite + React/Vue setup, distracts from "chat roundtrip is the release gate" (Lesson 432)
**Cost**: ~3 hours of UI work, ~1000 lines of code

### Option B — Openclaw has a built-in onboarding/login flow?
Investigate whether openclaw's bundled WebChat already supports a first-run login form. If it does, MC just needs to expose the auth endpoint as needed.

**Status**: I haven't found evidence of this. openclaw has `/v1/auth/login` but its own bundled WebChat may or may not have a login form. Need to check.

**Pros**: zero MC UI work
**Cons**: if openclaw doesn't have it, this option is dead

### Option C — Leverage MAIC's web dashboard for login, then deep-link back
Send user to `https://milagrocloud.com/login`, they log in there, JWT is returned to MC.

**Pros**: no MC UI work
**Cons**: worse UX, user context-switches, not what David asked for

### Option D — MC's own Tauri webview shows a login overlay BEFORE the gateway is even booted
Tauri webview loads MC's own HTML/JS. That page calls MAIC's `/v1/auth/login` directly (Rust `tauri::command` is just one option; we can also do it from JS via `fetch`). After login, store JWT, then load `localhost:28789` for the chat.

**Pros**: minimal UI, no React setup, JS-only
**Cons**: still real UI work, ~100-300 lines

## David's pre-existing context

- MEMORY.md Lesson 432: "Chat roundtrip is the release gate, not gateway boot"
- 17:14 MDT "no bandaids" rule
- Customer-facing install must Just Work on clean Windows

## What I propose (asking David)

**Build the minimal first-run login form in MC's own webview** (Option D, simpler than A):

1. Replace `index.html` with a small JS SPA-less login form
2. On startup, call `invoke('first_run_report')` to detect "no key configured"
3. If not configured: show login form → `fetch POST /v1/auth/login` directly (or via `invoke('maic_login')`)
4. On success: store JWT, hide login form, load `http://localhost:28789/` (openclaw WebChat) in same webview
5. Skip the login form if MAIC is already configured

This way:
- Customers see a clean login form on first install
- After login, they get the existing openclaw chat
- No major frontend refactor — Vite + plain JS, maybe ~200 lines

But before I commit to this, I want David's answer to **one question**:

**Question**: Is the customer-facing UX the priority, or is "chat roundtrip works end-to-end on the backend" the priority?

If customer-facing UX: build Option A (~1000 lines, 3 hours UI)
If "backend correctness first": skip the UI for now, keep the env-var + SecretRef path working, ship v1.0.1 as backend-correct, defer login UI to v1.0.2

I think the right answer is Option D (~200 lines, ~1 hour) for v1.0.1:
- Customers see a login form on first install
- Backend stays the same (already correct)
- v1.0.2 can polish the UI

But you tell me — am I building more than you want right now?

