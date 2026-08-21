# notes/ — Miracle Claw project tidbits

This folder is for project-internal notes that help us be **efficient and
not waste time relearning**. Drop tidbits here whenever you discover:

- A non-obvious gotcha
- A pattern that worked
- A "this is the integrated MC-OpenClaw build, not a separate CLI" reminder
- A design decision worth remembering
- A gotcha specific to *this* machine or *this* build (WSL paths,
  Docker image state, etc.)

## Convention

- **One topic per file**, named `TOPIC-NAME.md` in UPPER-KEBAB-CASE
- **Lead with Context** — date, who, why this matters
- **Include the WHY** — not just the WHAT, so future-me gets the reasoning
- **Link to code** — point at the file/line where the relevant code lives
- **Cross-link** — reference MEMORY.md sections if it's also a long-term lesson

## Existing notes

- `HOMEBOT-MAIC-OWNERSHIP.md` — what HomeBot owns vs. what MC owns
  (MAIC plugin versioning)
- `FIRST-RUN-LOGIN-DESIGN.md` — first-run login UI design decision
- `V1.1.0-DASHBOARD-PLAN.md` — v1.1.0 dashboard redesign plan
- `INTEGRATION-MC-OPENCLAW.md` — the rule: MC IS the integrated MC-OpenClaw build

## When to add a note

- ⏱️ Before you forget — write it down now
- 📂 Before context gets too long — distill into a persistent file
- 🔄 When you rediscover something — write it down so you don't re-learn
- 🐛 When a bug took >10 min to diagnose — write down the symptom → cause → fix

## When NOT to add a note

- ❌ One-line fixes that git commit already covers
- ❌ User-facing docs (those go in `docs/` instead)
- ❌ Secrets or credentials
- ❌ Stuff that's already in MEMORY.md (memory is the long-term store)

## See also

- `/home/adeal/.openclaw/workspace/MEMORY.md` — long-term memory
- `STATUS.md` — current build state
- `CHANGELOG.md` — every shipped change
