# Miracle Claw v2 — Tauri wrapper around OpenClaw WebChat + MAIC plugin
Created 2026-08-17 by David. Old MC code lives in ../old_mc_files/.

## What goes here
- Tauri Rust shell (wraps webview)
- OpenClaw WebChat bundle (or reference to bundled node openclaw gateway)
- ADeal branding (icons, splash, colors)
- NSIS installer config

## What does NOT go here
- Chat logic. OpenClaw does that.
- MAIC client wiring. The MAIC plugin does that.
- Tool call dispatch. OpenClaw + MAIC plugin do that.

## Architecture
```
Tauri.exe (Rust)
  └─ child process: node openclaw gateway --port 28789
  └─ webview: http://localhost:28789/
       └─ OpenClaw WebChat UI (HTML + JS bundle from openclaw/dist/webchat/)
       └─ talks to MAIC via OpenAI-compat transport
       └─ MAIC plugin injects tool_execution: "client" automatically
```
