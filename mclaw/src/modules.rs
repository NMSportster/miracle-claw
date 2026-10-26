//! `mclaw run`, `mclaw tools`, `mclaw marketplace` — module-driven surfaces (Week 5).
//!
//! These three commands back the third client surface in the rc56.0
//! Module-Driven Surfaces plan (spec:
//! `docs/mc-module-driven-surfaces-2026-09.md` §5.3). They share the
//! same MAIC endpoints as the Tauri launchpad and the mc-terminal TUI,
//! so installing a module here makes it appear in all three surfaces.
//!
//! ## `mclaw run <module>.<tool> [args]`
//!
//! One-shot invocation. Parses positional args as `--key=value` (and a
//! leading non-flag value as the tool name if missing). Sends to
//! `POST /v1/modules/{module_id}/run` with `surface: "mclaw"`. Prints
//! the model's response to stdout. `--json` for machine-readable output
//! that includes the run_id (for tracing in MAIC's `module_runs` table).
//!
//! ## `mclaw tools`
//!
//! Lists installed modules grouped by category. Default output is a
//! compact `category/module [icon] (tools: t1, t2, ...)` format; `--json`
//! dumps the full MAIC response including `client_manifest` per module.
//!
//! ## `mclaw marketplace [category]`
//!
//! Lists marketplace modules. Optional positional `category` filters by
//! category id (e.g. `data`, `content`, `social`). Without a filter, shows
//! all active modules. TTY output uses a compact table; `--json` for
//! machine-readable. `mclaw marketplace install <module_id>` is a
//! shortcut for `mclaw run`-style install without the run.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::banner;
use crate::client::{self, Client, RunJsonOut};

// ─────────────────────────────────────────────────────────────────────
// `mclaw run <module>.<tool> [args]`
// ─────────────────────────────────────────────────────────────────────

/// Run a tool on an installed module.
#[derive(Args, Debug)]
#[command(about = "Run a tool on an installed module", long_about = "Run a tool on an installed first-party module. The module must be installed first via `mclaw marketplace install <id>`.\n\nThe `module.tool` shorthand is preferred; the explicit `--tool` form is for shells where `.` is awkward.\n\nEXAMPLES:\n  mclaw run firecrawl.fetch_url --input url=https://example.com\n  mclaw run firecrawl.fetch_url --input url=https://example.com --json\n  mclaw run data-analyzer.summarize --input csv_url=https://example.com/data.csv --input focus=revenue\n  mclaw run brand-asset-kit.bio --input platform=linkedin --input 'keywords=auto repair, Albuquerque'")]
pub struct RunArgs {
    /// Module ID (e.g. "firecrawl") OR `module.tool` shorthand.
    positional: Vec<String>,

    /// Tool name when `module.tool` shorthand is not used.
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,

    /// Input as `key=value`. May be repeated. Numbers and JSON literals
    /// (true, false, null, [..], {..}) are parsed; everything else is a string.
    #[arg(long = "input", value_name = "K=V", value_parser = parse_kv)]
    inputs: Vec<(String, Value)>,

    /// Output machine-readable JSON.
    #[arg(long)]
    json: bool,
}

pub async fn run(args: RunArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let client = Client::new(&config)?;

    let (module_id, tool) = parse_module_tool(&args)?;

    // Build the inputs object from --input k=v pairs (or stdin if no flags).
    let mut inputs_map = serde_json::Map::new();
    for (k, v) in &args.inputs {
        inputs_map.insert(k.clone(), v.clone());
    }
    let inputs: Value = if inputs_map.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        Value::Object(inputs_map)
    };

    eprintln!(
        "{} running {}.{} ({} inputs)...",
        banner::version_line(),
        module_id,
        tool,
        inputs.as_object().map(|o| o.len()).unwrap_or(0)
    );

    let start = std::time::Instant::now();
    let resp = client.run_module(&module_id, &tool, &inputs).await.map_err(map_err)?;
    let elapsed = start.elapsed();

    if args.json {
        let out = RunJsonOut {
            module_id: resp.module_id,
            tool: resp.tool,
            outputs: resp.outputs,
            run_id: resp.run_id,
            duration_ms: resp.duration_ms,
            cli_duration_ms: elapsed.as_millis(),
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", resp.outputs);
        if !resp.outputs.ends_with('\n') {
            println!();
        }
        eprintln!(
            "[{}ms MAIC / {}ms CLI; run_id={}]",
            resp.duration_ms,
            elapsed.as_millis(),
            resp.run_id
        );
    }
    Ok(0)
}

/// Parse `module_id` and `tool` from the positional args + `--tool` flag.
///
/// Accepts these shapes:
///   `mclaw run firecrawl.scrape --input url=https://...`
///   `mclaw run firecrawl --tool scrape --input url=https://...`
///   `mclaw run firecrawl scrape --input url=https://...`   (legacy; the
///       second positional is treated as the tool name when no dot is present)
fn parse_module_tool(args: &RunArgs) -> Result<(String, String)> {
    // Shape 1: positional = ["module.tool"]
    if let Some(p) = args.positional.first() {
        if let Some((m, t)) = p.split_once('.') {
            if !m.is_empty() && !t.is_empty() {
                // If --tool was also passed, prefer the explicit flag (overrides).
                let tool = args.tool.clone().unwrap_or_else(|| t.to_string());
                return Ok((m.to_string(), tool));
            }
        }
    }

    // Shape 2: positional = ["module", "tool"] (legacy 2-arg)
    if args.positional.len() >= 2 {
        let module = args.positional[0].clone();
        let tool = args.positional[1].clone();
        return Ok((module, tool));
    }

    // Shape 3: positional = ["module"] + --tool
    if let Some(p) = args.positional.first() {
        if let Some(tool) = &args.tool {
            return Ok((p.clone(), tool.clone()));
        }
    }

    Err(anyhow::anyhow!(
        "usage: mclaw run <module>.<tool> [--input k=v ...]\n\
         or:    mclaw run <module> --tool <name> [--input k=v ...]"
    ))
}

/// Parse a `--input key=value` flag. The value can be a JSON literal
/// (true, false, null, numbers, [..], {..}) or a plain string.
fn parse_kv(s: &str) -> Result<(String, Value), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got '{}'", s))?;
    if k.is_empty() {
        return Err(format!("empty key in '{}'", s));
    }
    // Try JSON first; fall back to plain string.
    let parsed: Value = serde_json::from_str(v)
        .unwrap_or_else(|_| Value::String(v.to_string()));
    Ok((k.to_string(), parsed))
}

// ─────────────────────────────────────────────────────────────────────
// `mclaw tools`
// ─────────────────────────────────────────────────────────────────────

/// List installed modules grouped by category.
#[derive(Args, Debug)]
#[command(about = "List installed modules grouped by category", long_about = "List installed modules grouped by category.\n\nEXAMPLES:\n  mclaw tools\n  mclaw tools --json\n  mclaw tools --category data")]
pub struct ToolsArgs {
    /// Output machine-readable JSON (includes full client_manifest).
    #[arg(long)]
    json: bool,

    /// Filter by category id (e.g. "data", "content").
    #[arg(long, value_name = "CAT")]
    category: Option<String>,
}

pub async fn tools(args: ToolsArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let client = Client::new(&config)?;
    let installed = client.installed_modules().await.map_err(map_err)?;

    // Optional filter by category id.
    let modules: Vec<_> = installed
        .modules
        .into_iter()
        .filter(|m| match &args.category {
            Some(c) => m.category == *c,
            None => true,
        })
        .collect();

    if modules.is_empty() {
        if let Some(c) = &args.category {
            eprintln!("No installed modules in category '{}'.", c);
            eprintln!("Run `mclaw marketplace` to see what's available.");
        } else {
            eprintln!("No modules installed yet.");
            eprintln!("Run `mclaw marketplace` to see what's available, or");
            eprintln!("  mclaw marketplace install <module_id> to add one.");
        }
        // Still print empty JSON for scripting.
        if args.json {
            println!("{{\"modules\": [], \"grouped_by_category\": {{}}}}");
        }
        return Ok(0);
    }

    if args.json {
        // Re-wrap as {modules, grouped_by_category} so the JSON shape matches
        // the raw MAIC response (minus installed_modules's missing keys we omit).
        let grouped = installed.grouped_by_category;
        let out = serde_json::json!({
            "modules": modules,
            "grouped_by_category": grouped,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        // Group by category for the TTY view. We re-group here so the filter
        // is respected (don't use the server's grouped_by_category which
        // includes unfiltered modules).
        let mut by_cat: BTreeMap<String, Vec<&client::InstalledModule>> = BTreeMap::new();
        for m in &modules {
            by_cat.entry(m.category.clone()).or_default().push(m);
        }
        for (cat, mods) in &by_cat {
            println!("\n{} ({}):", cat, mods.len());
            for m in mods {
                let tools: Vec<String> = m
                    .client_manifest
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                println!(
                    "  {} {} {} — tools: {}",
                    m.icon,
                    m.module_id,
                    if tools.is_empty() {
                        String::new()
                    } else {
                        format!("({})", tools.join(", "))
                    },
                    m.name
                );
            }
        }
        let total = modules.len();
        eprintln!("\n{} installed module{}", total, if total == 1 { "" } else { "s" });
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────
// `mclaw marketplace`
// ─────────────────────────────────────────────────────────────────────

/// `mclaw marketplace [category] | install <id> | uninstall <id>`
///
/// Browse the marketplace. The first form (`marketplace [category]`)
/// lists modules; the second/third are explicit sub-commands for
/// install / uninstall. We model them as sub-commands so `mclaw
/// marketplace install firecrawl` binds the right way, instead of
/// clap matching 'install' to the optional category positional.
#[derive(Args, Debug)]
#[command(about = "Browse the module marketplace (list / install / uninstall)", long_about = "Browse the module marketplace.\n\nEXAMPLES:\n  mclaw marketplace list\n  mclaw marketplace list data\n  mclaw marketplace install firecrawl\n  mclaw marketplace install data-analyzer --json\n  mclaw marketplace uninstall firecrawl")]
pub struct MarketplaceArgs {
    /// Action: `list` (default), `install <module_id>`, `uninstall <module_id>`.
    #[command(subcommand)]
    action: MarketplaceAction,
}

#[derive(Subcommand, Debug)]
enum MarketplaceAction {
    /// List marketplace modules. Optional `category` filter.
    List {
        /// Filter by category id (e.g. "data", "content").
        category: Option<String>,

        /// Output machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install a module into your `Your Tools` list.
    Install {
        /// Module id (e.g. "firecrawl").
        module_id: String,
        /// Output machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Uninstall a module (removes from your list; doesn't delete the catalog).
    Uninstall {
        /// Module id (e.g. "firecrawl").
        module_id: String,
        /// Output machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn marketplace(args: MarketplaceArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let client = Client::new(&config)?;

    match args.action {
        MarketplaceAction::List { category, json } => {
            marketplace_list(&client, &category, json).await
        }
        MarketplaceAction::Install { module_id, json } => {
            eprintln!("Installing module '{}'...", module_id);
            let resp = client.install_module(&module_id).await.map_err(map_err)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                eprintln!(
                    "Installed '{}' (manifest has {} top-level keys).",
                    module_id,
                    resp.client_manifest.as_object().map(|o| o.len()).unwrap_or(0)
                );
                eprintln!("Run `mclaw tools` to see it in your list.");
            }
            Ok(0)
        }
        MarketplaceAction::Uninstall { module_id, json } => {
            eprintln!("Uninstalling module '{}'...", module_id);
            let resp = client.uninstall_module(&module_id).await.map_err(map_err)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                eprintln!("Uninstalled '{}'.", module_id);
            }
            Ok(0)
        }
    }
}

async fn marketplace_list(client: &Client, category: &Option<String>, json: bool) -> Result<i32> {
    let modules = client.marketplace(category.as_deref()).await.map_err(map_err)?;

    if modules.is_empty() {
        if let Some(c) = category {
            eprintln!("No modules in category '{}'.", c);
        } else {
            eprintln!("No modules in the marketplace.");
        }
        if json {
            println!("[]");
        }
        return Ok(0);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&modules)?);
    } else {
        // Group by category for the TTY view.
        let mut by_cat: BTreeMap<String, Vec<&client::ModuleSummary>> = BTreeMap::new();
        for m in &modules {
            by_cat.entry(m.category.clone()).or_default().push(m);
        }
        for (cat, mods) in &by_cat {
            println!("\n{} ({}):", cat, mods.len());
            for m in mods {
                let tier_badge = match m.tier_required.as_str() {
                    "free" => " ",
                    _ => "★",
                };
                println!(
                    "  {} {} {:<22} — {}  [{} · {} installs{}]",
                    tier_badge,
                    m.icon,
                    m.module_id,
                    m.name,
                    m.tier_required,
                    m.install_count,
                    if m.is_first_party { " · 1st-party" } else { "" }
                );
            }
        }
        let total = modules.len();
        eprintln!(
            "\n{} module{} in marketplace.",
            total,
            if total == 1 { "" } else { "s" }
        );
        eprintln!("Install with: mclaw marketplace install <module_id>");
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────
// Shared error mapper
// ─────────────────────────────────────────────────────────────────────

fn map_err(e: client::Error) -> anyhow::Error {
    match e {
        client::Error::NoAuth => {
            anyhow::anyhow!("not logged in: run `mclaw login` first (or set MAIC_JWT)")
        }
        client::Error::AuthFailed(msg) => {
            anyhow::anyhow!("authentication failed: {}\nrun `mclaw login` to re-authenticate", msg)
        }
        client::Error::RateLimited(d) => anyhow::anyhow!("rate limited; try again in {:?}", d),
        other => anyhow::anyhow!("{}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_string_value() {
        let (k, v) = parse_kv("url=https://example.com").unwrap();
        assert_eq!(k, "url");
        assert_eq!(v, Value::String("https://example.com".to_string()));
    }

    #[test]
    fn parse_kv_json_literals() {
        assert_eq!(parse_kv("count=5").unwrap().1, Value::from(5));
        assert_eq!(parse_kv("enabled=true").unwrap().1, Value::Bool(true));
        assert_eq!(parse_kv("tags=[\"a\",\"b\"]").unwrap().1,
                   Value::Array(vec![Value::from("a"), Value::from("b")]));
    }

    #[test]
    fn parse_kv_rejects_missing_equals() {
        assert!(parse_kv("no_equals_sign").is_err());
    }

    #[test]
    fn parse_kv_rejects_empty_key() {
        assert!(parse_kv("=value").is_err());
    }

    #[test]
    fn parse_module_tool_shorthand() {
        let args = RunArgs {
            positional: vec!["firecrawl.scrape".to_string()],
            tool: None,
            inputs: vec![],
            json: false,
        };
        let (m, t) = parse_module_tool(&args).unwrap();
        assert_eq!(m, "firecrawl");
        assert_eq!(t, "scrape");
    }

    #[test]
    fn parse_module_tool_shorthand_with_override() {
        let args = RunArgs {
            positional: vec!["firecrawl.scrape".to_string()],
            tool: Some("deep_scrape".to_string()),
            inputs: vec![],
            json: false,
        };
        let (m, t) = parse_module_tool(&args).unwrap();
        assert_eq!(m, "firecrawl");
        // Explicit --tool wins over the dot-shorthand.
        assert_eq!(t, "deep_scrape");
    }

    #[test]
    fn parse_module_tool_two_positional() {
        let args = RunArgs {
            positional: vec!["firecrawl".to_string(), "scrape".to_string()],
            tool: None,
            inputs: vec![],
            json: false,
        };
        let (m, t) = parse_module_tool(&args).unwrap();
        assert_eq!(m, "firecrawl");
        assert_eq!(t, "scrape");
    }

    #[test]
    fn parse_module_tool_with_flag() {
        let args = RunArgs {
            positional: vec!["firecrawl".to_string()],
            tool: Some("scrape".to_string()),
            inputs: vec![],
            json: false,
        };
        let (m, t) = parse_module_tool(&args).unwrap();
        assert_eq!(m, "firecrawl");
        assert_eq!(t, "scrape");
    }

    #[test]
    fn parse_module_tool_rejects_ambiguous() {
        let args = RunArgs {
            positional: vec!["firecrawl".to_string()],
            tool: None,
            inputs: vec![],
            json: false,
        };
        assert!(parse_module_tool(&args).is_err());
    }

    #[test]
    fn parse_module_tool_rejects_empty_shorthand() {
        let args = RunArgs {
            positional: vec![".scrape".to_string()],
            tool: None,
            inputs: vec![],
            json: false,
        };
        assert!(parse_module_tool(&args).is_err());
    }
}
