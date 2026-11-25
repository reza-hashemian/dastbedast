//! Using a paired device as a remote control. The tests are built without the `remote` feature,
//! so no key is ever pressed on the machine that runs them; they check who may ask and what they are told.

mod common;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn remote_is_refused_with_a_reason() {
    let a = Device::start("remote-a").await;
    let b = Device::start("remote-b").await;
    let c = Device::start("remote-c").await;
    pair(&a, &b).await;
    let b_id = b.core.id.clone();
    until("linked", || a.peer(&b_id)["online"] == true).await;

    assert_eq!(b.snap()["settings"]["remote_on"], true, "on by default");
    assert_eq!(b.snap()["me"]["remote"], false, "a build without the feature says it cannot be controlled");

    // A build that cannot press keys says so, instead of silently doing nothing.
    let err = a.core.call("remote", json!({ "peer": b_id, "act": "next" })).await.unwrap_err();
    assert!(err.to_string().contains("از راه دور کنترل"), "{err}");

    // Switched off on the controlled device.
    b.call("set_remote_on", json!({ "on": false })).await;
    assert_eq!(b.snap()["settings"]["remote_on"], false);
    let err = a.core.call("remote", json!({ "peer": b_id, "act": "next" })).await.unwrap_err();
    assert!(err.to_string().contains("خاموش"), "{err}");

    // Only paired devices.
    let err = c.core.call("remote", json!({ "peer": b_id, "act": "next" })).await.unwrap_err();
    assert!(err.to_string().contains("جفت نشده"), "{err}");
}

/// A clipboard in memory, standing in for the system's.
struct Mem(std::sync::Mutex<String>);

impl dbd_core::Clipboard for Mem {
    fn get(&self) -> Option<String> {
        Some(self.0.lock().unwrap().clone())
    }
    fn set(&self, text: &str) -> bool {
        *self.0.lock().unwrap() = text.to_string();
        true
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_and_screen() {
    use std::sync::Arc;
    let a = Device::start("clip-a").await;
    let b = Device::start("clip-b").await;
    pair(&a, &b).await;
    let b_id = b.core.id.clone();
    until("linked", || a.peer(&b_id)["online"] == true).await;

    // Without a clipboard (a headless device) both directions say so.
    assert_eq!(a.snap()["me"]["clip"], false);
    let err = a.core.call("clip_push", json!({ "peer": b_id, "text": "hello" })).await.unwrap_err();
    assert!(err.to_string().contains("کلیپ‌بورد"), "{err}");

    let ca = Arc::new(Mem(Default::default()));
    let cb = Arc::new(Mem(Default::default()));
    a.core.set_clipboard(ca.clone());
    b.core.set_clipboard(cb.clone());
    assert_eq!(a.snap()["me"]["clip"], true);

    // This device's clipboard goes over…
    *ca.0.lock().unwrap() = "سلام from A".into();
    a.call("clip_push", json!({ "peer": b_id })).await;
    assert_eq!(*cb.0.lock().unwrap(), "سلام from A");

    // …and the other one's comes back.
    *cb.0.lock().unwrap() = "copied on B".into();
    assert_eq!(a.call("clip_pull", json!({ "peer": b_id })).await["text"], "copied on B");
    assert_eq!(*ca.0.lock().unwrap(), "copied on B");

    // An empty clipboard is not sent.
    *ca.0.lock().unwrap() = String::new();
    let err = a.core.call("clip_push", json!({ "peer": b_id })).await.unwrap_err();
    assert!(err.to_string().contains("خالی"), "{err}");

    // A build without the remote cannot show its screen.
    let err = a.core.call("remote_shot", json!({ "peer": b_id })).await.unwrap_err();
    assert!(err.to_string().contains("از راه دور کنترل"), "{err}");

    // Reading the clipboard or the screen follows the remote's switch; putting text there does not.
    b.call("set_remote_on", json!({ "on": false })).await;
    for cmd in ["clip_pull", "remote_shot"] {
        let err = a.core.call(cmd, json!({ "peer": b_id })).await.unwrap_err();
        assert!(err.to_string().contains("خاموش"), "{cmd}: {err}");
    }
    a.call("clip_push", json!({ "peer": b_id, "text": "still fine" })).await;
    assert_eq!(*cb.0.lock().unwrap(), "still fine");
}
