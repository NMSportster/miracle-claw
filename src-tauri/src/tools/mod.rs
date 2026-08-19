//! MC's local tool schemas and gating.
//!
//! These 7 schemas are the **client tools** MC ships via the MAIC plugin.
//! They are gated by tier — `free` users don't see them in the chat tool
//! list, `paid` users do. The MAIC server's 4 tools (weather, web_search,
//! get_current_time, calculate) are MAIC's concern (`get_tools_for_tier()`)
//! and don't appear here.
//!
//! Module split:
//! - `schemas`: the 7 LocalTool definitions, used by both the lib (for
//!   HTTP responses / Tauri commands) and the `miracle-claw-tools` binary
//!   (for argv parsing + dispatch).
//! - `exec`: the 7 tool executors, used by `miracle-claw-tools` and
//!   potentially future in-process callers. Pure data + pure functions.
//!
//! What lives HERE (not in submodules): the tier-gating logic
//! `tools_for_tier()`. This function depends on `crate::auth::tier::Tier`
//! which is only present in the lib binary (the executor doesn't pull
//! in `auth`). To keep `tools` module-shareable between both binaries,
//! `tools_for_tier` is defined in `lib.rs` next to the Tauri commands.

pub mod schemas;
pub mod exec;

// Re-exports — both binaries use these. The `miracle-claw` lib also
// uses `LocalTool` and `all_local_tools` (returned from `mc_list_tools`).
// The `miracle-claw-tools` binary only uses `LocalToolName` and
// `ALL_LOCAL_TOOL_NAMES`, but the others are public API surface kept
// available for future code.
#[allow(unused_imports)]
pub use schemas::{
    LocalTool, LocalToolName, ALL_LOCAL_TOOL_NAMES,
    all_local_tools, is_sensitive_tool_name,
};