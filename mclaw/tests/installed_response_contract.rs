//! Cross-surface wire contract test for `/v1/modules/installed`.
//!
//! Week 7.5: All three surfaces (Tauri launchpad, mc-terminal TUI,
//! mclaw CLI) deserialize the same `GET /v1/modules/installed`
//! response from MAIC. To catch schema drift, this test pins the
//! canonical fixture and asserts mclaw's deserializer accepts it
//! (mclaw is the most type-strict surface; if mclaw parses, the
//! raw-Value Tauri's `mc_list_installed_modules` and the
//! serde-derived mc-terminal `InstalledModulesResponse` will too).
//!
//! The fixture is also referenced verbatim in
//! `src-tauri/src/bin/mc_terminal.rs` (struct `InstalledModule`)
//! and `src-tauri/src/lib.rs` (`mc_list_installed_modules` returns
//! the raw `serde_json::Value` and lets the frontend pick what it
//! needs). See `docs/mc-module-driven-surfaces-2026-09.md` §3.1
//! for the authoritative shape contract.
//!
//! Run: `cargo test --test installed_response_contract`

use mclaw::client::{InstalledModule, InstalledResponse};

/// Canonical MAIC response for `GET /v1/modules/installed`.
/// MUST match the wire shape in `api/routes/modules.py::list_installed`.
const CANONICAL_FIXTURE: &str = r#"{
    "modules": [
        {
            "module_id": "firecrawl",
            "name": "Firecrawl Web Scraper",
            "description": "Scrape any URL to clean markdown.",
            "icon": "🔥",
            "category": "data",
            "subcategory": null,
            "installed_at": "2026-09-10T10:00:00Z",
            "last_used_at": "2026-09-10T10:05:00Z",
            "client_manifest": {
                "module_id": "firecrawl",
                "name": "Firecrawl Web Scraper",
                "tools": [
                    {
                        "name": "scrape",
                        "description": "Scrape a URL to clean markdown",
                        "form_schema": {
                            "fields": [
                                {"name": "url", "type": "text", "label": "URL", "required": true}
                            ]
                        }
                    }
                ]
            }
        },
        {
            "module_id": "data-analyzer",
            "name": "Data Analyzer",
            "description": "Summarize CSV/JSON datasets.",
            "icon": "📊",
            "category": "data",
            "subcategory": "analytics",
            "installed_at": "2026-09-09T08:30:00Z",
            "last_used_at": null,
            "client_manifest": {
                "module_id": "data-analyzer",
                "tools": [
                    {"name": "summarize", "form_schema": {"fields": []}}
                ]
            }
        }
    ],
    "grouped_by_category": {
        "data": ["firecrawl", "data-analyzer"]
    }
}"#;

#[test]
fn canonical_fixture_deserializes_through_mclaw() {
    // mclaw is the strictest deserializer: every field is typed and
    // required (with #[serde(default)] on a few). If mclaw accepts
    // the fixture, the looser surfaces (raw Value in Tauri,
    // permissive serde in mc-terminal) will too.
    let parsed: InstalledResponse = serde_json::from_str(CANONICAL_FIXTURE)
        .expect("canonical /v1/modules/installed fixture must deserialize through mclaw's InstalledResponse");
    assert_eq!(parsed.modules.len(), 2, "expected 2 modules in fixture");
    assert_eq!(parsed.grouped_by_category.get("data").unwrap().len(), 2);
    let firecrawl: &InstalledModule = parsed
        .modules
        .iter()
        .find(|m| m.module_id == "firecrawl")
        .expect("firecrawl must be in fixture");
    assert_eq!(firecrawl.category, "data");
    assert!(firecrawl.last_used_at.is_some());
    let analyzer: &InstalledModule = parsed
        .modules
        .iter()
        .find(|m| m.module_id == "data-analyzer")
        .expect("data-analyzer must be in fixture");
    assert_eq!(analyzer.subcategory.as_deref(), Some("analytics"));
    assert!(analyzer.last_used_at.is_none());
}

#[test]
fn minimal_response_with_empty_modules_array_is_accepted() {
    // mc-terminal relies on this shape for its first-launch landing
    // (when no modules are installed). The deserializer must NOT
    // require a non-empty `modules` array.
    let resp: InstalledResponse = serde_json::from_str(r#"{
        "modules": [],
        "grouped_by_category": {}
    }"#)
    .expect("empty installed-modules response must be accepted");
    assert!(resp.modules.is_empty());
    assert!(resp.grouped_by_category.is_empty());
}

#[test]
fn client_manifest_is_opaque_value_so_all_surfaces_can_use_it() {
    // The `client_manifest` field is a `serde_json::Value` in all
    // three deserializers. We assert that:
    //   - The fixture's manifest is the full object (not a string).
    //   - All three surfaces can extract `tools` from it without
    //     re-deserializing.
    let resp: InstalledResponse = serde_json::from_str(CANONICAL_FIXTURE)
        .expect("fixture must parse");
    let firecrawl = &resp.modules[0];
    let tools = firecrawl
        .client_manifest
        .get("tools")
        .and_then(|v| v.as_array())
        .expect("client_manifest.tools must be a JSON array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "scrape");
}

#[test]
fn grouped_by_category_keys_must_match_module_categories() {
    // Soft contract: every module_id in `grouped_by_category[cat]`
    // should also appear in `modules[].module_id` with the same
    // `category`. This catches the case where the server returns
    // a category for a module that doesn't exist (or vice versa).
    let resp: InstalledResponse = serde_json::from_str(CANONICAL_FIXTURE)
        .expect("fixture must parse");
    let module_by_id: std::collections::HashMap<&str, &InstalledModule> = resp
        .modules
        .iter()
        .map(|m| (m.module_id.as_str(), m))
        .collect();
    for (cat, ids) in &resp.grouped_by_category {
        for id in ids {
            let m = module_by_id
                .get(id.as_str())
                .unwrap_or_else(|| panic!("grouped_by_category[{}] references unknown module_id: {}", cat, id));
            assert_eq!(
                &m.category, cat,
                "module {} has category {} but is listed under grouped_by_category[{}]",
                id, m.category, cat
            );
        }
    }
}
