//! Authentication, tier, and quota surface for MC v1.0.7+.
//!
//! Modules:
//! - `tier` — fetches `/v1/auth/me` and exposes `mc_get_tier` (5-min cache).
//! - `nudge` — fetches `/v1/usage/quota` and returns threshold + copy.
//!
//! Both read from the same MAIC JWT the chat session uses (via
//! `crate::maic::current_jwt`); neither mutates it.

pub mod tier;
pub mod nudge;

pub use tier::{Tier, TierInfo, current_tier, fetch_tier_cached, invalidate_tier_cache};
pub use nudge::{NudgeDecision, NudgeKind, evaluate_nudge, fetch_quota_cached};