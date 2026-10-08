//! The shared board: things one device puts out for all the others.
//!
//! The list of items is small and every device holds all of it. Two devices exchange their
//! whole list when they connect and again whenever it changes, so it spreads from device to
//! device and reaches the ones that were away. Text travels inside the list. The content of a
//! file is fetched only when a device asks for it, from any connected device that has it.
//! An always-on device asks for everything, which keeps files available while their owner is off.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use tokio::time::timeout;

use crate::{
    i18n::{remote, tr},
    model::{BoardEntry, BoardItem, BoardLocal, Config},
    new_id, now,
    proto::{read_msg, write_msg, LinkMsg, Req, Resp},
    transfer::{clean_rel, collect_sizes, move_path, source_exists},
    unique_path, Core, Runtime, Stream,
};

const TEXT_MAX: usize = 64 * 1024;
const LIVE_MAX: usize = 500;
/// How long a removal is remembered, so that it still reaches a device that was away.
const GONE_SECS: u64 = 30 * 86400;
/// How many files an always-on device fetches at the same time.
const AUTO_PARALLEL: usize = 2;

pub(crate) fn prune(cfg: &mut Config) {
    let cutoff = now().saturating_sub(GONE_SECS);
    cfg.board.retain(|e| e.item.gone == 0 || e.item.gone > cutoff);
}

fn valid(it: &BoardItem) -> bool {
    !it.id.is_empty()
        && it.id.len() <= 64
        && it.id.bytes().all(|b| b.is_ascii_alphanumeric())
        && matches!(it.kind.as_str(), "file" | "text")
        && it.text.len() <= TEXT_MAX
        && it.name.len() <= 512
        && it.from.len() <= 128
        && it.from_name.len() <= 256
}

/// This device can hand out the item's content.
fn has_content(e: &BoardEntry) -> bool {
    e.item.gone == 0 && e.item.kind == "file" && e.local.as_ref().is_some_and(|l| source_exists(&l.path))
}

/// Takes an item off the board. A copy this device fetched goes with it; the user's own files stay.
fn bury(e: &mut BoardEntry, gone: u64, doomed: &mut Vec<PathBuf>) {
    e.item.gone = gone.max(1);
    e.item.text.clear();
    if let Some(l) = e.local.take() {
        if !l.own && !l.kept {
            doomed.push(l.path.into());
        }
    }
}

fn delete_paths(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        for p in paths {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    });
}

/// A fetch of this item was just requested or is arriving right now.
fn busy(rt: &Runtime, id: &str) -> bool {
    rt.board_wanted.get(id).is_some_and(|t| t.elapsed() < Duration::from_secs(20))
        || rt.transfers.iter().any(|t| !t.outgoing && t.board.as_deref() == Some(id) && t.state == "active")
}

impl Core {
    pub(crate) fn board_msg(&self) -> LinkMsg {
        let cfg = self.cfg.lock().unwrap();
        LinkMsg::Board {
            items: cfg.board.iter().map(|e| e.item.clone()).collect(),
            have: cfg.board.iter().filter(|e| has_content(e)).map(|e| e.item.id.clone()).collect(),
        }
    }

    fn board_announce(&self, except: Option<&str>) {
        let msg = self.board_msg();
        for (pid, links) in self.rt.lock().unwrap().links.iter() {
            if Some(pid.as_str()) == except {
                continue;
            }
            if let Some(l) = links.first() {
                let _ = l.tx.send(msg.clone());
            }
        }
    }

    /// Stops a fetch of `id` that is under way and forgets that it was asked for.
    fn board_stop_fetch(&self, id: &str) {
        let mut rt = self.rt.lock().unwrap();
        rt.board_wanted.remove(id);
        for t in rt.transfers.iter().filter(|t| !t.outgoing && t.board.as_deref() == Some(id)) {
            t.cancel.cancel();
        }
    }

    /// Another device sent its board: take over what is new here and tell it what it is missing.
    pub(crate) fn board_merge(&self, pid: &str, items: Vec<BoardItem>, have: Vec<String>) {
        if items.len() > 20 * LIVE_MAX {
            return;
        }
        let mut doomed = vec![];
        let mut buried = vec![];
        let (changed, peer_behind) = {
            let mut cfg = self.cfg.lock().unwrap();
            let mut changed = false;
            let mut live = cfg.board.iter().filter(|e| e.item.gone == 0).count();
            for it in items.iter().filter(|it| valid(it)) {
                match cfg.board.iter_mut().find(|e| e.item.id == it.id) {
                    Some(e) => {
                        if it.gone > 0 && e.item.gone == 0 {
                            bury(e, it.gone, &mut doomed);
                            buried.push(it.id.clone());
                            live -= 1;
                            changed = true;
                        }
                    }
                    None => {
                        let mut item = it.clone();
                        if item.gone > 0 {
                            item.text.clear();
                        } else if live >= LIVE_MAX {
                            continue;
                        } else {
                            live += 1;
                        }
                        cfg.board.push(BoardEntry { item, local: None });
                        changed = true;
                    }
                }
            }
            if changed {
                cfg.save(&self.dir);
            }
            let theirs = |id: &str| items.iter().find(|it| it.id == id);
            let peer_behind = cfg.board.iter().any(|e| match theirs(&e.item.id) {
                None => true,
                Some(it) => e.item.gone > 0 && it.gone == 0,
            });
            (changed, peer_behind)
        };
        for id in &buried {
            self.board_stop_fetch(id);
        }
        delete_paths(doomed);
        self.rt.lock().unwrap().board_have.insert(pid.to_string(), have.into_iter().filter(|id| id.len() <= 64).collect());
        if changed {
            self.board_announce(Some(pid));
        }
        if peer_behind {
            let msg = self.board_msg();
            if let Some(l) = self.rt.lock().unwrap().links.get(pid).and_then(|v| v.first()) {
                let _ = l.tx.send(msg);
            }
        }
        self.changed();
        self.board_auto();
    }

    pub(crate) fn board_add(&self, entries: Vec<BoardEntry>) -> Result<()> {
        {
            let mut cfg = self.cfg.lock().unwrap();
            let live = cfg.board.iter().filter(|e| e.item.gone == 0).count();
            if live + entries.len() > LIVE_MAX {
                bail!(tr!("میز مشترک پر است؛ اول چند مورد را بردار", "The shared board is full; take some items off first"));
            }
            for e in entries {
                // Compared as paths, so that `a\\b` and `a/b` on Windows are the same file.
                let same = |x: &BoardEntry| x.item.gone == 0 && x.local.as_ref().is_some_and(|l| l.own && Some(Path::new(&l.path)) == e.local.as_ref().map(|l| Path::new(&l.path)));
                if e.local.is_none() || !cfg.board.iter().any(same) {
                    cfg.board.push(e);
                }
            }
            cfg.save(&self.dir);
        }
        self.changed();
        self.board_announce(None);
        Ok(())
    }

    pub(crate) fn board_item(&self, kind: &str, name: String, text: String, size: u64, files: usize) -> BoardItem {
        BoardItem { id: new_id(), kind: kind.into(), name, text, size, files, from: self.id.clone(), from_name: self.info().name, at: now(), gone: 0 }
    }

    /// Puts files or folders on the board. They are shared from where they are; nothing is copied.
    pub async fn board_put(&self, paths: Vec<String>) -> Result<()> {
        let mut entries = vec![];
        for p in paths {
            let name = Path::new(&p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .ok_or_else(|| anyhow!(tr!("مسیر نامعتبر: {}", "Not a valid path: {}", p)))?;
            let one = vec![p.clone()];
            let (files, size) = tokio::task::spawn_blocking(move || collect_sizes(one)).await??;
            if files == 0 {
                bail!(tr!("«{}» خالی است", "“{}” is empty", name));
            }
            entries.push(BoardEntry {
                item: self.board_item("file", name, String::new(), size, files),
                local: Some(BoardLocal { path: p, own: true, kept: false }),
            });
        }
        if entries.is_empty() {
            bail!(tr!("چیزی برای گذاشتن نیست", "There is nothing to put on the board"));
        }
        self.board_add(entries)
    }

    /// Puts a piece of text (a note, a link) on the board.
    pub fn board_text(&self, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!(tr!("متن خالی است", "The text is empty"));
        }
        if text.len() > TEXT_MAX {
            bail!(tr!("متن خیلی بلند است؛ آن را به شکل فایل بگذار", "The text is too long; put it on the board as a file"));
        }
        self.board_add(vec![BoardEntry { item: self.board_item("text", String::new(), text.to_string(), text.len() as u64, 0), local: None }])
    }

    /// Takes an item off the board on every device.
    pub fn board_remove(&self, id: &str) -> Result<()> {
        let mut doomed = vec![];
        {
            let mut cfg = self.cfg.lock().unwrap();
            let e = cfg.board.iter_mut().find(|e| e.item.id == id && e.item.gone == 0).ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
            bury(e, now(), &mut doomed);
            cfg.save(&self.dir);
        }
        self.board_stop_fetch(id);
        delete_paths(doomed);
        self.changed();
        self.board_announce(None);
        Ok(())
    }

    /// Path of the content on this device, if it is here.
    pub(crate) fn board_path(&self, id: &str) -> Option<PathBuf> {
        self.cfg.lock().unwrap().board.iter().find(|e| e.item.id == id && has_content(e)).and_then(|e| e.local.as_ref()).map(|l| PathBuf::from(&l.path))
    }

    /// Moves a fetched copy to the keep folder. It then stays when the item leaves the board.
    pub async fn board_keep(&self, id: &str) -> Result<()> {
        let (src, name, dest_dir) = {
            let cfg = self.cfg.lock().unwrap();
            let e = cfg.board.iter().find(|e| e.item.id == id && has_content(e)).ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
            let l = e.local.as_ref().expect("has content");
            if l.own || l.kept {
                return Ok(());
            }
            let src = PathBuf::from(&l.path);
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            (src, name, PathBuf::from(&cfg.keep_dir))
        };
        let dest = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            std::fs::create_dir_all(&dest_dir)?;
            let dest = unique_path(&dest_dir, &name);
            move_path(&src, &dest)?;
            Ok(dest)
        })
        .await??;
        {
            let mut cfg = self.cfg.lock().unwrap();
            if let Some(e) = cfg.board.iter_mut().find(|e| e.item.id == id) {
                e.local = Some(BoardLocal { path: dest.to_string_lossy().into_owned(), own: false, kept: true });
            }
            cfg.save(&self.dir);
        }
        self.changed();
        Ok(())
    }

    // ---------------------------------------------------------------- fetching content

    /// Asks a connected device that has the item to send it here.
    pub async fn board_get(&self, id: &str) -> Result<()> {
        {
            let cfg = self.cfg.lock().unwrap();
            let e = cfg.board.iter().find(|e| e.item.id == id && e.item.gone == 0 && e.item.kind == "file").ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
            if has_content(e) {
                return Ok(());
            }
        }
        let holders: Vec<String> = {
            let mut rt = self.rt.lock().unwrap();
            if busy(&rt, id) {
                return Ok(());
            }
            let holders: Vec<String> = rt.board_have.iter().filter(|(p, set)| set.contains(id) && rt.links.contains_key(*p)).map(|(p, _)| p.clone()).collect();
            if holders.is_empty() {
                bail!(tr!("الان هیچ دستگاهِ وصلی این فایل را ندارد", "No connected device has this file right now"));
            }
            // An earlier attempt that stalled is replaced; its sender is turned away if it comes back.
            let stale: Vec<String> = rt.transfers.iter().filter(|t| !t.outgoing && t.board.as_deref() == Some(id)).map(|t| t.id.clone()).collect();
            for tid in &stale {
                rt.accepted.remove(tid);
                rt.refused.insert(tid.clone());
            }
            rt.transfers.retain(|t| !stale.contains(&t.id));
            rt.board_wanted.insert(id.to_string(), Instant::now());
            holders
        };
        self.changed();
        let mut last = anyhow!("no holder");
        for pid in holders {
            match self.board_ask(&pid, id).await {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
        }
        self.rt.lock().unwrap().board_wanted.remove(id);
        self.changed();
        Err(last)
    }

    async fn board_ask(&self, pid: &str, id: &str) -> Result<()> {
        let mut stream = timeout(Duration::from_secs(15), self.open_stream(pid)).await??;
        write_msg(&mut stream, &Req::BoardGet { item: id.to_string() }).await?;
        match timeout(Duration::from_secs(10), read_msg::<_, Resp>(&mut stream)).await?? {
            Resp::Ok => Ok(()),
            Resp::Err { msg } => bail!(remote(&msg)),
            _ => bail!(tr!("پاسخ نامعتبر", "Unexpected reply")),
        }
    }

    /// A paired device asked for the content of an item: send it like any other transfer.
    pub(crate) async fn board_serve(&self, mut stream: Stream, pid: String, item: String) -> Result<()> {
        let Some(path) = self.board_path(&item) else {
            return write_msg(&mut stream, &Resp::Err { msg: "board_gone".into() }).await;
        };
        write_msg(&mut stream, &Resp::Ok).await?;
        let running = |t: &crate::Transfer| matches!(t.state, "connecting" | "asking" | "active" | "waiting");
        let already = self.rt.lock().unwrap().transfers.iter().any(|t| t.outgoing && t.peer_id == pid && t.board.as_deref() == Some(&item) && running(t));
        if !already {
            self.send_as(&pid, vec![path.to_string_lossy().into_owned()], Some(item)).await?;
        }
        Ok(())
    }

    /// The content of an item finished arriving at `src`; give it its place in the board folder.
    pub(crate) async fn board_arrived(&self, id: &str, src: &Path, board_dir: &Path) -> bool {
        let name = {
            let cfg = self.cfg.lock().unwrap();
            cfg.board.iter().find(|e| e.item.id == id && e.item.gone == 0).map(|e| {
                let last = e.item.name.rsplit(['/', '\\']).next().unwrap_or_default();
                clean_rel(last).unwrap_or_else(|| "file".into())
            })
        };
        let Some(name) = name else { return false };
        let dest = unique_path(board_dir, &name);
        if tokio::fs::rename(src, &dest).await.is_err() {
            return false;
        }
        let shown = {
            let mut cfg = self.cfg.lock().unwrap();
            let Some(e) = cfg.board.iter_mut().find(|e| e.item.id == id && e.item.gone == 0) else {
                // Taken off the board while it was arriving.
                drop(cfg);
                delete_paths(vec![dest]);
                return false;
            };
            e.local = Some(BoardLocal { path: dest.to_string_lossy().into_owned(), own: false, kept: false });
            let shown = e.item.name.clone();
            cfg.save(&self.dir);
            shown
        };
        self.emit(json!({ "type": "board_got", "name": shown }));
        self.board_announce(None);
        true
    }

    pub(crate) fn board_unwant(&self, item: &Option<String>) {
        if let Some(id) = item {
            self.rt.lock().unwrap().board_wanted.remove(id);
            self.changed();
            self.board_auto();
        }
    }

    /// An always-on device fetches whatever it does not have yet, a few at a time.
    /// So does a device that serves a phone through the browser link: the phone can only take what is here.
    pub(crate) fn board_auto(&self) {
        let missing: Vec<String> = {
            let cfg = self.cfg.lock().unwrap();
            if !cfg.always_on && !cfg.guest_on {
                return;
            }
            cfg.board.iter().filter(|e| e.item.gone == 0 && e.item.kind == "file" && !has_content(e)).map(|e| e.item.id.clone()).collect()
        };
        let pick: Vec<String> = {
            let rt = self.rt.lock().unwrap();
            let running = missing.iter().filter(|id| busy(&rt, id)).count();
            let online: HashSet<&String> = rt.board_have.iter().filter(|(p, _)| rt.links.contains_key(*p)).flat_map(|(_, set)| set.iter()).collect();
            missing.iter().filter(|id| !busy(&rt, id) && online.contains(id)).take(AUTO_PARALLEL.saturating_sub(running)).cloned().collect()
        };
        for id in pick {
            let c = self.clone();
            tokio::spawn(async move {
                let _ = c.board_get(&id).await;
            });
        }
    }

    pub(crate) async fn board_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            self.board_auto();
        }
    }

    /// The board as the UI shows it, newest first.
    pub(crate) fn board_view(&self, cfg: &Config, rt: &Runtime) -> Vec<Value> {
        let peer_name = |id: &str| cfg.peers.iter().find(|p| p.id == id).map(|p| p.name.clone());
        cfg.board
            .iter()
            .rev()
            .filter(|e| e.item.gone == 0)
            .map(|e| {
                let it = &e.item;
                let here = has_content(e);
                let sources: Vec<String> =
                    rt.board_have.iter().filter(|(p, set)| set.contains(&it.id) && rt.links.contains_key(*p)).filter_map(|(p, _)| peer_name(p)).collect();
                json!({
                    "id": it.id, "kind": it.kind, "name": it.name, "text": it.text, "size": it.size, "files": it.files,
                    "mine": it.from == self.id,
                    // The current name of a device we know, otherwise the name it had when it shared this.
                    "from": peer_name(&it.from).unwrap_or_else(|| it.from_name.clone()),
                    "at": it.at,
                    "state": if it.kind == "text" || here { "here" } else if busy(rt, &it.id) { "fetching" } else { "away" },
                    "own": e.local.as_ref().is_some_and(|l| l.own),
                    "kept": e.local.as_ref().is_some_and(|l| l.kept),
                    "path": e.local.as_ref().map(|l| l.path.clone()),
                    "sources": sources,
                })
            })
            .collect()
    }
}
