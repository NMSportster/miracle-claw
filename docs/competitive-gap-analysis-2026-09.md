# Miracle Claw Competitive Gap Analysis

**Date:** 2026-09-08
**Author:** ABQShop Claw 🦞
**Status:** Living document — revisit quarterly
**Sources surveyed:** Claude Code (v2.1.263), Cursor CLI, Gemini CLI (v0.58.0), Codex CLI, Aider, OpenCode, Warp, Amp

---

## Why this exists

David asked (2026-09-08): "Looking at Cursor and other CLI programs. What are some of the common tools they have that Miracle Claw does not?"

This is the canonical answer. Updated as we close gaps.

---

## How MC differs from a CLI tool (terminology check, Lesson 829)

**MC is a Tauri desktop app with an embedded Terminal tile** (xterm.js + the `mc-openclaw` TUI subprocess). The Terminal tile is NOT headless CLI mode in the competitor sense.

| Capability | MC Terminal tile | True headless CLI (Claude Code `claude -p`) |
|---|---|---|
| Invoke from any shell | ❌ | ✅ |
| Pipe stdout to other tools | ❌ | ✅ |
| Exit code for CI | ❌ | ✅ |
| Scriptable (cron, GitHub Actions) | ❌ | ✅ |
| JSON output for tool chains | ❌ | ✅ |
| Interactive REPL in a window | ✅ | ❌ |

**The real gap**: we have the interactive REPL but no standalone invokable binary that works outside the Tauri app. That's what closes the CI / GitHub Actions / cron story.

**Lesson 829** (David, 2026-09-08): "Don't conflate 'Terminal tile in a GUI app' with 'headless CLI mode'. They look similar; they solve different problems."

---

## 🟢 MC already has these (no gap — play to these strengths)

| Feature | MC surface | Source |
|---|---|---|
| Multi-file editing | `apply_patch`, `edit_file`, `write_file` tools | tools_main.rs |
| MCP client | OpenClaw MCP transport | gateway |
| Custom tools | 8 paid-tier local tools | `~/.openclaw/extensions/maic/openclaw.plugin.json` v0.3.3 |
| Streaming output | `stream_chat` | MAIC routes |
| Slash commands | `/model`, `/new`, etc. in chat | OpenClaw chat surface |
| BYO models + provider keys | AES-256-GCM vault | `provider_keys.rs` |
| Runs locally/offline | Ollama + `milagro-local-*` models | **REAL MOAT — nobody else does this cleanly** |
| Web search/fetch | MAIC server-side `web_search` tool | `/opt/maic/api/tools/` |
| Persistent sessions | `Session` resource | MAIC auth |
| Image/vision input | Chat surface supports image upload | MAIC routes |
| Todo/Task list | `Tasks` page | MC frontend |
| Module catalog | Voice, FireCrawl, LeadGen, Translation, OCR, YouTube, Email | **UNIQUE — no competitor has this** |
| Auto git commits | Aider ships this; MC doesn't | gap |
| Subscription login | MAIC JWT + tier mapping | MC auth |

---

## 🟡 MC has a weak/incomplete version

| Feature | Current MC state | What leaders ship | Ship priority |
|---|---|---|---|
| Plan mode | Implicit (model plans in prose) | Claude Code `/plan` + approval gate; Cursor `/plan [prompt]`; Gemini Planning Mode | Medium |
| Background tasks | None surfaced | Claude Code + Gemini CLI run async tasks while you keep chatting | Medium |
| Checkpointing / rewind | None | Claude Code `/rewind`, Cursor `/rewind` — jump back to any message | High |
| Diff view | Partial (chat shows patch prose) | Cursor/Gemini show side-by-side diffs inline | Low |
| MCP server (expose MC as MCP) | No | Claude Code + Gemini both ship. Becoming table-stakes for agent interop | Medium |
| Sandboxing / permission modes | No (bash_run runs anything) | Codex CLI 3-tier (read-only / auto / full). **Trust gap for enterprise** | **HIGH** |
| Voice input in chat surface | Module shipped (MC-Voice) but separate | Claude Code `/voice` = global STT → prompt | Low (Voice is its own thing in MC) |
| Context compaction | Implicit (MAIC-side only) | Claude Code `/compact`, `/summarize` — user-facing trim | Medium |
| Git/PR integration | No | Aider auto-commit; Claude Code `@claude` GH mentions | Medium |

---

## 🔴 MC is missing entirely (the real backlog)

| Feature | Why it matters | Effort | Reference leader |
|---|---|---|---|
| **Subagents / parallel agents** | The single biggest "wow MC is behind" feature. Claude Code has shipped this since April 2025 (348-day lead). OpenCode multi-session runs multiple agents on the same project. | Hard (3-6 weeks) | Claude Code subagents |
| **Hooks** (run shell on edit/task-finish) | Power-user feature. Enables CI integration, auto-format, custom linters. Claude Code has shipped since June 2025. | Medium (1-2 weeks) | Claude Code hooks |
| **Checkpointing / `/rewind`** | Users EXPECT this once they hit 50+ turn sessions. Without it, long sessions are a sunk-cost trap. | Medium (2-3 weeks) | Claude Code `/rewind` |
| **Permission modes** | Codex CLI's 3-tier model is the right pattern. Customers won't install MC in corporate environments without this. | **Hard (3-4 weeks)** | Codex CLI permissions |
| **Headless / non-interactive mode** | Every 6 leaders ship `claude -p "fix bug"` style invocation. MC has no CLI binary. The original Python `miracle-claw` CLI was deprecated when we moved to Tauri (Aug 2026). | **Medium (1-2 weeks)** — needs resurrection | Claude Code `claude -p` |
| **GitHub integration** (`@maic` mention bot) | Claude Code's `@claude` GitHub Action is a growth channel — every GH repo using it sees the brand. | Hard (1-2 months) | `@claude` GH Action |
| **Voice input in chat input bar** | Capability exists (Voice module) but isn't wired to the chat input. Easy win. | Easy (3-5 days) | Claude Code `/voice` |

---

## 💎 MC advantages not being marketed

| Feature | Leader equivalent | MC's advantage |
|---|---|---|
| **Local-only mode with full tool-use** | Aider, OpenCode with Ollama (limited — chat only) | MC runs full 14B with tool-use + module catalog OFFLINE |
| **Module marketplace** | None of them have this | FireCrawl, Voice, LeadGen, Translation, OCR, YouTube, Email — pick & mix |
| **Multi-provider billing aggregation** | Cursor has it for models, bare-bones | MC's billing routes 5 plans (free → enterprise) + usage tracking |
| **AES-256-GCM secret vault** | Most competitors store keys in plain text | MC's `provider_keys.rs` is real security work |
| **Mobile companion (pairing)** | None of the desktop CLIs have phone pair | Phase 2.3 just landed (Sep 7, 2026) |

---

## Recommendations

**Top 3 picks** (in order):

1. **Permission modes / sandboxing** — closes enterprise trust gap. Without this, no Fortune 500 customer.
2. **Headless CLI mode** — resurrects the `miracle-claw` binary as a standalone invokable process. Unlocks CI, GitHub Actions, cron.
3. **Subagents / parallel agents** — closes the biggest capability gap. But 6 weeks.

**Easy wins** (in order):

1. **Voice input in chat bar** (3-5 days)
2. **Hook framework** (1-2 weeks)
3. **MCP server mode** for MC (1-2 weeks)

---

## Changelog

- **2026-09-08**: Initial analysis. Created from David's webchat question. 18 features surveyed across 8 leader tools.