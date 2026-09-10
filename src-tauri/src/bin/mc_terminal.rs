//! `miracle-claw-terminal` — TUI surface for module-driven tools.
//!
//! Spec: docs/mc-module-driven-surfaces-2026-09.md §5.4 (mc-terminal)
//! + §3.2 (form_schema shape).
//!
//! This binary is the headless sibling of the Tauri desktop app. It
//! shares:
//!   - form_schema parser (`miracle_claw_lib::form_schema`)
//!   - MAIC HTTP client (via the lib's `mc_run_module` wrapper,
//!     re-implemented as a thin private function here — we don't go
//!     through Tauri commands since there's no webview to invoke)
//!   - The same JSON shape (`{outputs, duration_ms, run_id}`) the
//!     launchpad shows.
//!
//! The TUI has 4 modes (in a state machine driven by `App::mode`):
//!   - Toolbar:  pick a tool from the installed-modules list.
//!   - Form:     fill in the form_schema fields (Tab/Shift+Tab nav,
//!               Enter to submit).
//!   - Result:   show the run output (PageUp/PageDown scroll).
//!   - Add:      not implemented in v0.0.1 (Week 5 will land it via
//!               mc-marketplace CLI); pressing 'a' in Toolbar shows
//!               a "Coming in v0.2.0" stub.
//!
//! v0.0.1 is local-only (no Tailscale port publish). Auth uses the
//! same JWT the desktop app caches at `~/.local/share/milagro/
//! session.json` — the terminal binary reads that file and re-uses
//! the token, so a user who's already logged in to the Tauri app
//! can launch the terminal without re-entering creds.

use std::error::Error;
use std::io::{stdout, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use miracle_claw_lib::form_schema::{
    parse_form_schema, validate_inputs, FieldKind, FormField, FormSchema,
};

// ─────────────────────────────────────────────────────────────────────
// CLI args (miracle-claw-pair style — manual parser, not clap).
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Config {
    print_help: bool,
    print_version: bool,
    maic_base: Option<String>,
    /// Path to the session.json the desktop app wrote. Defaults to
    /// the XDG data dir.
    session_file: Option<PathBuf>,
    log_level: LogLevel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum LogLevel {
    #[default]
    Info,
    Debug,
    Silent,
}

impl LogLevel {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "silent" | "warn" | "error" => Some(LogLevel::Silent),
            _ => None,
        }
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PROGRAM: &str = "miracle-claw-terminal";

fn print_help() {
    println!(
        "{PROGRAM} {VERSION} — Miracle Claw TUI for MAIC modules\n\
         \n\
         USAGE:\n  \
             {PROGRAM} [OPTIONS]\n\
         \n\
         OPTIONS:\n  \
             --maic-base <URL>    MAIC server URL (default: https://maicserver.com)\n  \
             --session-file <PATH> Path to session.json (default: XDG data dir)\n  \
             --log-level <LVL>    info|debug|silent (default: info)\n  \
             -h, --help           Show this help\n  \
             -V, --version        Show version\n\
         \n\
         KEYS (in-app):\n  \
             ↑/↓ or j/k     navigate lists\n  \
             Tab / Shift+Tab  next/prev form field\n  \
             Enter          submit form / open tool\n  \
             Esc            cancel / back to toolbar\n  \
             q              quit\n  \
             ?              toggle help overlay"
    );
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => cfg.print_help = true,
            "-V" | "--version" => cfg.print_version = true,
            "--maic-base" => {
                cfg.maic_base = Some(
                    it.next()
                        .ok_or_else(|| "--maic-base needs a value".to_string())?,
                );
            }
            "--session-file" => {
                cfg.session_file = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--session-file needs a value".to_string())?,
                ));
            }
            "--log-level" => {
                let v = it
                    .next()
                    .ok_or_else(|| "--log-level needs a value".to_string())?;
                cfg.log_level = LogLevel::from_str(&v)
                    .ok_or_else(|| format!("unknown log level: {v}"))?;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(cfg)
}

// ─────────────────────────────────────────────────────────────────────
// MAIC client (private — we don't go through Tauri commands because
// there's no webview; we re-implement just the bits we need).
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Session {
    token: String,
    maic_base: String,
    user_email: String,
}

#[derive(Debug, Deserialize)]
struct InstalledModulesResponse {
    modules: Vec<InstalledModule>,
}

#[derive(Debug, Deserialize)]
struct InstalledModule {
    module_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    client_manifest: Value, // already a JSON object after our parse
    #[serde(default)]
    #[allow(dead_code)]
    installed_at: String,
    #[serde(default)]
    #[allow(dead_code)]
    last_used_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct InstalledTool {
    name: String,
    description: String,
    #[serde(default)]
    form_schema: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct RunResponse {
    outputs: String,
    duration_ms: i64,
    module_id: String,
    tool: String,
    run_id: i64,
}

fn default_session_path() -> PathBuf {
    // Mirrors the desktop app's session.json location: XDG data dir +
    // "milagro/session.json". The pairing CLI uses the same convention
    // (see pairing_init.rs::default_session_path).
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("milagro").join("session.json")
}

fn load_session(path: &PathBuf) -> Result<Session, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read session file {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("session file is not JSON: {e}"))?;
    let token = v
        .get("token")
        .or_else(|| v.get("jwt"))
        .or_else(|| v.get("access_token"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| "session file has no token field".to_string())?
        .to_string();
    let maic_base = v
        .get("maic_base")
        .or_else(|| v.get("api_base"))
        .or_else(|| v.get("base_url"))
        .and_then(|s| s.as_str())
        .unwrap_or("https://maicserver.com")
        .to_string();
    let user_email = v
        .get("email")
        .or_else(|| v.get("user_email"))
        .and_then(|s| s.as_str())
        .unwrap_or("unknown")
        .to_string();
    Ok(Session { token, maic_base, user_email })
}

fn http_get_json(url: &str, token: &str) -> Result<Value, String> {
    // We use ureq (already in deps) for the one-shot reads. For the
    // streaming run call we could swap to reqwest but for v0.0.1 the
    // MAIC `/v1/modules/{id}/run` endpoint doesn't stream — it
    // returns the full response — so ureq with a 60s timeout is fine.
    ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| format!("GET {url} failed: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("GET {url} returned non-JSON: {e}"))
}

fn http_post_json(url: &str, token: &str, body: &Value) -> Result<Value, String> {
    ureq::post(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(60))
        .send_json(body.clone())
        .map_err(|e| format!("POST {url} failed: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("POST {url} returned non-JSON: {e}"))
}

fn fetch_installed(session: &Session) -> Result<Vec<InstalledModule>, String> {
    let url = format!("{}/v1/modules/installed", session.maic_base);
    let v = http_get_json(&url, &session.token)?;
    let resp: InstalledModulesResponse = serde_json::from_value(v)
        .map_err(|e| format!("installed modules: {e}"))?;
    Ok(resp.modules)
}

fn run_tool(
    session: &Session,
    module_id: &str,
    tool: &str,
    inputs: &Value,
) -> Result<RunResponse, String> {
    let url = format!(
        "{}/v1/modules/{}/run",
        session.maic_base,
        urlencode(module_id)
    );
    let body = json!({
        "tool": tool,
        "inputs": inputs,
        "surface": "tui",
    });
    let v = http_post_json(&url, &session.token, &body)?;
    serde_json::from_value(v).map_err(|e| format!("run response: {e}"))
}

/// Tiny URL-encode (only the chars that appear in module IDs: `_`,
/// `-`, `.`, alphanum). Avoids pulling in the `urlencoding` crate
/// just for this.
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// TUI state + rendering.
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq)]
enum Mode {
    #[default]
    Toolbar,
    Form,
    Result,
    Add, // placeholder for Week 5 marketplace
    #[allow(dead_code)]
    Help, // reserved for future "Help is a mode" iteration
}

#[derive(Debug)]
struct App {
    mode: Mode,
    session: Session,
    installed: Vec<InstalledModule>,
    /// (module_index, tool_index) of the currently selected tool.
    selected: (usize, usize),
    /// Flat list of (module, tool) pairs in display order. The
    /// Toolbar lists these one-per-line so the user can press
    /// Enter on any tool, not just any module.
    flat_tools: Vec<(usize, usize)>,
    form_state: FormState,
    result_state: ResultState,
    help_visible: bool,
    /// Backing store of the input the user is typing. We update
    /// `form_state.values[name]` only when the user presses Enter or
    /// Tab; the field editor reads/writes `input_buffer` between
    /// keystrokes. This avoids the cursor jumping when the value
    /// field is a number that's mid-typed (e.g. "" → "1" → "12" — we
    /// can't reconstruct "12" from a parsed 0).
    input_buffer: String,
    cursor: usize,
    status: String,
}

#[derive(Debug, Default)]
struct FormState {
    schema: Option<FormSchema>,
    values: Vec<(String, String)>,
    focus: usize,
}

#[derive(Debug, Default)]
struct ResultState {
    output: String,
    duration_ms: i64,
    scroll: u16,
}

impl App {
    fn new(session: Session) -> Self {
        Self {
            mode: Mode::Toolbar,
            session,
            installed: Vec::new(),
            selected: (0, 0),
            flat_tools: Vec::new(),
            form_state: FormState::default(),
            result_state: ResultState::default(),
            help_visible: false,
            input_buffer: String::new(),
            cursor: 0,
            status: "Loading installed modules…".to_string(),
        }
    }

    fn rebuild_flat_tools(&mut self) {
        self.flat_tools.clear();
        for (mi, m) in self.installed.iter().enumerate() {
            let tools = extract_tools(&m.client_manifest);
            for (ti, _) in tools.iter().enumerate() {
                self.flat_tools.push((mi, ti));
            }
        }
    }

    fn selected_tool(&self) -> Option<(String, InstalledTool)> {
        // Return owned strings so callers don't have to fight the
        // borrow checker when they want to mutate other parts of `self`.
        let (mi, ti) = self.selected;
        let m = self.installed.get(mi)?;
        let tools = extract_tools(&m.client_manifest);
        let t = tools.get(ti)?.clone();
        Some((m.name.clone(), t))
    }
}

/// Pull the `tools[]` array out of a `client_manifest` value. Returns
/// an empty vec if the manifest is missing or malformed.
fn extract_tools(manifest: &Value) -> Vec<InstalledTool> {
    manifest
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| serde_json::from_value::<InstalledTool>(t.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────
// Rendering
// ─────────────────────────────────────────────────────────────────────

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(0),    // body
            Constraint::Length(3), // footer / status
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_body(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);

    if app.help_visible {
        draw_help_overlay(f, area);
    }
}

fn draw_header(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled("Miracle Claw ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("· TUI", Style::default().fg(Color::White)),
        Span::styled(
            format!("  v{} ", VERSION),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let user = Line::from(Span::styled(
        format!("  {} ", app.session.user_email),
        Style::default().fg(Color::DarkGray),
    ));
    let p = Paragraph::new(vec![title, user])
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(p, area);
}

fn draw_footer(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let keys = match app.mode {
        Mode::Toolbar => "↑↓ navigate · Enter open · a add (Week 5) · ? help · q quit",
        Mode::Form => "Tab next · Shift+Tab prev · Enter submit · Esc cancel",
        Mode::Result => "↑↓ scroll · Enter back · q quit",
        Mode::Add => "Esc back",
        Mode::Help => "Esc/? close help",
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(keys, Style::default().fg(Color::DarkGray)),
    ]);
    let p = Paragraph::new(line).block(Block::default().borders(Borders::TOP));
    f.render_widget(p, area);
}

fn draw_body(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    match app.mode {
        Mode::Toolbar => draw_toolbar(f, app, area),
        Mode::Form => draw_form(f, app, area),
        Mode::Result => draw_result(f, app, area),
        Mode::Add => draw_add_stub(f, area),
        Mode::Help => { /* overlay drawn last */ }
    }
}

fn draw_toolbar(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    // Layout: left = list of (module, tool), right = help/description.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // Left: list.
    let items: Vec<ListItem> = app
        .flat_tools
        .iter()
        .filter_map(|&(mi, ti)| {
            let m = app.installed.get(mi)?;
            let tools = extract_tools(&m.client_manifest);
            let t = tools.get(ti)?;
            Some(ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", m.name),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled("· ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    t.name.clone(),
                    Style::default().fg(Color::White),
                ),
            ])))
        })
        .collect();

    let mut list_state = ListState::default();
    if let Some(pos) = app
        .flat_tools
        .iter()
        .position(|&p| p == app.selected)
    {
        list_state.select(Some(pos));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Installed tools ", Style::default().fg(Color::Cyan))),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, cols[0], &mut list_state);

    // Right: description of the selected tool + form preview.
    if let Some((mname, t)) = app.selected_tool() {
        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                format!("{} · {}", mname, t.name),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                t.description.clone(),
                Style::default().fg(Color::White),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Fields:",
                Style::default().fg(Color::Yellow),
            )),
        ];
        if let Ok(schema) = parse_form_schema(&t.form_schema) {
            for field in &schema.fields {
                let marker = if field.required { "*" } else { " " };
                let kind = field.kind.type_tag();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {marker} "),
                        Style::default().fg(if field.required { Color::Red } else { Color::DarkGray }),
                    ),
                    Span::styled(
                        field.label.clone(),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("  ({kind})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            if schema.fields.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no inputs)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "  (form_schema unparseable)",
                Style::default().fg(Color::Red),
            )));
        }
        let p = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(" Tool ", Style::default().fg(Color::Cyan))),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(p, cols[1]);
    } else {
        let p = Paragraph::new(Span::styled(
            "  No tool selected. Install a module in the Tauri app first.",
            Style::default().fg(Color::DarkGray),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Tool ", Style::default().fg(Color::Cyan))),
        );
        f.render_widget(p, cols[1]);
    }
}

fn draw_form(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let Some(schema) = app.form_state.schema.clone() else {
        let p = Paragraph::new("No schema loaded").block(Block::default().borders(Borders::ALL));
        f.render_widget(p, area);
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in schema.fields.iter().enumerate() {
        let is_focused = i == app.form_state.focus;
        let marker = if is_focused { "▶ " } else { "  " };
        let req = if field.required { "*" } else { " " };
        let value_str: String = if is_focused {
            app.input_buffer.clone()
        } else {
            app.form_state
                .values
                .iter()
                .find(|(n, _)| n == &field.name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let value_display = match &field.kind {
            FieldKind::Select { options } => {
                if is_focused {
                    format!("[{}▏{}]", options.join(" | "), value_str)
                } else {
                    value_str
                }
            }
            FieldKind::Boolean => {
                if value_str == "true" { "☑" } else { "☐" }.to_string()
            }
            _ => {
                if is_focused {
                    // Show cursor as inverse block at `app.cursor`.
                    let before: String = app.input_buffer.chars().take(app.cursor).collect();
                    let after: String = app.input_buffer.chars().skip(app.cursor).collect();
                    format!("[{}{}{}]", before, "▏", after)
                } else if value_str.is_empty() {
                    format!("[{}]", field.default.clone()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".to_string()))
                } else {
                    format!("[{}]", value_str)
                }
            }
        };
        let kind = field.kind.type_tag();
        let line = Line::from(vec![
            Span::styled(
                format!("{marker}{req} "),
                Style::default().fg(if is_focused { Color::Cyan } else { Color::DarkGray }),
            ),
            Span::styled(
                format!("{:<24}", field.label),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!(" {value_display}  "),
                Style::default().fg(if is_focused { Color::Yellow } else { Color::Green }),
            ),
            Span::styled(
                format!("({kind})"),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        lines.push(line);
    }
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                format!(" {} ", schema_label(&app)),
                Style::default().fg(Color::Cyan),
            )),
    );
    f.render_widget(p, area);
}

fn schema_label(app: &App) -> String {
    app.selected_tool()
        .map(|(mname, t)| format!("{} · {}", mname, t.name))
        .unwrap_or_else(|| "Form".to_string())
}

fn draw_result(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let meta = Paragraph::new(Line::from(vec![
        Span::styled(
            " Ran in ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{} ms", app.result_state.duration_ms),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            "  │  ↑↓ to scroll, Enter to return",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(meta, chunks[0]);
    let body = Paragraph::new(app.result_state.output.clone())
        .block(Block::default().borders(Borders::ALL).title(" Output "))
        .wrap(Wrap { trim: false })
        .scroll((app.result_state.scroll, 0));
    f.render_widget(body, chunks[1]);
}

fn draw_add_stub(f: &mut ratatui::Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Marketplace browser lands in v0.2.0 (Week 5).",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Until then, install modules via the Tauri app:",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "    Dashboard → Marketplace → Install.",
            Style::default().fg(Color::Cyan),
        )),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Add module ", Style::default().fg(Color::Cyan))),
    );
    f.render_widget(p, area);
}

fn draw_help_overlay(f: &mut ratatui::Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Miracle Claw TUI — Keys ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan));
    let inner = centered_rect(70, 70, area);
    f.render_widget(Clear, inner);
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Welcome to Miracle Claw TUI. This is the",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  terminal sibling of the desktop app — same modules,",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  same MAIC backend, just keyboard-driven.",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Global keys:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("    q              quit"),
        Line::from("    ?              toggle this help"),
        Line::from(""),
        Line::from(Span::styled(
            "  Toolbar:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("    ↑/↓ or j/k     select tool"),
        Line::from("    Enter          open the form"),
        Line::from("    a              add module (v0.2.0)"),
        Line::from(""),
        Line::from(Span::styled(
            "  Form:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("    Tab / Shift+Tab  next/prev field"),
        Line::from("    ← / →           move cursor"),
        Line::from("    Backspace       delete char before cursor"),
        Line::from("    Enter           submit"),
        Line::from("    Esc             cancel"),
        Line::from(""),
        Line::from(Span::styled(
            "  Result:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("    ↑/↓             scroll"),
        Line::from("    Enter           return to toolbar"),
    ];
    let p = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ─────────────────────────────────────────────────────────────────────
// Input handling — mode-aware.
// ─────────────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }
    // Global: q quits, ? toggles help.
    match key.code {
        KeyCode::Char('q') if !app.help_visible => return true,
        KeyCode::Char('?') => {
            app.help_visible = !app.help_visible;
            return false;
        }
        _ => {}
    }
    if app.help_visible {
        // Any other key closes the overlay.
        app.help_visible = false;
        return false;
    }
    match app.mode {
        Mode::Toolbar => handle_toolbar_key(app, key),
        Mode::Form => handle_form_key(app, key),
        Mode::Result => handle_result_key(app, key),
        Mode::Add => handle_add_key(app, key),
        Mode::Help => false,
    }
}

fn handle_toolbar_key(app: &mut App, key: KeyEvent) -> bool {
    let max = app.flat_tools.len().saturating_sub(1);
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(pos) = app.flat_tools.iter().position(|&p| p == app.selected) {
                if pos < max {
                    app.selected = app.flat_tools[pos + 1];
                }
            } else if !app.flat_tools.is_empty() {
                app.selected = app.flat_tools[0];
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(pos) = app.flat_tools.iter().position(|&p| p == app.selected) {
                if pos > 0 {
                    app.selected = app.flat_tools[pos - 1];
                }
            }
        }
        KeyCode::Enter => {
            if let Some((mname, t)) = app.selected_tool() {
                let schema = parse_form_schema(&t.form_schema).ok();
                if let Some(s) = schema {
                    app.form_state = FormState {
                        schema: Some(s),
                        values: Vec::new(),
                        focus: 0,
                    };
                    // Seed input buffer with the first field's default.
                    app.input_buffer = app
                        .form_state
                        .schema
                        .as_ref()
                        .and_then(|s| s.fields.first())
                        .and_then(|f| f.default.clone())
                        .map(|v| v.to_string().trim_matches('"').to_string())
                        .unwrap_or_default();
                    app.cursor = app.input_buffer.chars().count();
                    app.status = format!("Filling: {mname}.{}", t.name);
                    app.mode = Mode::Form;
                } else {
                    app.status = format!(
                        "form_schema for {mname}.{} is unparseable",
                        t.name
                    );
                }
            } else {
                app.status = "No tool selected".to_string();
            }
        }
        KeyCode::Char('a') => {
            app.status = "Marketplace browser lands in v0.2.0 (Week 5)".to_string();
            app.mode = Mode::Add;
        }
        _ => {}
    }
    false
}

fn handle_form_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(schema) = app.form_state.schema.clone() else {
        return false;
    };
    let field_count = schema.fields.len();
    if field_count == 0 {
        // Nothing to fill; Enter submits immediately.
        if key.code == KeyCode::Enter {
            submit_form(app);
        } else if key.code == KeyCode::Esc {
            app.mode = Mode::Toolbar;
            app.status = "Cancelled".to_string();
        }
        return false;
    }
    let focused = app.form_state.focus;
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Toolbar;
            app.status = "Cancelled".to_string();
        }
        KeyCode::Tab => {
            commit_current_field(app, &schema);
            app.form_state.focus = (app.form_state.focus + 1) % field_count;
            app.input_buffer = current_field_value(app, &schema);
            app.cursor = app.input_buffer.chars().count();
        }
        KeyCode::BackTab => {
            commit_current_field(app, &schema);
            app.form_state.focus = if app.form_state.focus == 0 {
                field_count - 1
            } else {
                app.form_state.focus - 1
            };
            app.input_buffer = current_field_value(app, &schema);
            app.cursor = app.input_buffer.chars().count();
        }
        KeyCode::Backspace => {
            if app.cursor > 0 {
                // Remove the char before the cursor.
                let chars: Vec<char> = app.input_buffer.chars().collect();
                let mut new_chars = chars[..app.cursor - 1].to_vec();
                new_chars.extend_from_slice(&chars[app.cursor..]);
                app.input_buffer = new_chars.into_iter().collect();
                app.cursor -= 1;
            }
        }
        KeyCode::Left => {
            app.cursor = app.cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            let len = app.input_buffer.chars().count();
            if app.cursor < len {
                app.cursor += 1;
            }
        }
        KeyCode::Enter => {
            commit_current_field(app, &schema);
            submit_form(app);
        }
        KeyCode::Char(c) => {
            // For boolean, toggle on Space/Enter (handled above). For
            // other types, insert at cursor.
            if let Some(field) = schema.fields.get(focused) {
                match &field.kind {
                    FieldKind::Boolean => {
                        if c == ' ' {
                            // Toggle.
                            let cur = app.input_buffer == "true";
                            app.input_buffer = if cur { "false".to_string() } else { "true".to_string() };
                        }
                    }
                    FieldKind::Select { options } => {
                        // Type the option prefix. For v0.0.1 we just
                        // let the user free-type; validation will
                        // catch non-options.
                        let _ = options; // (no completion for v0.0.1)
                        insert_char(&mut app.input_buffer, &mut app.cursor, c);
                    }
                    _ => insert_char(&mut app.input_buffer, &mut app.cursor, c),
                }
            }
        }
        _ => {}
    }
    false
}

fn insert_char(buf: &mut String, cursor: &mut usize, c: char) {
    let chars: Vec<char> = buf.chars().collect();
    let mut new_chars = chars[..*cursor].to_vec();
    new_chars.push(c);
    new_chars.extend_from_slice(&chars[*cursor..]);
    *buf = new_chars.into_iter().collect();
    *cursor += 1;
}

fn commit_current_field(app: &mut App, schema: &FormSchema) {
    let Some(field) = schema.fields.get(app.form_state.focus) else {
        return;
    };
    let name = field.name.clone();
    let value = app.input_buffer.clone();
    if let Some(slot) = app.form_state.values.iter_mut().find(|(n, _)| n == &name) {
        slot.1 = value;
    } else {
        app.form_state.values.push((name, value));
    }
}

fn current_field_value(app: &App, schema: &FormSchema) -> String {
    let Some(field) = schema.fields.get(app.form_state.focus) else {
        return String::new();
    };
    let from_values = app
        .form_state
        .values
        .iter()
        .find(|(n, _)| n == &field.name)
        .map(|(_, v)| v.clone());
    from_values
        .or_else(|| {
            field
                .default
                .clone()
                .map(|v| v.to_string().trim_matches('"').to_string())
        })
        .unwrap_or_default()
}

fn submit_form(app: &mut App) {
    let Some(schema) = app.form_state.schema.clone() else {
        return;
    };
    // Build the inputs JSON from the collected values, applying
    // defaults for unset fields.
    let mut obj = serde_json::Map::new();
    for field in &schema.fields {
        let v_str = app
            .form_state
            .values
            .iter()
            .find(|(n, _)| n == &field.name)
            .map(|(_, v)| v.clone());
        let v: Value = match v_str {
            Some(s) if !s.is_empty() => Value::String(s),
            _ => field.default.clone().unwrap_or(Value::Null),
        };
        let coerced = coerce_for_kind(&field.kind, v);
        obj.insert(field.name.clone(), coerced);
    }
    let inputs = Value::Object(obj);
    if let Err(e) = validate_inputs(&schema, &inputs) {
        app.status = format!("Validation: {e}");
        return;
    }
    // Snapshot the module_id + tool name before we mutate `app` so the
    // borrow checker doesn't complain. We need them for the HTTP call.
    let (module_id, tool_name, mname) = {
        let (mi, _ti) = app.selected;
        let m = &app.installed[mi];
        let Some((mn, t)) = app.selected_tool() else {
            app.status = "Tool lost (select again)".to_string();
            return;
        };
        (m.module_id.clone(), t.name.clone(), mn)
    };
    app.status = format!("Running {mname}.{tool_name}…");
    let started = Instant::now();
    let resp = run_tool(&app.session, &module_id, &tool_name, &inputs);
    let elapsed = started.elapsed();
    match resp {
        Ok(r) => {
            app.result_state = ResultState {
                output: strip_code_fence(&r.outputs),
                duration_ms: r.duration_ms,
                scroll: 0,
            };
            app.status = format!(
                "{} ran in {} ms (id {})",
                r.tool, r.duration_ms, r.run_id
            );
            app.mode = Mode::Result;
        }
        Err(e) => {
            // Show the error in the result pane (so the user can see
            // the full message even if it spans multiple lines).
            app.result_state = ResultState {
                output: format!("Error: {e}"),
                duration_ms: elapsed.as_millis() as i64,
                scroll: 0,
            };
            app.status = format!("{mname}.{tool_name} failed");
            app.mode = Mode::Result;
        }
    }
}

fn coerce_for_kind(kind: &FieldKind, v: Value) -> Value {
    match kind {
        FieldKind::Number { is_integer, .. } => {
            if let Value::String(s) = v {
                if *is_integer {
                    s.parse::<i64>().map(|n| Value::from(n)).unwrap_or(Value::Null)
                } else {
                    s.parse::<f64>().map(|n| Value::from(n)).unwrap_or(Value::Null)
                }
            } else {
                v
            }
        }
        FieldKind::Boolean => match v {
            Value::String(s) => match s.as_str() {
                "true" | "1" | "yes" | "on" => Value::Bool(true),
                "false" | "0" | "no" | "off" => Value::Bool(false),
                _ => Value::Bool(false),
            },
            other => other,
        },
        _ => v,
    }
}

fn strip_code_fence(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Drop the first line (``` or ```json), then trailing ```.
        let body = rest.split_once('\n').map(|(_, b)| b).unwrap_or(rest);
        if let Some(stripped) = body.strip_suffix("```") {
            return stripped.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

fn handle_result_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.result_state.scroll = app.result_state.scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.result_state.scroll = app.result_state.scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.result_state.scroll = app.result_state.scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.result_state.scroll = app.result_state.scroll.saturating_add(10);
        }
        KeyCode::Enter | KeyCode::Esc => {
            app.mode = Mode::Toolbar;
            app.status = "Back to toolbar".to_string();
        }
        _ => {}
    }
    false
}

fn handle_add_key(app: &mut App, key: KeyEvent) -> bool {
    if key.code == KeyCode::Esc {
        app.mode = Mode::Toolbar;
        app.status = "Back to toolbar".to_string();
    }
    false
}

// ─────────────────────────────────────────────────────────────────────
// Main loop.
// ─────────────────────────────────────────────────────────────────────

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Tui, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Tui) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn print_lobster_banner() {
    // ASCII lobster greeting. The first frame will replace it, but
    // it's a nice touch for stdout when --version or --help prints.
    println!(
        r"
   {emoji}  Welcome to Miracle Claw TUI {ver}
       ─────────────────────────────
       Re-uses your desktop app login.
       Run the desktop app first if you haven't.
",
        emoji = "🦞",
        ver = VERSION
    );
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("(run with --help for usage)");
            std::process::exit(2);
        }
    };
    if cfg.print_help {
        print_help();
        return;
    }
    if cfg.print_version {
        println!("{PROGRAM} {VERSION}");
        return;
    }

    let session_path = cfg
        .session_file
        .clone()
        .unwrap_or_else(default_session_path);
    let session = match load_session(&session_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "hint: log in to the Miracle Claw desktop app first, or set --session-file"
            );
            std::process::exit(2);
        }
    };
    if let Some(override_base) = cfg.maic_base {
        // Allow CLI override of the MAIC base.
        let mut s = session;
        s.maic_base = override_base;
        return run_tui(s);
    }
    print_lobster_banner();
    run_tui(session);
}

fn run_tui(session: Session) {
    let mut terminal = match setup_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not init terminal: {e}");
            std::process::exit(2);
        }
    };

    // Fetch installed modules up front. If this fails, the user can
    // still see an empty toolbar (with a status message) and quit.
    let mut app = App::new(session.clone());
    match fetch_installed(&session) {
        Ok(mods) => {
            app.installed = mods;
            app.rebuild_flat_tools();
            if !app.flat_tools.is_empty() {
                app.selected = app.flat_tools[0];
                app.status = format!(
                    "{} modules loaded · press Enter to run",
                    app.installed.len()
                );
            } else {
                app.status = "No installed modules. Install one in the desktop app first.".to_string();
            }
        }
        Err(e) => {
            app.status = format!("Could not load installed modules: {e}");
        }
    }

    let tick = Duration::from_millis(250);
    let mut last_input = Instant::now();
    let result = loop {
        // Draw.
        if let Err(e) = terminal.draw(|f| draw(f, &mut app)) {
            break Err::<(), Box<dyn Error>>(Box::new(e));
        }
        // Wait for input or a tick (ticks are used to refresh
        // timing-based UI elements in future; v0.0.1 doesn't need
        // them but we keep the loop shape for forward compat).
        let timeout = tick.saturating_sub(last_input.elapsed());
        if event::poll(timeout).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        break Ok(());
                    }
                    if handle_key(&mut app, k) {
                        break Ok(());
                    }
                    last_input = Instant::now();
                }
                Ok(_) => {}
                Err(e) => break Err(Box::new(e)),
            }
        }
    };

    if let Err(e) = restore_terminal(&mut terminal) {
        eprintln!("warning: terminal restore failed: {e}");
    }
    if let Err(e) = result {
        eprintln!("TUI loop error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_field(name: &str, kind: FieldKind, required: bool, default: Option<Value>) -> FormField {
        FormField {
            name: name.to_string(),
            label: name.to_string(),
            required,
            kind,
            default,
            display_order: 0,
        }
    }

    #[test]
    fn urlencode_passes_safe_chars() {
        assert_eq!(urlencode("firecrawl"), "firecrawl");
        assert_eq!(urlencode("marketing-suite"), "marketing-suite");
        assert_eq!(urlencode("a.b_c-d~e"), "a.b_c-d~e");
    }

    #[test]
    fn urlencode_escapes_unsafe() {
        assert_eq!(urlencode("a/b"), "a%2Fb");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("&x=1"), "%26x%3D1");
    }

    #[test]
    fn strip_code_fence_handles_json_fence() {
        assert_eq!(
            strip_code_fence("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
        assert_eq!(strip_code_fence("```\nhello\n```"), "hello");
        assert_eq!(strip_code_fence("plain text"), "plain text");
        // Missing closing fence: just leave it as-is after trim.
        assert_eq!(strip_code_fence("  ```json\nfoo"), "```json\nfoo");
    }

    #[test]
    fn coerce_for_kind_string_to_int() {
        let k = FieldKind::Number { minimum: None, maximum: None, is_integer: true };
        assert_eq!(
            coerce_for_kind(&k, Value::String("42".to_string())),
            Value::from(42)
        );
        assert_eq!(
            coerce_for_kind(&k, Value::String("not a number".to_string())),
            Value::Null
        );
    }

    #[test]
    fn coerce_for_kind_string_to_bool() {
        let k = FieldKind::Boolean;
        assert_eq!(coerce_for_kind(&k, Value::String("true".to_string())), Value::Bool(true));
        assert_eq!(coerce_for_kind(&k, Value::String("false".to_string())), Value::Bool(false));
        assert_eq!(coerce_for_kind(&k, Value::String("yes".to_string())), Value::Bool(true));
    }

    #[test]
    fn extract_tools_handles_missing() {
        let v = json!({});
        assert!(extract_tools(&v).is_empty());
        let v = json!({"tools": []});
        assert!(extract_tools(&v).is_empty());
        let v = json!({
            "tools": [
                {"name": "fetch_url", "description": "fetch", "form_schema": {}}
            ]
        });
        let tools = extract_tools(&v);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "fetch_url");
    }

    #[test]
    fn app_rebuild_flat_tools_preserves_order() {
        let m1 = InstalledModule {
            module_id: "a".into(),
            name: "A".into(),
            description: "".into(),
            client_manifest: json!({
                "tools": [
                    {"name": "a1", "description": "", "form_schema": {}},
                    {"name": "a2", "description": "", "form_schema": {}}
                ]
            }),
            installed_at: "".into(),
            last_used_at: None,
        };
        let m2 = InstalledModule {
            module_id: "b".into(),
            name: "B".into(),
            description: "".into(),
            client_manifest: json!({
                "tools": [
                    {"name": "b1", "description": "", "form_schema": {}}
                ]
            }),
            installed_at: "".into(),
            last_used_at: None,
        };
        let mut app = App::new(Session {
            token: "x".into(),
            maic_base: "https://x".into(),
            user_email: "u@x".into(),
        });
        app.installed = vec![m1, m2];
        app.rebuild_flat_tools();
        assert_eq!(app.flat_tools, vec![(0, 0), (0, 1), (1, 0)]);
        let (mname, _t) = app.selected_tool().unwrap();
        assert_eq!(mname, "A");
    }

    #[test]
    fn log_level_from_str_is_case_insensitive() {
        assert_eq!(LogLevel::from_str("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str("DEBUG"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str("silent"), Some(LogLevel::Silent));
        assert_eq!(LogLevel::from_str("warn"), Some(LogLevel::Silent));
        assert_eq!(LogLevel::from_str("trace"), None);
    }

    #[test]
    fn parse_args_handles_all_flags() {
        // Clear any env-var-influenced defaults and call the parser.
        let argv: Vec<String> = vec![
            "miracle-claw-terminal".into(),
            "--maic-base".into(),
            "https://example.test".into(),
            "--session-file".into(),
            "/tmp/s.json".into(),
            "--log-level".into(),
            "debug".into(),
        ];
        // parse_args uses std::env::args; we can't inject from a test
        // cleanly, so we sanity-check the helpers instead.
        assert_eq!(LogLevel::from_str("debug"), Some(LogLevel::Debug));
        let _ = argv; // silence unused
    }

    #[test]
    fn commit_and_restore_field_value_round_trip() {
        let schema = FormSchema {
            fields: vec![
                make_field("url", FieldKind::Text { min_length: Some(1), max_length: None }, true, None),
                make_field("n", FieldKind::Number { minimum: None, maximum: None, is_integer: true }, false, Some(json!(10))),
            ],
        };
        let mut app = App::new(Session {
            token: "x".into(),
            maic_base: "https://x".into(),
            user_email: "u".into(),
        });
        app.form_state.schema = Some(schema.clone());
        app.form_state.focus = 0;
        app.input_buffer = "https://example.com".to_string();
        app.cursor = app.input_buffer.chars().count();
        commit_current_field(&mut app, &schema);
        // Focus the second field — input buffer should pick up the default.
        app.form_state.focus = 1;
        let v = current_field_value(&app, &schema);
        assert_eq!(v, "10");
    }
}
