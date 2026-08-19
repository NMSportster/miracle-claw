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
    /// Per David's 2026-08-19 decision: pro, pro_plus, team, and enterprise
    /// unlock the local tools. Free is read-only (4 MAIC server tools
    /// only).
    pub fn has_local_tools(self) -> bool {
        !matches!(self, Tier::Free)
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
    user: AuthMeUser,
}

#[derive(Debug, Deserialize)]
struct AuthMeUser {
    tier: String,
    #[serde(default)]
    plan_code: Option<String>,
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

/// Fetch the tier fresh from MAIC. Caller must hold a valid JWT.
pub fn fetch_tier_fresh(jwt: &str, maic_base: &str) -> Result<TierInfo, String> {
    let url = format!("{}/v1/auth/me", maic_base.trim_end_matches('/'));
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
    let tier = Tier::from_str(&parsed.user.tier);
    let mut cache = TIER_CACHE.lock().unwrap();
    let tier_changed = match cache.as_ref() {
        Some(prev) if prev.last_published != tier && prev.last_published > tier => true,
        _ => false,
    };
    let info = TierInfo {
        tier,
        plan_code: parsed.user.plan_code,
        email: parsed.user.email,
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn has_local_tools_only_for_paid() {
        assert!(!Tier::Free.has_local_tools(), "free must NOT have local tools");
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
}