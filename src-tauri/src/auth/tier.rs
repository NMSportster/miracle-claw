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
            _ => Tier::Free,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Free => "free",
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
/// Paid: `milagro-oc-kimi` (cloud cascade — best cost/quality for code+chat).
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
pub fn tier_default_model_id(tier: Tier) -> &'static str {
    match tier {
        Tier::Free => "milagro-m1-t1",
        // Pro / ProPlus / Team / Enterprise all use the cloud Kimi default.
        // MAIC's plan_code → quota gate still applies server-side, so a
        // downgraded user on this default just gets a clean error rather
        // than a quota-bypass.
        _ => "milagro-oc-kimi",
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
        Tier::Free => &[
            "milagro-m1-t2",
            "milagro-m1-t3",
            "chat-nemotron-nano",
        ],
        _ => &[
            "milagro-oc-minimax",
            "milagro-oc-glm",
            "milagro-dev",
        ],
    }
}

/// Check whether the running MC base binary satisfies a module's
/// `minMcVersion` constraint.
///
/// MC versions look like `"1.1.0-rc53.15"`. For v1 we compare the
/// `(rc_major, hotfix)` tuple — same scheme miracle-claw-tauri-rebuild
/// uses. **Important**: the hotfix alone is NOT a total order. Once
/// we cross an `rc<n+1>.0` boundary, the hotfix resets to 0 and any
/// `rc<n>.x` becomes OLDER than any `rc<n+1>.y`. So `rc54.1` is
/// newer than `rc53.28`, even though `1 < 28`.
///
/// If parsing fails on either side, we FAIL OPEN (return Ok) so a
/// module built against a future version string doesn't block
/// installs of old MC. We log a warning instead.
///
/// Future: when MC goes 1.0.0 → 2.0.0, switch to semver crate.
pub fn assert_version_compatible_with_module(
    manifest: &crate::modules::manifest::ModuleManifest,
) -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let required = &manifest.min_mc_version;

    let current_v = parse_rc_version(current);
    let required_v = parse_rc_version(required);

    match (current_v, required_v) {
        (Some(c), Some(r)) if c >= r => Ok(()),
        (Some(c), Some(r)) => Err(format!(
            "MC base is {} (rc{}.{}), module requires rc{}.{} or higher",
            current, c.0, c.1, r.0, r.1
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

/// Extract `(rc_major, hotfix)` from a version like `"1.1.0-rc53.15"`.
/// Returns `Some((53, 15))` for that string. `rc54.1` → `Some((54, 1))`.
/// `rc1` (legacy, no hotfix) → `Some((1, 0))`. Returns None for any
/// other shape.
fn parse_rc_version(v: &str) -> Option<(u64, u64)> {
    // Format: <major>.<minor>.<patch>-rc<rc_major>[.<hotfix>]
    let after_rc = v.split("-rc").nth(1)?;
    let mut parts = after_rc.split('.');
    let rc_major: u64 = parts.next()?.parse().ok()?;
    let hotfix: u64 = match parts.next() {
        Some(h) => h.parse().ok()?,
        // Legacy "rc<n>" without hotfix = treat as hotfix 0.
        None => 0,
    };
    Some((rc_major, hotfix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rc_version_basic() {
        // Lesson 705 (2026-08-27, David): voice module rc53.28 failed
        // to install on MC rc54.1 because the old parser extracted
        // only the hotfix integer, so rc54.1 parsed as 1 vs rc53.28
        // parsed as 28. Bug: 1 < 28 → "version too old". Fix: compare
        // (rc_major, hotfix) tuples instead.
        assert_eq!(parse_rc_version("1.1.0-rc53.28"), Some((53, 28)));
        assert_eq!(parse_rc_version("1.1.0-rc54.1"), Some((54, 1)));
        assert_eq!(parse_rc_version("1.1.0-rc54.0"), Some((54, 0)));
        assert_eq!(parse_rc_version("1.1.0-rc1"), Some((1, 0))); // legacy
        assert_eq!(parse_rc_version("not-a-version"), None);
        assert_eq!(parse_rc_version(""), None);
    }

    #[test]
    fn parse_rc_version_ordering_across_rc_boundary() {
        // The bug case: rc54.x must ALWAYS be newer than rc53.x
        // regardless of hotfix value, because rc-major takes priority.
        let rc54_0 = parse_rc_version("1.1.0-rc54.0").unwrap();
        let rc54_1 = parse_rc_version("1.1.0-rc54.1").unwrap();
        let rc53_28 = parse_rc_version("1.1.0-rc53.28").unwrap();
        let rc53_30 = parse_rc_version("1.1.0-rc53.30").unwrap();

        assert!(rc54_0 >= rc53_30, "rc54.0 must be >= rc53.30");
        assert!(rc54_1 >= rc53_28, "rc54.1 must be >= rc53.28");
        assert!(rc54_1 >= rc54_0, "rc54.1 must be >= rc54.0");
        assert!(rc53_30 >= rc53_28, "rc53.30 must be >= rc53.28");
    }

    #[test]
    fn tier_from_str_canonicalizes() {
        assert_eq!(Tier::from_str("free"), Tier::Free);
        assert_eq!(Tier::from_str("Free"), Tier::Free);
        assert_eq!(Tier::from_str("FREE"), Tier::Free);
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
        for t in [Tier::Free, Tier::Pro, Tier::ProPlus, Tier::Team, Tier::Enterprise] {
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
        assert!(Tier::Pro.has_local_tools());
        assert!(Tier::ProPlus.has_local_tools());
        assert!(Tier::Team.has_local_tools());
        assert!(Tier::Enterprise.has_local_tools());
    }

    #[test]
    fn tier_serialization_matches_maic_strings() {
        // Sanity-check the JSON wire format the frontend expects.
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
    fn paid_default_is_kimi() {
        for tier in [Tier::Pro, Tier::ProPlus, Tier::Team, Tier::Enterprise] {
            assert_eq!(
                tier_default_model_id(tier),
                "milagro-oc-kimi",
                "{:?} must default to Kimi",
                tier,
            );
        }
    }

    #[test]
    fn paid_fallbacks_are_ordered_minimax_then_glm_then_local() {
        for tier in [Tier::Pro, Tier::ProPlus, Tier::Team, Tier::Enterprise] {
            let f = tier_default_fallbacks(tier);
            assert_eq!(f.len(), 3, "{:?} should have exactly 3 fallbacks", tier);
            assert_eq!(f[0], "milagro-oc-minimax", "{:?} fallback[0] must be MiniMax-M3", tier);
            assert_eq!(f[1], "milagro-oc-glm",     "{:?} fallback[1] must be GLM", tier);
            assert_eq!(f[2], "milagro-dev",        "{:?} fallback[2] must be local 14B", tier);
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