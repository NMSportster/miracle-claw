//! mclaw — Miracle Claw CLI library
//!
//! Exposes the same modules the binary uses, so integration tests in
//! `tests/` can call into `ask::run`, `client::Client`, etc. without
//! re-implementing them. The `main.rs` binary is a thin wrapper that
//! parses CLI args and dispatches to the entry points here.
//!
//! ## Public API
//!
//! - [`client::Client`] — MAIC HTTP client
//! - [`auth`] — JWT keychain (with `MAIC_JWT` env var fallback)
//! - [`config::Config`] — base URL + config file
//! - [`ask::run`] — one-shot ask
//! - [`chat::chat`] — REPL
//! - [`login::login`] / [`login::logout`] — interactive auth
//! - [`info::whoami`] / [`info::models`] / [`info::usage`]
//!
//! See `docs/mc-headless-companion-2026-09.md` for the design doc.

pub mod ask;
pub mod auth;
pub mod banner;
pub mod chat;
pub mod client;
pub mod config;
pub mod info;
pub mod login;
pub mod modules;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
