//! The one entry point a front end uses: named commands with JSON arguments.
//! The Tauri shell and the headless daemon both forward to [`Core::call`].

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{
    i18n::tr,
    model::{Network, PeerAddr},
    Core,
};

fn text(args: &Value, key: &str) -> Result<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("missing `{key}`"))
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn port(args: &Value, key: &str) -> Result<u16> {
    args.get(key).and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).ok_or_else(|| anyhow!(tr!("پورت نامعتبر است", "Not a valid port")))
}

fn paths(args: &Value) -> Vec<String> {
    args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// "ask" / "auto", or `None` to fall back to the less specific setting.
fn mode(args: &Value) -> Result<Option<String>> {
    match args.get("mode").and_then(Value::as_str) {
        None | Some("") | Some("default") => Ok(None),
        Some(m @ ("ask" | "auto")) => Ok(Some(m.to_string())),
        Some(_) => bail!(tr!("حالت نامعتبر است", "Not a valid mode")),
    }
}

impl Core {
    /// Everything the UI shows, in one object. The UI re-reads it whenever a `changed` event arrives.
    pub fn snapshot(&self) -> Value {
        let cfg = self.cfg.lock().unwrap().clone();
        let rt = self.rt.lock().unwrap();
        let net_id = rt.net.as_ref().map(|n| n.id.clone());
        let saved = net_id.as_ref().and_then(|id| cfg.networks.iter().find(|n| n.id == *id));

        let peers: Vec<Value> = cfg
            .peers
            .iter()
            .map(|p| {
                let link = rt.links.get(&p.id).and_then(|v| v.first());
                json!({
                    "id": p.id, "name": p.name, "kind": p.kind,
                    "online": link.is_some(),
                    "addr": link.map(|l| l.addr.to_string()),
                    "mode": p.accept_mode,
                    "addrs": p.addrs,
                    // Reachable on this network or through a static address.
                    "here": link.is_some() || p.addrs.iter().any(|a| a.net.is_none() || a.net == net_id),
                })
            })
            .collect();
        let nearby: Vec<Value> = rt
            .nearby
            .iter()
            .filter(|(id, n)| n.seen.elapsed() < n.ttl && !cfg.peers.iter().any(|p| p.id == **id))
            .map(|(id, n)| json!({ "id": id, "name": n.info.name, "kind": n.info.kind, "addr": n.addr.to_string() }))
            .collect();
        let name_of = |id: &str| cfg.peers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
        let transfers: Vec<Value> = rt
            .transfers
            .iter()
            .rev()
            .map(|t| {
                json!({
                    "id": t.id, "peer": name_of(&t.peer_id), "outgoing": t.outgoing, "name": t.name, "count": t.count,
                    "total": t.total, "done": t.done, "state": t.state, "error": t.error, "speed": t.speed, "at": t.at,
                })
            })
            .collect();
        let pairs: Vec<Value> = rt
            .pair_prompts
            .iter()
            .map(|(id, p)| json!({ "id": id, "name": p.info.name, "kind": p.info.kind, "sas": p.sas, "initiator": p.initiator, "waiting": p.waiting }))
            .collect();
        let offers: Vec<Value> = rt
            .offers
            .iter()
            .map(|(id, o)| json!({ "id": id, "peer": name_of(&o.peer_id), "name": o.name, "count": o.count, "total": o.total }))
            .collect();
        let inbox: Vec<Value> = cfg
            .inbox
            .iter()
            .rev()
            .map(|i| {
                let mut v = serde_json::to_value(i).unwrap_or_default();
                v["missing"] = json!(!Path::new(&i.path).exists());
                v
            })
            .collect();

        json!({
            "me": { "id": self.id, "name": cfg.name, "kind": crate::model::device_kind(), "port": self.port, "config_port": cfg.port,
                    "ip": rt.net.as_ref().map(|n| n.local_ip.to_string()) },
            "network": rt.net.as_ref().map(|n| json!({
                "id": n.id, "saved": saved.is_some(), "name": saved.map(|s| s.name.clone()),
                "mode": saved.and_then(|s| s.accept_mode.clone()),
                "suggest": rt.net_suggest, "gateway": n.gateway.map(|g| g.to_string()),
            })),
            "networks": cfg.networks.iter().map(|n| json!({
                "id": n.id, "name": n.name, "mode": n.accept_mode, "current": Some(&n.id) == net_id.as_ref(),
                "devices": cfg.peers.iter().filter(|p| p.addrs.iter().any(|a| a.net.as_ref() == Some(&n.id))).count(),
            })).collect::<Vec<_>>(),
            "peers": peers,
            "nearby": nearby,
            "transfers": transfers,
            "pairs": pairs,
            "offers": offers,
            "inbox": inbox,
            "board": self.board_view(&cfg, &rt),
            "settings": { "accept_mode": cfg.accept_mode, "auto_max": cfg.auto_max, "inbox_dir": cfg.inbox_dir, "keep_dir": cfg.keep_dir,
                          "board_dir": cfg.board_dir, "always_on": cfg.always_on, "guest_port": cfg.guest_port, "lang": cfg.lang },
            "pairing_left": rt.pairing_until.map(|t| t.saturating_duration_since(Instant::now()).as_secs()).unwrap_or(0),
            "guest": rt.guest.as_ref().map(|g| json!({
                "url": g.url, "qr": g.qr,
                "shared": g.shared.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>(),
            })),
        })
    }

    fn edit<R>(&self, f: impl FnOnce(&mut crate::model::Config) -> R) -> R {
        let out = {
            let mut cfg = self.cfg.lock().unwrap();
            let out = f(&mut cfg);
            cfg.save(&self.dir);
            out
        };
        self.changed();
        self.wake.notify_one();
        out
    }

    pub async fn call(&self, cmd: &str, args: Value) -> Result<Value> {
        match cmd {
            "snapshot" => return Ok(self.snapshot()),

            // ---- this device
            "set_name" => {
                let name = text(&args, "name")?.trim().to_string();
                if name.is_empty() {
                    bail!(tr!("اسم خالی است", "The name is empty"));
                }
                self.edit(|c| c.name = name);
                self.announce_info();
            }
            "set_lang" => {
                let lang = match text(&args, "lang")?.as_str() {
                    "en" => "en",
                    _ => "fa",
                };
                crate::i18n::set(lang);
                self.edit(|c| c.lang = lang.into());
            }
            "set_accept" => {
                let m = mode(&args)?.ok_or_else(|| anyhow!(tr!("حالت نامعتبر است", "Not a valid mode")))?;
                let max = args.get("auto_max").and_then(Value::as_u64);
                self.edit(|c| {
                    c.accept_mode = m;
                    if let Some(max) = max {
                        c.auto_max = max;
                    }
                });
            }
            "set_dirs" => {
                let dir = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty());
                let (inbox, keep, board) = (dir("inbox_dir"), dir("keep_dir"), dir("board_dir"));
                for dir in [inbox, keep, board].into_iter().flatten() {
                    std::fs::create_dir_all(dir).map_err(|e| anyhow!(tr!("این پوشه ساخته نشد: {}", "Could not create this folder: {}", e)))?;
                    // A folder can exist and still refuse this app, as most of a phone's storage does without permission.
                    let probe = Path::new(dir).join(format!(".dbd-{}", crate::new_id()));
                    std::fs::write(&probe, b"").map_err(|_| anyhow!(tr!("برنامه اجازهٔ نوشتن در این پوشه را ندارد: {}", "The app is not allowed to write in this folder: {}", dir)))?;
                    let _ = std::fs::remove_file(&probe);
                }
                self.edit(|c| {
                    if let Some(d) = inbox {
                        c.inbox_dir = d.to_string();
                    }
                    if let Some(d) = keep {
                        c.keep_dir = d.to_string();
                    }
                    if let Some(d) = board {
                        c.board_dir = d.to_string();
                    }
                });
            }
            "set_port" => {
                let p = port(&args, "port")?;
                self.edit(|c| c.port = p);
            }
            "set_guest_port" => {
                // 0 lets the system pick a free port each time the guest link is switched on.
                let p = args.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).ok_or_else(|| anyhow!(tr!("پورت نامعتبر است", "Not a valid port")))?;
                self.edit(|c| c.guest_port = p);
            }
            "set_always_on" => {
                let on = flag(&args, "on");
                self.edit(|c| c.always_on = on);
                self.board_auto();
            }

            // ---- choosing a folder from inside the UI, where the system has no folder picker to offer
            "list_dir" => {
                let asked = args.get("path").and_then(Value::as_str).unwrap_or_default().trim();
                let mut dir = if asked.is_empty() { browse_root() } else { PathBuf::from(asked) };
                // A phone's shared storage is the only place worth browsing: above it there is nothing an app
                // may see, and beside it only the app's own hidden folders.
                let floor = cfg!(target_os = "android").then(browse_root);
                if floor.as_ref().is_some_and(|f| !dir.starts_with(f)) {
                    dir = browse_root();
                }
                while !dir.is_dir() {
                    match dir.parent() {
                        Some(p) => dir = p.to_path_buf(),
                        None => {
                            dir = browse_root();
                            break;
                        }
                    }
                }
                let mut dirs: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect())
                    .unwrap_or_default();
                dirs.sort_by_key(|n| n.to_lowercase());
                let parent = if floor.as_ref() == Some(&dir) { None } else { dir.parent() };
                return Ok(json!({ "path": dir, "parent": parent, "dirs": dirs }));
            }
            "make_dir" => {
                let name = text(&args, "name")?.trim().to_string();
                if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
                    bail!(tr!("اسم پوشه نامعتبر است", "Not a valid folder name"));
                }
                let dir = PathBuf::from(text(&args, "path")?).join(name);
                std::fs::create_dir(&dir).map_err(|e| anyhow!(tr!("این پوشه ساخته نشد: {}", "Could not create this folder: {}", e)))?;
                return Ok(json!({ "path": dir }));
            }

            // ---- networks
            "save_network" => {
                let name = text(&args, "name")?.trim().to_string();
                let id = self.rt.lock().unwrap().net.as_ref().map(|n| n.id.clone()).ok_or_else(|| anyhow!(tr!("به هیچ شبکه‌ای وصل نیستی", "You are not connected to any network")))?;
                if name.is_empty() {
                    bail!(tr!("اسم خالی است", "The name is empty"));
                }
                self.edit(|c| match c.networks.iter_mut().find(|n| n.id == id) {
                    Some(n) => n.name = name,
                    None => c.networks.push(Network { id, name, accept_mode: None }),
                });
            }
            "rename_network" => {
                let (id, name) = (text(&args, "id")?, text(&args, "name")?.trim().to_string());
                if name.is_empty() {
                    bail!(tr!("اسم خالی است", "The name is empty"));
                }
                self.edit(|c| {
                    if let Some(n) = c.networks.iter_mut().find(|n| n.id == id) {
                        n.name = name;
                    }
                });
            }
            "set_network_mode" => {
                let (id, m) = (text(&args, "id")?, mode(&args)?);
                self.edit(|c| {
                    if let Some(n) = c.networks.iter_mut().find(|n| n.id == id) {
                        n.accept_mode = m;
                    }
                });
            }
            "forget_network" => {
                let id = text(&args, "id")?;
                self.edit(|c| {
                    c.networks.retain(|n| n.id != id);
                    for p in &mut c.peers {
                        p.addrs.retain(|a| a.net.as_deref() != Some(&id));
                    }
                });
            }

            // ---- new connections
            "pairing_open" => {
                let secs = args.get("secs").and_then(Value::as_u64).unwrap_or(300).min(3600);
                self.rt.lock().unwrap().pairing_until = Some(Instant::now() + Duration::from_secs(secs));
                self.changed();
            }
            "pairing_close" => {
                self.rt.lock().unwrap().pairing_until = None;
                self.changed();
            }
            "pair_addr" => self.pair_addr(&text(&args, "host")?, port(&args, "port")?, flag(&args, "anywhere")).await?,
            "pair_nearby" => {
                let id = text(&args, "id")?;
                let addr = self.rt.lock().unwrap().nearby.get(&id).map(|n| n.addr).ok_or_else(|| anyhow!(tr!("این دستگاه دیگر دیده نمی‌شود", "This device is no longer visible")))?;
                self.pair_addr(&addr.ip().to_string(), addr.port(), false).await?;
            }
            "pair_decide" => {
                let (id, ok) = (text(&args, "id")?, flag(&args, "ok"));
                if let Some(tx) = self.rt.lock().unwrap().pair_prompts.get_mut(&id).and_then(|p| p.tx.take()) {
                    let _ = tx.send(ok);
                }
            }
            "scan" => return Ok(json!({ "found": self.scan().await? })),

            // ---- paired devices
            "unpair" => self.unpair(&text(&args, "id")?).await,
            "set_peer_mode" => {
                let (id, m) = (text(&args, "id")?, mode(&args)?);
                self.edit(|c| {
                    if let Some(p) = c.peers.iter_mut().find(|p| p.id == id) {
                        p.accept_mode = m;
                    }
                });
            }
            "add_peer_addr" => {
                let id = text(&args, "id")?;
                let net = if flag(&args, "anywhere") { None } else { Some(self.saved_net_id().ok_or_else(|| anyhow!(tr!("اول این شبکه را ذخیره کن", "Save this network first")))?) };
                let addr = PeerAddr { host: text(&args, "host")?.trim().to_string(), port: port(&args, "port")?, net };
                self.edit(|c| {
                    if let Some(p) = c.peers.iter_mut().find(|p| p.id == id) {
                        if !p.addrs.contains(&addr) {
                            p.addrs.push(addr);
                        }
                    }
                });
            }
            "remove_peer_addr" => {
                let (id, host, p) = (text(&args, "id")?, text(&args, "host")?, port(&args, "port")?);
                self.edit(|c| {
                    if let Some(peer) = c.peers.iter_mut().find(|x| x.id == id) {
                        peer.addrs.retain(|a| !(a.host == host && a.port == p));
                    }
                });
            }

            // ---- transfers
            "send" => return Ok(json!({ "id": self.send(&text(&args, "peer")?, paths(&args)).await? })),
            "cancel" => self.cancel(&text(&args, "id")?),
            "offer_decide" => self.offer_decide(&text(&args, "id")?, flag(&args, "ok")),
            "remove_transfer" => {
                let id = text(&args, "id")?;
                self.rt.lock().unwrap().transfers.retain(|t| t.id != id || matches!(t.state, "connecting" | "asking" | "active" | "waiting"));
                self.changed();
            }
            "clear_transfers" => {
                self.rt.lock().unwrap().transfers.retain(|t| matches!(t.state, "connecting" | "asking" | "active" | "waiting"));
                self.changed();
            }

            // ---- inbox
            "inbox_keep" => self.inbox_keep(&text(&args, "id")?, args.get("dir").and_then(Value::as_str).map(str::to_string)).await?,
            "inbox_delete" => self.inbox_delete(&text(&args, "id")?).await?,
            "inbox_open" => {
                let id = text(&args, "id")?;
                let path = self.cfg.lock().unwrap().inbox.iter().find(|i| i.id == id).map(|i| i.path.clone()).ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
                let target = if flag(&args, "folder") { Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_default() } else { path.into() };
                open_in_os(&target)?;
            }

            // ---- shared board
            "board_put" => self.board_put(paths(&args)).await?,
            "board_text" => self.board_text(&text(&args, "text")?)?,
            "board_get" => self.board_get(&text(&args, "id")?).await?,
            "board_remove" => self.board_remove(&text(&args, "id")?)?,
            "board_keep" => self.board_keep(&text(&args, "id")?).await?,
            "board_open" => {
                let path = self.board_path(&text(&args, "id")?).ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
                let target = if flag(&args, "folder") { path.parent().map(|p| p.to_path_buf()).unwrap_or_default() } else { path };
                open_in_os(&target)?;
            }
            "open_url" => {
                // Text on the board comes from other devices; only plain web links are handed to the system.
                let url = text(&args, "url")?;
                let web = (url.starts_with("http://") || url.starts_with("https://")) && !url.chars().any(|c| c.is_whitespace() || c.is_control());
                if !web {
                    bail!(tr!("این یک لینک وب نیست", "This is not a web link"));
                }
                open_in_os(Path::new(&url))?;
            }

            // ---- browser guest
            "guest_start" => self.guest_start().await?,
            "guest_stop" => self.guest_stop(),
            "guest_reset" => self.guest_reset(),
            "guest_clear" => self.guest_clear(),
            "guest_share" => self.guest_share(paths(&args)),

            other => bail!("unknown command `{other}`"),
        }
        Ok(Value::Null)
    }
}

/// Where browsing for a folder starts.
fn browse_root() -> PathBuf {
    if cfg!(target_os = "android") {
        PathBuf::from("/storage/emulated/0")
    } else {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    }
}

fn open_in_os(path: &Path) -> Result<()> {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn().map_err(|e| anyhow!(tr!("باز نشد: {}", "Could not open: {}", e)))?;
    Ok(())
}
