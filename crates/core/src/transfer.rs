//! Sending and receiving files, and the inbox they land in.
//!
//! Files are streamed in fixed-size chunks, so memory use does not depend on file size.
//! Every file ends with its BLAKE3 hash. An interrupted transfer leaves partial files
//! behind and continues from their length on the next attempt.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use serde_json::json;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    i18n::{remote, tr},
    model::InboxItem,
    new_id, now,
    proto::{read_msg, write_msg, Done, FileEntry, Offer, Req, Resp},
    unique_path, Core, PendingOffer, Stream, Transfer,
};

const CHUNK: usize = 1 << 20;
const PARTIAL_DIR: &str = ".partial";

struct OutFile {
    abs: PathBuf,
    rel: String,
    size: u64,
}

enum SendErr {
    /// The connection failed; worth retrying when the peer is back.
    Net(String),
    /// Retrying cannot help.
    Fatal(String),
    Rejected(String),
}

fn net<E: std::fmt::Display>(e: E) -> SendErr {
    SendErr::Net(e.to_string())
}

/// Turns the selected paths into a flat file list. Folders keep their structure under their own name.
pub(crate) fn collect_sizes(paths: Vec<String>) -> Result<(usize, u64)> {
    let files = collect(paths)?;
    Ok((files.len(), files.iter().map(|f| f.size).sum()))
}

fn collect(paths: Vec<String>) -> Result<Vec<OutFile>> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<OutFile>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(meta) = e.path().symlink_metadata() else { continue };
            let rel = format!("{rel}/{}", e.file_name().to_string_lossy());
            if meta.is_dir() {
                walk(&e.path(), &rel, out);
            } else if meta.is_file() {
                out.push(OutFile { abs: e.path(), rel, size: meta.len() });
            }
        }
    }
    let mut out = vec![];
    let mut tops = HashSet::new();
    for p in paths {
        let path = PathBuf::from(&p);
        let meta = std::fs::metadata(&path).map_err(|_| anyhow!(tr!("پیدا نشد: {}", "Not found: {}", p)))?;
        let base = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| anyhow!(tr!("مسیر نامعتبر: {}", "Not a valid path: {}", p)))?;
        let mut top = base.clone();
        let mut n = 2;
        while !tops.insert(top.clone()) {
            top = format!("{base} ({n})");
            n += 1;
        }
        if meta.is_dir() {
            walk(&path, &top, &mut out);
        } else {
            out.push(OutFile { abs: path, rel: top, size: meta.len() });
        }
    }
    Ok(out)
}

/// Makes a path received from another device safe to create below the inbox.
pub(crate) fn clean_rel(rel: &str) -> Option<String> {
    let mut parts = vec![];
    for comp in rel.split(['/', '\\']) {
        let c: String = comp.chars().map(|ch| if ch.is_control() || "<>:\"|?*".contains(ch) { '_' } else { ch }).collect();
        let c = c.trim().trim_end_matches(['.', ' ']).to_string();
        if c.is_empty() || c == "." || c == ".." {
            return None;
        }
        parts.push(c);
    }
    (!parts.is_empty() && parts[0] != PARTIAL_DIR).then(|| parts.join("/"))
}

fn top_of(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

fn summary(rels: &[String]) -> String {
    let mut tops: Vec<&str> = vec![];
    for r in rels {
        let t = top_of(r);
        if !tops.contains(&t) {
            tops.push(t);
        }
    }
    match tops.len() {
        0 => String::new(),
        1 => tops[0].to_string(),
        n => tr!("{} و {} مورد دیگر", "{} and {} more", tops[0], n - 1),
    }
}

impl Core {
    fn add_transfer(&self, t: Transfer) {
        let mut rt = self.rt.lock().unwrap();
        rt.transfers.retain(|x| x.id != t.id);
        let finished = |x: &Transfer| !matches!(x.state, "connecting" | "asking" | "active" | "waiting");
        while rt.transfers.len() >= 40 {
            match rt.transfers.iter().position(finished) {
                Some(i) => drop(rt.transfers.remove(i)),
                None => break,
            }
        }
        rt.transfers.push(t);
    }

    fn set_state(&self, id: &str, state: &'static str, error: &str) {
        if let Some(t) = self.rt.lock().unwrap().transfers.iter_mut().find(|t| t.id == id) {
            t.state = state;
            t.error = error.to_string();
            t.speed = 0.0;
        }
        self.changed();
    }

    fn set_done(&self, id: &str, done: u64) {
        if let Some(t) = self.rt.lock().unwrap().transfers.iter_mut().find(|t| t.id == id) {
            t.done = done;
            t.mark_done = done;
            t.mark_time = Instant::now();
        }
    }

    /// Counts transferred bytes and tells the UI at most a few times per second.
    fn progress(&self, id: &str, n: usize) {
        let ev = {
            let mut rt = self.rt.lock().unwrap();
            let Some(t) = rt.transfers.iter_mut().find(|t| t.id == id) else { return };
            t.done += n as u64;
            let dt = t.mark_time.elapsed();
            if dt < Duration::from_millis(200) && t.done < t.total {
                return;
            }
            let rate = (t.done - t.mark_done) as f64 / dt.as_secs_f64().max(0.001);
            t.speed = if t.speed == 0.0 { rate } else { t.speed * 0.6 + rate * 0.4 };
            t.mark_time = Instant::now();
            t.mark_done = t.done;
            json!({ "type": "progress", "id": t.id, "done": t.done, "total": t.total, "speed": t.speed })
        };
        self.emit(ev);
    }

    pub fn cancel(&self, id: &str) {
        {
            let mut rt = self.rt.lock().unwrap();
            let Some(t) = rt.transfers.iter_mut().find(|t| t.id == id) else { return };
            t.cancel.cancel();
            if !t.outgoing {
                // An interrupted incoming transfer has no running task left to notice the cancel.
                if t.state == "waiting" {
                    t.state = "cancelled";
                }
                let board = t.board.clone();
                rt.accepted.remove(id);
                rt.refused.insert(id.to_string());
                if let Some(item) = board {
                    rt.board_wanted.remove(&item);
                }
            }
        }
        self.changed();
    }

    // ---------------------------------------------------------------- sending

    pub async fn send(&self, peer_id: &str, paths: Vec<String>) -> Result<String> {
        self.send_as(peer_id, paths, None).await
    }

    /// `board` names the shared-board item whose content this is, when the peer asked for one.
    pub(crate) async fn send_as(&self, peer_id: &str, paths: Vec<String>, board: Option<String>) -> Result<String> {
        if !self.is_paired(peer_id) {
            bail!(remote("not_paired"));
        }
        let files = tokio::task::spawn_blocking(move || collect(paths)).await??;
        if files.is_empty() {
            bail!(tr!("چیزی برای فرستادن نیست", "There is nothing to send"));
        }
        let id = new_id();
        let rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
        let cancel = CancellationToken::new();
        self.add_transfer(Transfer {
            id: id.clone(),
            peer_id: peer_id.to_string(),
            outgoing: true,
            name: summary(&rels),
            count: files.len(),
            total: files.iter().map(|f| f.size).sum(),
            done: 0,
            state: "connecting",
            error: String::new(),
            speed: 0.0,
            at: now(),
            board: board.clone(),
            cancel: cancel.clone(),
            mark_time: Instant::now(),
            mark_done: 0,
        });
        self.changed();
        let (c, tid, pid) = (self.clone(), id.clone(), peer_id.to_string());
        tokio::spawn(async move { c.send_job(tid, pid, Arc::new(files), board, cancel).await });
        Ok(id)
    }

    async fn send_job(&self, id: String, pid: String, files: Arc<Vec<OutFile>>, board: Option<String>, cancel: CancellationToken) {
        loop {
            let res = tokio::select! {
                r = self.send_attempt(&id, &pid, &files, &board) => r,
                _ = cancel.cancelled() => return self.set_state(&id, "cancelled", ""),
            };
            match res {
                Ok(failed) if failed.is_empty() => return self.set_state(&id, "done", ""),
                Ok(failed) => return self.set_state(&id, "failed", &tr!("{} فایل سالم نرسید", "{} file(s) did not arrive intact", failed.len())),
                Err(SendErr::Rejected(reason)) => return self.set_state(&id, "rejected", &reason),
                Err(SendErr::Fatal(e)) => return self.set_state(&id, "failed", &e),
                Err(SendErr::Net(e)) => {
                    // Keep the job and continue from where it stopped once the peer is reachable again.
                    self.set_state(&id, "waiting", &e);
                    tokio::select! {
                        _ = cancel.cancelled() => return self.set_state(&id, "cancelled", ""),
                        _ = async { tokio::time::sleep(Duration::from_secs(2)).await; self.wait_online(&pid).await } => {}
                    }
                }
            }
        }
    }

    async fn send_attempt(&self, id: &str, pid: &str, files: &[OutFile], board: &Option<String>) -> Result<Vec<usize>, SendErr> {
        if !self.is_paired(pid) {
            return Err(SendErr::Fatal(tr!("اتصال این دستگاه حذف شده است", "The connection to this device was removed")));
        }
        self.set_state(id, "connecting", "");
        let mut stream = self.open_stream(pid).await.map_err(net)?;
        let offer = Offer { id: id.to_string(), files: files.iter().map(|f| FileEntry { rel: f.rel.clone(), size: f.size }).collect(), board: board.clone() };
        write_msg(&mut stream, &Req::Offer { offer }).await.map_err(net)?;
        self.set_state(id, "asking", "");
        let resume = match timeout(Duration::from_secs(600), read_msg::<_, Resp>(&mut stream)).await {
            Ok(Ok(Resp::Accept { resume })) if resume.len() == files.len() => resume,
            Ok(Ok(Resp::Reject { reason })) => return Err(SendErr::Rejected(remote(&reason))),
            Ok(Ok(Resp::Err { msg })) => return Err(SendErr::Fatal(remote(&msg))),
            Ok(Ok(_)) => return Err(SendErr::Fatal(tr!("پاسخ نامعتبر", "Unexpected reply"))),
            Ok(Err(e)) => return Err(net(e)),
            Err(_) => return Err(SendErr::Rejected(tr!("پاسخی داده نشد", "No answer was given"))),
        };
        self.set_done(id, files.iter().zip(&resume).map(|(f, r)| (*r).min(f.size)).sum());
        self.set_state(id, "active", "");

        let changed = |f: &OutFile| SendErr::Fatal(tr!("فایل هنگام ارسال تغییر کرد: {}", "The file changed while it was being sent: {}", f.rel));
        let mut buf = vec![0u8; CHUNK];
        for (f, offset) in files.iter().zip(resume) {
            let offset = offset.min(f.size);
            let mut file = File::open(&f.abs).await.map_err(|e| SendErr::Fatal(format!("{}: {e}", f.rel)))?;
            let mut hasher = blake3::Hasher::new();
            // The receiver already has this part; it only goes through the hash.
            let mut left = offset;
            while left > 0 {
                let n = file.read(&mut buf[..left.min(CHUNK as u64) as usize]).await.map_err(|_| changed(f))?;
                if n == 0 {
                    return Err(changed(f));
                }
                hasher.update(&buf[..n]);
                left -= n as u64;
            }
            let mut left = f.size - offset;
            while left > 0 {
                let n = file.read(&mut buf[..left.min(CHUNK as u64) as usize]).await.map_err(|_| changed(f))?;
                if n == 0 {
                    return Err(changed(f));
                }
                hasher.update(&buf[..n]);
                stream.write_all(&buf[..n]).await.map_err(net)?;
                left -= n as u64;
                self.progress(id, n);
            }
            stream.write_all(hasher.finalize().as_bytes()).await.map_err(net)?;
        }
        stream.flush().await.map_err(net)?;
        let done: Done = timeout(Duration::from_secs(300), read_msg(&mut stream)).await.map_err(net)?.map_err(net)?;
        Ok(done.failed)
    }

    // ---------------------------------------------------------------- receiving

    fn auto_accept(&self, pid: &str, total: u64) -> bool {
        let net = self.saved_net_id();
        let cfg = self.cfg.lock().unwrap();
        let peer_mode = cfg.peers.iter().find(|p| p.id == pid).and_then(|p| p.accept_mode.clone());
        let net_mode = net.and_then(|id| cfg.networks.iter().find(|n| n.id == id).and_then(|n| n.accept_mode.clone()));
        // Most specific setting wins: this sender, then this network, then the device default.
        let mode = peer_mode.or(net_mode).unwrap_or_else(|| cfg.accept_mode.clone());
        mode == "auto" && (cfg.auto_max == 0 || total <= cfg.auto_max)
    }

    pub(crate) async fn receive(&self, mut stream: Stream, pid: String, offer: Offer) -> Result<()> {
        let rels: Option<Vec<String>> = offer.files.iter().map(|f| clean_rel(&f.rel)).collect();
        let rels = match rels {
            Some(r) if !r.is_empty() && r.iter().collect::<HashSet<_>>().len() == r.len() => r,
            _ => return write_msg(&mut stream, &Resp::Err { msg: "bad_list".into() }).await,
        };
        let sizes: Vec<u64> = offer.files.iter().map(|f| f.size).collect();
        let total: u64 = sizes.iter().sum();
        let name = summary(&rels);
        let from = self.peer_name(&pid);

        if self.rt.lock().unwrap().refused.contains(&offer.id) {
            return write_msg(&mut stream, &Resp::Reject { reason: "cancelled".into() }).await;
        }
        // Content of a board item counts only if this device asked for it; otherwise it is an ordinary offer.
        let board = offer.board.clone().filter(|b| self.rt.lock().unwrap().board_wanted.contains_key(b));
        let known = board.is_some() || self.rt.lock().unwrap().accepted.contains(&offer.id);
        if !known && !self.auto_accept(&pid, total) {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.rt.lock().unwrap().offers.insert(
                offer.id.clone(),
                PendingOffer { peer_id: pid.clone(), name: name.clone(), count: rels.len(), total, tx: Some(tx) },
            );
            self.emit(json!({ "type": "incoming", "from": from, "name": name }));
            self.changed();
            let mut probe = [0u8; 1];
            let ok = tokio::select! {
                r = timeout(Duration::from_secs(590), rx) => matches!(r, Ok(Ok(true))),
                // The sender stays silent while we decide, so any read result means it went away.
                _ = stream.read(&mut probe) => false,
            };
            self.rt.lock().unwrap().offers.remove(&offer.id);
            self.changed();
            if !ok {
                let _ = write_msg(&mut stream, &Resp::Reject { reason: "declined".into() }).await;
                return Ok(());
            }
        }
        self.rt.lock().unwrap().accepted.insert(offer.id.clone());

        let inbox = {
            let cfg = self.cfg.lock().unwrap();
            PathBuf::from(if board.is_some() { cfg.board_dir.clone() } else { cfg.inbox_dir.clone() })
        };
        // Keyed by content list, so re-sending the same files continues the same partial copy.
        let mut key = blake3::Hasher::new();
        key.update(pid.as_bytes());
        for (rel, size) in rels.iter().zip(&sizes) {
            key.update(rel.as_bytes());
            key.update(&size.to_le_bytes());
        }
        let root = inbox.join(PARTIAL_DIR).join(&key.finalize().to_hex()[..16]);
        let mut resume = Vec::with_capacity(rels.len());
        for (rel, size) in rels.iter().zip(&sizes) {
            let len = tokio::fs::metadata(root.join(rel)).await.map(|m| m.len()).unwrap_or(0);
            resume.push(if len <= *size { len } else { 0 });
        }
        write_msg(&mut stream, &Resp::Accept { resume: resume.clone() }).await?;

        let cancel = CancellationToken::new();
        self.add_transfer(Transfer {
            id: offer.id.clone(),
            peer_id: pid.clone(),
            outgoing: false,
            name: name.clone(),
            count: rels.len(),
            total,
            done: resume.iter().sum(),
            state: "active",
            error: String::new(),
            speed: 0.0,
            at: now(),
            board: board.clone(),
            cancel: cancel.clone(),
            mark_time: Instant::now(),
            mark_done: resume.iter().sum(),
        });
        self.changed();

        let body = self.receive_body(&mut stream, &offer.id, &root, &rels, &sizes, &resume);
        let res = tokio::select! {
            r = body => r,
            _ = cancel.cancelled() => {
                let _ = tokio::fs::remove_dir_all(&root).await;
                self.set_state(&offer.id, "cancelled", "");
                self.board_unwant(&board);
                return Ok(());
            }
        };
        let failed = match res {
            Ok(failed) => failed,
            Err(e) => {
                self.set_state(&offer.id, "waiting", &e.to_string());
                return Ok(());
            }
        };

        if let Some(item) = &board {
            // A board item is a single file or folder; it lands in the board folder under the item's own name.
            let src = root.join(top_of(&rels[0]));
            let landed = failed.is_empty() && self.board_arrived(item, &src, &inbox).await;
            let _ = tokio::fs::remove_dir_all(&root).await;
            self.rt.lock().unwrap().accepted.remove(&offer.id);
            let _ = write_msg(&mut stream, &Done { failed: failed.clone() }).await;
            if landed {
                self.set_state(&offer.id, "done", "");
            } else {
                self.set_state(&offer.id, "failed", &tr!("{} فایل سالم نرسید", "{} file(s) did not arrive intact", failed.len().max(1)));
            }
            self.board_unwant(&board);
            return Ok(());
        }

        // Move finished entries out of the partial area, one inbox item per top-level file or folder.
        let mut items = vec![];
        let mut seen: Vec<&str> = vec![];
        for rel in &rels {
            let top = top_of(rel);
            if seen.contains(&top) {
                continue;
            }
            seen.push(top);
            let src = root.join(top);
            if !src.exists() {
                continue;
            }
            let dest = unique_path(&inbox, top);
            if tokio::fs::rename(&src, &dest).await.is_err() {
                continue;
            }
            let inside = |i: &usize| top_of(&rels[*i]) == top && !failed.contains(i);
            items.push(InboxItem {
                id: new_id(),
                name: dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                path: dest.to_string_lossy().into_owned(),
                size: (0..rels.len()).filter(inside).map(|i| sizes[i]).sum(),
                files: (0..rels.len()).filter(inside).count(),
                from: from.clone(),
                at: now(),
                state: "new".into(),
            });
        }
        let _ = tokio::fs::remove_dir_all(&root).await;
        self.add_inbox(items);
        self.rt.lock().unwrap().accepted.remove(&offer.id);
        let _ = write_msg(&mut stream, &Done { failed: failed.clone() }).await;
        if failed.is_empty() {
            self.set_state(&offer.id, "done", "");
            self.emit(json!({ "type": "received", "from": from, "name": name }));
        } else {
            self.set_state(&offer.id, "failed", &tr!("{} فایل سالم نرسید", "{} file(s) did not arrive intact", failed.len()));
        }
        Ok(())
    }

    async fn receive_body(&self, stream: &mut Stream, id: &str, root: &Path, rels: &[String], sizes: &[u64], resume: &[u64]) -> Result<Vec<usize>> {
        let mut buf = vec![0u8; CHUNK];
        let mut failed = vec![];
        for (i, rel) in rels.iter().enumerate() {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut file = tokio::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).await?;
            let mut hasher = blake3::Hasher::new();
            let mut left = resume[i];
            if left == 0 {
                file.set_len(0).await?;
            }
            while left > 0 {
                let n = file.read(&mut buf[..left.min(CHUNK as u64) as usize]).await?;
                if n == 0 {
                    bail!("partial file changed");
                }
                hasher.update(&buf[..n]);
                left -= n as u64;
            }
            let mut left = sizes[i] - resume[i];
            while left > 0 {
                // TLS hands data over in 16 KB records; gather a full chunk so the disk sees few, large writes.
                let want = left.min(CHUNK as u64) as usize;
                let mut n = 0;
                let mut closed = false;
                while n < want {
                    match stream.read(&mut buf[n..want]).await {
                        Ok(0) | Err(_) => {
                            closed = true;
                            break;
                        }
                        Ok(got) => n += got,
                    }
                }
                hasher.update(&buf[..n]);
                if let Err(e) = file.write_all(&buf[..n]).await {
                    bail!(tr!("نوشتن روی دیسک ناموفق بود: {}", "Writing to disk failed: {}", e));
                }
                left -= n as u64;
                self.progress(id, n);
                if closed {
                    // Keep what arrived, so the next attempt continues from here.
                    file.flush().await.ok();
                    bail!(tr!("اتصال قطع شد", "The connection dropped"));
                }
            }
            file.flush().await?;
            drop(file);
            let mut sent = [0u8; 32];
            stream.read_exact(&mut sent).await?;
            if sent != *hasher.finalize().as_bytes() {
                failed.push(i);
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
        Ok(failed)
    }

    pub fn offer_decide(&self, id: &str, ok: bool) {
        if let Some(tx) = self.rt.lock().unwrap().offers.get_mut(id).and_then(|o| o.tx.take()) {
            let _ = tx.send(ok);
        }
    }

    // ---------------------------------------------------------------- inbox

    pub(crate) fn add_inbox(&self, items: Vec<InboxItem>) {
        if items.is_empty() {
            return;
        }
        let mut cfg = self.cfg.lock().unwrap();
        cfg.inbox.extend(items);
        let extra = cfg.inbox.len().saturating_sub(500);
        cfg.inbox.drain(..extra);
        cfg.save(&self.dir);
    }

    /// Moves an inbox item to `dir` (the default keep folder when `None`).
    pub async fn inbox_keep(&self, id: &str, dir: Option<String>) -> Result<()> {
        let (item, dest_dir) = {
            let cfg = self.cfg.lock().unwrap();
            let item = cfg.inbox.iter().find(|i| i.id == id).cloned().ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
            (item, PathBuf::from(dir.unwrap_or_else(|| cfg.keep_dir.clone())))
        };
        let src = PathBuf::from(&item.path);
        let name = item.name.clone();
        let dest = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            std::fs::create_dir_all(&dest_dir)?;
            let dest = unique_path(&dest_dir, &name);
            move_path(&src, &dest)?;
            Ok(dest)
        })
        .await??;
        let mut cfg = self.cfg.lock().unwrap();
        if let Some(i) = cfg.inbox.iter_mut().find(|i| i.id == id) {
            i.path = dest.to_string_lossy().into_owned();
            i.state = "kept".into();
        }
        cfg.save(&self.dir);
        drop(cfg);
        self.changed();
        Ok(())
    }

    /// Removes an item from the inbox. The file itself is deleted only while it is still undecided;
    /// a kept file belongs to the user and is left alone.
    pub async fn inbox_delete(&self, id: &str) -> Result<()> {
        let item = {
            let mut cfg = self.cfg.lock().unwrap();
            let pos = cfg.inbox.iter().position(|i| i.id == id).ok_or_else(|| anyhow!(tr!("پیدا نشد", "Not found")))?;
            let item = cfg.inbox.remove(pos);
            cfg.save(&self.dir);
            item
        };
        if item.state == "new" {
            let path = PathBuf::from(item.path);
            tokio::task::spawn_blocking(move || {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            })
            .await?;
        }
        self.changed();
        Ok(())
    }

    /// Deletes partial downloads nobody has continued for a week.
    pub(crate) fn sweep_partials(&self) {
        let dirs = {
            let cfg = self.cfg.lock().unwrap();
            [cfg.inbox_dir.clone(), cfg.board_dir.clone()]
        };
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(PathBuf::from(dir).join(PARTIAL_DIR)) else { continue };
            for e in rd.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d > Duration::from_secs(7 * 86400));
                if old {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

/// Rename, falling back to copy-then-delete when the destination is on another drive.
pub(crate) fn move_path(src: &Path, dest: &Path) -> Result<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    copy_tree(src, dest)?;
    if src.is_dir() {
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::remove_file(src)?;
    }
    Ok(())
}
