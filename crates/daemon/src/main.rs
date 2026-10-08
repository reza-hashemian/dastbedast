//! Headless DastBeDast: the core plus the UI served to a browser on this machine only.
//! Useful on a server or an always-on box, and for working on the UI without the desktop shell.

use std::{collections::HashMap, convert::Infallible, path::PathBuf};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use dbd_core::{Core, Options};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
struct App {
    core: Core,
    token: String,
    /// Serve the UI from this folder instead of the copy built into the binary.
    ui_dir: Option<PathBuf>,
}

const ASSETS: &[(&str, &str, &[u8])] = &[
    ("index.html", "text/html; charset=utf-8", include_bytes!("../../../ui/index.html")),
    ("style.css", "text/css; charset=utf-8", include_bytes!("../../../ui/style.css")),
    ("app.js", "text/javascript; charset=utf-8", include_bytes!("../../../ui/app.js")),
    ("fonts/vazirmatn.woff2", "font/woff2", include_bytes!("../../../ui/fonts/vazirmatn.woff2")),
];

async fn asset(app: &App, name: &str) -> Response {
    let Some((_, mime, built_in)) = ASSETS.iter().find(|a| a.0 == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = match &app.ui_dir {
        Some(dir) => tokio::fs::read(dir.join(name)).await.unwrap_or_else(|_| built_in.to_vec()),
        None => built_in.to_vec(),
    };
    ([(header::CONTENT_TYPE, *mime), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

async fn index(State(app): State<App>) -> Response {
    asset(&app, "index.html").await
}

async fn file(State(app): State<App>, Path(name): Path<String>) -> Response {
    asset(&app, &name).await
}

async fn call(State(app): State<App>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if headers.get("x-dbd-token").and_then(|v| v.to_str().ok()) != Some(app.token.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let cmd = body.get("cmd").and_then(Value::as_str).unwrap_or_default();
    let args = body.get("args").cloned().unwrap_or(Value::Null);
    Json(match app.core.call(cmd, args).await {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    })
    .into_response()
}

async fn events(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Response {
    if q.get("t") != Some(&app.token) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let stream = futures_util::stream::unfold(app.core.subscribe(), |mut rx| async move {
        let data = match rx.recv().await {
            Ok(v) => v.to_string(),
            // Too slow to keep up: one "changed" makes the page reload everything.
            Err(RecvError::Lagged(_)) => r#"{"type":"changed"}"#.to_string(),
            Err(RecvError::Closed) => return None,
        };
        Some((Ok::<_, Infallible>(Event::default().data(data)), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("dbd [--dir <config dir>] [--port <device port>] [--ui-port <port>] [--no-discovery]");
        return Ok(());
    }
    let dir = arg("--dir").map(PathBuf::from).unwrap_or_else(dbd_core::default_dir);
    let port = arg("--port").map(|p| p.parse()).transpose().context("--port")?;
    let ui_port: u16 = arg("--ui-port").map(|p| p.parse()).transpose().context("--ui-port")?.unwrap_or(47810);
    let discovery = !std::env::args().any(|a| a == "--no-discovery");

    let core = Core::start(dir.clone(), Options { port, discovery }).await?;
    // The token keeps other local programs and web pages from driving this device.
    let token_path = dir.join("ui-token");
    let token = match std::fs::read_to_string(&token_path) {
        Ok(t) if t.trim().len() >= 16 => t.trim().to_string(),
        _ => {
            let t = format!("{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>());
            std::fs::write(&token_path, &t)?;
            t
        }
    };
    let app = App { core: core.clone(), token: token.clone(), ui_dir: std::env::var_os("DBD_UI_DIR").map(PathBuf::from) };
    let router = Router::new()
        .route("/", get(index))
        .route("/api/call", post(call))
        .route("/api/events", get(events))
        .route("/*name", get(file))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", ui_port)).await.with_context(|| format!("UI port {ui_port}"))?;
    println!("device port {}", core.port);
    println!("open http://127.0.0.1:{ui_port}/#{token}");
    axum::serve(listener, router).await?;
    Ok(())
}
