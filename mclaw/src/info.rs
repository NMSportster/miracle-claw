//! `mclaw whoami` — current user info
//!
//! `mclaw usage` and `mclaw models` are stubbed for now; the MAIC endpoints
//! they would call (quota tracking, model catalog) don't have stable shapes
//! we can rely on yet. They'll be added in rc55.27 once we decide on the
//! canonical wire formats with the MAIC team.

use anyhow::Result;
use clap::Args;

use crate::client;

#[derive(Args, Debug)]
pub struct WhoAmIArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn whoami(args: WhoAmIArgs) -> Result<i32> {
    let config = crate::config::Config::load()?;
    let c = client::Client::new(&config)?;
    let me = c.whoami().await.map_err(|e| match e {
        client::Error::NoAuth => anyhow::anyhow!("not logged in: run `mclaw login` first"),
        other => anyhow::anyhow!("{}", other),
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&me)?);
    } else {
        println!("Name:     {}", me.name);
        println!("Tier:     {}", me.tier);
        if let Some(plan) = me.plan_code {
            println!("Plan:     {}", plan);
        }
        if me.is_master {
            println!("Master:   yes");
        }
    }
    Ok(0)
}

#[derive(Args, Debug)]
pub struct UsageArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn usage(_args: UsageArgs) -> Result<i32> {
    eprintln!("`mclaw usage` is not yet implemented; coming in rc55.27.");
    eprintln!("(MAIC's /v1/dashboard/usage endpoint shape is still in flux.)");
    Ok(1)
}

#[derive(Args, Debug)]
pub struct ModelsArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn models(args: ModelsArgs) -> Result<i32> {
    // /v1/models does exist; let's just call it. The shape is OpenAI-compatible.
    let config = crate::config::Config::load()?;
    let c = client::Client::new(&config)?;
    let list = c.models().await.map_err(|e| match e {
        client::Error::NoAuth => anyhow::anyhow!("not logged in: run `mclaw login` first"),
        other => anyhow::anyhow!("{}", other),
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else {
        for m in list {
            println!("{}", m.id);
        }
    }
    Ok(0)
}
