// Miracle Claw — Tauri main entry
//
// Starts the OpenClaw gateway as a child process and opens the webview.
//
// Architecture (from /home/adeal/.openclaw/workspace/projects/miracle-claw/README.md):
//   Tauri.exe (Rust)
//     └─ child process: node openclaw gateway --port 28789
//     └─ webview: http://localhost:28789/
//          └─ OpenClaw WebChat UI (HTML + JS bundle from openclaw/dist/webchat/)
//          └─ talks to MAIC via OpenAI-compat transport
//          └─ MAIC plugin injects tool_execution: "client" automatically
//
// What this file does:
//   1. On launch, spawn `node openclaw gateway --port 28789` as a child process
//   2. Wait for the gateway to start (poll /v1/models)
//   3. Show the webview pointed at http://localhost:28789/
//   4. On exit, kill the gateway child process
//
// This is a minimal placeholder. Real implementation lands tomorrow.

fn main() {
    miracle_claw_lib::run()
}