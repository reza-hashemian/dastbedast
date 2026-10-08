//! Browser guest: a device without the app opens a link on the local network
//! and can upload into the inbox or download what was put out for it.
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
    routing::{get, put},
    Json, Router,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

use crate::{i18n::tr, model::InboxItem, new_id, now, unique_path, Core};

pub(crate) struct Guest {
    pub token: String,
    pub url: String,
    pub qr: String,
    pub shared: Vec<PathBuf>,
    stop: Option<oneshot::Sender<()>>,
}

const PAGE: &str = include_str!("guest.html");

fn qr_svg(text: &str) -> String {
    qrcode::QrCode::new(text.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
        .unwrap_or_default()
}

fn pct(name: &str) -> String {
    name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl Core {
    pub async fn guest_start(&self) -> Result<()> {
        if self.rt.lock().unwrap().guest.is_some() {
            return Ok(());
        }
        let port = self.cfg.lock().unwrap().guest_port;
        let listener = TcpListener::bind(("0.0.0.0", port)).await.with_context(|| tr!("پورت {} در دسترس نیست", "Port {} is not available", port))?;
        let port = listener.local_addr()?.port();
        let ip = self.rt.lock().unwrap().net.as_ref().map(|n| n.local_ip.to_string()).unwrap_or_else(|| "127.0.0.1".into());
        let token = new_id();
        let url = format!("http://{ip}:{port}/g/{token}");
        let app = Router::new()
            .route("/g/:token", get(page))
            .route("/g/:token/list", get(list))
            .route("/g/:token/dl/:idx", get(download))
            .route("/g/:token/up/:name", put(upload))
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
        self.rt.lock().unwrap().guest = Some(Guest { token, qr: qr_svg(&url), url, shared: vec![], stop: Some(stop) });
        self.changed();
        Ok(())
    }

    pub fn guest_stop(&self) {
        if let Some(mut g) = self.rt.lock().unwrap().guest.take() {
            if let Some(stop) = g.stop.take() {
                let _ = stop.send(());
            }
        }
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

    fn guest_ok(&self, token: &str) -> bool {
        self.rt.lock().unwrap().guest.as_ref().is_some_and(|g| g.token == token)
    }
}

async fn page(State(core): State<Core>, Path(token): Path<String>) -> Response {
    if !core.guest_ok(&token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let name = core.info().name.replace('&', "&amp;").replace('<', "&lt;");
    let lang = if crate::i18n::en() { "en" } else { "fa" };
    Html(PAGE.replace("{{HOST}}", &name).replace("{{LANG}}", lang)).into_response()
}

async fn list(State(core): State<Core>, Path(token): Path<String>) -> Response {
    let rt = core.rt.lock().unwrap();
    let Some(g) = rt.guest.as_ref().filter(|g| g.token == token) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let files: Vec<Value> = g
        .shared
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "idx": i,
                "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "size": std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            })
        })
        .collect();
    Json(files).into_response()
}

async fn download(State(core): State<Core>, Path((token, idx)): Path<(String, usize)>) -> Response {
    let path = core.rt.lock().unwrap().guest.as_ref().filter(|g| g.token == token).and_then(|g| g.shared.get(idx).cloned());
    let Some(path) = path else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(file) = tokio::fs::File::open(&path).await else { return StatusCode::NOT_FOUND.into_response() };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().expect("static"));
    headers.insert(header::CONTENT_LENGTH, len.into());
    if let Ok(v) = format!("attachment; filename*=UTF-8''{}", pct(&name)).parse() {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    (headers, Body::from_stream(tokio_util::io::ReaderStream::with_capacity(file, 1 << 18))).into_response()
}

async fn upload(State(core): State<Core>, Path((token, name)): Path<(String, String)>, body: Body) -> Response {
    if !core.guest_ok(&token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Only the last path component, with characters no filesystem accepts replaced.
    let name: String = name.rsplit(['/', '\\']).next().unwrap_or("").chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let name = name.trim().trim_matches('.').to_string();
    if name.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let inbox = PathBuf::from(core.cfg.lock().unwrap().inbox_dir.clone());
    let tmp_dir = inbox.join(".partial");
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
    let size = match res {
        Ok(size) => size,
        Err(_) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let dest = unique_path(&inbox, &name);
    if tokio::fs::rename(&tmp, &dest).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let shown = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
    let guest = tr!("مهمان مرورگر", "Browser guest");
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
