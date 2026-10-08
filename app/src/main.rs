//! Desktop shell: a window around the shared UI, forwarding everything to the core.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dbd_core::{Core, Options};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;

struct AppCore(Core);

#[tauri::command]
async fn call(state: tauri::State<'_, AppCore>, cmd: String, args: Value) -> Result<Value, String> {
    state.0.call(&cmd, args).await.map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let core = tauri::async_runtime::block_on(Core::start(dbd_core::default_dir(), Options { port: None, discovery: true }))?;
            let mut events = core.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let ev = match events.recv().await {
                        Ok(ev) => ev,
                        Err(RecvError::Lagged(_)) => json!({ "type": "changed" }),
                        Err(RecvError::Closed) => break,
                    };
                    let _ = handle.emit("core", ev);
                }
            });
            app.manage(AppCore(core));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("failed to start DastBeDast");
}
