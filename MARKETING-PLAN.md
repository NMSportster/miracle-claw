# Miracle Claw — Inexpensive Marketing Plan

**Author:** ABQShop Claw 🦞  **Date:** 2026-09-02  **Version:** v0.1 (draft)
**Target:** Paid user growth from current near-zero baseline, on **$0–$200/mo cash spend** + sweat equity.

---

## 0. What we are actually selling (so we don't waste money)

**Product:** Tauri desktop app (Windows installer, ~57 MB), wraps a local OpenClaw gateway that talks to MAIC (`milagrocloud.com`).

**Tagline on the landing page:** *"Built for You, not for everyone."* — use that, it's already approved.

**Real differentiators (all true, all on the page):**
- **Module catalog** — Voice, FireCrawl, Lead Generator, Translation, OCR, YouTube, Email, etc. (12 listed; 3 built, 9 in pipeline)
- **BYO models** — local (3B / 7B / 14B) or cloud (`milagro-dev`, `milagro-oc-*`)
- **BYO provider keys** — OpenAI / Anthropic / etc. via in-app vault (AES-256-GCM at rest)
- **No telemetry hostage-taking** — "Choose your own modules, your own models, your own setup."
- **Per-module quota** — Free tier usable, Pro/Team scale up. No module is paywalled.

**Who this is for (per Lesson 574, 2026-08-25):**
Worldwide, **NOT** ABQ/NM. Generic knowledge workers who:
- Want one desktop app instead of 12 SaaS subscriptions
- Want local-model privacy OR a single point for all their cloud models
- Are technical enough to install a `.exe` but not technical enough to build their own Open WebUI

**Excluded (don't market to these):**
- Enterprise procurement (slow, expensive to win, we have no sales team)
- Pure chat-no-tools users (ChatGPT / Claude.ai is fine for them)
- Mac / Linux desktop users (we ship Windows only right now — **gap**, see §6)

---

## 1. North-star + KPIs (90-day)

| Metric | Today (est.) | 90-day target |
|---|---|---|
| Free signups | <50 | 500 |
| Pro subscribers ($X/mo) | 1 | 25 |
| GitHub stars (public repo) | 0 | 200 |
| Discord members | 0 | 150 |
| NPS (in-app survey, quarterly) | — | ≥30 |
| Email open rate (digest) | n/a | ≥35% |

**Conversion assumption:** 5% free→paid at $9/mo starter = $225 MRR by day 90. Not retirement money — but it's the *foundation* that proves the funnel works, so we can put real spend behind it in Q4.

---

## 2. Positioning statement (use everywhere)

> **Miracle Claw is the desktop AI shell you actually own.**
> Pick your modules, pick your models, run it locally if you want. No telemetry hostage, no model lock-in, no $20/mo per seat.
> One install. Every AI you use. Forever free to try.

**Three-second pitch variants:**
- **Dev:** "ChatGPT, Claude, and Ollama in one desktop window. Bring your own keys."
- **Power user:** "Like Open WebUI, but it ships as an `.exe` and remembers your API keys."
- **Privacy:** "Your chats stay on your machine unless you choose a cloud model."

---

## 3. The four cheap channels (ranked by ROI for our budget)

### 🏆 Channel 1 — Show, don't tell (free, highest leverage)

YouTube + short-form demos. We have a working product and a recognizable mascot (the gold-bolt lobster). Each module = one 60–90s demo.

**Cost:** $0 (you + your phone, OBS for screen capture)
**Effort:** 1 video/week, 2 hours total (record 30 min + edit 90 min, ship by Sunday)
**Channels:** YouTube (long), TikTok + X/Twitter + LinkedIn (clipped 30s cuts of the same footage)

**Content calendar (rotate):**
- W1 — "5-minute setup walkthrough" (the actual landing-page steps, recorded)
- W2 — "Why I left ChatGPT for a desktop app" (rant with receipts)
- W3 — "BYO keys: switch models mid-sentence" (the killer demo)
- W4 — "Module: Voice for MiracleClaw" (Lesson 572 work, real product)
- W5 — "Module: FireCrawl for MiracleClaw"
- W6 — "Module: Lead Generator"
- W7 — "Local 14B on a $0 GPU rental" (Hetzner moondream story from MEMORY.md)
- W8 — "Build a real CLAUDE.md in 90 seconds" (configurable prompt setup)
- W9 — "MC vs Open WebUI vs Jan.ai" (honest comparison — wins us trust)
- W10 — "What's coming in Q4"
- W11 — "M1 training data: how MC actually runs on local 3B" (MAIC nerdery)
- W12 — "Year-end: what 500 free users taught us"

**Why this works:** you already spend hours building features — turn that into content with a screen recorder open. Zero marginal cost, long-tail SEO, social proof.

### 🥈 Channel 2 — Communities where our users already hang out (free, medium effort)

One daily post in **one** of these. Not all five. Not "spray and pray."

**Pick the TWO with our actual users** (per the worldwide framing — NOT r/Albuquerque, NOT r/AutoRepair):
| Where | Why fit | Cadence | My role |
|---|---|---|---|
| **r/LocalLLaMA** | local-model crowd, hates SaaS, will roast us but link us | 1 post/week | lurk daily, post once |
| **r/SelfHosted** | same crowd, more pragmatic | 1 post/week | same |
| **Hacker News (Show HN)** | one good launch post > 50 other posts | **1 shot, do it right** | ship the post, don't half-ass |
| **IndieHackers.com** | solo builders, want the funnel story | 1 post/month | reply to comments |
| **Dev.to / Hashnode** | long-form SEO, compounds for years | 1 article/month | cross-post from YT scripts |
| **Mastodon `#AI` / `#LocalAI`** | smaller, higher signal-to-noise | 2 posts/week | one-liners with screenshots |
| **Discord servers** (Open WebUI, Jan.ai, Ollama) | lurk, answer support questions, mention MC when relevant | 15 min/day | helpful, not spammy |

**Show HN post (one shot, must land):**
- Title: "Show HN: Miracle Claw – Desktop AI shell with BYO models and a module catalog"
- Body: 4 paragraphs. (1) what it is, (2) screenshot of the app, (3) why I built it (ChatGPT fatigue + model lock-in), (4) what's free / what's paid / roadmap
- **Cost:** $0
- **Effort:** 2 hours to write + 4 hours answering comments for 48 hours (have to show up, that's the deal)

### 🥉 Channel 3 — SEO + landing page (free to start, $20/mo to scale)

The landing page on `milagrocloud.com` already exists and ranks for nothing because it has zero backlinks. Fix that by:

1. **Add a `/modules/` page per module** (Voice, FireCrawl, Lead Generator, etc.) — long-tail keywords like "local voice transcription app", "open source web crawler desktop". $0.
2. **Add a `/docs/` section** (we have docs in `projects/miracle-claw/docs/`) — publish them publicly. $0.
3. **Add a `/changelog` page** — versioned, linked from the in-app updater. Each release = a fresh indexable URL. $0.
4. **Submit to directories once** (free): ProductHunt (launch day), Hacker News (above), BetaList, SourceForge, AlternativeTo. 2 hours total.
5. **Optional: Buy 1 backlink/month** ($50-150) from a single relevant blog post (e.g., a guest post on `The New Stack` or `Hacker Noon` if they accept; most do for free). Defer until we have 100 free signups, so we know the funnel converts.

**Effort:** 1 weekend to write the 3 module pages + changelog scaffold. After that, 30 min/week to update.

### 🏅 Channel 4 — Discord + email list (cheap, compounds over years)

**Discord server** (free, via existing OpenClaw / Milagro community if it exists; otherwise spin up a 5-channel server in 30 min):
- `#welcome` `#announcements` `#support` `#modules` `#showcase`
- **Rule:** anyone who posts a screenshot of MC working with a custom module gets a 🦞-react from you. Always.
- **Cost:** $0
- **Effort:** 15 min/day to answer 3-5 questions

**Email list** (free tier: Buttondown or Mailerlite up to 1k subscribers):
- One weekly email, Friday AM. 5 links from the week (releases, blog posts, community highlights, one "what I'm thinking about").
- **Why email > Twitter:** email list compounds; Twitter reach collapsed 2023+.

---

## 4. The "expensive" things we explicitly do NOT do yet

These all cost money OR attention OR credibility we haven't earned. **Skip until we hit 100 free users:**

| Tactic | Cost | Why skip for now |
|---|---|---|
| Google / Reddit ads | $500+/mo | We don't have landing-page conversion data yet. Burning money on <2% CVR. |
| Sponsored newsletter slots | $200-$2,000 per send | Same. Plus our pitch isn't sharp enough yet. |
| Influencer (YouTuber / TikToker) | $500-$10k per video | Won't convert at our pricing. Revisit at 1k MRR. |
| Conference booth / sponsorship | $2k-$20k | Zero ROI for desktop indie at this stage. |
| Hiring a marketer | $4k+/mo | Premature; we don't have PMF signal. |
| Native macOS / Linux builds | 2-4 weeks engineering | Big audience upside but ships nothing new for current Windows users. **Do as Q1 2027 stretch goal.** |
| Enterprise tier | n/a (engineering time) | "Sales tax" — we'd need SSO, audit logs, MSA, on-call rotation. Not now. |
| Code signing cert (Authenticode) | $200-400/yr | **DO get this** if it's cheap — it removes the SmartScreen "unknown publisher" warning. Pays for itself in 10 installs. See §6. |

---

## 5. The "free but painful" things we SHOULD do (do these first)

These cost time, not money. Prioritized by impact:

1. **Make the installer less scary** (1 day)
   - Code-sign it (Authenticode cert, ~$200/yr from SSL.com or SignTool). Removes SmartScreen warning.
   - Add a one-page "What this installs" PDF that the installer links to. Trust signal.

2. **Ship a public GitHub repo** (1 day)
   - Even if the MAIC provider plugin is private, ship the Tauri shell, launcher, and module installer as open source.
   - Stars + visibility + bug reports + contributions. The classic indie play.

3. **First-run experience rewrite** (1 week)
   - Skip "create account" until the second prompt. First-run should be: install → open → see a working chat in 30 seconds.
   - **This is the single highest-leverage thing in the entire funnel.** Every leaked signup at the login wall = a lost user.

4. **In-app "share this" button** (2 hours)
   - After a useful response, button: "📣 Share this thread". Pre-fills a tweet. Costs nothing, drives organic social.

5. **Module marketplace landing pages** (1 weekend)
   - One page per module with a 60s demo GIF + install button. Each page is a marketing site that converts.

6. **Quarterly NPS survey** (1 hour per quarter)
   - One question in-app: "How likely are you to recommend Miracle Claw?" with an optional textbox.
   - Track over time. Show up when the score dips.

7. **Public roadmap** (2 hours)
   - GitHub Projects board, public. People upvote the things they want. This is the cheapest market research ever.

---

## 6. Other "first $X" we should spend (the cheap wins)

| Item | Cost | Why |
|---|---|---|
| **Authenticode code signing** | $200/yr | Removes the "Unknown publisher" warning. 10x ROI on install conversion alone. |
| **Domain `milagrocloud.com` renewal** | $15/yr | Already paid; just don't let it lapse. |
| **Buttondown free tier** | $0 | Email list up to 100 subs. |
| **Discord Nitro (basic)** | $0 | Free tier is fine for our size. |
| **OBS Studio + DaVinci Resolve** | $0 | Already have, or get for free. |
| **1x backlink per month** (Q4 only) | $100-150 | After we have 100+ free users to convert. |
| **TOTAL Q3 spend** | **~$215** | |

---

## 7. Timeline + who does what

| Week | Owner | Deliverable |
|---|---|---|
| W1 | David | Code-sign installer (Authenticode cert purchase + integration) |
| W1 | ABQShop Claw | First 3 YouTube demo videos recorded + edited |
| W1 | ABQShop Claw | GitHub repo public (Tauri shell + launcher, MAIC plugin kept private) |
| W2 | David | First-run UX rewrite shipped |
| W2 | ABQShop Claw | Show HN post drafted (do NOT post yet — wait until W3) |
| W2 | ABQShop Claw | `/modules/voice`, `/modules/firecrawl`, `/modules/lead-gen` landing pages |
| W3 | David | "Ship" Show HN post (4hr window of availability for comment replies) |
| W3 | ABQShop Claw | Discord server live + 10 invite links in seed communities |
| W4 | ABQShop Claw | ProductHunt launch (do Show HN first; PH likes a known project) |
| W5-12 | ABQShop Claw | 1 YouTube video/week + 1 long-form blog post/month + 2 community posts/week |
| W5-12 | David | Ship 1 module/month (Voice, FireCrawl, Lead Gen, Translation, OCR, etc.) |
| W12 | Both | NPS survey + 90-day retro |

---

## 8. The single biggest risk (and how to dodge it)

**Risk:** We get 500 free signups, none convert to paid, and we can't figure out why.

**Dodge:** Don't ship Channel 4 (email/Discord scaling) until W4. Force ourselves to learn from W1-W3 first. **Track the funnel weekly:**
- Landing page visitors → free signup → first chat → 10th chat → paid conversion
- Find the leak. Fix it. THEN scale.

If W4 shows <2% free→paid, the problem isn't marketing — it's pricing or onboarding. Pause, fix, resume.

---

## 9. What "success" looks like (so we know when to scale)

- **Day 90:** 500 free users, 25 paid = $225 MRR, GitHub 200 stars, Discord 150 members. Decision point.
- **If yes:** Double down. Add macOS/Linux builds (open up half the addressable market). Bump ad spend to $500/mo.
- **If no:** Pivot to enterprise or to a different module catalog. Don't pour more into a broken funnel.

---

## Appendix A — The 12-module catalog (from MEMORY.md, current source of truth)

This is what we market, in this priority order. Modules that don't exist yet are marketing promises we need to either ship or remove:

| # | Module | Status |
|---|---|---|
| 1 | 🎙 Voice | ✅ Built (rc53.14) |
| 2 | 🔥 FireCrawl | 🟡 Next |
| 3 | 🎯 Lead Generator | 🟡 After FireCrawl |
| 4 | 🌐 Translation | ⏳ Future |
| 5 | 📄 PDF & Document Parser | ⏳ Future |
| 6 | 🎥 YouTube Transcript | ⏳ Future |
| 7 | 📧 Email Writer | ⏳ Future |
| 8 | 🔗 CRM Sync | ⏳ Future |
| 9 | 🔍 Local File Search | ⏳ Future |
| 10 | 🔊 Text-to-Speech | ⏳ Future |
| 11 | 📸 Local OCR | ⏳ Future |
| 12 | 📅 Calendar Assistant | ⏳ Future |

**Rule:** never market a module that's not built. Marketing vaporware erodes trust faster than no marketing.

---

## Appendix B — Anti-patterns to refuse

Even when they feel cheap:
- ❌ Reddit / X spam (one post → downvote spiral → banned)
- ❌ Buying fake GitHub stars
- ❌ "AI wrapper" hype tweets with no substance
- ❌ Fake urgency ("only 24 hours left!!!") on a perpetual license
- ❌ Hidden costs at checkout
- ❌ Dark patterns in the installer
- ❌ Auto-opt-in to anything on first run

**Trust is the only moat for indie desktop software in 2026. Don't burn it for short-term growth.**

---

*Next step: David review this, tell me which channels he wants to actually own (he should do Show HN himself, I can run the rest), and we'll lock the W1-W4 calendar.*
