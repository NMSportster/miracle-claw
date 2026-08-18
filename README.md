# Miracle Claw — ADeal Auto Repair's desktop chat client for MAIC
Created 2026-08-17 by David. Old MC code lives in ../old_mc_files/.

## What goes here
- Tauri Rust shell (wraps webview)
- `miracle-claw-launcher` sidecar (Rust, ~200 LoC, std-only) that owns the
  lookup of the bundled Node runtime + chat gateway bundle and execs it
- ADeal branding (icons, splash, colors)
- NSIS installer config

## What does NOT go here
- Chat logic. The bundled chat gateway does that.
- MAIC client wiring. The MAIC plugin does that.
- Tool call dispatch. The MAIC plugin does that.

## Architecture
```
Tauri.exe (miracle-claw)
  └─ sidecar: miracle-claw-launcher.exe --gateway-port 28789
       └─ node openclaw.mjs gateway --port 28789 --bind loopback --auth none
            └─ WebChat UI on http://127.0.0.1:28789/
                  └─ talks to MAIC via OpenAI-compat transport
                  └─ MAIC plugin (vendored from ~/.openclaw/extensions/maic/) injects
                      tool_execution: "client" automatically

First-run path (idempotent):
  1.  Ensure ~/.openclaw/extensions/maic/ has the bundled plugin files
      (SHA-256 manifest check; skip-on-match)
  2.  Ensure ~/.openclaw/openclaw.json has pluginRoots and plugins.roots
      pointing at the user's extensions dir (deep-merge, not overwrite)
  3.  Spawn the launcher sidecar
  4.  Poll TCP connect to 127.0.0.1:28789 until bound (15s cap)
  5.  Show webview (already pointing at localhost:28789/)
  6.  On RunEvent::ExitRequested, kill the launcher child
```

## Build

```bash
# Build everything (sidecar + main binary)
bash scripts/build-launcher-sidecar.sh
cd src-tauri && cargo build --bin miracle-claw

# Or both via Tauri workflow
cargo tauri build
```

## Key files

| File | Purpose |
|---|---|
| `src-tauri/src/lib.rs` | Tauri main; runs setup() on launch |
| `src-tauri/src/launcher.rs` | Sidecar; `miracle-claw-launcher` binary |
| `src-tauri/src/launcher_info.rs` | Shared constants (port, plugin filenames) |
| `src-tauri/capabilities/main.json` | Tauri permission allowlist (webview can spawn only the launcher with `--gateway-port N`) |
| `scripts/build-launcher-sidecar.sh` | Stage the sidecar binary for tauri-build |
| `STATUS.md` | What's done / what's not / lessons learned |
