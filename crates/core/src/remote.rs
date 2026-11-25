//! One device as the remote control of another: slides, scrolling a PDF, music, the pointer.
//!
//! The controlling device opens one connection to the controlled one and keeps it while the
//! remote is in use, so a press costs one small message instead of a new TLS handshake.
//! The controlled device turns each press into a key or pointer event of its own system.
//! Only a fixed set of keys can be pressed this way; nothing can be typed.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use tokio::{sync::mpsc, time::timeout};

use crate::{
    i18n::{remote, tr},
    proto::{read_msg, write_msg, RemoteCmd, Req, Resp},
    screen, Clipboard, Core, Stream,
};

/// Largest text moved between clipboards.
const CLIP_MAX: usize = 1 << 20;

/// A remote that has not been used for this long lets its connection go; the next press opens a new one.
const IDLE_OUT: Duration = Duration::from_secs(10 * 60);
/// The controlled side waits a little longer, so it is the controller that closes first.
const IDLE_IN: Duration = Duration::from_secs(15 * 60);

impl Core {
    /// Sends one press to a paired device, opening the connection for it if needed.
    pub(crate) async fn remote_send(&self, pid: &str, cmd: RemoteCmd) -> Result<()> {
        if !self.is_paired(pid) {
            bail!(remote("not_paired"));
        }
        let mut sessions = self.remote.lock().await;
        if let Some(tx) = sessions.get(pid) {
            if tx.send(cmd.clone()).is_ok() {
                return Ok(());
            }
            // The connection ended (idle, or the other side went away); a fresh one is opened below.
            sessions.remove(pid);
        }
        let mut stream = self.open_stream(pid).await?;
        write_msg(&mut stream, &Req::Remote).await?;
        match timeout(Duration::from_secs(10), read_msg::<_, Resp>(&mut stream)).await {
            Ok(Ok(Resp::Ok)) => {}
            Ok(Ok(Resp::Err { msg })) => bail!(remote(&msg)),
            // An older version closes the connection on a request it does not know.
            Ok(_) => bail!(old_version()),
            Err(_) => bail!(tr!("دستگاه پاسخ نداد", "The device did not answer")),
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(cmd);
        sessions.insert(pid.to_string(), tx);
        tokio::spawn(pump(stream, rx));
        Ok(())
    }

    /// The other side of [`Core::remote_send`]: a paired device asked to control this one.
    pub(crate) async fn remote_serve(&self, mut stream: Stream, pid: String) -> Result<()> {
        if !self.cfg.lock().unwrap().remote_on {
            return write_msg(&mut stream, &Resp::Err { msg: "remote_off".into() }).await;
        }
        if let Err(e) = inject::ready() {
            self.toast("warn", tr!("کنترل از راه دور روی این دستگاه کار نمی‌کند: {}", "The remote does not work on this device: {}", e));
            return write_msg(&mut stream, &Resp::Err { msg: "remote_none".into() }).await;
        }
        write_msg(&mut stream, &Resp::Ok).await?;
        self.toast("info", tr!("«{}» این دستگاه را از راه دور کنترل می‌کند", "“{}” is using this device as a remote", self.peer_name(&pid)));
        while let Ok(Ok(cmd)) = timeout(IDLE_IN, read_msg::<_, RemoteCmd>(&mut stream)).await {
            // Switching the remote off, or removing the pairing, takes effect at the next press.
            if !self.cfg.lock().unwrap().remote_on || !self.is_paired(&pid) {
                break;
            }
            self.remote_apply(cmd);
        }
        Ok(())
    }

    /// Carries out one press on this device.
    fn remote_apply(&self, cmd: RemoteCmd) {
        match cmd.act.as_str() {
            // The red dot is drawn by the shell, in a window above everything else.
            "laser" | "laser_off" => {
                self.emit(json!({ "type": "laser", "on": cmd.act == "laser", "x": cmd.x.clamp(-400, 400), "y": cmd.y.clamp(-400, 400) }))
            }
            "lock" | "screen_off" | "sleep" => {
                let core = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = screen::system(&cmd.act) {
                        core.toast("warn", tr!("این کار روی این دستگاه انجام نشد: {}", "This could not be done on this device: {}", e));
                    }
                });
            }
            _ => inject::press(cmd),
        }
    }

    // ---- clipboard

    pub fn set_clipboard(&self, clip: Arc<dyn Clipboard>) {
        *self.clipboard.lock().unwrap() = Some(clip);
    }

    pub(crate) fn has_clipboard(&self) -> bool {
        self.clipboard.lock().unwrap().is_some()
    }

    fn clip(&self) -> Option<Arc<dyn Clipboard>> {
        self.clipboard.lock().unwrap().clone()
    }

    /// Puts this device's clipboard (or `text`) on the other device's clipboard.
    pub(crate) async fn clip_push(&self, pid: &str, text: Option<String>) -> Result<()> {
        let text = match text {
            Some(t) => t,
            None => {
                let clip = self.clip().ok_or_else(|| anyhow!(tr!("این دستگاه به کلیپ‌بورد دسترسی ندارد", "This device has no clipboard to use")))?;
                tokio::task::spawn_blocking(move || clip.get()).await?.unwrap_or_default()
            }
        };
        if text.is_empty() {
            bail!(remote("clip_empty"));
        }
        if text.len() > CLIP_MAX {
            bail!(tr!("متن کلیپ‌بورد بیش از حد بزرگ است", "The clipboard text is too large"));
        }
        let mut stream = self.open_stream(pid).await?;
        write_msg(&mut stream, &Req::Clip { text }).await?;
        answer(&mut stream).await.map(|_| ())
    }

    /// Takes the other device's clipboard and puts it on this one's. Returns the text.
    pub(crate) async fn clip_pull(&self, pid: &str) -> Result<String> {
        let mut stream = self.open_stream(pid).await?;
        write_msg(&mut stream, &Req::ClipGet).await?;
        let Resp::Clip { text } = answer(&mut stream).await? else { bail!(old_version()) };
        if let Some(clip) = self.clip() {
            let t = text.clone();
            tokio::task::spawn_blocking(move || clip.set(&t)).await?;
        }
        Ok(text)
    }

    pub(crate) async fn clip_receive(&self, mut stream: Stream, pid: String, text: String) -> Result<()> {
        let Some(clip) = self.clip() else { return write_msg(&mut stream, &Resp::Err { msg: "clip_none".into() }).await };
        if text.len() > CLIP_MAX {
            return write_msg(&mut stream, &Resp::Err { msg: "clip_empty".into() }).await;
        }
        let ok = tokio::task::spawn_blocking(move || clip.set(&text)).await.unwrap_or(false);
        if !ok {
            return write_msg(&mut stream, &Resp::Err { msg: "clip_none".into() }).await;
        }
        self.toast("info", tr!("کلیپ‌بورد «{}» اینجا آمد", "The clipboard of “{}” is now here", self.peer_name(&pid)));
        write_msg(&mut stream, &Resp::Ok).await
    }

    /// Reading this device's clipboard reveals what the user copied, so it is part of the remote
    /// and follows its switch.
    pub(crate) async fn clip_give(&self, mut stream: Stream, pid: String) -> Result<()> {
        if !self.cfg.lock().unwrap().remote_on {
            return write_msg(&mut stream, &Resp::Err { msg: "remote_off".into() }).await;
        }
        let Some(clip) = self.clip() else { return write_msg(&mut stream, &Resp::Err { msg: "clip_none".into() }).await };
        let text = tokio::task::spawn_blocking(move || clip.get()).await.ok().flatten().unwrap_or_default();
        if text.is_empty() || text.len() > CLIP_MAX {
            return write_msg(&mut stream, &Resp::Err { msg: "clip_empty".into() }).await;
        }
        self.toast("info", tr!("«{}» کلیپ‌بورد این دستگاه را برداشت", "“{}” took the clipboard of this device", self.peer_name(&pid)));
        write_msg(&mut stream, &Resp::Clip { text }).await
    }

    // ---- screen preview

    /// A small picture of the other device's screen, as base64 JPEG.
    pub(crate) async fn shot_take(&self, pid: &str) -> Result<Value> {
        let mut stream = self.open_stream(pid).await?;
        write_msg(&mut stream, &Req::Shot).await?;
        let Resp::Shot { jpeg } = answer(&mut stream).await? else { bail!(old_version()) };
        Ok(json!({ "jpeg": jpeg }))
    }

    pub(crate) async fn shot_give(&self, mut stream: Stream, pid: String) -> Result<()> {
        if !self.cfg.lock().unwrap().remote_on {
            return write_msg(&mut stream, &Resp::Err { msg: "remote_off".into() }).await;
        }
        match tokio::task::spawn_blocking(screen::preview_base64).await? {
            Ok(jpeg) => {
                self.watched_by(&self.peer_name(&pid));
                write_msg(&mut stream, &Resp::Shot { jpeg }).await
            }
            Err(e) => write_msg(&mut stream, &Resp::Err { msg: e }).await,
        }
    }

    /// The preview for the browser page this device serves.
    pub(crate) async fn shot_local(&self) -> Result<Vec<u8>> {
        if !self.cfg.lock().unwrap().remote_on {
            bail!(remote("remote_off"));
        }
        let jpeg = tokio::task::spawn_blocking(screen::preview).await?.map_err(|e| anyhow!(e))?;
        self.watched_by(&tr!("مرورگر", "the browser"));
        Ok(jpeg)
    }

    /// Tells the user someone is looking at this screen, once in a while rather than for every picture.
    fn watched_by(&self, who: &str) {
        let fresh = {
            let mut w = self.watched.lock().unwrap();
            let fresh = w.get(who).is_none_or(|t| t.elapsed() > Duration::from_secs(600));
            w.insert(who.to_string(), Instant::now());
            fresh
        };
        if fresh {
            self.toast("info", tr!("«{}» صفحهٔ این دستگاه را می‌بیند", "“{}” is viewing the screen of this device", who));
        }
    }

    /// A press from the browser page this device serves; the page controls this device itself.
    pub(crate) fn remote_local(&self, cmd: RemoteCmd) -> Result<()> {
        if !self.cfg.lock().unwrap().remote_on {
            bail!(remote("remote_off"));
        }
        inject::ready().map_err(|e| anyhow!(e))?;
        self.remote_apply(cmd);
        Ok(())
    }
}

/// The other device's answer to a one-off request.
async fn answer(stream: &mut Stream) -> Result<Resp> {
    match timeout(Duration::from_secs(20), read_msg::<_, Resp>(stream)).await {
        Ok(Ok(Resp::Err { msg })) => bail!(remote(&msg)),
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => bail!(old_version()),
        Err(_) => bail!(tr!("دستگاه پاسخ نداد", "The device did not answer")),
    }
}

fn old_version() -> String {
    tr!("برنامهٔ آن دستگاه قدیمی است؛ به‌روزش کن", "The app on that device is too old for this; update it")
}

async fn pump(mut stream: Stream, mut rx: mpsc::UnboundedReceiver<RemoteCmd>) {
    while let Ok(Some(cmd)) = timeout(IDLE_OUT, rx.recv()).await {
        if write_msg(&mut stream, &cmd).await.is_err() {
            break;
        }
    }
}

/// Whether this build can press keys at all.
pub(crate) fn supported() -> bool {
    cfg!(all(feature = "remote", not(target_os = "android")))
}

#[cfg(all(feature = "remote", not(target_os = "android")))]
mod inject {
    //! The system's keyboard and pointer, driven from one thread of its own:
    //! the handle to the display is not shared between threads on every system.

    use std::sync::{mpsc, Mutex};

    use enigo::{Axis, Button, Coordinate, Direction::Click, Enigo, Key, Keyboard, Mouse, Settings};

    use crate::proto::RemoteCmd;

    static TX: Mutex<Option<mpsc::Sender<RemoteCmd>>> = Mutex::new(None);

    /// Starts the input thread if it is not running; fails when the system refuses it.
    pub fn ready() -> Result<(), String> {
        let mut tx = TX.lock().unwrap();
        if tx.is_some() {
            return Ok(());
        }
        let (ctx, crx) = mpsc::channel::<RemoteCmd>();
        let (otx, orx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("dbd-remote".into())
            .spawn(move || {
                let mut enigo = match Enigo::new(&Settings::default()) {
                    Ok(e) => {
                        let _ = otx.send(Ok(()));
                        e
                    }
                    Err(e) => {
                        let _ = otx.send(Err(e.to_string()));
                        return;
                    }
                };
                for cmd in crx {
                    // A key the system refuses is skipped; the next one may still work.
                    let _ = run(&mut enigo, &cmd);
                }
            })
            .map_err(|e| e.to_string())?;
        orx.recv().map_err(|e| e.to_string())??;
        *tx = Some(ctx);
        Ok(())
    }

    pub fn press(cmd: RemoteCmd) {
        let mut tx = TX.lock().unwrap();
        if tx.as_ref().is_some_and(|t| t.send(cmd).is_err()) {
            *tx = None;
        }
    }

    fn run(e: &mut Enigo, cmd: &RemoteCmd) -> enigo::InputResult<()> {
        // A phone sends small steps; anything larger is a mistake, not a gesture.
        let (x, y) = (cmd.x.clamp(-400, 400), cmd.y.clamp(-400, 400));
        let key = match cmd.act.as_str() {
            // Presentation clickers send Page Down / Page Up: PowerPoint, Impress, Keynote,
            // Google Slides and PDF viewers all take them as next and previous.
            "next" => Key::PageDown,
            "prev" => Key::PageUp,
            "start" => Key::F5,
            "stop" => Key::Escape,
            "black" => Key::Unicode('b'),
            "first" => Key::Home,
            "last" => Key::End,
            "up" => Key::UpArrow,
            "down" => Key::DownArrow,
            "left" => Key::LeftArrow,
            "right" => Key::RightArrow,
            "space" => Key::Space,
            "enter" => Key::Return,
            "play" => Key::MediaPlayPause,
            "track_next" => Key::MediaNextTrack,
            "track_prev" => Key::MediaPrevTrack,
            "vol_up" => Key::VolumeUp,
            "vol_down" => Key::VolumeDown,
            "mute" => Key::VolumeMute,
            "move" => return e.move_mouse(x, y, Coordinate::Rel),
            "scroll" => return e.scroll(y, Axis::Vertical),
            "click" => return e.button(Button::Left, Click),
            "rclick" => return e.button(Button::Right, Click),
            _ => return Ok(()),
        };
        e.key(key, Click)
    }
}

#[cfg(not(all(feature = "remote", not(target_os = "android"))))]
mod inject {
    use crate::proto::RemoteCmd;

    pub fn ready() -> Result<(), String> {
        Err(crate::i18n::remote("remote_none"))
    }

    pub fn press(_: RemoteCmd) {}
}
