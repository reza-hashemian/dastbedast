//! Wire messages. Each one is a u32 big-endian length followed by JSON.
//! File bytes are sent raw between messages.

use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::model::BoardItem;

pub const PROTO_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 47800;
pub const BEACON_PORT: u16 = 47801;
pub const DEFAULT_GUEST_PORT: u16 = 47802;
const MAX_FRAME: usize = 64 << 20;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Info {
    pub name: String,
    pub kind: String,
    pub port: u16,
    pub v: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub rel: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Offer {
    pub id: String,
    pub files: Vec<FileEntry>,
    /// Set when this is the content of a shared-board item the receiver asked for.
    #[serde(default)]
    pub board: Option<String>,
}

/// First message on every connection.
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
pub enum Req {
    Hello { info: Info },
    Pair { info: Info, commit: String },
    Link { info: Info },
    Callback { token: String },
    Offer { offer: Offer },
    /// "Send me the content of this board item."
    BoardGet { item: String },
    Unpair,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
pub enum Resp {
    Hello { info: Info, paired: bool },
    PairNonce { info: Info, nonce: String },
    Ok,
    Err { msg: String },
    Accept { resume: Vec<u64> },
    Reject { reason: String },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
pub enum PairMsg {
    Reveal { nonce: String },
    Confirm { ok: bool },
}

/// Messages on the long-lived link between two paired devices.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t")]
pub enum LinkMsg {
    Ping,
    /// "Open a connection to me": used when the sender of this message cannot dial the other side (NAT).
    CallMe { token: String },
    Bye,
    /// This device was renamed.
    Info { info: Info },
    /// The sender's whole shared board, and the items whose content it can hand out.
    Board { items: Vec<BoardItem>, have: Vec<String> },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Done {
    pub failed: Vec<usize>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Beacon {
    pub app: String,
    pub id: String,
    pub info: Info,
}

pub async fn write_msg<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_u32(body.len() as u32).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_msg<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let len = r.read_u32().await? as usize;
    if len > MAX_FRAME {
        bail!("frame too large");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}
