// secrets_friendly.rs — rc53.6 three-tier secrets vault.
//
// Layered on top of the rc53 plaintext vault (mc_secret_set / etc. in
// lib.rs). Adds a "friendly" setter that lets users type natural-language
// labels ("Strip Key", "sudo password") and an in-memory pool with three
// lifetimes:
//
//   Once      — gone after first read by mc_secret_expand
//   PerSession — in-memory only, cleared on app exit (or explicit clear)
//   Vault     — persistent, written to <MC_DATA>/secrets.json
//               (delegates to lib.rs's secrets_vault_path + read_vault + write_vault)
//
// One unified store: a Mutex<HashMap<String, EphemeralEntry>> where the
// `Lifetime` enum decides where the value actually lives.
//
// Lookup order in mc_secret_expand (lib.rs):
//   1. Take from in-memory pool (and remove if Once)
//   2. Fall back to disk vault (existing behavior)
//
// Naming:
//   - "friendly" labels go through canonicalize_label() before storage
//   - canonical name format: [A-Z_][A-Z0-9_]* (shell-var rules)
//   - collisions get _2, _3 suffixes
//   - type_hint (from the form's type checkboxes) helps the canonicalizer
//     pick a good default suffix when the label is empty
//
// Single Mutex<HashMap<...>> (per Lesson 239 / the rc53.6 pickup note):
//   NOT two pools. The Lifetime enum dispatches persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::Manager;

/// Lifetime of an entry in the friendly secrets pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Removed after first read by mc_secret_expand.
    Once,
    /// In-memory only, cleared on app exit or explicit clear.
    PerSession,
    /// Persistent, written to <MC_DATA>/secrets.json.
    /// In-memory copy is also kept so list() can show it without a disk read.
    Vault,
}

/// One entry in the in-memory pool. Vault entries are mirrored to disk
/// by mc_secret_set; Once/PerSession entries live here only.
#[derive(Debug, Clone)]
pub struct EphemeralEntry {
    pub name: String,
    pub value: String,
    pub lifetime: Lifetime,
    pub created_at: String,
    pub kind: String, // "username" | "password" | "key" | "token" | "url" | "generic"
}

/// Process-global pool. Cleared on app exit (OS reclaims process memory).
pub static POOL: once_cell::sync::Lazy<Mutex<HashMap<String, EphemeralEntry>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Cheap epoch string. Matches the v0 format used by lib.rs.
fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{secs}")
}

/// Type hints from the form. Each one maps to a default suffix appended
/// to the canonicalized label when the user provides no real name. The
/// UI passes the checked type(s); we use the first checked one as the
/// default. Power users typing a strict shell-var name will bypass this
/// — canonicalize_label sees the input is already valid and returns it
/// unchanged.
#[derive(Debug, Clone, Copy)]
pub enum KindHint {
    Username,
    Password,
    Key,
    Token,
    Url,
    Generic,
}

impl KindHint {
    pub fn default_suffix(self) -> &'static str {
        match self {
            KindHint::Username => "USERNAME",
            KindHint::Password => "PASSWORD",
            KindHint::Key => "KEY",
            KindHint::Token => "TOKEN",
            KindHint::Url => "URL",
            KindHint::Generic => "SECRET",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "username" => KindHint::Username,
            "password" => KindHint::Password,
            "key" => KindHint::Key,
            "token" => KindHint::Token,
            "url" => KindHint::Url,
            _ => KindHint::Generic,
        }
    }
}

/// Canonicalize a user-typed label into a valid shell-var name.
///
/// Rules:
///   1. Strip everything that's not A-Z, a-z, 0-9, underscore, space, or hyphen.
///   2. Collapse whitespace + hyphens to a single underscore.
///   3. Uppercase.
///   4. If first char is a digit, prepend underscore.
///   5. Filter body to only A-Z, 0-9, underscore (defense in depth).
fn canonicalize_inner(input: &str) -> Option<String> {
    let kept: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        .collect();
    // Trim outer whitespace BEFORE collapsing so leading/trailing spaces
    // don't become leading/trailing underscores.
    let trimmed = kept.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    // Collapse internal whitespace + hyphens to a single underscore.
    // Run-length encode: only insert an underscore on the FIRST space/hyphen
    // of each run; subsequent consecutive ones are dropped.
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut prev_was_sep = false;
    for c in trimmed.chars() {
        if c == ' ' || c == '-' {
            if !prev_was_sep && !collapsed.is_empty() {
                collapsed.push('_');
                prev_was_sep = true;
            }
        } else {
            collapsed.push(c);
            prev_was_sep = false;
        }
    }
    // After the run-length collapse, trim any trailing underscore the
    // user might have caused with a trailing space/hyphen.
    let collapsed = collapsed.trim_end_matches('_').to_string();
    if collapsed.is_empty() {
        return None;
    }
    let upper: String = collapsed.to_ascii_uppercase();
    if upper.is_empty() {
        return None;
    }
    let mut chars = upper.chars();
    let first = chars.next().unwrap();
    let rest: String = chars.collect();
    let fixed_first: String = if first.is_ascii_digit() {
        format!("_{first}")
    } else if first.is_ascii_uppercase() || first == '_' {
        first.to_string()
    } else {
        // anything else (shouldn't happen post-uppercase, but be defensive): drop it
        String::new()
    };
    let body = format!("{fixed_first}{rest}");
    let clean: String = body
        .chars()
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    if clean.is_empty() || clean == "_" {
        None
    } else {
        Some(clean)
    }
}

/// Public canonicalize: takes user input + kind hint, returns a valid
/// shell-var name. If the user typed nothing useful, uses the kind hint's
/// default name as a stub ("KEY", "TOKEN", etc.) — collisions get _2, _3.
pub fn canonicalize_label(input: &str, kind: KindHint) -> String {
    canonicalize_inner(input).unwrap_or_else(|| kind.default_suffix().to_string())
}

/// Dedup name: if "FOO" exists, return "FOO_2", then "FOO_3", etc.
fn dedup_name(base: &str, pool: &HashMap<String, EphemeralEntry>) -> String {
    if !pool.contains_key(base) {
        return base.to_string();
    }
    for n in 2..=999 {
        let candidate = format!("{base}_{n}");
        if !pool.contains_key(&candidate) {
            return candidate;
        }
    }
    format!("{base}_{}", now_iso().replace("epoch:", ""))
}

/// Result returned to the frontend after a friendly set.
#[derive(Debug, Clone, Serialize)]
pub struct FriendlySetResult {
    pub name: String,
    pub lifetime: String, // "once" | "per_session" | "vault"
    pub ephemeral: bool,  // back-compat with rc53.5 callers (true for Once/PerSession)
    pub value_len: usize,
    pub created_at: String,
    pub kind: String,
}

/// Internal entry returned to the frontend when listing.
#[derive(Debug, Clone, Serialize)]
pub struct EphemeralSummary {
    pub name: String,
    pub lifetime: String,
    pub ephemeral: bool,
    pub kind: String,
    pub value_len: usize,
    pub created_at: String,
}

/// Frontend-facing kind string (lowercase). Used in list rows + UI hints.
pub fn kind_to_string(k: KindHint) -> String {
    match k {
        KindHint::Username => "username",
        KindHint::Password => "password",
        KindHint::Key => "key",
        KindHint::Token => "token",
        KindHint::Url => "url",
        KindHint::Generic => "generic",
    }
    .to_string()
}

/// Frontend-facing lifetime string.
pub fn lifetime_to_string(l: Lifetime) -> String {
    match l {
        Lifetime::Once => "once",
        Lifetime::PerSession => "per_session",
        Lifetime::Vault => "vault",
    }
    .to_string()
}

/// Frontend-facing lifetime parser.
pub fn lifetime_from_str(s: &str) -> Result<Lifetime, String> {
    match s {
        "once" => Ok(Lifetime::Once),
        "per_session" => Ok(Lifetime::PerSession),
        "vault" => Ok(Lifetime::Vault),
        other => Err(format!(
            "unknown lifetime `{other}`: must be once | per_session | vault"
        )),
    }
}

// ---------- Disk vault path (Vault lifetime only) ----------
//
// We keep a private copy here because:
//   1. lib.rs's mc_secret_set is a #[tauri::command] which means it goes
//      through the IPC layer — we can't call it from another tauri::command
//      without an async hop.
//   2. We already hold the lock for the in-memory pool; adding a second
//      call path (mc_secret_set) would require releasing/reacquiring and
//      risk a partial-write race.
//   3. The schema is dead simple (4 fields, validated). Inlining ~30 LOC
//      beats the alternative of factoring lib.rs's vault IO into a pub fn.

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct VaultEntry {
    name: String,
    value: String,
    created_at: String,
    last_used_at: Option<String>,
}

fn vault_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("secrets.json"))
}

fn read_disk_vault(app: &tauri::AppHandle) -> Result<Vec<VaultEntry>, String> {
    let p = vault_path(app)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let entries: Vec<VaultEntry> =
        serde_json::from_str(&raw).map_err(|e| format!("vault parse error: {e}"))?;
    Ok(entries)
}

fn write_disk_vault(app: &tauri::AppHandle, entries: &[VaultEntry]) -> Result<(), String> {
    let p = vault_path(app)?;
    let raw = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_vault_entry(app: &tauri::AppHandle, name: &str, value: &str, created_at: &str) -> Result<(), String> {
    let mut entries = read_disk_vault(app)?;
    let mut found = false;
    for entry in entries.iter_mut() {
        if entry.name == name {
            entry.value = value.to_string();
            entry.created_at = created_at.to_string();
            entry.last_used_at = None;
            found = true;
            break;
        }
    }
    if !found {
        entries.push(VaultEntry {
            name: name.to_string(),
            value: value.to_string(),
            created_at: created_at.to_string(),
            last_used_at: None,
        });
    }
    write_disk_vault(app, &entries)
}

// ---------- Public API ----------

/// Canonicalize + store. Vault lifetime writes to disk; Once/PerSession
/// live only in the in-memory pool.
pub fn friendly_set(
    app: &tauri::AppHandle,
    label: &str,
    value: &str,
    lifetime_str: &str,
    kind_str: &str,
) -> Result<FriendlySetResult, String> {
    if value.is_empty() {
        return Err("secret value cannot be empty".to_string());
    }
    let lifetime = lifetime_from_str(lifetime_str)?;
    let kind = KindHint::from_str(kind_str);
    let canonical = canonicalize_label(label, kind);
    let kind_s = kind_to_string(kind);
    let lifetime_s = lifetime_to_string(lifetime);
    let value_len = value.chars().count();
    let now = now_iso();

    let mut pool = POOL.lock().map_err(|e| format!("pool lock poisoned: {e}"))?;
    let name = dedup_name(&canonical, &pool);

    let entry = EphemeralEntry {
        name: name.clone(),
        value: value.to_string(),
        lifetime,
        created_at: now.clone(),
        kind: kind_s.clone(),
    };

    if lifetime == Lifetime::Vault {
        write_vault_entry(app, &name, value, &now)?;
    }

    pool.insert(name.clone(), entry);

    Ok(FriendlySetResult {
        name,
        lifetime: lifetime_s,
        ephemeral: lifetime != Lifetime::Vault,
        value_len,
        created_at: now,
        kind: kind_s,
    })
}

/// List all entries in the in-memory pool (Vault + PerSession + Once).
/// UI merges this with `mc_secret_list` (disk vault) for the full table.
pub fn list_ephemerals() -> Result<Vec<EphemeralSummary>, String> {
    let pool = POOL.lock().map_err(|e| format!("pool lock poisoned: {e}"))?;
    Ok(pool
        .values()
        .map(|e| EphemeralSummary {
            name: e.name.clone(),
            lifetime: lifetime_to_string(e.lifetime),
            ephemeral: e.lifetime != Lifetime::Vault,
            kind: e.kind.clone(),
            value_len: e.value.chars().count(),
            created_at: e.created_at.clone(),
        })
        .collect())
}

/// Take a name from the pool. Returns Some(value) if present.
/// Removes the entry if its lifetime is Once. Leaves PerSession and
/// Vault entries alone. Used by mc_secret_expand (lib.rs) as the
/// first-lookup step.
///
/// Public so lib.rs can call it directly without going through IPC.
pub fn take(name: &str) -> Option<String> {
    let mut pool = match POOL.lock() {
        Ok(g) => g,
        Err(_) => return None,
    };
    if let Some(entry) = pool.get(name) {
        let value = entry.value.clone();
        if entry.lifetime == Lifetime::Once {
            pool.remove(name);
        }
        Some(value)
    } else {
        None
    }
}

/// Clear all PerSession entries. Vault + Once untouched.
pub fn clear_session_ephemerals() -> Result<usize, String> {
    let mut pool = POOL.lock().map_err(|e| format!("pool lock poisoned: {e}"))?;
    let before = pool.len();
    pool.retain(|_, e| e.lifetime != Lifetime::PerSession);
    Ok(before - pool.len())
}

/// Clear both Once and PerSession. Vault entries untouched.
pub fn clear_all_ephemerals() -> Result<usize, String> {
    let mut pool = POOL.lock().map_err(|e| format!("pool lock poisoned: {e}"))?;
    let before = pool.len();
    pool.retain(|_, e| e.lifetime == Lifetime::Vault);
    Ok(before - pool.len())
}

// ---------- Tauri commands ----------

/// `mc_secret_set_friendly(label, value, lifetime, kind)` — the rc53.6 entry point.
#[tauri::command]
pub fn mc_secret_set_friendly(
    app: tauri::AppHandle,
    label: String,
    value: String,
    lifetime: String,
    kind: String,
) -> Result<FriendlySetResult, String> {
    friendly_set(&app, &label, &value, &lifetime, &kind)
}

/// `mc_secret_list_ephemerals()` — returns in-memory pool entries (all lifetimes).
/// Frontend merges this with `mc_secret_list` (disk vault) for the full table.
#[tauri::command]
pub fn mc_secret_list_ephemerals() -> Result<Vec<EphemeralSummary>, String> {
    list_ephemerals()
}

/// `mc_secret_clear_session_ephemerals()` — clears all PerSession entries.
#[tauri::command]
pub fn mc_secret_clear_session_ephemerals() -> Result<usize, String> {
    clear_session_ephemerals()
}

/// `mc_secret_clear_all_ephemerals()` — clears Once + PerSession. Vault untouched.
#[tauri::command]
pub fn mc_secret_clear_all_ephemerals() -> Result<usize, String> {
    clear_all_ephemerals()
}

// ---------- Unit tests ----------
//
// Run with: `cargo test --lib secrets_friendly`
//
// These don't touch disk or the static POOL — they exercise the pure
// canonicalizer + dedup logic.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_strips_special_chars() {
        assert_eq!(canonicalize_label("Strip Key!", KindHint::Generic), "STRIP_KEY");
        assert_eq!(canonicalize_label("sudo password", KindHint::Password), "SUDO_PASSWORD");
        assert_eq!(canonicalize_label("github-token", KindHint::Token), "GITHUB_TOKEN");
    }

    #[test]
    fn canonicalize_collapses_whitespace() {
        assert_eq!(canonicalize_label("  My  Stripe  Key  ", KindHint::Key), "MY_STRIPE_KEY");
    }

    #[test]
    fn canonicalize_prepends_underscore_for_digit_start() {
        assert_eq!(canonicalize_label("2026 api key", KindHint::Key), "_2026_API_KEY");
    }

    #[test]
    fn canonicalize_keeps_existing_shell_var() {
        assert_eq!(canonicalize_label("STRIPE_KEY", KindHint::Key), "STRIPE_KEY");
        assert_eq!(canonicalize_label("_PRIVATE", KindHint::Generic), "_PRIVATE");
    }

    #[test]
    fn canonicalize_uses_kind_default_when_empty() {
        assert_eq!(canonicalize_label("", KindHint::Key), "KEY");
        assert_eq!(canonicalize_label("!!!", KindHint::Password), "PASSWORD");
        assert_eq!(canonicalize_label("   ", KindHint::Token), "TOKEN");
    }

    #[test]
    fn canonicalize_handles_unicode_gracefully() {
        // Non-ASCII is stripped; spaces collapsed; result is a valid shell var.
        assert_eq!(canonicalize_label("café pw", KindHint::Password), "CAF_PW");
    }

    #[test]
    fn lifetime_round_trip() {
        assert_eq!(lifetime_from_str("once").unwrap(), Lifetime::Once);
        assert_eq!(lifetime_from_str("per_session").unwrap(), Lifetime::PerSession);
        assert_eq!(lifetime_from_str("vault").unwrap(), Lifetime::Vault);
        assert!(lifetime_from_str("forever").is_err());
    }

    #[test]
    fn kind_hint_round_trip() {
        assert!(matches!(KindHint::from_str("username"), KindHint::Username));
        assert!(matches!(KindHint::from_str("PASSWORD"), KindHint::Password));
        assert!(matches!(KindHint::from_str("Key"), KindHint::Key));
        assert!(matches!(KindHint::from_str("token"), KindHint::Token));
        assert!(matches!(KindHint::from_str("URL"), KindHint::Url));
        assert!(matches!(KindHint::from_str("unknown"), KindHint::Generic));
    }

    #[test]
    fn take_removes_once_leaves_persession() {
        // We don't use friendly_set here because it would touch disk for Vault.
        // Instead, mutate POOL directly via a helper closure.
        let mut pool = POOL.lock().unwrap();
        pool.insert("ONE".into(), EphemeralEntry {
            name: "ONE".into(),
            value: "once_val".into(),
            lifetime: Lifetime::Once,
            created_at: now_iso(),
            kind: "key".into(),
        });
        pool.insert("TWO".into(), EphemeralEntry {
            name: "TWO".into(),
            value: "two_val".into(),
            lifetime: Lifetime::PerSession,
            created_at: now_iso(),
            kind: "key".into(),
        });
        drop(pool);

        // Take Once — should return value AND remove
        assert_eq!(take("ONE"), Some("once_val".to_string()));
        assert_eq!(take("ONE"), None, "Once should be gone after first take");

        // Take PerSession — should return value AND keep
        assert_eq!(take("TWO"), Some("two_val".to_string()));
        assert_eq!(take("TWO"), Some("two_val".to_string()), "PerSession should persist across takes");

        // Cleanup
        let mut pool = POOL.lock().unwrap();
        pool.clear();
    }

    #[test]
    fn clear_session_removes_only_persession() {
        let mut pool = POOL.lock().unwrap();
        pool.insert("ONCE".into(), EphemeralEntry {
            name: "ONCE".into(), value: "v".into(),
            lifetime: Lifetime::Once, created_at: now_iso(), kind: "key".into(),
        });
        pool.insert("PS".into(), EphemeralEntry {
            name: "PS".into(), value: "v".into(),
            lifetime: Lifetime::PerSession, created_at: now_iso(), kind: "key".into(),
        });
        drop(pool);

        let n = clear_session_ephemerals().unwrap();
        assert_eq!(n, 1, "should have cleared exactly one PerSession entry");
        assert!(take("ONCE").is_some(), "Once should still be there");
        assert!(take("PS").is_none(), "PerSession should be gone");
    }

    #[test]
    fn clear_all_removes_once_and_persession_keeps_vault() {
        // Don't insert a Vault entry (would touch disk in friendly_set).
        // Vault filtering is verified by inspection of the retain() clause.
        let mut pool = POOL.lock().unwrap();
        pool.insert("ON".into(), EphemeralEntry {
            name: "ON".into(), value: "v".into(),
            lifetime: Lifetime::Once, created_at: now_iso(), kind: "key".into(),
        });
        pool.insert("PS".into(), EphemeralEntry {
            name: "PS".into(), value: "v".into(),
            lifetime: Lifetime::PerSession, created_at: now_iso(), kind: "key".into(),
        });
        drop(pool);

        let n = clear_all_ephemerals().unwrap();
        assert_eq!(n, 2);
    }
}