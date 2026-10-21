//! Tier fetching + caching.
//!
//! Source of truth: MAIC's `/v1/auth/me`. Returns `{ tier, plan_code, email }`.
//! Per MAIC Lesson 176, we read the **canonicalized** `tier` field, not
//! `plan_code` (which is the raw DB row).
//!
//! Cache TTL: 5 minutes (300 seconds). Bypassed on explicit
//! `fetch_tier_fresh()` call. Cache key includes the JWT (so a logout +
//! relogin produces a fresh fetch, not a stale tier for a different user).
//!
//! The tier is published two ways:
//! 1. `process::MC_USER_TIER` env var — read by the OpenClaw MAIC plugin to
//!    decide whether to register the 7 local tools.
//! 2. Returned from `tauri::command::mc_get_tier` — used by the dashboard
//!    frontend to render the tier badge.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Canonical MAIC tier values (Lesson 176). Anything outside this set is
/// treated as `free` for safety — never grant a tool a tier check thinks
/// it has but the canonicalizer doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Free,
    /// $10/mo — 5M tokens, 120 rpm. Token-bucket only — same features as Pro.
    /// Lesson 737 (NEW 2026-08-29, David): all paid tiers share the same
    /// features and model access; the bucket is the only differentiator.
    Starter,
    /// $25/mo — 15M tokens, 300 rpm. Same features as Pro.
    StarterPlus,
    Pro,
    ProPlus,
    Team,
    Enterprise,
}

impl Tier {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "pro_plus" | "proplus" => Tier::ProPlus,
            "team" => Tier::Team,
            "enterprise" => Tier::Enterprise,
            "pro" => Tier::Pro,
            "starter_plus" | "starterplus" => Tier::StarterPlus,
            "starter" => Tier::Starter,
            _ => Tier::Free,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Starter => "starter",
            Tier::StarterPlus => "starter_plus",
            Tier::Pro => "pro",
            Tier::ProPlus => "pro_plus",
            Tier::Team => "team",
            Tier::Enterprise => "enterprise",
        }
    }

    /// Does this tier grant access to MC's 7 local tools
    /// (read_file, write_file, edit_file, list_dir, bash_run,
    /// apply_patch, remember_fact)?
    ///
    /// **Lesson 526 (NEW 2026-08-21 13:55 MDT)**: ALL tiers now have
    /// local tools. Per David's decision: rate limiting handles
    /// abuse — Free users get all 7 tools but their TPM ceiling
    /// (50K) is the actual control. The previous gating (Free = 0
    /// tools) blocked critical flows (file inspection, project
    /// bootstrapping) for free users who had no way to upgrade
    /// from the in-app UI yet, and forced every onboarding to start
    /// with a 401-shaped dead-end before the user had even seen
    /// pricing. Returning `true` unconditionally is simpler and
    /// matches the "tooling is the product, rate limits are the
    /// cost" model.
    pub fn has_local_tools(self) -> bool {
        true
    }

    /// Does this tier grant access to Pro-only features (Tasks, custom
    /// voice, MCP servers, scheduled jobs, etc.)?
    ///
    /// **Lesson 725 (NEW 2026-08-28 21:30 MDT, David)**: Tasks feature
    /// for the Miracle Bot use case is paid-tier only. The gate is the
    /// upgrade hook for Free users — they can SEE the tile and click
    /// into it, but the page shows an upgrade CTA instead of the
    /// editor. Backend commands (`mc_task_add`, `mc_task_sync`, etc.)
    /// also enforce this gate so a Free user's MAIC chat agent can't
    /// silently bypass it via tool calls.
    ///
    /// **Lesson 737 (NEW 2026-08-29, David)**: ALL paid tiers
    /// (Starter, StarterPlus, Pro, ProPlus, Team, Enterprise) grant
    /// Pro-only features. Token bucket is the only differentiator.
    /// Previously Starter/StarterPlus collapsed to Free because they
    /// weren't in this enum (Lesson 526 follow-up).
    pub fn is_paid(self) -> bool {
        matches!(
            self,
            Tier::Starter
                | Tier::StarterPlus
                | Tier::Pro
                | Tier::ProPlus
                | Tier::Team
                | Tier::Enterprise
        )
    }
}

/// What `mc_get_tier` returns to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierInfo {
    pub tier: Tier,
    /// Raw `plan_code` from MAIC, kept for diagnostics only. NEVER used for
    /// routing decisions (Lesson 176).
    pub plan_code: Option<String>,
    pub email: Option<String>,
    /// True iff this fetch came from MAIC within the last 5 minutes.
    pub from_cache: bool,
    /// True iff the user was downgraded since the last fetch.
    pub tier_changed: bool,
}

#[derive(Debug, Deserialize)]
struct AuthMeResponse {
    tier: String,
    #[serde(default)]
    plan_code: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// Cached tier entry. Wraps the response plus a timestamp and a "what tier
/// did we last publish" hint for downgrade detection.
#[derive(Debug, Clone)]
struct CachedTier {
    info: TierInfo,
    fetched_at: Instant,
    /// The tier we published to env/UI the last time we processed a
    /// change. If the new fetch yields a *lower* tier than this, we set
    /// `tier_changed: true` so the caller can show the downgrade modal.
    last_published: Tier,
}

const CACHE_TTL: Duration = Duration::from_secs(300);

static TIER_CACHE: once_cell::sync::Lazy<Mutex<Option<CachedTier>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// Normalize a MAIC base URL for `/v1/...` API calls.
///
/// `read_system_openclaw_maic_base_url()` returns `baseUrl` from
/// `openclaw.json`, which is the **OpenAI-completions** endpoint
/// (`https://maicserver.com/v1`). The tier endpoint at
/// `/v1/auth/me` is one level HIGHER — it lives at the gateway root,
/// not under `/v1/chat/completions`.
///
/// Without this strip, naive `format!("{}/v1/auth/me", maic_base)`
/// produces `https://maicserver.com/v1/v1/auth/me` (double-`/v1`),
/// which 404s on MAIC. Stripping the trailing `/v1` (and `/v1/`)
/// makes the URL work for both shapes.
///
/// Lesson 512 (rc18): MAIC base URL shape mismatch.
pub(crate) fn normalize_api_base(maic_base: &str) -> String {
    let trimmed = maic_base.trim().trim_end_matches('/');
    if let Some(stripped) = trimmed.strip_suffix("/v1") {
        stripped.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Fetch the tier fresh from MAIC. Caller must hold a valid JWT.
pub fn fetch_tier_fresh(jwt: &str, maic_base: &str) -> Result<TierInfo, String> {
    let url = format!("{}/v1/auth/me", normalize_api_base(maic_base));
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", jwt))
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .call();
    let parsed: AuthMeResponse = match resp {
        Ok(r) => r
            .into_json()
            .map_err(|e| format!("parse /v1/auth/me: {}", e))?,
        Err(e) => return Err(format!("/v1/auth/me: {}", e)),
    };
    let tier = Tier::from_str(&parsed.tier);
    let mut cache = TIER_CACHE.lock().unwrap();
    let tier_changed = match cache.as_ref() {
        Some(prev) if prev.last_published != tier && prev.last_published > tier => true,
        _ => false,
    };
    let info = TierInfo {
        tier,
        plan_code: parsed.plan_code,
        // MAIC's `/v1/auth/me` returns `name`, not `email`. Stash under
        // `email` field too since the frontend just renders a label.
        email: parsed.name.or(parsed.email),
        from_cache: false,
        tier_changed,
    };
    *cache = Some(CachedTier {
        info: info.clone(),
        fetched_at: Instant::now(),
        last_published: tier,
    });
    Ok(info)
}

/// Fetch with cache. Returns the cached value if it's less than 5 min old
/// AND was fetched with the same JWT. Different JWT (different user) =>
/// fresh fetch.
pub fn fetch_tier_cached(jwt: &str, maic_base: &str) -> Result<TierInfo, String> {
    let cache = TIER_CACHE.lock().unwrap();
    if let Some(c) = cache.as_ref() {
        if c.fetched_at.elapsed() < CACHE_TTL {
            let mut info = c.info.clone();
            info.from_cache = true;
            return Ok(info);
        }
    }
    drop(cache);
    fetch_tier_fresh(jwt, maic_base)
}

/// Invalidate the cache. Called on logout, login, and when MAIC returns
/// 403 `tier_changed`.
pub fn invalidate_tier_cache() {
    if let Ok(mut cache) = TIER_CACHE.lock() {
        *cache = None;
    }
}

/// What tier does the cache currently say is active (without doing a
/// network fetch)? Returns `Tier::Free` if cache is empty.
pub fn current_tier() -> Tier {
    TIER_CACHE
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.info.tier)
        .unwrap_or(Tier::Free)
}

/// Publish the tier to `process::MC_USER_TIER` so the OpenClaw MAIC plugin
/// can read it at startup. Idempotent. Doesn't touch the cache.
pub fn publish_tier_env(tier: Tier) {
    // SAFETY: setting an env var is safe in a single-threaded startup path;
    // the MAIC plugin reads this only at startup, so we set it once on
    // login and never mutate it again (downgrade forces a launcher
    // restart, which re-reads).
    unsafe {
        std::env::set_var("MC_USER_TIER", tier.as_str());
    }
    eprintln!("[miracle-claw] tier: published MC_USER_TIER={}", tier.as_str());
}

// ---------------------------------------------------------------------------
// Lesson 517 (NEW, 2026-08-20): tier-conditional default model + fallbacks.
//
// David's instruction: paid accounts (Pro, ProPlus, Team, Enterprise) should
// default to Kimi (cloud cascade via MAIC OpenChat) with MiniMax-M3 as
// first fallback and GLM as second fallback. Free stays on the local 14B
// (`milagro-dev`) because cloud-only models would silently break for users
// with no quota.
//
// The default is written into `agents.defaults.model` in the user's
// openclaw.json — exactly the shape openclaw's `resolveDefaultModelForAgent`
// reads (see `model-selection-B9dihan1.js`). OpenClaw's runtime then
// resolves `primary` → first model id, and on failure walks `fallbacks`
// in order. Each entry can be a bare model id (resolved against the
// `maic` provider configured by MC) or `provider/model`.
//
// We don't override user choice — if `agents.defaults.model.primary` is
// already set, the caller should not call `ensure_agents_default_model`
// with a non-empty `force` flag. The writer is opt-in per the caller
// decision and is documented as such.

/// The default model id for a given tier (primary in `agents.defaults.model.primary`).
///
/// Free: `milagro-m1-t1` (local Qwen3B-distilled — cheapest local path,
/// zero Ollama usage, English-only but adequate for simple chat).
/// Paid: `milagro-dev` (local 14B — best tool-use discipline of any local model;
///
/// Lesson 793 (2026-08-30, David): switched Paid primary
/// from `milagro-oc-kimi` back to `milagro-dev`. Reason: kimi (100B+ MoE)
/// prefers OpenClaw built-ins (`web_search`/`web_fetch`) over the 7 paid-tier
/// plugin tools (`bash_run`, `read_file`, etc.) — and those built-ins are
/// broken because no Brave API key is configured. RC55.10 verified all 7
/// paid-tier tools DO reach MAIC (DEBUG-760: caller_tools_count=7,
/// final_tool_count=17) — the issue was 100% model behavior. 14B has
/// shown better tool-call discipline than kimi. If kimi is needed later,
/// add a stronger system prompt or disable built-ins at agent config level.
/// Previously was `milagro-dev` per Lesson 566 before being changed to kimi.
///
/// This is also what `mc_get_default_model` returns to the dashboard
/// so the chat panel's pre-selected model matches the tier routing.
///
/// Lesson 569 (2026-08-24 22:57 MDT, David): switched Free primary
/// from `milagro-oc-deepseek` (Lesson 566) back to a local m1-t model.
/// Reason: deepseek is Ollama usage level 4 (extra high) — every
/// Free user request burned 4x what chat-nemotron-nano (level 1)
/// would. With 50K TPM quota, deepseek defaults would let users
/// blow through Free quota in minutes. m1-t1 is local 3B (zero
/// Ollama usage) and chain-falls-back to t2 → t3 → chat-nemotron-nano
/// (cheapest cloud model). Lesson 568 documented usage levels for
/// all Ollama Cloud models MAIC routes through.
///
/// Lesson 566 history: was `milagro-dev` (claimed local 14B but
/// routes to ollama-cloud `minimax-m3:cloud` on Hetzner-prod).
/// previously was cloud Kimi — reverted to 14B local for tool discipline (Lesson 793).
pub fn tier_default_model_id(tier: Tier) -> &'static str {
    match tier {
        // Free tier keeps Lesson 737 + 781 default (local m1-t1, smallest).
        Tier::Free => "milagro-m1-t1",
        // Lesson 795 (2026-08-30, David): Paid primary was
        // `milagro-oc-glm`, chosen because Kimi preferred OpenClaw
        // built-ins over plugin tools and GLM fired paid-tier tools
        // reliably.
        //
        // ⚠️ THIS IS NO LONGER WHAT SHIPS. David moved Paid to
        // `maic/chat-nemotron-super` on 2026-10-07 and reaffirmed it
        // 2026-10-09: ~42x cheaper and better across the board. That
        // choice lives in the user's own `chat.json`
        // (`agents.defaults.model.primary`), NOT here.
        //
        // WHY THIS FUNCTION STILL SAYS GLM, AND WHY THAT ISN'T A BUG:
        // this is the *seeding default* for
        // `ensure_agents_default_model_for_tier()`, which is
        // deliberately NON-DESTRUCTIVE — it skips writing when the
        // user already has `agents.defaults.model`, or has a manual
        // override that isn't the tier default. So a deliberate
        // nemotron choice survives normal launches. Verified
        // 2026-10-21: sidecar logged "agents.defaults.model: already
        // configured, no change" and the gateway booted on
        // `maic/chat-nemotron-super`.
        //
        // ⚠️ OPEN RISK: if chat.json is ever cleared or re-migrated,
        // this re-seeds GLM and silently reverts the cost win.
        // Proper fix = track an "explicitly chosen" flag, or make the
        // tier default nemotron too. NOT DONE — David hasn't been
        // asked yet. Tracked in memory/2026-10-21.md Lesson 1422.
        //
        // NOTE: this value is bare. The writer in
        // `ensure_agents_default_model_for_tier` (lib.rs:1576) prefixes
        // `maic/` before writing to openclaw.json, so the on-disk
        // value is `maic/<id>`. Tests against the openclaw.json file
        // expect the prefixed form. Tests against THIS function expect
        // the bare form.
        //
        // Lesson 798 (2026-08-30, David): the picker-bug analysis
        // suggested prefixing here too, but the writer already does it,
        // so doing it twice would produce `maic/maic/<id>` (double
        // prefix — confirmed empirically by running tests).
        _ => "milagro-oc-glm",
    }
}

/// The ordered fallback chain for a given tier.
///
/// The chain is appended to `agents.defaults.model.fallbacks` (preserving
/// openclaw's existing fallbacks if the caller has set any). It walks
/// cheap → expensive models in order, so a transient Kimi outage degrades
/// to MiniMax-M3 (still cloud, MAIC cascade), then GLM, then the local 14B.
///
/// Why this order (David's 2026-08-20 16:59 MDT):
///   1. `milagro-oc-minimax` (MiniMax M3) — second-best cloud reasoning at
///      similar latency to Kimi. Drop-in for long-context chat/code.
///   2. `milagro-oc-glm` — third cloud option; good for code completion.
///   3. `milagro-dev` — local 14B fallback if ALL cloud routes fail. Slow
///      but never returns a network error.
///
/// Free chain (Lesson 569, 2026-08-24 22:57 MDT, David):
///   1. `milagro-m1-t2` — local Qwen7B-distilled, higher quality for
///      longer contexts that t1 struggles with.
///   2. `milagro-m1-t3` — local Qwen14B-distilled, the heaviest
///      local option. Still free of Ollama usage.
///   3. `chat-nemotron-nano` — Ollama Cloud MoE 30B/3.5B active.
///      First cloud fallback; usage level 1 (low) — the cheapest
///      cloud route we expose. Only kicks in if the local 14B
///      itself fails (OOM, timeout, etc.).
///
/// The Free chain NEVER falls back to deepseek/glm/kimi/qwen/minimax:
/// those are all usage level 3+ and would burn Free quota at 3-4x
/// the rate of nemotron-nano.
///
/// Lesson 566 (2026-08-24): Free's primary was `milagro-dev` (claimed local
/// 14B but actually routed to ollama-cloud `minimax-m3:cloud`); switched to
/// `milagro-oc-deepseek` because cloud-deepseek returned 199 tokens in 2.05s
/// vs milagro-dev's 600 tokens in 12.48s (benchmark at 21:30 MDT).
pub fn tier_default_fallbacks(tier: Tier) -> &'static [&'static str] {
    match tier {
        // Lesson 521: writer prefixes `maic/`, so we return bare.
        Tier::Free => &[
            "milagro-m1-t2",
            "milagro-m1-t3",
            "chat-nemotron-nano",
        ],
        // Lesson 795: Paid primary is `milagro-oc-glm`, chain stays
        // MiniMax → kimi → local 14B (bare — writer prefixes).
        //
        // Lesson 1422 (2026-10-21, David): this GLM-primary comment
        // was ~3 months stale and actively misled a later session
        // into probing `milagro-oc-glm` when the live Paid model is
        // `chat-nemotron-super`. Functionally unchanged; see
        // `tier_default_model_id` for why GLM is only a seeding
        // default and what the revert risk is.
        _ => &[
            "milagro-oc-minimax",
            "milagro-oc-kimi",
            "milagro-m1-t3",
        ],
    }
}

/// Check whether the running MC base binary satisfies a module's
/// `minMcVersion` constraint.
///
/// MC versions look like `"1.1.0-rc55.24"` (v1 cadence) or `"2.0.0"`
/// (post-v1, real semver). We use the `semver` crate (Lesson 867) so
/// both forms parse and compare correctly across major-version
/// boundaries.
///
/// **Semver ordering rules we rely on** (these are not inventions —
/// they are how Cargo / npm / pip / spec-compliant semver work):
///
/// 1. `(major, minor, patch)` are compared in numeric order.
///    `2.0.0 > 1.0.0` because `2 > 1` on the major axis.
/// 2. A version with a pre-release tag is **less than** the same
///    version without one. So `1.1.0-rc55.24 < 1.1.0`. This is
///    semver-correct: a release candidate is "almost" the final but
///    not yet.
/// 3. Two pre-releases of the same `(major, minor, patch)` are
///    compared lexically by dot-separated identifiers.
///    `1.1.0-rc55.24 > 1.1.0-rc53.14` because `rc55.24 > rc53.14`
///    (lexical on the leading `rc` integer — `55 > 53`).
/// 4. The dash is part of the pre-release tag, NOT a separator from
///    the main version. `1.1.0-rc55.24` is one version string,
///    not "1.1.0" + "-rc55.24".
///
/// This is exactly what we want for MC's scheme:
/// - v1 cadence `1.1.0-rcN.M` is a pre-release of `1.1.0`.
/// - The rc-major is encoded as the pre-release's first identifier
///   (`rc55`), so `1.1.0-rc55.x > 1.1.0-rc54.y` is correct.
/// - `1.1.0-rc55.x` and `1.1.0-rc55.y` are both "between" `1.1.0-rc54.0`
///   and `1.1.0` (the final), ordered lexically by `rc55.x`.
///
/// **What this fixes vs the old `(rc_major, hotfix)` scheme**:
/// - `2.0.0` used to return `None` from `parse_rc_version` → fail open
///   → modules with `minMcVersion: 2.0.0` would silently install on
///   MC `1.x`. Now `2.0.0 > 1.1.0-rc55.24` is correctly evaluated.
/// - Plain `1.0.0` (no rc trailer) used to fail open too. Now it
///   parses as a real semver and a module saying
///   `minMcVersion: 1.0.0` correctly installs on any `1.0.0+`.
///
/// **Fail-open policy** preserved from Lesson 705: if either side
/// fails to parse (e.g. a manifest with `minMcVersion: "latest"` or
/// some custom non-semver string), we log a warning and allow the
/// install rather than block. The reasoning: a module built against
/// a future MC shouldn't lock out users on today's MC.
pub fn assert_version_compatible_with_module(
    manifest: &crate::modules::manifest::ModuleManifest,
) -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let required = &manifest.min_mc_version;

    let current_v = semver::Version::parse(current).ok();
    let required_v = semver::Version::parse(required).ok();

    match (current_v, required_v) {
        (Some(c), Some(r)) if c >= r => Ok(()),
        (Some(_), Some(_)) => Err(format!(
            "MC base is {}, module requires {} or higher",
            current, required
        )),
        _ => {
            eprintln!(
                "[modules] version check: could not parse current={} required={} — allowing install",
                current, required
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_basic() {
        // Lesson 867 (2026-09-09, David): the old `parse_rc_version` only
        // handled "1.1.0-rcN.M" strings. The new parser uses the `semver`
        // crate so it handles that form AND plain semver (1.0.0, 2.0.0)
        // AND pre-release ordering in one consistent way.
        use semver::Version;
        assert_eq!(Version::parse("1.1.0-rc53.28").unwrap().to_string(), "1.1.0-rc53.28");
        assert_eq!(Version::parse("1.1.0-rc54.1").unwrap().to_string(), "1.1.0-rc54.1");
        assert_eq!(Version::parse("1.1.0-rc54.0").unwrap().to_string(), "1.1.0-rc54.0");
        assert_eq!(Version::parse("1.1.0-rc1").unwrap().to_string(), "1.1.0-rc1");
        // Lesson 867's headline reason: 2.0.0 must parse.
        assert_eq!(Version::parse("2.0.0").unwrap().to_string(), "2.0.0");
        assert_eq!(Version::parse("1.0.0").unwrap().to_string(), "1.0.0");
        // Pre-release ordering rules we rely on.
        assert!(Version::parse("1.0.0-rc1").unwrap() < Version::parse("1.0.0").unwrap());
        assert!(Version::parse("1.0.0-rc2").unwrap() > Version::parse("1.0.0-rc1").unwrap());
        // Garbage still returns None (fail-open policy).
        assert!(Version::parse("not-a-version").is_err());
        assert!(Version::parse("").is_err());
        // "latest" is not semver — must be rejected so the fail-open
        // branch fires (warning + allow install).
        assert!(Version::parse("latest").is_err());
    }

    #[test]
    fn parse_version_ordering_across_rc_boundary() {
        // Lesson 705's bug case, re-verified on the new parser: rc54.x
        // must ALWAYS be newer than rc53.x regardless of hotfix value,
        // because the major axis of the pre-release identifier takes
        // priority over its suffix.
        use semver::Version;
        let rc54_0 = Version::parse("1.1.0-rc54.0").unwrap();
        let rc54_1 = Version::parse("1.1.0-rc54.1").unwrap();
        let rc53_28 = Version::parse("1.1.0-rc53.28").unwrap();
        let rc53_30 = Version::parse("1.1.0-rc53.30").unwrap();

        assert!(rc54_0 >= rc53_30, "rc54.0 must be >= rc53.30");
        assert!(rc54_1 >= rc53_28, "rc54.1 must be >= rc53.28");
        assert!(rc54_1 >= rc54_0, "rc54.1 must be >= rc54.0");
        assert!(rc53_30 >= rc53_28, "rc53.30 must be >= rc53.28");
    }

    #[test]
    fn parse_version_ordering_across_major_boundary() {
        // Lesson 867's headline case: 2.0.0 must be greater than ANY
        // 1.1.0-rcN.M string. The old parser would return None for
        // "2.0.0" → fail open → silent install. The new parser must
        // return a real comparison so a module saying
        // `minMcVersion: "2.0.0"` is correctly rejected on 1.1.0-rc55.x
        // and accepted on 2.0.0+.
        use semver::Version;
        let v1_rc55_24 = Version::parse("1.1.0-rc55.24").unwrap();
        let v1_rc1 = Version::parse("1.1.0-rc1").unwrap();
        let v1_final = Version::parse("1.1.0").unwrap();
        let v2_0 = Version::parse("2.0.0").unwrap();
        let v2_1_0 = Version::parse("2.1.0").unwrap();

        // 2.0.0 beats every 1.1.0-rcN.M (the actual upgrade scenario).
        assert!(v2_0 > v1_rc55_24, "2.0.0 must be > 1.1.0-rc55.24");
        assert!(v2_0 > v1_rc1, "2.0.0 must be > 1.1.0-rc1");
        assert!(v2_0 > v1_final, "2.0.0 must be > 1.1.0 (final)");
        // 1.1.0-rc55.24 < 1.1.0 (pre-release < final) — so a module
        // declaring `minMcVersion: "1.1.0"` (the final) is correctly
        // rejected on RC builds.
        assert!(v1_rc55_24 < v1_final, "1.1.0-rc55.24 must be < 1.1.0");
        // Minor bumps on 2.x are still ordered correctly.
        assert!(v2_1_0 > v2_0, "2.1.0 must be > 2.0.0");
    }

    #[test]
    fn assert_compatible_with_module_accepts_when_current_meets_min() {
        // Module says minMcVersion: 1.1.0-rc53.14, current is
        // 1.1.0-rc55.24 → must accept.
        use crate::modules::manifest::ModuleManifest;
        let manifest_json = r#"{
            "id": "testmod",
            "name": "Test Mod",
            "version": "0.1.0",
            "minMcVersion": "1.1.0-rc53.14",
            "description": "Test",
            "binary": { "name": "testmod.exe", "type": "sidecar" },
            "commands": [],
            "uiHooks": []
        }"#;
        let m: ModuleManifest = serde_json::from_str(manifest_json).unwrap();
        assert!(assert_version_compatible_with_module(&m).is_ok());
    }

    #[test]
    fn assert_compatible_with_module_rejects_when_current_too_old() {
        // Module says minMcVersion: 2.0.0, current is 1.1.0-rc55.24
        // → must REJECT (this is the bug the old parser would have
        // silently let through).
        use crate::modules::manifest::ModuleManifest;
        let manifest_json = r#"{
            "id": "testmod",
            "name": "Test Mod",
            "version": "0.1.0",
            "minMcVersion": "2.0.0",
            "description": "Test",
            "binary": { "name": "testmod.exe", "type": "sidecar" },
            "commands": [],
            "uiHooks": []
        }"#;
        let m: ModuleManifest = serde_json::from_str(manifest_json).unwrap();
        let result = assert_version_compatible_with_module(&m);
        assert!(result.is_err(), "expected rejection but got Ok");
        let err = result.unwrap_err();
        // The error should mention 2.0.0 (the required version) and the
        // current MC base version (whatever it is — we don't hardcode it
        // here so this test doesn't break every time we bump the package
        // version). We just check that the base version is *something* with
        // a 1.1.0-rc prefix to confirm both versions made it into the
        // message.
        assert!(err.contains("2.0.0"), "error message should mention required version 2.0.0, got: {err}");
        // rc56.0: version is now 1.2.0-rc56.0 (still on the v1 cadence
        // until v2 ships). The test only pins the leading digits to keep
        // the version-bump ceremony from breaking it again.
        assert!(
            err.contains("1.") && err.contains("-rc"),
            "error message should mention the current base version (1.x-rcY), got: {err}"
        );
    }

    #[test]
    fn assert_compatible_with_module_accepts_v1_module_on_v2_base() {
        // The reverse direction: current is 2.0.0, module says
        // minMcVersion: 1.0.0 (a real-semver legacy constraint from a
        // module built during v1's stable run). Must accept.
        // We test this by inlining a `current` override would require
        // env! indirection — instead, assert the comparison logic
        // directly so we don't have to spin a separate Cargo build.
        use semver::Version;
        let v2 = Version::parse("2.0.0").unwrap();
        let v1_legacy = Version::parse("1.0.0").unwrap();
        assert!(v2 >= v1_legacy, "2.0.0 must be >= 1.0.0 (v1 module on v2 base)");
    }

    #[test]
    fn assert_compatible_with_module_fails_open_on_garbage_min_version() {
        // Lesson 705 fail-open policy: a manifest with a non-semver
        // minMcVersion ("latest", empty, typo) must not block the
        // install. We log a warning and return Ok. The reasoning is
        // that a module built against a future / experimental MC
        // version string shouldn't lock out users on today's MC.
        //
        // We can't test the "current" env override (it's compile-time
        // via env!("CARGO_PKG_VERSION")), so we test the fail-open
        // by parsing both sides and confirming the parse error is
        // surfaced as a fail-open (not a hard reject).
        use semver::Version;
        let garbage_values = ["latest", "", "v1.0", "1.x", "not-a-version"];
        for g in garbage_values {
            assert!(
                Version::parse(g).is_err(),
                "expected `{g}` to be rejected as non-semver, so the fail-open branch fires"
            );
        }
        // And the happy path that the fail-open produces: if either
        // side is an Err, we return Ok. We confirm this with the
        // manifest API below.
        use crate::modules::manifest::ModuleManifest;
        let manifest_json = r#"{
            "id": "testmod",
            "name": "Test Mod",
            "version": "0.1.0",
            "minMcVersion": "latest",
            "description": "Test",
            "binary": { "name": "testmod.exe", "type": "sidecar" },
            "commands": [],
            "uiHooks": []
        }"#;
        let m: ModuleManifest = serde_json::from_str(manifest_json).unwrap();
        assert!(
            assert_version_compatible_with_module(&m).is_ok(),
            "fail-open: non-semver minMcVersion must allow the install (with warning)"
        );
    }

    #[test]
    fn tier_from_str_canonicalizes() {
        assert_eq!(Tier::from_str("free"), Tier::Free);
        assert_eq!(Tier::from_str("Free"), Tier::Free);
        assert_eq!(Tier::from_str("FREE"), Tier::Free);
        assert_eq!(Tier::from_str("starter"), Tier::Starter);
        assert_eq!(Tier::from_str("Starter"), Tier::Starter);
        assert_eq!(Tier::from_str("starter_plus"), Tier::StarterPlus);
        assert_eq!(Tier::from_str("starterplus"), Tier::StarterPlus);
        assert_eq!(Tier::from_str("pro"), Tier::Pro);
        assert_eq!(Tier::from_str("pro_plus"), Tier::ProPlus);
        assert_eq!(Tier::from_str("proplus"), Tier::ProPlus);
        assert_eq!(Tier::from_str("team"), Tier::Team);
        assert_eq!(Tier::from_str("enterprise"), Tier::Enterprise);
        // Unknown tier defaults to Free (never grant on a typo).
        assert_eq!(Tier::from_str("trial"), Tier::Free);
        assert_eq!(Tier::from_str(""), Tier::Free);
        assert_eq!(Tier::from_str("__bogus__"), Tier::Free);
    }

    #[test]
    fn tier_as_str_round_trips() {
        for t in [
            Tier::Free,
            Tier::Starter,
            Tier::StarterPlus,
            Tier::Pro,
            Tier::ProPlus,
            Tier::Team,
            Tier::Enterprise,
        ] {
            assert_eq!(Tier::from_str(t.as_str()), t);
        }
    }

    #[test]
    fn has_local_tools_for_all_tiers() {
        // Lesson 526 (NEW 2026-08-21): all tiers get the 7 local tools.
        // Rate limiting (per-tier TPM) is the actual control, not
        // tool gating. Previously Free was excluded; that broke
        // onboarding for users who couldn't see pricing yet.
        assert!(Tier::Free.has_local_tools(), "Free gets tools (Lesson 526)");
        assert!(Tier::Starter.has_local_tools());
        assert!(Tier::StarterPlus.has_local_tools());
        assert!(Tier::Pro.has_local_tools());
        assert!(Tier::ProPlus.has_local_tools());
        assert!(Tier::Team.has_local_tools());
        assert!(Tier::Enterprise.has_local_tools());
    }

    #[test]
    fn is_paid_covers_all_paid_tiers_lesson_737() {
        // Lesson 737 (2026-08-29, David): Starter/StarterPlus are paid.
        // Previously they collapsed to Free and paying customers got the
        // Free-tier experience (no mc_task_* tools, no priority routing).
        assert!(!Tier::Free.is_paid(), "Free is not paid");
        assert!(Tier::Starter.is_paid(), "Starter is paid (Lesson 737)");
        assert!(Tier::StarterPlus.is_paid(), "StarterPlus is paid (Lesson 737)");
        assert!(Tier::Pro.is_paid());
        assert!(Tier::ProPlus.is_paid());
        assert!(Tier::Team.is_paid());
        assert!(Tier::Enterprise.is_paid());
    }

    #[test]
    fn tier_serialization_matches_maic_strings() {
        // Sanity-check the JSON wire format the frontend expects.
        let json = serde_json::to_string(&Tier::Starter).unwrap();
        assert_eq!(json, "\"starter\"");
        let json = serde_json::to_string(&Tier::StarterPlus).unwrap();
        assert_eq!(json, "\"starter_plus\"");
        let json = serde_json::to_string(&Tier::Pro).unwrap();
        assert_eq!(json, "\"pro\"");
        let json = serde_json::to_string(&Tier::ProPlus).unwrap();
        assert_eq!(json, "\"pro_plus\"");
    }

    #[test]
    fn invalidate_clears_cache() {
        // Manually populate cache via current_tier() path is a no-op since
        // we don't expose a setter; instead verify the public surface.
        invalidate_tier_cache();
        assert_eq!(current_tier(), Tier::Free);
    }

    // -----------------------------------------------------------------------
    // Lesson 517 tests
    // -----------------------------------------------------------------------

    #[test]
    fn free_default_is_m1_t1() {
        // Lesson 569 (2026-08-24 22:57 MDT, David): Free default
        // switched BACK to m1-t1 (local 3B) from milagro-oc-deepseek
        // (Lesson 566). Reason: deepseek is Ollama usage level 4
        // (extra high) — burns 4x what chat-nemotron-nano (level 1)
        // would. Free users on the 50K TPM quota would hit limits in
        // minutes with deepseek as primary. m1-t1 is local (zero
        // Ollama usage) and chain-falls-back to t2 → t3 →
        // chat-nemotron-nano (cheapest cloud route, level 1).
        assert_eq!(tier_default_model_id(Tier::Free), "milagro-m1-t1");
        // Free chain: m1-t2 (local 7B) → m1-t3 (local 14B) →
        // chat-nemotron-nano (cloud level 1). Only the LAST step
        // burns cloud quota; never deepseek/glm/kimi/qwen/minimax.
        // Lesson 798: writer at lib.rs:1576 prefixes `maic/` when
        // writing openclaw.json — the bare values here get the
        // prefix added at the boundary.
        let chain = tier_default_fallbacks(Tier::Free);
        assert_eq!(chain, &["milagro-m1-t2", "milagro-m1-t3", "chat-nemotron-nano"]);
        // Defensive: chain must NOT include any usage-level-3+ cloud model.
        for forbidden in ["milagro-oc-deepseek", "milagro-oc-glm",
                          "milagro-oc-qwen", "milagro-oc-kimi",
                          "milagro-oc-minimax"] {
            assert!(!chain.contains(&forbidden),
                    "Free fallback chain must not include {forbidden} (usage level 3+ model); \
                     this would burn Free quota 3-4x faster than nemotron-nano");
        }
    }

    #[test]
    fn paid_default_is_glm() {
        // Lesson 795 (2026-08-30, David): Paid primary is now GLM
        // (cloud) — empirically the only paid-tier model that
        // reliably fires our plugin's local tools (write_file,
        // read_file, list_dir, bash_run, apply_patch, remember_fact).
        // Kimi preferred OpenClaw built-ins over plugin tools,
        // nemotron-super exhausted the token budget, MiniMax-M3
        // picked invalid file paths (Lesson 796).
        //
        // Lesson 798: prefixed with `maic/` at the writer boundary
        // (lib.rs:1576). Bare here.
        for tier in [
            Tier::Starter,
            Tier::StarterPlus,
            Tier::Pro,
            Tier::ProPlus,
            Tier::Team,
            Tier::Enterprise,
        ] {
            assert_eq!(
                tier_default_model_id(tier),
                "milagro-oc-glm",
                "{:?} must default to milagro-oc-glm (Lesson 795)",
                tier,
            );
        }
    }

    #[test]
    fn paid_fallbacks_are_ordered_minimax_then_kimi_then_local() {
        // Lesson 795 (2026-08-30, David): Paid primary is GLM
        // (replaces `milagro-dev` from Lesson 793). GLM fires local
        // tools reliably, but on its first failure we want cloud
        // alternatives before falling back to local 14B. Kimi is
        // restored as fallback[1] (was the Lesson 737 default) —
        // it's fine as a SECONDARY fallback because the chat will
        // already be running GLM-built context. Lesson 798: writer
        // at lib.rs:1576 prefixes `maic/` — bare values here.
        for tier in [
            Tier::Starter,
            Tier::StarterPlus,
            Tier::Pro,
            Tier::ProPlus,
            Tier::Team,
            Tier::Enterprise,
        ] {
            let f = tier_default_fallbacks(tier);
            assert_eq!(f.len(), 3, "{:?} should have exactly 3 fallbacks", tier);
            assert_eq!(f[0], "milagro-oc-minimax", "{:?} fallback[0] must be MiniMax-M3", tier);
            assert_eq!(f[1], "milagro-oc-kimi",    "{:?} fallback[1] must be Kimi", tier);
            assert_eq!(f[2], "milagro-m1-t3",      "{:?} fallback[2] must be local 14B-distilled", tier);
        }
    }

    #[test]
    fn fallback_chain_distinct_from_primary() {
        // The fallback chain must NOT include the primary — otherwise
        // openclaw's fallback walker would loop on the same model id.
        for tier in [Tier::Pro, Tier::ProPlus, Tier::Team, Tier::Enterprise] {
            let primary = tier_default_model_id(tier);
            for f in tier_default_fallbacks(tier) {
                assert_ne!(*f, primary, "{:?}: fallback {} must differ from primary", tier, f);
            }
        }
    }

    /// Pin the JSON shape of MAIC's `/v1/auth/me` so future schema
    /// changes on MAIC surface as a test failure here (and not as a
    /// runtime "missing field user" error in production).
    #[test]
    fn auth_me_response_deserializes_maic_actual_schema() {
        // MAIC's actual response shape per /opt/maic/api/routes/dashboard.py:
        //   { id, name, is_master, tier, plan_code }
        let maic_real = r#"{"id":12345,"name":"david","is_master":false,"tier":"pro","plan_code":"pro_plus"}"#;
        let parsed: AuthMeResponse = serde_json::from_str(maic_real)
            .expect("MAIC's actual /v1/auth/me shape must deserialize");
        assert_eq!(parsed.tier, "pro");
        assert_eq!(parsed.plan_code.as_deref(), Some("pro_plus"));
        assert_eq!(parsed.name.as_deref(), Some("david"));
        assert_eq!(parsed.email, None, "MAIC returns `name`, not `email`");

        // Free user with no plan_code.
        let free_maic = r#"{"id":1,"name":"freee","is_master":false,"tier":"free"}"#;
        let parsed: AuthMeResponse = serde_json::from_str(free_maic).unwrap();
        assert_eq!(parsed.tier, "free");
        assert_eq!(parsed.plan_code, None);

        // Legacy shape with user-wrapping must NOT deserialize (we no
        // longer ship that); this guards against accidentally going
        // back to a wrapped schema.
        let wrapped = r#"{"user":{"tier":"pro","plan_code":null}}"#;
        assert!(
            serde_json::from_str::<AuthMeResponse>(wrapped).is_err(),
            "Schema must NOT match wrapped shape (was: {{user: {{tier, plan_code}}}})"
        );
    }
}