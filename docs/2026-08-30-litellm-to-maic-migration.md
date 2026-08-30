# 2026-08-30 — LiteLLM → maic-server migration: the real fix

## TL;DR

Months of "models can't fire tools" pain came from LiteLLM stripping tool
schemas in transit. Routing everything through `maicserver.com` instead
preserves tool schemas and unlocks ALL paid-tier models — kimi, GLM,
MiniMax-M3, Nemotron. **No model surgery needed.**

## Before / After

### Before — broken routing

```
OpenClaw → LiteLLM proxy → Ollama Cloud
            │
            └─ strips tool schemas (because tools weren't registered
               in LiteLLM's litellm_params for those models)
```

Result: model sees no tools, falls back to OpenClaw built-ins
(`web_search`, `web_fetch`), which are broken (no Brave API key
configured). User sees "Agent couldn't generate a response" and
"No tools registered" in logs.

### After — fixed routing

```
OpenClaw → maicserver.com (FastAPI gateway) → LiteLLM → Ollama Cloud
              │
              └─ adds user's allowed tool list to request body with
                 `tool_execution: "client"` (Lesson 169)
              └─ preserves tool schemas end-to-end
```

Result: every paid-tier model (kimi, GLM, MiniMax-M3, Nemotron) can now
fire plugin's 7 paid-tier tools (`bash_run`, `read_file`, `write_file`,
`edit_file`, `list_dir`, `apply_patch`, `remember_fact`).

## What David saw at 02:02 MDT

> "Just so you know for the last 20+ minutes Kimi has had all tools and
> is doing work, look at the folder ai-tools set it up just to test and
> kimi is going off."

The kimi activity was already happening under rc55.11 — David noticed it
because he set up `/mnt/c/Users/Adeal/Documents/ai-tools/` as a test
folder and kimi started populating it with project scaffolds
(`meeting-notes-summarizer/`, `receipt-digitizer/`).

File creation timestamps: 00:44 → 01:53 MDT (~70 min of continuous work).

## Why I missed it earlier

I attributed kimi's improvement to:
- "Ollama-cloud hot-patched kimi" (wrong — same model version)
- "Different test prompt" (wrong — same kimi)
- "Model swap to GLM" (wrong — GLM was always working too, just under
  the same broken routing)

The truth: **all the models were always capable**. They just couldn't
see the tools because the routing layer was stripping schemas.

## What RC55.12 actually shipped

Three real fixes (defense-in-depth, not the root cause):
1. **Lesson 798**: prefix all model IDs with `"maic/"` in openclaw.json
   (fixes UI picker showing 4 models instead of 20)
2. **Lesson 795**: swap paid primary kimi → GLM (different tool-disciplined
   model as backup if kimi gets demoted)
3. **Lesson 796**: validate command body paths in bash_run before
   shell execution (MiniMax-M3 was picking paths outside Desktop)

**Not in RC55.12**: the LiteLLM → maic-server migration. That already
shipped earlier (commit history shows the endpoint change from
`http://litellm:4000` to `https://maicserver.com`). Tonight's
"breakthrough" was realizing that earlier migration was the actual fix.

## Postgres evidence

```sql
SELECT created_at, model, completion_tokens, raw_request->>'max_tokens'
FROM usage_events
WHERE created_at > now() - interval '30 min'
  AND model LIKE '%kimi%';
```

```
created_at           | model          | completion_tokens | max_tokens
---------------------+----------------+-------------------+------------
2026-08-30 08:00:36+00 | milagro-oc-kimi |                 0 |        600
2026-08-30 08:00:29+00 | milagro-oc-kimi |                 0 |        600
... (10 rows total, all completion_tokens=0, all max_tokens=600)
```

kimi is firing tools (file system shows real work) but the conversation
never reaches a "done" state because the 600-token ceiling kills it
mid-summary. RC55.13 will fix this with `max_tokens: 4000` override.

## Lessons

- **Lesson 794**: 600-token floor (postgres evidence)
- **Lesson 795**: model swap kimi → GLM (revised: both work, GLM kept for resilience)
- **Lesson 796**: bash_run path validation
- **Lesson 797**: MAIC /v1/models has no tier filter
- **Lesson 798**: maic/ prefix in openclaw.json
- **Lesson 799**: ask user what changed server-side before debugging model behavior

## Anti-patterns

- **AP-799-A**: When a model "suddenly" can't do what it should, suspect
  the routing layer first, not model capability.
- **AP-799-B**: Don't attribute success to the model when the fix was
  infrastructure.
- **AP-799-C**: Don't rebuild per-model logic when a one-line routing
  change fixes everything.

## File references

- Commit `be5690d` — RC55.12 ship
- `/home/adeal/.openclaw/workspace/memory/2026-08-30.md` — full session log
- `/home/adeal/.openclaw/workspace/MEMORY.md` — Lesson 794/795/796/798/799 entries
- `/home/adeal/Desktop/MC-rc55.12-status.md` — ship status
- `/home/adeal/Desktop/MC-rc55.12-readme.txt` — installer readme