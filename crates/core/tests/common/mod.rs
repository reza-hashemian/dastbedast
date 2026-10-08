//! Test devices on the loopback interface.
#![allow(dead_code)]

use std::{path::PathBuf, time::Duration};

use dbd_core::{Core, Options};
use serde_json::{json, Value};

pub struct Device {
    pub core: Core,
    pub dir: PathBuf,
}

impl Device {
    pub async fn start(tag: &str) -> Device {
        let dir = std::env::temp_dir().join(format!("dbd-test-{tag}-{:x}", rand::random::<u64>()));
        let core = Core::start(dir.join("cfg"), Options { port: Some(0), discovery: false, ..Default::default() }).await.unwrap();
        let args = json!({ "inbox_dir": dir.join("inbox"), "keep_dir": dir.join("keep"), "board_dir": dir.join("board") });
        core.call("set_dirs", args).await.unwrap();
        core.call("set_name", json!({ "name": tag })).await.unwrap();
        core.call("set_guest_port", json!({ "port": 0 })).await.unwrap();
        Device { core, dir }
    }
    pub async fn call(&self, cmd: &str, args: Value) -> Value {
        self.core.call(cmd, args).await.unwrap_or_else(|e| panic!("{cmd}: {e:#}"))
    }
    pub fn snap(&self) -> Value {
        self.core.snapshot()
    }
    pub fn file(&self, name: &str, data: &[u8]) -> String {
        let p = self.dir.join("src").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, data).unwrap();
        p.to_string_lossy().into_owned()
    }
    /// This device's view of the paired device `id`; null when it is not paired.
    pub fn peer(&self, id: &str) -> Value {
        self.snap()["peers"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap_or(Value::Null)
    }
    /// The shared-board item whose name or text is `what`; null when it is not on this device's board.
    pub fn board(&self, what: &str) -> Value {
        self.snap()["board"].as_array().unwrap().iter().find(|i| i["name"] == what || i["text"] == what).cloned().unwrap_or(Value::Null)
    }
    pub fn transfer_state(&self, id: &str) -> String {
        self.snap()["transfers"].as_array().unwrap().iter().find(|t| t["id"] == id).map(|t| t["state"].as_str().unwrap().to_string()).unwrap_or_default()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

pub fn random(len: usize) -> Vec<u8> {
    (0..len).map(|_| rand::random::<u8>()).collect()
}

pub async fn pair(a: &Device, b: &Device) {
    // Without "new connection" open on the target, pairing is refused outright.
    let err = a.core.call("pair_addr", json!({ "host": "127.0.0.1", "port": b.core.port })).await.unwrap_err();
    assert!(err.to_string().contains("اتصال جدید"), "{err}");

    b.call("pairing_open", json!({})).await;
    a.call("pair_addr", json!({ "host": "127.0.0.1", "port": b.core.port })).await;
    until("pair prompts", || a.snap()["pairs"].as_array().unwrap().len() == 1 && b.snap()["pairs"].as_array().unwrap().len() == 1).await;
    let (pa, pb) = (a.snap()["pairs"][0].clone(), b.snap()["pairs"][0].clone());
    assert_eq!(pa["sas"], pb["sas"], "both devices must show the same code");
    assert_eq!(pa["sas"].as_str().unwrap().len(), 6);
    a.call("pair_decide", json!({ "id": pa["id"], "ok": true })).await;
    b.call("pair_decide", json!({ "id": pb["id"], "ok": true })).await;
    until("both online", || a.peer(&b.core.id)["online"] == true && b.peer(&a.core.id)["online"] == true).await;
    assert_eq!(a.peer(&b.core.id)["name"], b.snap()["me"]["name"]);
    assert_eq!(b.peer(&a.core.id)["name"], a.snap()["me"]["name"]);
}

