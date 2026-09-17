# Smoke test: rc56.0 Module-Driven Surfaces (Weeks 1-7)

**When to run:** Before unveiling rc56.0, or any time you suspect
cross-surface drift between the Tauri launchpad, the `mc-terminal` TUI,
and the `mclaw` CLI. Each surface calls the same MAIC endpoints and
should produce consistent, predictable results.

**Time:** ~10-15 minutes if you've never run it; ~3 minutes once
familiar.

**Prerequisites:**

- `mclaw` binary on `$PATH` (built from `mclaw/target/release/mclaw`)
- `miracle-claw-terminal` binary on `$PATH` (bundled with the Tauri
  installer; or built from `src-tauri/target/debug/miracle-claw-terminal`)
- Tauri app built and installed (`MiracleClaw_1.2.0-rc56.0_*_x64-setup.exe`
  or `tauri dev`)
- A MAIC account with a verified email (use the test account
  `mclaw-bucket2-test@milagro.cloud` if you don't want to spam your own
  with module installs)

**What this verifies:**

1. The three surfaces call the same MAIC endpoints
2. The `client_manifest` schema is the same across all three
3. Install / uninstall / run work the same in all three
4. The empty-state landing page renders (mc-terminal, launchpad)
5. Form validation surfaces in all three surfaces
6. Error mapping (`401 session expired`, `404 not installed`, etc.) is
   friendly in all three

---

## Step 1 — Log in once

Pick one surface to log in. The other two will inherit the JWT.

```bash
# Easiest path: log in via the mclaw CLI (writes JWT to OS keychain).
mclaw login
# Email: <your test account>
# Password: <password>

# Confirm
mclaw whoami
# Expected: { "email": "...", "tier": "free", "plan_code": "free", ... }
```

**Sanity check (should match):**

- Tauri app: open the sidebar; your email + tier should be visible in
  the account dropdown.
- `mc-terminal`: launch and press `?`; the status line should show
  your email.

## Step 2 — List installed modules (the cross-surface test)

This is the key step — all three surfaces must show the same list.

### From mclaw

```bash
mclaw tools
```

Expected: a categorized list of installed modules (probably empty for
a fresh test account). Try `--json` to see the wire shape:

```bash
mclaw tools --json | jq '.grouped_by_category'
```

### From the Tauri launchpad

1. Open the Tauri app
2. Click the **Launchpad** tile on the dashboard
3. Expected:
   - **If no modules installed**: 4 starter tiles
     (data-analyzer, marketing-suite, brand-asset-kit, firecrawl) plus
     a "+ Browse all" tile
   - **If some installed**: an "Your Tools" grid with the same modules
     you saw in `mclaw tools`

### From mc-terminal

```bash
miracle-claw-terminal
```

Expected:
- **If no modules installed**: the empty-state landing page
  ("Welcome! You don't have any modules installed yet.")
- **If some installed**: a toolbar with one row per (module, tool)
  pair, e.g. `firecrawl · scrape`

**What to look for:**

| Symptom | Likely cause |
|---|---|
| mclaw shows 0 modules, Tauri shows 3 | JWT not shared — `mclaw logout && mclaw login` then re-launch the Tauri app |
| mclaw shows 3, Tauri shows 0 | Stale JWT in Tauri — sign out and back in |
| mc-terminal shows blank | `MIRACLE_CLAW_DEBUG=1` and check logs; usually a session-file path mismatch |
| Any surface 500s | MAIC outage — check `https://maicserver.com/v1/health` |

## Step 3 — Browse the marketplace (all three)

### From mclaw

```bash
mclaw marketplace
# or filter by category:
mclaw marketplace data
mclaw marketplace marketing
```

Expected: a list of ~16 modules grouped by category. Note the
`module_id` of one (e.g. `data-analyzer`).

### From the Tauri launchpad

1. Click the **Launchpad** tile, then **Browse all** (or navigate to
   the `/marketplace` page directly)
2. Expected: same 16 modules, grouped by category, with icons + short
   descriptions

### From mc-terminal

mc-terminal v0.1.x does NOT include a marketplace browser (it's a
read-only TUI for installed modules). Press `a` and you'll see the
"Coming in v0.2.0 (Week 5)" stub — use mclaw or Tauri to install.

## Step 4 — Install a module (all three should see it)

### From mclaw

```bash
mclaw marketplace install data-analyzer
```

Expected:

```
✓ Installed data-analyzer
```

Now `mclaw tools` should show `data-analyzer` in the data category.

### Verify in Tauri

1. Open (or re-open) the Launchpad page
2. The "Your Tools" grid should now show `data-analyzer`
3. Click it; the form should render the `summarize` tool's inputs

### Verify in mc-terminal

1. Quit the TUI (`q`) and re-launch
2. The toolbar should now show `data-analyzer · summarize`

**If the Tauri/TUI doesn't update:** the install happens server-side
in MAIC, but the local cache (`session.json`) doesn't auto-refresh.
Quit and re-launch the surface (the next launch re-fetches the
installed-modules list).

## Step 5 — Run a tool (all three should produce the same shape)

### From mclaw

```bash
mclaw run data-analyzer.summarize --input dataset=customers --input rows=10 --json
```

Expected:

```json
{
  "run_id": 7,
  "outputs": "...",
  "duration_ms": 1234
}
```

### From the Tauri launchpad

1. Open Launchpad → data-analyzer tile
2. Fill the form (or accept defaults)
3. Click **Run**
4. Expected: the same `{outputs, duration_ms, run_id}` shape, rendered
   in the result modal

### From mc-terminal

1. Select `data-analyzer · summarize` in the toolbar
2. Press `Enter` to fill the form
3. Tab through fields, type values
4. Press `Enter` to submit
5. Expected: same `{outputs, duration_ms, run_id}` shape in the
   Result view (PageUp/PageDown to scroll)

**What to look for:**

| Field | All three must include |
|---|---|
| `run_id` | integer (increments per run) |
| `outputs` | string (model-formatted answer) |
| `duration_ms` | integer (server-measured) |

If a surface is missing any of these, it's a regression in the
client-side deserializer.

## Step 6 — Form validation (Tauri + mclaw, mc-terminal TBD)

### Empty required field

```bash
mclaw run data-analyzer.summarize --input dataset="" --json
```

Expected:

```json
{ "error": "Validation failed: dataset is required" }
```

Exit code 64 (bad CLI args).

In the Tauri launchpad: try submitting a tool form with a required
field empty. The form should block submission with a red "required"
indicator.

### Non-numeric number field

```bash
mclaw run data-analyzer.summarize --input dataset=customers --input rows=abc --json
```

Expected:

```json
{ "error": "Validation failed: rows must be a number" }
```

Exit code 64.

## Step 7 — Friendly error mapping

### Unauthenticated (after logout)

```bash
mclaw logout
mclaw run data-analyzer.summarize --input dataset=customers --json
```

Expected (exit code 2):

```
error: not logged in. Run `mclaw login` first.
```

### Module not installed

```bash
mclaw login
mclaw run ghost.fetch_url --input url=https://example.com --json
```

Expected (exit code 1):

```
error: MAIC error (404): module 'ghost' not found
```

In the Tauri launchpad, the result modal should show the same headline
plus a `[<details>]` for the raw response.

### Server unavailable

```bash
mclaw run data-analyzer.summarize --input dataset=customers --json \
    --maic-base https://nonexistent.maicserver.com
```

Expected (exit code 1):

```
error: MAIC temporarily unavailable. Check https://maicserver.com
```

## Step 8 — Empty state (mc-terminal only)

This is the only way to verify the first-launch landing page.

```bash
# Use a brand-new test account
mclaw logout
mclaw login --username <new-account@example.com>
mclaw marketplace uninstall data-analyzer  # if it was installed
miracle-claw-terminal
```

Expected: a centered welcome screen with the lobster + "Get started" 3
steps. Press `q` to quit; press `?` to overlay the help.

## Step 9 — Cross-check: all three surfaces show the same data

Final sanity check: at the end of the smoke test, all three surfaces
should show the same set of installed modules.

```bash
# From mclaw:
mclaw tools --json | jq -r '.modules[].module_id' | sort

# From Tauri: eyeball the Launchpad "Your Tools" grid
# From mc-terminal: the toolbar should show the same modules
```

If the three lists diverge, file a bug; the canonical store is MAIC's
`module_installs` table, and any divergence is a client-side cache
issue.

---

## When this passes

- [ ] All 9 steps above completed without error
- [ ] No `cargo test` / `npm run test:js` regressions
- [ ] The Tauri app was killed cleanly (no orphan `node openclaw.mjs`
      processes; check with `pgrep -f "node.*openclaw"`)

## When this fails

- The contract test in `mclaw/tests/installed_response_contract.rs`
  and `src/pages/installed_response_contract.test.mjs` pins the wire
  shape. If a surface deserializes a fixture but not a live response
  (or vice versa), the server is the source of truth — file a bug
  against `api/routes/modules.py`.
- If you see a divergence between the Tauri launchpad and the mclaw
  CLI listing, the most likely culprit is a stale JWT in one of the
  surfaces. `mclaw logout && mclaw login` and re-launch the Tauri
  app to refresh.
