//! Browser guest: a device without the app opens a link on the local network and can send
//! files to this device, take what was put out for it, and use the shared board.
//!
//! This is also how an iPhone takes part: the page can be added to the home screen and then
//! opens like an app. A page in a browser cannot be a device of its own (it can neither listen
//! nor open raw connections), so it always works through the device that serves it.
//!
//! The server only runs while the user has switched it on, and the link carries a random token.
//! It is plain HTTP, meant for the local network only.

use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

use crate::{
    i18n::tr,
    model::{BoardEntry, BoardLocal, InboxItem},
    new_id, now, unique_path, Core,
};

pub(crate) struct Guest {
    pub token: String,
    port: u16,
    pub url: String,
    pub qr: String,
    pub shared: Vec<PathBuf>,
    /// Transfers this device is passing on for the guest.
    relays: Vec<String>,
    stop: Option<oneshot::Sender<()>>,
}

const PAGE: &str = include_str!("guest.html");
const ICON: &[u8] = include_bytes!("guest-icon.png");

fn qr_svg(text: &str) -> String {
    qrcode::QrCode::new(text.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
        .unwrap_or_default()
}

fn pct(name: &str) -> String {
    name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn guest_name() -> String {
    tr!("مهمان مرورگر", "Browser guest")
}

impl Core {
    pub async fn guest_start(&self) -> Result<()> {
        if self.rt.lock().unwrap().guest.is_some() {
            return Ok(());
        }
        let (port, token) = {
            let mut cfg = self.cfg.lock().unwrap();
            if cfg.guest_token.is_empty() {
                cfg.guest_token = new_id();
            }
            cfg.guest_on = true;
            cfg.save(&self.dir);
            (cfg.guest_port, cfg.guest_token.clone())
        };
        let listener = TcpListener::bind(("0.0.0.0", port)).await.with_context(|| tr!("پورت {} در دسترس نیست", "Port {} is not available", port))?;
        let port = listener.local_addr()?.port();
        let app = Router::new()
            .route("/g/:token", get(page))
            .route("/g/:token/manifest.webmanifest", get(manifest))
            .route("/g/:token/icon.png", get(icon))
            .route("/g/:token/state", get(state))
            .route("/g/:token/to/:peer/:name", put(relay))
            .route("/g/:token/list", get(list))
            .route("/g/:token/dl/:idx", get(download))
            .route("/g/:token/up/:name", put(upload))
            .route("/g/:token/board", get(board_list))
            .route("/g/:token/board/text", post(board_text))
            .route("/g/:token/board/up/:name", put(board_upload))
            .route("/g/:token/board/dl/:id", get(board_download))
            .layer(DefaultBodyLimit::disable())
            .with_state(self.clone());
        let (stop, stopped) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await;
        });
        self.rt.lock().unwrap().guest = Some(Guest { token, port, url: String::new(), qr: String::new(), shared: vec![], relays: vec![], stop: Some(stop) });
        self.guest_refresh();
        self.changed();
        self.board_auto();
        Ok(())
    }

    /// The link shows this device's current address; it is rebuilt when the network changes.
    pub(crate) fn guest_refresh(&self) {
        let mut rt = self.rt.lock().unwrap();
        let ip = rt.net.as_ref().map(|n| n.local_ip.to_string()).unwrap_or_else(|| "127.0.0.1".into());
        if let Some(g) = rt.guest.as_mut() {
            let url = format!("http://{ip}:{}/g/{}", g.port, g.token);
            if url != g.url {
                g.qr = qr_svg(&url);
                g.url = url;
            }
        }
    }

    pub fn guest_stop(&self) {
        if let Some(mut g) = self.rt.lock().unwrap().guest.take() {
            if let Some(stop) = g.stop.take() {
                let _ = stop.send(());
            }
        }
        {
            let mut cfg = self.cfg.lock().unwrap();
            cfg.guest_on = false;
            cfg.save(&self.dir);
        }
        self.changed();
    }

    /// Replaces the link. Every phone that saved the old one loses access.
    pub fn guest_reset(&self) {
        let token = new_id();
        {
            let mut cfg = self.cfg.lock().unwrap();
            cfg.guest_token = token.clone();
            cfg.save(&self.dir);
        }
        if let Some(g) = self.rt.lock().unwrap().guest.as_mut() {
            g.token = token;
            g.url.clear();
        }
        self.guest_refresh();
        self.changed();
    }

    /// Puts files out for the guest to download. Folders are not offered to a browser.
    pub fn guest_share(&self, paths: Vec<String>) {
        if let Some(g) = self.rt.lock().unwrap().guest.as_mut() {
            for p in paths.into_iter().map(PathBuf::from) {
                if p.is_file() && !g.shared.contains(&p) {
                    g.shared.push(p);
                }
            }
        }
        self.changed();
    }

    /// Takes back everything put out for the guest.
    pub fn guest_clear(&self) {
        if let Some(g) = self.rt.lock().unwrap().guest.as_mut() {
            g.shared.clear();
        }
        self.changed();
    }

    fn guest_ok(&self, token: &str) -> bool {
        self.rt.lock().unwrap().guest.as_ref().is_some_and(|g| g.token == token)
    }
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

async fn page(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let name = core.info().name.replace('&', "&amp;").replace('<', "&lt;");
    let lang = if crate::i18n::en() { "en" } else { "fa" };
    let html = PAGE.replace("{{HOST}}", &name).replace("{{LANG}}", lang).replace("{{TOKEN}}", &token);
    ([(header::CACHE_CONTROL, "no-cache")], Html(html)).into_response()
}

/// Lets a phone add the page to its home screen as an app.
async fn manifest(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let name = if crate::i18n::en() { "DastBeDast" } else { "دست‌به‌دست" };
    let body = json!({
        "name": name, "short_name": name,
        "start_url": format!("/g/{token}"), "scope": format!("/g/{token}"),
        "display": "standalone", "background_color": "#EDF1F4", "theme_color": "#0A7486",
        "icons": [{ "src": format!("/g/{token}/icon.png"), "sizes": "512x512", "type": "image/png", "purpose": "any maskable" }],
    });
    ([(header::CONTENT_TYPE, "application/manifest+json")], body.to_string()).into_response()
}

async fn icon(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=86400")], ICON).into_response()
}

fn shared_json(g: &Guest) -> Vec<Value> {
    g.shared
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "idx": i,
                "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "size": std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            })
        })
        .collect()
}

async fn list(State(core): State<Core>, Path(token): Path<String>) -> Response {
    let rt = core.rt.lock().unwrap();
    let Some(g) = rt.guest.as_ref().filter(|g| g.token == token) else {
        return not_found();
    };
    Json(shared_json(g)).into_response()
}

/// Everything the page shows, in one answer: the devices it can send to, what waits for it,
/// the shared board, and how the files it handed over for other devices are getting on.
async fn state(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let board = board_json(&core);
    let peers = core.cfg.lock().unwrap().peers.clone();
    let info = core.info();
    let rt = core.rt.lock().unwrap();
    let Some(g) = rt.guest.as_ref() else { return not_found() };
    let name_of = |id: &str| peers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
    let relays: Vec<Value> = rt
        .transfers
        .iter()
        .rev()
        .filter(|t| g.relays.contains(&t.id))
        .map(|t| json!({ "id": t.id, "name": t.name, "peer": name_of(&t.peer_id), "state": t.state, "done": t.done, "total": t.total, "error": t.error }))
        .collect();
    Json(json!({
        "host": { "name": info.name, "kind": info.kind },
        "peers": peers.iter().map(|p| json!({ "id": p.id, "name": p.name, "kind": p.kind, "online": rt.links.contains_key(&p.id) })).collect::<Vec<_>>(),
        "inbox": shared_json(g),
        "board": board,
        "relays": relays,
    }))
    .into_response()
}

/// The guest sends a file to one of this device's paired devices: it is taken in here and passed on.
async fn relay(State(core): State<Core>, Path((token, peer, name)): Path<(String, String, String)>, body: Body) -> Response {
    if !core.guest_ok(&token) || !core.is_paired(&peer) {
        return not_found();
    }
    let dir = PathBuf::from(core.cfg.lock().unwrap().inbox_dir.clone()).join(".partial").join(format!("relay-{}", new_id()));
    let Some((dest, _, _)) = store_upload(dir.clone(), &name, body).await else {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return StatusCode::BAD_REQUEST.into_response();
    };
    let id = match core.send(&peer, vec![dest.to_string_lossy().into_owned()]).await {
        Ok(id) => id,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    if let Some(g) = core.rt.lock().unwrap().guest.as_mut() {
        g.relays.push(id.clone());
    }
    // The copy held here is only needed until the transfer has ended one way or another.
    let (c, tid) = (core.clone(), id.clone());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let state = c.rt.lock().unwrap().transfers.iter().find(|t| t.id == tid).map(|t| t.state);
            if !matches!(state, Some("connecting" | "asking" | "active" | "waiting")) {
                let _ = tokio::fs::remove_dir_all(&dir).await;
                break;
            }
        }
    });
    Json(json!({ "id": id })).into_response()
}

async fn send_file(path: PathBuf, name: String) -> Response {
    let Ok(file) = tokio::fs::File::open(&path).await else { return not_found() };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().expect("static"));
    headers.insert(header::CONTENT_LENGTH, len.into());
    if let Ok(v) = format!("attachment; filename*=UTF-8''{}", pct(&name)).parse() {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    (headers, Body::from_stream(tokio_util::io::ReaderStream::with_capacity(file, 1 << 18))).into_response()
}

async fn download(State(core): State<Core>, Path((token, idx)): Path<(String, usize)>) -> Response {
    let path = core.rt.lock().unwrap().guest.as_ref().filter(|g| g.token == token).and_then(|g| g.shared.get(idx).cloned());
    let Some(path) = path else { return not_found() };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    send_file(path, name).await
}

/// Stores an uploaded body in `dir` under a safe version of `name`. Returns where it ended up.
async fn store_upload(dir: PathBuf, name: &str, body: Body) -> Option<(PathBuf, String, u64)> {
    // Only the last path component, with characters no filesystem accepts replaced.
    let name: String = name.rsplit(['/', '\\']).next().unwrap_or("").chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim().trim_matches('.').to_string();
    if name.is_empty() {
        return None;
    }
    let tmp_dir = dir.join(".partial");
    let tmp = tmp_dir.join(format!("guest-{}", new_id()));
    let res: Result<u64> = async {
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            size += chunk.len() as u64;
        }
        file.flush().await?;
        Ok(size)
    }
    .await;
    let dest = unique_path(&dir, &name);
    match res {
        Ok(size) if tokio::fs::rename(&tmp, &dest).await.is_ok() => {
            let shown = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            Some((dest, shown, size))
        }
        _ => {
            let _ = tokio::fs::remove_file(&tmp).await;
            None
        }
    }
}

async fn upload(State(core): State<Core>, Path((token, name)): Path<(String, String)>, body: Body) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let inbox = PathBuf::from(core.cfg.lock().unwrap().inbox_dir.clone());
    let Some((dest, shown, size)) = store_upload(inbox, &name, body).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let guest = guest_name();
    core.add_inbox(vec![InboxItem {
        id: new_id(),
        name: shown.clone(),
        path: dest.to_string_lossy().into_owned(),
        size,
        files: 1,
        from: guest.clone(),
        at: now(),
        state: "new".into(),
    }]);
    core.emit(json!({ "type": "received", "from": guest, "name": shown }));
    core.changed();
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------- the shared board, for the guest

fn board_json(core: &Core) -> Vec<Value> {
    let cfg = core.cfg.lock().unwrap();
    cfg
        .board
        .iter()
        .rev()
        .filter(|e| e.item.gone == 0)
        .map(|e| {
            let it = &e.item;
            let from = cfg.peers.iter().find(|p| p.id == it.from).map(|p| p.name.clone()).unwrap_or_else(|| it.from_name.clone());
            // A browser takes single files, and only those this device holds itself.
            let here = it.kind == "file" && e.local.as_ref().is_some_and(|l| std::path::Path::new(&l.path).is_file());
            json!({ "id": it.id, "kind": it.kind, "name": it.name, "text": it.text, "size": it.size, "files": it.files, "from": from, "at": it.at, "here": here })
        })
        .collect()
}

async fn board_list(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    Json(board_json(&core)).into_response()
}

async fn board_text(State(core): State<Core>, Path(token): Path<String>, text: String) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    match core.board_text(&text) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn board_upload(State(core): State<Core>, Path((token, name)): Path<(String, String)>, body: Body) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let dir = PathBuf::from(core.cfg.lock().unwrap().board_dir.clone());
    let Some((dest, shown, size)) = store_upload(dir, &name, body).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut item = core.board_item("file", shown, String::new(), size, 1);
    item.from_name = format!("{} ({})", guest_name(), item.from_name);
    // The copy belongs to the board: it goes when the item is taken off.
    let entry = BoardEntry { item, local: Some(BoardLocal { path: dest.to_string_lossy().into_owned(), own: false, kept: false }) };
    match core.board_add(vec![entry]) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}

async fn board_download(State(core): State<Core>, Path((token, id)): Path<(String, String)>) -> Response {
    if !core.guest_ok(&token) {
        return not_found();
    }
    let found = core.cfg.lock().unwrap().board.iter().find(|e| e.item.id == id && e.item.gone == 0).and_then(|e| Some((PathBuf::from(&e.local.as_ref()?.path), e.item.name.clone())));
    match found {
        Some((path, name)) if path.is_file() => send_file(path, name).await,
        _ => not_found(),
    }
}
