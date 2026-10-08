//! DastBeDast core: identity, pairing, presence, transfers, inbox.
//!
//! The core owns all I/O and runs on its own tasks. A front end only calls
//! [`Core::call`] and listens on [`Core::subscribe`], so it can never be blocked by a transfer.

mod api;
mod board;
mod discovery;
mod guest;
mod i18n;
pub mod model;
mod netid;
pub mod proto;
mod tls;
mod transfer;

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, oneshot, Notify},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use i18n::{remote, tr};
use model::{Config, Network, Peer, PeerAddr};
pub use transfer::adopt_file;
use proto::{read_msg, write_msg, Info, LinkMsg, PairMsg, Req, Resp};

pub type Stream = tokio_rustls::TlsStream<TcpStream>;

#[derive(Default)]
pub struct Options {
    /// Listen on this port instead of the configured one. 0 picks a free port.
    pub port: Option<u16>,
    /// Announce this device on the local network and listen for others.
    pub discovery: bool,
    /// Where received files go on a first start, for platforms without a usual home folder.
    pub files_dir: Option<PathBuf>,
}

pub(crate) struct LinkHandle {
    lid: u64,
    tx: mpsc::UnboundedSender<LinkMsg>,
    /// We dialed the peer, so its address is known to be reachable from here.
    outbound: bool,
    addr: SocketAddr,
}

pub(crate) struct NearbyDev {
    info: Info,
    addr: SocketAddr,
    seen: Instant,
    ttl: Duration,
}

pub(crate) struct PairPrompt {
    info: Info,
    sas: String,
    initiator: bool,
    waiting: bool,
    tx: Option<oneshot::Sender<bool>>,
}

pub(crate) struct PendingOffer {
    peer_id: String,
    name: String,
    count: usize,
    total: u64,
    tx: Option<oneshot::Sender<bool>>,
}

pub(crate) struct Transfer {
    pub id: String,
    pub peer_id: String,
    pub outgoing: bool,
    pub name: String,
    pub count: usize,
    pub total: u64,
    pub done: u64,
    pub state: &'static str,
    pub error: String,
    pub speed: f64,
    pub at: u64,
    /// The shared-board item this transfer carries, if any.
    pub board: Option<String>,
    pub cancel: CancellationToken,
    pub mark_time: Instant,
    pub mark_done: u64,
}

#[derive(Default)]
pub(crate) struct Runtime {
    net: Option<netid::NetInfo>,
    net_suggest: Option<String>,
    links: HashMap<String, Vec<LinkHandle>>,
    dialing: HashSet<String>,
    nearby: HashMap<String, NearbyDev>,
    pair_prompts: HashMap<String, PairPrompt>,
    offers: HashMap<String, PendingOffer>,
    transfers: Vec<Transfer>,
    callbacks: HashMap<String, (String, oneshot::Sender<Stream>)>,
    /// Transfers the user already accepted; a retry of the same transfer is not asked about again.
    accepted: HashSet<String>,
    /// Transfers the user cancelled on this side; the sender's automatic retry is turned away.
    refused: HashSet<String>,
    pairing_until: Option<Instant>,
    /// Board items each connected peer can hand out.
    board_have: HashMap<String, HashSet<String>>,
    /// Board items this device has asked for, and when.
    board_wanted: HashMap<String, Instant>,
    guest: Option<guest::Guest>,
    next_lid: u64,
}

pub struct Inner {
    dir: PathBuf,
    pub id: String,
    pub port: u16,
    tls: tls::Tls,
    cfg: Mutex<Config>,
    rt: Mutex<Runtime>,
    events: broadcast::Sender<Value>,
    /// Wakes the link loop: something about reachability may have changed.
    wake: Notify,
    /// Fires when a link to any peer comes up.
    online: Notify,
}

#[derive(Clone)]
pub struct Core(Arc<Inner>);

impl std::ops::Deref for Core {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// Where settings and the device key live unless the caller picks another place.
pub fn default_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("dastbedast")
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

/// `name (2).ext` style path that does not exist yet.
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("unbounded")
}

fn sas_code(initiator: &str, responder: &str, nonce_a: &str, nonce_b: &str) -> String {
    let h = blake3::hash(format!("{initiator}|{responder}|{nonce_a}|{nonce_b}").as_bytes());
    let n = u32::from_be_bytes(h.as_bytes()[..4].try_into().expect("4 bytes")) % 1_000_000;
    format!("{n:06}")
}

struct Candidate {
    host: String,
    port: u16,
    discovered: bool,
}

impl Core {
    pub async fn start(dir: PathBuf, opts: Options) -> Result<Core> {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let tls = tls::Tls::load_or_create(&dir)?;
        let fresh = !dir.join("config.json").exists();
        let mut cfg = Config::load(&dir);
        if let Some(base) = opts.files_dir.as_ref().filter(|_| fresh) {
            cfg.inbox_dir = base.join("Inbox").to_string_lossy().into_owned();
            cfg.keep_dir = base.join("Files").to_string_lossy().into_owned();
            cfg.board_dir = base.join("Board").to_string_lossy().into_owned();
        }
        if cfg.name.is_empty() {
            cfg.name = model::default_name();
        }
        i18n::set(&cfg.lang);
        board::prune(&mut cfg);
        let want = opts.port.unwrap_or(cfg.port);
        let listener = TcpListener::bind(("0.0.0.0", want))
            .await
            .with_context(|| tr!("پورت {} در دسترس نیست؛ شاید برنامه از قبل باز است", "Port {} is not available; the app may already be running", want))?;
        let port = listener.local_addr()?.port();
        std::fs::create_dir_all(&cfg.inbox_dir).ok();
        cfg.save(&dir);

        let (events, _) = broadcast::channel(256);
        let core = Core(Arc::new(Inner {
            dir,
            id: tls.id.clone(),
            port,
            tls,
            cfg: Mutex::new(cfg),
            rt: Mutex::new(Runtime::default()),
            events,
            wake: Notify::new(),
            online: Notify::new(),
        }));
        core.rt.lock().unwrap().net = netid::detect();
        core.sweep_partials();

        let c = core.clone();
        tokio::spawn(async move { c.accept_loop(listener).await });
        let c = core.clone();
        tokio::spawn(async move { c.link_loop().await });
        let c = core.clone();
        tokio::spawn(async move { c.net_loop().await });
        let c = core.clone();
        tokio::spawn(async move { c.board_loop().await });
        if opts.discovery {
            if let Err(e) = discovery::spawn(core.clone()) {
                eprintln!("discovery disabled: {e:#}");
            }
        }
        Ok(core)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub(crate) fn emit(&self, v: Value) {
        let _ = self.events.send(v);
    }

    pub(crate) fn changed(&self) {
        self.emit(json!({ "type": "changed" }));
    }

    pub(crate) fn toast(&self, kind: &str, msg: impl Into<String>) {
        self.emit(json!({ "type": "toast", "kind": kind, "msg": msg.into() }));
    }

    pub(crate) fn info(&self) -> Info {
        Info { name: self.cfg.lock().unwrap().name.clone(), kind: model::device_kind().into(), port: self.port, v: proto::PROTO_VERSION }
    }

    pub(crate) fn is_paired(&self, id: &str) -> bool {
        self.cfg.lock().unwrap().peers.iter().any(|p| p.id == id)
    }

    pub(crate) fn peer_name(&self, id: &str) -> String {
        self.cfg.lock().unwrap().peers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default()
    }

    fn has_link(&self, id: &str) -> bool {
        self.rt.lock().unwrap().links.get(id).is_some_and(|v| !v.is_empty())
    }

    /// Id of the current network, only if the user has saved it.
    pub(crate) fn saved_net_id(&self) -> Option<String> {
        let id = self.rt.lock().unwrap().net.as_ref()?.id.clone();
        self.cfg.lock().unwrap().networks.iter().any(|n| n.id == id).then_some(id)
    }

    pub(crate) fn pairing_open(&self) -> bool {
        self.rt.lock().unwrap().pairing_until.is_some_and(|t| t > Instant::now())
    }

    /// Discovery is active on saved networks, and anywhere while the user has "new connection" open.
    pub(crate) fn discovery_active(&self) -> bool {
        self.pairing_open() || self.saved_net_id().is_some()
    }

    /// Saves the current network under its suggested name if it is not saved yet.
    fn ensure_network(&self) -> Option<String> {
        let (id, suggest) = {
            let rt = self.rt.lock().unwrap();
            (rt.net.as_ref()?.id.clone(), rt.net_suggest.clone())
        };
        let mut cfg = self.cfg.lock().unwrap();
        if !cfg.networks.iter().any(|n| n.id == id) {
            let name = suggest.unwrap_or_else(|| tr!("شبکهٔ {}", "Network {}", cfg.networks.len() + 1));
            cfg.networks.push(Network { id: id.clone(), name, accept_mode: None });
            cfg.save(&self.dir);
        }
        Some(id)
    }

    // ---------------------------------------------------------------- connections

    pub(crate) async fn dial(&self, host: &str, port: u16) -> Result<(Stream, String, SocketAddr)> {
        let mut addrs: Vec<SocketAddr> = timeout(Duration::from_secs(4), tokio::net::lookup_host((host, port))).await??.collect();
        addrs.sort_by_key(|a| !a.is_ipv4());
        let addr = *addrs.first().ok_or_else(|| anyhow!(tr!("آدرس پیدا نشد", "Address not found")))?;
        let tcp = timeout(Duration::from_secs(3), TcpStream::connect(addr)).await??;
        tcp.set_nodelay(true).ok();
        let stream: Stream = timeout(Duration::from_secs(8), self.tls.connector.connect(tls::server_name(), tcp)).await??.into();
        let pid = tls::peer_id(&stream)?;
        Ok((stream, pid, addr))
    }

    async fn accept_loop(&self, listener: TcpListener) {
        loop {
            let Ok((tcp, addr)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };
            let c = self.clone();
            tokio::spawn(async move {
                let _ = c.handle_conn(tcp, addr).await;
            });
        }
    }

    async fn handle_conn(&self, tcp: TcpStream, addr: SocketAddr) -> Result<()> {
        tcp.set_nodelay(true).ok();
        let mut stream: Stream = timeout(Duration::from_secs(10), self.tls.acceptor.accept(tcp)).await??.into();
        let pid = tls::peer_id(&stream)?;
        let req: Req = timeout(Duration::from_secs(10), read_msg(&mut stream)).await??;
        let paired = self.is_paired(&pid);
        let deny = Resp::Err { msg: "not_paired".into() };
        match req {
            Req::Hello { .. } => write_msg(&mut stream, &Resp::Hello { info: self.info(), paired }).await?,
            Req::Pair { info, commit } => self.pair_respond(stream, pid, addr, info, commit).await?,
            Req::Link { info } => {
                if !paired {
                    return write_msg(&mut stream, &deny).await;
                }
                self.refresh_peer(&pid, &info);
                write_msg(&mut stream, &Resp::Ok).await?;
                self.run_link(stream, pid, false, SocketAddr::new(addr.ip(), info.port)).await;
            }
            Req::Callback { token } => {
                let waiter = {
                    let mut rt = self.rt.lock().unwrap();
                    match rt.callbacks.get(&token) {
                        Some((expect, _)) if *expect == pid => rt.callbacks.remove(&token).map(|(_, tx)| tx),
                        _ => None,
                    }
                };
                if let Some(tx) = waiter {
                    let _ = tx.send(stream);
                }
            }
            Req::Offer { offer } => {
                if !paired {
                    return write_msg(&mut stream, &deny).await;
                }
                self.receive(stream, pid, offer).await?;
            }
            Req::BoardGet { item } => {
                if !paired {
                    return write_msg(&mut stream, &deny).await;
                }
                self.board_serve(stream, pid, item).await?;
            }
            Req::Unpair => self.peer_left(&pid),
        }
        Ok(())
    }

    /// The other device removed this pairing.
    fn peer_left(&self, pid: &str) {
        if self.is_paired(pid) {
            let name = self.peer_name(pid);
            self.forget_peer(pid);
            self.toast("info", tr!("«{}» اتصال را حذف کرد", "“{}” removed the connection", name));
        }
    }

    /// A fresh connection to a paired, online peer, whichever side is able to dial.
    pub(crate) async fn open_stream(&self, pid: &str) -> Result<Stream> {
        let (out_addr, in_tx) = {
            let rt = self.rt.lock().unwrap();
            let links = rt.links.get(pid);
            (
                links.and_then(|v| v.iter().find(|l| l.outbound)).map(|l| l.addr),
                links.and_then(|v| v.first()).map(|l| l.tx.clone()),
            )
        };
        if let Some(addr) = out_addr {
            let (stream, id, _) = self.dial(&addr.ip().to_string(), addr.port()).await?;
            if id != pid {
                bail!(tr!("دستگاه دیگری روی این آدرس جواب داد", "A different device answered at this address"));
            }
            return Ok(stream);
        }
        let Some(tx) = in_tx else { bail!(tr!("دستگاه در دسترس نیست", "The device is not reachable")) };
        let token = new_id();
        let (otx, orx) = oneshot::channel();
        self.rt.lock().unwrap().callbacks.insert(token.clone(), (pid.to_string(), otx));
        let _ = tx.send(LinkMsg::CallMe { token: token.clone() });
        match timeout(Duration::from_secs(12), orx).await {
            Ok(Ok(stream)) => Ok(stream),
            _ => {
                self.rt.lock().unwrap().callbacks.remove(&token);
                bail!(tr!("دستگاه پاسخ نداد", "The device did not answer"))
            }
        }
    }

    /// Raw protocol access for tests that play a misbehaving device.
    #[doc(hidden)]
    pub async fn raw_stream(&self, pid: &str) -> Result<Stream> {
        self.open_stream(pid).await
    }

    #[doc(hidden)]
    pub async fn raw_dial(&self, host: &str, port: u16) -> Result<Stream> {
        Ok(self.dial(host, port).await?.0)
    }

    // ---------------------------------------------------------------- links (presence)

    fn candidates(&self, peer: &Peer) -> Vec<Candidate> {
        let net = self.saved_net_id();
        let mut out = vec![];
        if net.is_some() {
            if let Some(n) = self.rt.lock().unwrap().nearby.get(&peer.id) {
                if n.seen.elapsed() < n.ttl {
                    out.push(Candidate { host: n.addr.ip().to_string(), port: n.addr.port(), discovered: true });
                }
            }
        }
        for a in &peer.addrs {
            if (a.net.is_none() || a.net == net) && !out.iter().any(|c| c.host == a.host && c.port == a.port) {
                out.push(Candidate { host: a.host.clone(), port: a.port, discovered: false });
            }
        }
        out
    }

    async fn link_loop(&self) {
        loop {
            let peers = self.cfg.lock().unwrap().peers.clone();
            for peer in peers {
                let cands = self.candidates(&peer);
                if cands.is_empty() || self.has_link(&peer.id) || !self.rt.lock().unwrap().dialing.insert(peer.id.clone()) {
                    continue;
                }
                let c = self.clone();
                tokio::spawn(async move { c.try_link(peer, cands).await });
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    }

    async fn try_link(&self, peer: Peer, cands: Vec<Candidate>) {
        for cand in cands {
            let Ok((mut stream, pid, addr)) = self.dial(&cand.host, cand.port).await else { continue };
            if pid != peer.id || write_msg(&mut stream, &Req::Link { info: self.info() }).await.is_err() {
                continue;
            }
            match timeout(Duration::from_secs(5), read_msg::<_, Resp>(&mut stream)).await {
                Ok(Ok(Resp::Ok)) => {}
                _ => continue,
            }
            if cand.discovered {
                self.remember_addr(&peer.id, &cand.host, cand.port);
            }
            self.rt.lock().unwrap().dialing.remove(&peer.id);
            self.run_link(stream, pid, true, addr).await;
            return;
        }
        self.rt.lock().unwrap().dialing.remove(&peer.id);
    }

    /// Stores where a peer was found on the current network, replacing the previous address for it.
    fn remember_addr(&self, pid: &str, host: &str, port: u16) {
        let Some(net) = self.saved_net_id() else { return };
        let mut cfg = self.cfg.lock().unwrap();
        let Some(peer) = cfg.peers.iter_mut().find(|p| p.id == pid) else { return };
        let new = PeerAddr { host: host.into(), port, net: Some(net.clone()) };
        if peer.addrs.contains(&new) {
            return;
        }
        peer.addrs.retain(|a| a.net.as_deref() != Some(&net));
        peer.addrs.push(new);
        cfg.save(&self.dir);
    }

    fn refresh_peer(&self, pid: &str, info: &Info) {
        let mut cfg = self.cfg.lock().unwrap();
        if let Some(p) = cfg.peers.iter_mut().find(|p| p.id == pid) {
            if p.name != info.name || p.kind != info.kind {
                p.name = info.name.clone();
                p.kind = info.kind.clone();
                cfg.save(&self.dir);
            }
        }
    }

    async fn run_link(&self, stream: Stream, pid: String, outbound: bool, addr: SocketAddr) {
        let (mut r, mut w) = tokio::io::split(stream);
        let (tx, mut rx) = mpsc::unbounded_channel::<LinkMsg>();
        // Both sides open with their board, so whatever changed while they were apart is exchanged.
        let _ = tx.send(self.board_msg());
        let lid = {
            let mut rt = self.rt.lock().unwrap();
            rt.next_lid += 1;
            let lid = rt.next_lid;
            rt.links.entry(pid.clone()).or_default().push(LinkHandle { lid, tx, outbound, addr });
            lid
        };
        self.changed();
        self.online.notify_waiters();

        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                let msg = tokio::select! {
                    m = rx.recv() => match m { Some(m) => m, None => break },
                    _ = tick.tick() => LinkMsg::Ping,
                };
                if write_msg(&mut w, &msg).await.is_err() {
                    break;
                }
            }
            let _ = w.shutdown().await;
        });

        loop {
            match timeout(Duration::from_secs(16), read_msg::<_, LinkMsg>(&mut r)).await {
                Ok(Ok(LinkMsg::Ping)) => {}
                Ok(Ok(LinkMsg::CallMe { token })) if outbound => {
                    let (c, pid) = (self.clone(), pid.clone());
                    tokio::spawn(async move {
                        let _ = c.call_back(pid, addr, token).await;
                    });
                }
                Ok(Ok(LinkMsg::CallMe { .. })) => {}
                Ok(Ok(LinkMsg::Info { info })) => {
                    self.refresh_peer(&pid, &info);
                    self.changed();
                }
                Ok(Ok(LinkMsg::Board { items, have })) => self.board_merge(&pid, items, have),
                _ => break,
            }
        }
        writer.abort();
        {
            let mut rt = self.rt.lock().unwrap();
            if let Some(v) = rt.links.get_mut(&pid) {
                v.retain(|l| l.lid != lid);
                if v.is_empty() {
                    rt.links.remove(&pid);
                    rt.board_have.remove(&pid);
                }
            }
        }
        self.changed();
        self.wake.notify_one();
    }

    /// The peer asked us to connect because it cannot dial us; it then speaks first on that connection.
    async fn call_back(&self, pid: String, addr: SocketAddr, token: String) -> Result<()> {
        let (mut stream, id, _) = self.dial(&addr.ip().to_string(), addr.port()).await?;
        if id != pid {
            bail!("unexpected device");
        }
        write_msg(&mut stream, &Req::Callback { token }).await?;
        match timeout(Duration::from_secs(30), read_msg(&mut stream)).await?? {
            Req::Offer { offer } => self.receive(stream, pid, offer).await?,
            Req::BoardGet { item } => self.board_serve(stream, pid, item).await?,
            Req::Unpair => self.peer_left(&pid),
            _ => {}
        }
        Ok(())
    }

    pub(crate) async fn wait_online(&self, pid: &str) {
        while !self.has_link(pid) {
            tokio::select! {
                _ = self.online.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            }
        }
    }

    async fn net_loop(&self) {
        let mut first = true;
        loop {
            let cur = tokio::task::spawn_blocking(netid::detect).await.ok().flatten();
            let switched = {
                let mut rt = self.rt.lock().unwrap();
                let switched = rt.net.as_ref().map(|n| &n.id) != cur.as_ref().map(|n| &n.id);
                rt.net = cur.clone();
                switched
            };
            if switched || first {
                first = false;
                let suggest = match &cur {
                    Some(n) => netid::suggest_name(&n.iface).await,
                    None => None,
                };
                self.rt.lock().unwrap().net_suggest = suggest;
                self.changed();
                self.wake.notify_one();
            }
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    }

    /// Tells every connected peer about this device's current name.
    pub(crate) fn announce_info(&self) {
        let info = self.info();
        for links in self.rt.lock().unwrap().links.values() {
            for l in links {
                let _ = l.tx.send(LinkMsg::Info { info: info.clone() });
            }
        }
    }

    // ---------------------------------------------------------------- pairing

    /// Starts pairing with the device listening at `host:port`.
    /// `anywhere` marks the address as static: it is then tried from every network.
    pub async fn pair_addr(&self, host: &str, port: u16, anywhere: bool) -> Result<()> {
        let host = host.trim().to_string();
        let (mut stream, pid, _) = self.dial(&host, port).await.map_err(|_| anyhow!(tr!("به {}:{} وصل نشد", "Could not connect to {}:{}", host, port)))?;
        if pid == self.id {
            bail!(tr!("این آدرسِ همین دستگاه است", "That is this device's own address"));
        }
        let nonce_a = hex::encode(rand::random::<[u8; 16]>());
        let commit = hex::encode(blake3::hash(nonce_a.as_bytes()).as_bytes());
        write_msg(&mut stream, &Req::Pair { info: self.info(), commit }).await?;
        let (info, nonce_b) = match timeout(Duration::from_secs(10), read_msg::<_, Resp>(&mut stream)).await?? {
            Resp::PairNonce { info, nonce } => (info, nonce),
            Resp::Err { msg } => bail!(remote(&msg)),
            _ => bail!(tr!("پاسخ نامعتبر", "Unexpected reply")),
        };
        write_msg(&mut stream, &PairMsg::Reveal { nonce: nonce_a.clone() }).await?;
        let sas = sas_code(&self.id, &pid, &nonce_a, &nonce_b);
        let c = self.clone();
        tokio::spawn(async move {
            if c.pair_confirm(stream, &info, sas, true).await {
                let net = if anywhere { None } else { c.ensure_network() };
                let addr = (anywhere || net.is_some()).then_some(PeerAddr { host, port, net });
                c.add_peer(&pid, &info, addr);
            }
        });
        Ok(())
    }

    async fn pair_respond(&self, mut stream: Stream, pid: String, addr: SocketAddr, info: Info, commit: String) -> Result<()> {
        if !self.pairing_open() || pid == self.id {
            return write_msg(&mut stream, &Resp::Err { msg: "pairing_closed".into() }).await;
        }
        let nonce_b = hex::encode(rand::random::<[u8; 16]>());
        write_msg(&mut stream, &Resp::PairNonce { info: self.info(), nonce: nonce_b.clone() }).await?;
        let PairMsg::Reveal { nonce: nonce_a } = timeout(Duration::from_secs(10), read_msg(&mut stream)).await?? else {
            bail!("expected reveal");
        };
        // The initiator committed to its nonce before seeing ours, so neither side can steer the code.
        if hex::encode(blake3::hash(nonce_a.as_bytes()).as_bytes()) != commit {
            bail!("commitment mismatch");
        }
        let sas = sas_code(&pid, &self.id, &nonce_a, &nonce_b);
        if self.pair_confirm(stream, &info, sas, false).await {
            let ip = addr.ip();
            let local = match ip {
                std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
                std::net::IpAddr::V6(v6) => v6.is_loopback(),
            };
            // An address seen across the internet is the other side's NAT, not something we could dial.
            let saved = if local { self.ensure_network() } else { None };
            let addr = saved.map(|net| PeerAddr { host: ip.to_string(), port: info.port, net: Some(net) });
            self.add_peer(&pid, &info, addr);
        }
        Ok(())
    }

    /// Shows the 6-digit code and waits until both users have answered.
    async fn pair_confirm(&self, stream: Stream, info: &Info, sas: String, initiator: bool) -> bool {
        let pair_id = new_id();
        let (tx, mut rx) = oneshot::channel::<bool>();
        self.rt.lock().unwrap().pair_prompts.insert(
            pair_id.clone(),
            PairPrompt { info: info.clone(), sas, initiator, waiting: false, tx: Some(tx) },
        );
        self.changed();

        let (mut r, mut w) = tokio::io::split(stream);
        let remote = async move {
            matches!(timeout(Duration::from_secs(180), read_msg::<_, PairMsg>(&mut r)).await, Ok(Ok(PairMsg::Confirm { ok: true })))
        };
        tokio::pin!(remote);
        let mut remote_ok: Option<bool> = None;
        let local_ok = loop {
            tokio::select! {
                l = &mut rx => break l.unwrap_or(false),
                r = &mut remote, if remote_ok.is_none() => {
                    remote_ok = Some(r);
                    if !r { break false; }
                }
            }
        };
        let _ = write_msg(&mut w, &PairMsg::Confirm { ok: local_ok }).await;
        let ok = if local_ok {
            if let Some(p) = self.rt.lock().unwrap().pair_prompts.get_mut(&pair_id) {
                p.waiting = true;
            }
            self.changed();
            match remote_ok {
                Some(v) => v,
                None => remote.await,
            }
        } else {
            false
        };
        self.rt.lock().unwrap().pair_prompts.remove(&pair_id);
        if !ok && (local_ok || remote_ok == Some(false)) {
            self.toast("warn", tr!("جفت‌سازی با «{}» انجام نشد", "Pairing with “{}” did not complete", info.name));
        }
        self.changed();
        ok
    }

    fn add_peer(&self, pid: &str, info: &Info, addr: Option<PeerAddr>) {
        {
            let mut cfg = self.cfg.lock().unwrap();
            if let Some(p) = cfg.peers.iter_mut().find(|p| p.id == pid) {
                p.name = info.name.clone();
                p.kind = info.kind.clone();
                if let Some(a) = addr {
                    if !p.addrs.contains(&a) {
                        p.addrs.push(a);
                    }
                }
            } else {
                cfg.peers.push(Peer {
                    id: pid.into(),
                    name: info.name.clone(),
                    kind: info.kind.clone(),
                    accept_mode: None,
                    addrs: addr.into_iter().collect(),
                    paired_at: now(),
                });
            }
            cfg.save(&self.dir);
        }
        self.toast("ok", tr!("«{}» جفت شد", "“{}” is paired", info.name));
        self.changed();
        self.wake.notify_one();
    }

    fn forget_peer(&self, pid: &str) {
        {
            let mut cfg = self.cfg.lock().unwrap();
            cfg.peers.retain(|p| p.id != pid);
            cfg.save(&self.dir);
        }
        if let Some(links) = self.rt.lock().unwrap().links.remove(pid) {
            for l in links {
                let _ = l.tx.send(LinkMsg::Bye);
            }
        }
        self.changed();
    }

    pub async fn unpair(&self, pid: &str) {
        if let Ok(Ok(mut stream)) = timeout(Duration::from_secs(4), self.open_stream(pid)).await {
            let _ = write_msg(&mut stream, &Req::Unpair).await;
        }
        self.forget_peer(pid);
    }

    /// Asks the device at `addr` who it is, without pairing.
    pub(crate) async fn hello(&self, addr: SocketAddr) -> Result<(String, Info)> {
        let (mut stream, pid, _) = self.dial(&addr.ip().to_string(), addr.port()).await?;
        write_msg(&mut stream, &Req::Hello { info: self.info() }).await?;
        match timeout(Duration::from_secs(5), read_msg::<_, Resp>(&mut stream)).await?? {
            Resp::Hello { info, .. } => Ok((pid, info)),
            _ => bail!("unexpected reply"),
        }
    }

    /// Looks for devices on the local subnet by connecting to each address.
    /// For networks where broadcast discovery is filtered.
    pub async fn scan(&self) -> Result<usize> {
        use futures_util::StreamExt;
        let net = self.rt.lock().unwrap().net.clone().ok_or_else(|| anyhow!(tr!("به هیچ شبکه‌ای وصل نیستی", "You are not connected to any network")))?;
        let mut ports = vec![proto::DEFAULT_PORT];
        if self.port != proto::DEFAULT_PORT {
            ports.push(self.port);
        }
        let targets: Vec<SocketAddr> =
            netid::subnet_hosts(&net).into_iter().flat_map(|ip| ports.iter().map(move |p| SocketAddr::from((ip, *p)))).collect();
        let found = std::sync::atomic::AtomicUsize::new(0);
        futures_util::stream::iter(targets)
            .for_each_concurrent(128, |addr| {
                let found = &found;
                async move {
                    let open = matches!(timeout(Duration::from_millis(600), TcpStream::connect(addr)).await, Ok(Ok(_)));
                    if !open {
                        return;
                    }
                    if let Ok((pid, info)) = self.hello(addr).await {
                        if pid != self.id {
                            found.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            self.saw_device(pid, info, addr, Duration::from_secs(180));
                        }
                    }
                }
            })
            .await;
        Ok(found.into_inner())
    }

    pub(crate) fn saw_device(&self, pid: String, info: Info, addr: SocketAddr, ttl: Duration) {
        let fresh = {
            let mut rt = self.rt.lock().unwrap();
            let fresh = !rt.nearby.get(&pid).is_some_and(|n| n.seen.elapsed() < n.ttl && n.addr == addr && n.info.name == info.name);
            rt.nearby.insert(pid.clone(), NearbyDev { info, addr, seen: Instant::now(), ttl });
            fresh
        };
        if fresh {
            self.changed();
            if self.is_paired(&pid) && !self.has_link(&pid) {
                self.wake.notify_one();
            }
        }
    }
}
