//! The shell around the shared UI, on desktop and on Android. Everything is forwarded to the core.

use std::sync::Arc;

use dbd_core::{Clipboard, Core, Options};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;

struct AppCore(Core);

#[tauri::command]
async fn call(state: tauri::State<'_, AppCore>, cmd: String, args: Value) -> Result<Value, String> {
    state.0.call(&cmd, args).await.map_err(|e| e.to_string())
}

/// Android gives the files a user picked, or shared from another app, as `content://` addresses,
/// which only the app's own process may open. Each one is opened here and handed to the core as
/// a stand-in path. `names` are the names the system knows them by, where it does.
#[tauri::command]
fn adopt(app: tauri::AppHandle, uris: Vec<String>, names: Option<Vec<String>>) -> Result<Vec<String>, String> {
    use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
    let names = names.unwrap_or_default();
    uris.into_iter()
        .enumerate()
        .map(|(i, uri)| {
            let mut opts = OpenOptions::new();
            opts.read(true);
            let path: FilePath = uri.parse().map_err(|_| uri.clone())?;
            let file = app.fs().open(path, opts).map_err(|e| e.to_string())?;
            let known = names.get(i).map(|n| n.trim()).filter(|n| !n.is_empty());
            let name = known.map(str::to_string).unwrap_or_else(|| picked_name(&file, &uri));
            Ok(dbd_core::adopt_file(file, &name))
        })
        .collect()
}

/// The name of a picked file. Shared storage shows the real path behind the handle;
/// otherwise the last part of the address has to do.
fn picked_name(file: &std::fs::File, uri: &str) -> String {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok();
        let shared = real.as_ref().filter(|p| p.starts_with("/storage") || p.starts_with("/mnt") || p.starts_with("/sdcard"));
        if let Some(name) = shared.and_then(|p| p.file_name()) {
            return name.to_string_lossy().into_owned();
        }
    }
    #[cfg(not(unix))]
    let _ = file;
    let last = uri.rsplit('/').next().unwrap_or_default();
    let decoded = percent_decode(last);
    let name = decoded.rsplit(['/', ':']).next().unwrap_or_default().trim();
    if name.contains('.') { name.to_string() } else { format!("file-{name}") }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where the core keeps its settings, and how it starts, on this platform.
fn start_options(app: &tauri::App) -> (std::path::PathBuf, Options) {
    #[cfg(mobile)]
    {
        let data = app.path().app_data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Received files should be where the phone's file manager shows them. An app may create
        // its own folder under Download; if this phone refuses, they stay inside the app.
        let public = std::path::PathBuf::from("/storage/emulated/0/Download/DastBeDast");
        let probe = public.join("Inbox").join(".probe");
        let writable = std::fs::create_dir_all(public.join("Inbox")).is_ok() && std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        let files_dir = if writable { public } else { data.join("files") };
        (data.join("core"), Options { port: None, discovery: true, files_dir: Some(files_dir) })
    }
    #[cfg(desktop)]
    {
        let _ = app;
        (dbd_core::default_dir(), Options { port: None, discovery: true, files_dir: None })
    }
}

/// The system clipboard, through the shell's plugin, which reaches it on desktops and phones alike.
struct ShellClipboard(tauri::AppHandle);

impl Clipboard for ShellClipboard {
    fn get(&self) -> Option<String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.0.clipboard().read_text().ok()
    }
    fn set(&self, text: &str) -> bool {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.0.clipboard().write_text(text.to_string()).is_ok()
    }
}

/// The laser pointer of the remote: a red dot in a see-through window above everything,
/// which the mouse passes through. It covers the screen the mouse pointer is on.
#[cfg(desktop)]
fn laser(app: &tauri::AppHandle, ev: &Value) {
    let on = ev["on"].as_bool().unwrap_or(false);
    let win = match app.get_webview_window("laser") {
        Some(w) => w,
        None if !on => return,
        None => match tauri::WebviewWindowBuilder::new(app, "laser", tauri::WebviewUrl::App("laser.html".into()))
            .title("DastBeDast")
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build()
        {
            Ok(w) => w,
            Err(e) => return eprintln!("laser window: {e}"),
        },
    };
    let shown = win.is_visible().unwrap_or(false);
    if on && !shown {
        let monitor = app
            .cursor_position()
            .ok()
            .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        if let Some(m) = monitor {
            let _ = win.set_position(*m.position());
            let _ = win.set_size(*m.size());
        }
        // Only a shown window can be made click-through: on Linux a hidden one has nothing to apply it to yet.
        let _ = win.show();
        let _ = win.set_ignore_cursor_events(true);
        let _ = win.set_always_on_top(true);
    } else if !on && shown {
        let _ = win.hide();
    }
    let _ = app.emit("laser", ev);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let (dir, opts) = start_options(app);
            let core = tauri::async_runtime::block_on(Core::start(dir, opts))?;
            core.set_clipboard(Arc::new(ShellClipboard(app.handle().clone())));
            let mut events = core.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let ev = match events.recv().await {
                        Ok(ev) => ev,
                        Err(RecvError::Lagged(_)) => json!({ "type": "changed" }),
                        Err(RecvError::Closed) => break,
                    };
                    if ev["type"] == "laser" {
                        #[cfg(desktop)]
                        laser(&handle, &ev);
                        continue;
                    }
                    let _ = handle.emit("core", ev);
                }
            });
            app.manage(AppCore(core));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call, adopt])
        .run(tauri::generate_context!())
        .expect("failed to start DastBeDast");
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_from_addresses() {
        let f = std::fs::File::open("Cargo.toml").unwrap();
        assert_eq!(super::picked_name(&f, "content://com.android.externalstorage.documents/document/primary%3ADownload%2Fmy%20plan.pdf"), "my plan.pdf");
        assert_eq!(super::picked_name(&f, "content://com.android.providers.media.documents/document/image%3A1234"), "file-1234");
    }
}
