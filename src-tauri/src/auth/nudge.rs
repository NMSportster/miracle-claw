//! Token-quota nudge evaluation.
//!
//! Free tier: nudge at 500 / 1000 / cap-of-monthly-quota tokens used.
//! Paid tier (pro/pro_plus/team/enterprise): nudge at 80% / 95% of monthly
//! quota.
//!
//! Copy is served from MAIC's response so we can A/B test without
//! shipping a new MC binary. MAIC returns a `messages` array per `kind`;
//! MC picks the first message whose `audience` matches the user's tier.
//!
//! Source: MAIC endpoint `GET /v1/usage/quota` returns:
//! ```json
//! {
//!   "tier": "pro",
//!   "used": 12345,
//!   "limit": 1000000,
//!   "period": "monthly",
//!   "period_start": "2026-08-01T00:00:00Z",
//!   "period_end":   "2026-09-01T00:00:00Z",
//!   "reset_at":     "2026-09-01T00:00:00Z",
//!   "messages": [
//!     { "kind": "soft",   "audience": "free",  "text": "You've used 500 tokens. ..." },
//!     { "kind": "hard",   "audience": "free",  "text": "You've hit the free cap..." },
//!     { "kind": "soft80", "audience": "paid",  "text": "You've used 80% of your monthly quota..." },
//!     { "kind": "hard95", "audience": "paid",  "text": "You've used 95%..." }
//!   ]
//! }
//! ```
//!
//! If MAIC doesn't return a matching message for the user's tier, MC
//! falls back to a hard-coded generic message (don't block the user
//! behind a server bug).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::tier::Tier;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeKind {
    None,
    /// Free tier: 500 tokens used. Mild, non-blocking.
    Free500,
    /// Free tier: 1000 tokens used. Stronger, has a CTA.
    Free1000,
    /// Free tier: monthly cap reached. Hard stop.
    FreeCap,
    /// Paid tier: 80% of monthly quota. Informational.
    Paid80,
    /// Paid tier: 95% of monthly quota. Stronger, has a CTA.
    Paid95,
    /// Paid tier: 100% of monthly quota. MAIC handles the hard stop
    /// (it returns 402 Payment Required); MC shows a courtesy message.
    Paid100,
}

impl NudgeKind {
    pub fn audience(self) -> &'static str {
        match self {
            NudgeKind::Free500 | NudgeKind::Free1000 | NudgeKind::FreeCap => "free",
            NudgeKind::Paid80 | NudgeKind::Paid95 | NudgeKind::Paid100 => "paid",
            NudgeKind::None => "none",
        }
    }

    pub fn wire_label(self) -> &'static str {
        match self {
            NudgeKind::None => "none",
            NudgeKind::Free500 => "soft",
            NudgeKind::Free1000 => "soft",
            NudgeKind::FreeCap => "hard",
            NudgeKind::Paid80 => "soft80",
            NudgeKind::Paid95 => "hard95",
            NudgeKind::Paid100 => "hard100",
        }
    }

    pub fn blocks_input(self) -> bool {
        // Free tier hard-stop is enforced client-side too, in case MAIC
        // isn't reachable to do the server-side block.
        matches!(self, NudgeKind::FreeCap)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeDecision {
    pub kind: NudgeKind,
    pub used: u64,
    pub limit: u64,
    pub text: String,
    /// True iff this came from MAIC. False iff we used the fallback
    /// hard-coded text.
    pub from_server: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QuotaResponse {
    #[serde(default)]
    used: u64,
    #[serde(default)]
    limit: u64,
    #[serde(default)]
    messages: Vec<QuotaMessage>,
}

#[derive(Debug, Deserialize, Clone)]
struct QuotaMessage {
    kind: String,
    #[serde(default)]
    audience: Option<String>,
    text: String,
}

const CACHE_TTL: Duration = Duration::from_secs(30); // tighter than tier — nudge is dynamic

static QUOTA_CACHE: once_cell::sync::Lazy<Mutex<Option<(QuotaResponse, Instant)>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

pub fn fetch_quota_cached(jwt: &str, maic_base: &str) -> Result<QuotaResponse, String> {
    let cache = QUOTA_CACHE.lock().unwrap();
    if let Some((c, ts)) = cache.as_ref() {
        if ts.elapsed() < CACHE_TTL {
            return Ok(c.clone());
        }
    }
    drop(cache);
    fetch_quota_fresh(jwt, maic_base)
}

pub fn fetch_quota_fresh(jwt: &str, maic_base: &str) -> Result<QuotaResponse, String> {
    // Lesson 512: strip trailing /v1 from the base URL. `openclaw.json`'s
    // `baseUrl` is the OpenAI-completions endpoint (e.g. `.../v1`), but the
    // tier/quota endpoints live at the gateway root (`/v1/auth/me`),
    // not under `/v1/`. Without stripping, requests double-up to
    // `.../v1/v1/usage/quota` and 404. Shared with tier.rs.
    let url = format!("{}/v1/usage/quota", crate::auth::tier::normalize_api_base(maic_base));
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", jwt))
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .call();
    let parsed: QuotaResponse = match resp {
        Ok(r) => r
            .into_json()
            .map_err(|e| format!("parse /v1/usage/quota: {}", e))?,
        Err(e) => return Err(format!("/v1/usage/quota: {}", e)),
    };
    if let Ok(mut cache) = QUOTA_CACHE.lock() {
        *cache = Some((parsed.clone(), Instant::now()));
    }
    Ok(parsed)
}

pub fn invalidate_quota_cache() {
    if let Ok(mut cache) = QUOTA_CACHE.lock() {
        *cache = None;
    }
}

/// Evaluate whether a nudge should fire right now given the user's tier
/// and quota. Returns the appropriate message (from MAIC if available,
/// fallback otherwise).
pub fn evaluate_nudge(tier: Tier, quota: &QuotaResponse) -> NudgeDecision {
    let kind = compute_kind(tier, quota.used, quota.limit);
    let text = pick_message(kind, &quota.messages);
    NudgeDecision {
        kind,
        used: quota.used,
        limit: quota.limit,
        text,
        from_server: quota.messages.iter().any(|m| m.audience.as_deref() == Some(kind.audience())
            && m.kind == kind.wire_label()),
    }
}

fn compute_kind(tier: Tier, used: u64, limit: u64) -> NudgeKind {
    match tier {
        Tier::Free => {
            // Three explicit thresholds. Don't extrapolate past the
            // monthly limit; that's MAIC's job (returns 402).
            if limit > 0 && used >= limit {
                NudgeKind::FreeCap
            } else if used >= 1000 {
                NudgeKind::Free1000
            } else if used >= 500 {
                NudgeKind::Free500
            } else {
                NudgeKind::None
            }
        }
        Tier::Pro | Tier::ProPlus | Tier::Team | Tier::Enterprise => {
            if limit == 0 {
                return NudgeKind::None; // No quota configured; nothing to nudge.
            }
            let pct = (used as f64 / limit as f64) * 100.0;
            if pct >= 100.0 {
                NudgeKind::Paid100
            } else if pct >= 95.0 {
                NudgeKind::Paid95
            } else if pct >= 80.0 {
                NudgeKind::Paid80
            } else {
                NudgeKind::None
            }
        }
    }
}

fn pick_message(kind: NudgeKind, messages: &[QuotaMessage]) -> String {
    let wanted_audience = kind.audience();
    let wanted_kind = kind.wire_label();
    // Prefer an audience+kind match. Fall back to just-kind match across
    // audiences (so MAIC can ship a generic "soft" copy without breaking
    // us). Final fallback is a hard-coded generic.
    if let Some(m) = messages
        .iter()
        .find(|m| m.kind == wanted_kind && m.audience.as_deref() == Some(wanted_audience))
    {
        return m.text.clone();
    }
    if let Some(m) = messages.iter().find(|m| m.kind == wanted_kind) {
        return m.text.clone();
    }
    fallback_text(kind)
}

fn fallback_text(kind: NudgeKind) -> String {
    match kind {
        NudgeKind::None => String::new(),
        NudgeKind::Free500 => {
            "You've used 500 tokens on the free tier. Upgrade to Pro for file tools and unlimited usage.".to_string()
        }
        NudgeKind::Free1000 => {
            "You're approaching the free-tier limit. Upgrade to Pro to keep going.".to_string()
        }
        NudgeKind::FreeCap => {
            "You've hit the free-tier cap. Upgrade to Pro to continue chatting.".to_string()
        }
        NudgeKind::Paid80 => {
            "You've used 80% of your monthly quota. Resets on the 1st.".to_string()
        }
        NudgeKind::Paid95 => {
            "You've used 95% of your monthly quota. Consider upgrading for headroom.".to_string()
        }
        NudgeKind::Paid100 => {
            "You've hit your monthly quota. Chat will resume after reset.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(used: u64, limit: u64) -> QuotaResponse {
        QuotaResponse {
            used,
            limit,
            messages: vec![],
        }
    }

    #[test]
    fn free_thresholds_500_1000_cap() {
        assert_eq!(compute_kind(Tier::Free, 0, 100000), NudgeKind::None);
        assert_eq!(compute_kind(Tier::Free, 499, 100000), NudgeKind::None);
        assert_eq!(compute_kind(Tier::Free, 500, 100000), NudgeKind::Free500);
        assert_eq!(compute_kind(Tier::Free, 999, 100000), NudgeKind::Free500);
        assert_eq!(compute_kind(Tier::Free, 1000, 100000), NudgeKind::Free1000);
        assert_eq!(compute_kind(Tier::Free, 99999, 100000), NudgeKind::Free1000);
        assert_eq!(compute_kind(Tier::Free, 100000, 100000), NudgeKind::FreeCap);
    }

    #[test]
    fn paid_thresholds_80_95_100() {
        // 1M tokens monthly
        assert_eq!(compute_kind(Tier::Pro, 0, 1_000_000), NudgeKind::None);
        assert_eq!(compute_kind(Tier::Pro, 799_999, 1_000_000), NudgeKind::None);
        assert_eq!(compute_kind(Tier::Pro, 800_000, 1_000_000), NudgeKind::Paid80);
        assert_eq!(compute_kind(Tier::Pro, 949_999, 1_000_000), NudgeKind::Paid80);
        assert_eq!(compute_kind(Tier::Pro, 950_000, 1_000_000), NudgeKind::Paid95);
        assert_eq!(compute_kind(Tier::Pro, 999_999, 1_000_000), NudgeKind::Paid95);
        assert_eq!(compute_kind(Tier::Pro, 1_000_000, 1_000_000), NudgeKind::Paid100);
        assert_eq!(compute_kind(Tier::Pro, 2_000_000, 1_000_000), NudgeKind::Paid100);
    }

    #[test]
    fn paid_no_quota_means_no_nudge() {
        assert_eq!(compute_kind(Tier::Pro, 9999, 0), NudgeKind::None);
    }

    #[test]
    fn evaluate_uses_server_message_when_present() {
        let mut q = quota(500, 100000);
        q.messages.push(QuotaMessage {
            kind: "soft".into(),
            audience: Some("free".into()),
            text: "Server copy: 500 tokens".into(),
        });
        let d = evaluate_nudge(Tier::Free, &q);
        assert_eq!(d.kind, NudgeKind::Free500);
        assert!(d.from_server);
        assert_eq!(d.text, "Server copy: 500 tokens");
    }

    #[test]
    fn evaluate_falls_back_to_hardcoded_when_no_server_message() {
        let q = quota(1000, 100000);
        let d = evaluate_nudge(Tier::Free, &q);
        assert_eq!(d.kind, NudgeKind::Free1000);
        assert!(!d.from_server);
        assert!(d.text.contains("free-tier"));
    }

    #[test]
    fn evaluate_prefers_audience_match_over_generic_kind_match() {
        let mut q = quota(800_000, 1_000_000);
        q.messages.push(QuotaMessage {
            kind: "soft80".into(),
            audience: Some("paid".into()),
            text: "Audience-matched: 80%".into(),
        });
        q.messages.push(QuotaMessage {
            kind: "soft80".into(),
            audience: Some("free".into()), // wrong audience
            text: "Wrong-audience: 80%".into(),
        });
        let d = evaluate_nudge(Tier::Pro, &q);
        assert_eq!(d.text, "Audience-matched: 80%");
    }

    #[test]
    fn blocks_input_only_on_free_cap() {
        assert!(NudgeKind::FreeCap.blocks_input());
        assert!(!NudgeKind::Free1000.blocks_input());
        assert!(!NudgeKind::Free500.blocks_input());
        assert!(!NudgeKind::Paid80.blocks_input());
        assert!(!NudgeKind::Paid95.blocks_input());
        assert!(!NudgeKind::Paid100.blocks_input());
        assert!(!NudgeKind::None.blocks_input());
    }

    #[test]
    fn fallback_text_non_empty_for_all_active_kinds() {
        for k in [
            NudgeKind::Free500,
            NudgeKind::Free1000,
            NudgeKind::FreeCap,
            NudgeKind::Paid80,
            NudgeKind::Paid95,
            NudgeKind::Paid100,
        ] {
            assert!(
                !fallback_text(k).is_empty(),
                "fallback_text empty for {:?}",
                k
            );
        }
        assert_eq!(fallback_text(NudgeKind::None), "");
    }
}