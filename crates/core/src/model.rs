//! Everything that is saved to disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::proto::{DEFAULT_GUEST_PORT, DEFAULT_PORT};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Network {
    pub id: String,
    pub name: String,
    /// "ask" | "auto"; overrides the device default while on this network.
    #[serde(default)]
    pub accept_mode: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PeerAddr {
    pub host: String,
    pub port: u16,
    /// Network this address belongs to. `None` is a static address that is tried from every network.
    #[serde(default)]
    pub net: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub accept_mode: Option<String>,
    #[serde(default)]
    pub addrs: Vec<PeerAddr>,
    #[serde(default)]
    pub paired_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InboxItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: usize,
    pub from: String,
    pub at: u64,
    /// "new" | "kept"
    pub state: String,
}

/// One thing on the shared board, as every device sees it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BoardItem {
    pub id: String,
    /// "file" (a file or a folder) | "text"
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub files: usize,
    /// Id and name of the device that put it on the board.
    pub from: String,
    #[serde(default)]
    pub from_name: String,
    pub at: u64,
    /// When it was taken off the board; 0 while it is on it.
    /// Removed items are remembered for a while so that the removal reaches devices that were away.
    #[serde(default)]
    pub gone: u64,
}

/// Where this device has the content of a board item.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BoardLocal {
    pub path: String,
    /// The user's own file, shared from where it lives. Never moved or deleted by the board.
    #[serde(default)]
    pub own: bool,
    /// A fetched copy the user chose to keep; it stays when the item leaves the board.
    #[serde(default)]
    pub kept: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BoardEntry {
    #[serde(flatten)]
    pub item: BoardItem,
    #[serde(default)]
    pub local: Option<BoardLocal>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub name: String,
    pub port: u16,
    pub guest_port: u16,
    /// The browser link is switched on; it comes back by itself after a restart.
    pub guest_on: bool,
    /// The secret part of the browser link. It stays the same so that a phone that saved the page keeps working.
    pub guest_token: String,
    /// "fa" | "en"
    pub lang: String,
    pub inbox_dir: String,
    pub keep_dir: String,
    /// Copies fetched from the shared board.
    pub board_dir: String,
    /// "ask" | "auto"
    pub accept_mode: String,
    /// Largest transfer accepted without asking, in bytes. 0 means no limit.
    pub auto_max: u64,
    /// This device fetches everything put on the shared board, so the others can get it
    /// from here while the device that shared it is away.
    pub always_on: bool,
    pub networks: Vec<Network>,
    pub peers: Vec<Peer>,
    pub inbox: Vec<InboxItem>,
    pub board: Vec<BoardEntry>,
}

impl Default for Config {
    fn default() -> Self {
        let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("DastBeDast");
        Config {
            name: String::new(),
            port: DEFAULT_PORT,
            guest_port: DEFAULT_GUEST_PORT,
            guest_on: false,
            guest_token: String::new(),
            lang: "fa".into(),
            inbox_dir: base.join("Inbox").to_string_lossy().into_owned(),
            board_dir: base.join("Board").to_string_lossy().into_owned(),
            keep_dir: base.join("Files").to_string_lossy().into_owned(),
            accept_mode: "ask".into(),
            auto_max: 5 << 30,
            always_on: false,
            networks: vec![],
            peers: vec![],
            inbox: vec![],
            board: vec![],
        }
    }
}

impl Config {
    pub fn load(dir: &Path) -> Config {
        std::fs::read(dir.join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        let tmp = dir.join("config.json.tmp");
        if let Ok(body) = serde_json::to_vec_pretty(self) {
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("config.json"));
            }
        }
    }
}

pub fn device_kind() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn default_name() -> String {
    // On a phone the model name is what the user recognises.
    #[cfg(target_os = "android")]
    if let Some(model) = std::process::Command::new("getprop").arg("ro.product.model").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|m| !m.is_empty()) {
        return model;
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| crate::i18n::tr!("دستگاه من", "My device"))
}
