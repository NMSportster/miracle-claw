// Miracle Claw — Tauri lib
//
// Real implementation lands tomorrow. This stub exists so the project compiles
// when we run `cargo check` for the first time and to lock in the entry point.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|_app| {
            // TODO tomorrow: spawn node openclaw gateway as child process,
            // poll http://localhost:28789/v1/models until 200,
            // then return (webview already configured in tauri.conf.json).
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}