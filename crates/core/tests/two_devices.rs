//! Two devices on the loopback interface, driven through the same command API the UI uses.

mod common;

use std::path::PathBuf;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn pair_send_receive() {
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    let b_id = b.core.id.clone();
    let a_id = a.core.id.clone();

    // --- single file, receiver asks first
    let data = random(5 * 1024 * 1024 + 123);
    let path = a.file("report.bin", &data);
    let id = a.call("send", json!({ "peer": b_id, "paths": [path] })).await["id"].as_str().unwrap().to_string();
    until("offer shown", || b.snap()["offers"].as_array().unwrap().len() == 1).await;
    assert_eq!(b.snap()["offers"][0]["total"], data.len());
    assert!(b.snap()["inbox"].as_array().unwrap().is_empty(), "nothing lands before the user accepts");
    b.call("offer_decide", json!({ "id": id, "ok": true })).await;
    until("sent", || a.transfer_state(&id) == "done").await;
    until("received", || b.transfer_state(&id) == "done").await;
    let item = b.snap()["inbox"][0].clone();
    assert_eq!(item["name"], "report.bin");
    assert_eq!(std::fs::read(item["path"].as_str().unwrap()).unwrap(), data);

    // --- rejected offer
    let id = a.call("send", json!({ "peer": b_id, "paths": [a.file("no.bin", b"nope")] })).await["id"].as_str().unwrap().to_string();
    until("offer shown", || b.snap()["offers"].as_array().unwrap().len() == 1).await;
    b.call("offer_decide", json!({ "id": id, "ok": false })).await;
    until("rejected", || a.transfer_state(&id) == "rejected").await;
    assert_eq!(b.snap()["inbox"].as_array().unwrap().len(), 1);

    // --- folder, no confirmation; same name again must not overwrite
    b.call("set_accept", json!({ "mode": "auto" })).await;
    a.file("album/one.txt", b"one");
    a.file("album/deep/two.txt", b"two two");
    a.file("album/deep/empty.txt", b"");
    let folder = a.dir.join("src/album").to_string_lossy().into_owned();
    let id = a.call("send", json!({ "peer": b_id, "paths": [folder, a.file("report.bin", b"second")] })).await["id"].as_str().unwrap().to_string();
    until("folder sent", || a.transfer_state(&id) == "done").await;
    until("folder received", || b.transfer_state(&id) == "done").await;
    let inbox = b.dir.join("inbox");
    assert_eq!(std::fs::read(inbox.join("album/deep/two.txt")).unwrap(), b"two two");
    assert_eq!(std::fs::read(inbox.join("album/deep/empty.txt")).unwrap(), b"");
    assert_eq!(std::fs::read(inbox.join("report (2).bin")).unwrap(), b"second");
    assert_eq!(std::fs::read(inbox.join("report.bin")).unwrap(), data);

    // --- per-sender override beats the device default
    b.call("set_peer_mode", json!({ "id": a_id, "mode": "ask" })).await;
    let id = a.call("send", json!({ "peer": b_id, "paths": [a.file("ask.bin", b"ask me")] })).await["id"].as_str().unwrap().to_string();
    until("asks despite auto default", || b.snap()["offers"].as_array().unwrap().len() == 1).await;
    b.call("offer_decide", json!({ "id": id, "ok": true })).await;
    until("sent", || a.transfer_state(&id) == "done").await;
    b.call("set_peer_mode", json!({ "id": a_id, "mode": "default" })).await;

    // --- the other direction
    a.call("set_accept", json!({ "mode": "auto" })).await;
    let back = random(700_000);
    let id = b.call("send", json!({ "peer": a_id, "paths": [b.file("back.bin", &back)] })).await["id"].as_str().unwrap().to_string();
    until("reverse sent", || b.transfer_state(&id) == "done").await;
    until("reverse received", || a.transfer_state(&id) == "done").await;
    assert_eq!(std::fs::read(a.dir.join("inbox/back.bin")).unwrap(), back);

    // --- inbox: keep moves the file, delete removes it
    let items = b.snap()["inbox"].as_array().unwrap().clone();
    let report = items.iter().find(|i| i["name"] == "report.bin").unwrap();
    b.call("inbox_keep", json!({ "id": report["id"] })).await;
    assert_eq!(std::fs::read(b.dir.join("keep/report.bin")).unwrap(), data);
    assert!(!inbox.join("report.bin").exists());
    let album = items.iter().find(|i| i["name"] == "album").unwrap();
    b.call("inbox_delete", json!({ "id": album["id"] })).await;
    assert!(!inbox.join("album").exists());
    // Deleting the entry of a kept file leaves the file alone.
    b.call("inbox_delete", json!({ "id": report["id"] })).await;
    assert!(b.dir.join("keep/report.bin").exists());

    // --- a rename shows up on the other device without reconnecting
    a.call("set_name", json!({ "name": "alpha-2" })).await;
    until("rename reaches peer", || b.snap()["peers"][0]["name"] == "alpha-2").await;

    // --- unpair reaches the other side
    a.call("unpair", json!({ "id": b_id })).await;
    until("both forgot", || a.snap()["peers"].as_array().unwrap().is_empty() && b.snap()["peers"].as_array().unwrap().is_empty()).await;
    assert!(a.core.call("send", json!({ "peer": b_id, "paths": [a.file("x", b"x")] })).await.is_err());
}

fn partial_path(receiver: &Device, sender_id: &str, rel: &str, size: u64) -> PathBuf {
    let mut key = blake3::Hasher::new();
    key.update(sender_id.as_bytes());
    key.update(rel.as_bytes());
    key.update(&size.to_le_bytes());
    receiver.dir.join("inbox/.partial").join(&key.finalize().to_hex()[..16]).join(rel)
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_and_corruption() {
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    b.call("set_accept", json!({ "mode": "auto" })).await;
    let b_id = b.core.id.clone();

    // A transfer that stopped after 3 MB continues from there and still verifies.
    let data = random(8 * 1024 * 1024 + 7);
    let part = partial_path(&b, &a.core.id, "big.bin", data.len() as u64);
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, &data[..3 * 1024 * 1024]).unwrap();
    let mut events = a.core.subscribe();
    let id = a.call("send", json!({ "peer": b_id, "paths": [a.file("big.bin", &data)] })).await["id"].as_str().unwrap().to_string();
    until("resumed transfer done", || a.transfer_state(&id) == "done" && b.transfer_state(&id) == "done").await;
    assert_eq!(std::fs::read(b.dir.join("inbox/big.bin")).unwrap(), data);
    let mut first_progress = None;
    while let Ok(ev) = events.try_recv() {
        if ev["type"] == "progress" && first_progress.is_none() {
            first_progress = ev["done"].as_u64();
        }
    }
    assert!(first_progress.unwrap() > 3 * 1024 * 1024, "sending must start after the part that already arrived");

    // A partial file with the wrong content is caught by the hash and never reaches the inbox.
    let data = random(2 * 1024 * 1024);
    let part = partial_path(&b, &a.core.id, "bad.bin", data.len() as u64);
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, vec![0u8; 1024 * 1024]).unwrap();
    let id = a.call("send", json!({ "peer": b_id, "paths": [a.file("bad.bin", &data)] })).await["id"].as_str().unwrap().to_string();
    until("corruption reported", || a.transfer_state(&id) == "failed" && b.transfer_state(&id) == "failed").await;
    assert!(!b.dir.join("inbox/bad.bin").exists());
    // The bad partial is gone, so trying again succeeds.
    let id = a.call("send", json!({ "peer": b_id, "paths": [a.file("bad.bin", &data)] })).await["id"].as_str().unwrap().to_string();
    until("clean retry done", || a.transfer_state(&id) == "done").await;
    assert_eq!(std::fs::read(b.dir.join("inbox/bad.bin")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_paths_stay_inside_inbox() {
    use dbd_core::proto::{read_msg, write_msg, FileEntry, Offer, Req, Resp};
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    b.call("set_accept", json!({ "mode": "auto" })).await;
    for rel in ["../escape.txt", "/etc/escape.txt", "ok/../../escape.txt", ".partial/x"] {
        let mut stream = a.core.raw_stream(&b.core.id).await.unwrap();
        let offer = Offer { id: format!("{:x}", rand::random::<u64>()), files: vec![FileEntry { rel: rel.into(), size: 1 }], board: None };
        write_msg(&mut stream, &Req::Offer { offer }).await.unwrap();
        let resp: Resp = read_msg(&mut stream).await.unwrap();
        assert!(matches!(resp, Resp::Err { .. }), "{rel} must be refused, got {resp:?}");
    }
    assert!(!b.dir.join("escape.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_device_cannot_send() {
    use dbd_core::proto::{read_msg, write_msg, FileEntry, Offer, Req, Resp};
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    b.call("set_accept", json!({ "mode": "auto" })).await;
    let mut stream = a.core.raw_dial("127.0.0.1", b.core.port).await.unwrap();
    let offer = Offer { id: "1".into(), files: vec![FileEntry { rel: "x".into(), size: 1 }], board: None };
    write_msg(&mut stream, &Req::Offer { offer }).await.unwrap();
    let resp: Resp = read_msg(&mut stream).await.unwrap();
    assert!(matches!(resp, Resp::Err { .. }));
    assert!(b.snap()["offers"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_guest() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let b = Device::start("bravo").await;
    b.call("guest_start", json!({})).await;
    let url = b.snap()["guest"]["url"].as_str().unwrap().to_string();
    assert!(b.snap()["guest"]["qr"].as_str().unwrap().contains("<svg"));
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let port: u16 = host.rsplit_once(':').unwrap().1.parse().unwrap();

    async fn http(port: u16, req: String, body: &[u8]) -> Vec<u8> {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        s.write_all(body).await.unwrap();
        let mut out = vec![];
        s.read_to_end(&mut out).await.unwrap();
        out
    }
    let body = random(300_000);
    let put = |path: &str| format!("PUT /{path}/up/note.bin HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let resp = http(port, put(path), &body).await;
    assert!(resp.starts_with(b"HTTP/1.1 204"), "{}", String::from_utf8_lossy(&resp));
    assert_eq!(std::fs::read(b.dir.join("inbox/note.bin")).unwrap(), body);
    assert_eq!(b.snap()["inbox"][0]["from"], "مهمان مرورگر");

    // A wrong token gets nothing.
    // (Sent without a body: a server that refuses before reading one may reset the connection.)
    let resp = http(port, "PUT /g/0000000000000000/up/note.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), b"").await;
    assert!(resp.starts_with(b"HTTP/1.1 404"));

    // Download of a file put out for the guest.
    b.call("guest_share", json!({ "paths": [b.file("gift.bin", &body)] })).await;
    let resp = http(port, format!("GET /{path}/dl/0 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"), b"").await;
    assert!(resp.starts_with(b"HTTP/1.1 200"));
    assert!(resp.ends_with(&body));

    // What a phone needs to keep the page on its home screen.
    let resp = http(port, format!("GET /{path}/manifest.webmanifest HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"), b"").await;
    let text = String::from_utf8_lossy(&resp).into_owned();
    assert!(text.contains("\"display\":\"standalone\"") && text.contains(&format!("\"start_url\":\"/{path}\"")), "{text}");
    let resp = http(port, format!("GET /{path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"), b"").await;
    assert!(String::from_utf8_lossy(&resp).contains("apple-mobile-web-app-capable"));

    // The shared board through the browser: put a note and a file, read the list, take the file back.
    let note = "from the phone";
    let post = format!("POST /{path}/board/text HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", note.len());
    assert!(http(port, post, note.as_bytes()).await.starts_with(b"HTTP/1.1 204"));
    let put = format!("PUT /{path}/board/up/photo.jpg HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    assert!(http(port, put, &body).await.starts_with(b"HTTP/1.1 204"));
    assert_eq!(b.board(note)["kind"], "text");
    assert_eq!(b.board("photo.jpg")["state"], "here");
    assert_eq!(std::fs::read(b.dir.join("board/photo.jpg")).unwrap(), body);
    let resp = http(port, format!("GET /{path}/board HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"), b"").await;
    let text = String::from_utf8_lossy(&resp).into_owned();
    assert!(text.contains("from the phone") && text.contains("\"here\":true"), "{text}");
    let id = b.board("photo.jpg")["id"].as_str().unwrap().to_string();
    let resp = http(port, format!("GET /{path}/board/dl/{id} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"), b"").await;
    assert!(resp.starts_with(b"HTTP/1.1 200") && resp.ends_with(&body));
    // Taken off the board, the uploaded copy goes too.
    b.call("board_remove", json!({ "id": id })).await;
    until("uploaded copy removed", || !b.dir.join("board/photo.jpg").exists()).await;

    b.call("guest_stop", json!({})).await;
    assert!(b.snap()["guest"].is_null());
    // Switched on again, the link is the same one, so a phone that saved it keeps working...
    b.call("guest_start", json!({})).await;
    assert!(b.snap()["guest"]["url"].as_str().unwrap().ends_with(path));
    // ...until the user asks for a new one.
    b.call("guest_reset", json!({})).await;
    assert!(!b.snap()["guest"]["url"].as_str().unwrap().ends_with(path));
    b.call("guest_stop", json!({})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn file_handed_over_open() {
    // What Android does: the picker gives an open file instead of a path.
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    b.call("set_accept", json!({ "mode": "auto" })).await;
    let data = random(1_500_000);
    let real = a.file("hidden-name-1234", &data);
    let stand_in = dbd_core::adopt_file(std::fs::File::open(&real).unwrap(), "holiday.jpg");
    std::fs::remove_file(&real).unwrap();

    let id = a.call("send", json!({ "peer": b.core.id, "paths": [stand_in] })).await["id"].as_str().unwrap().to_string();
    until("sent", || a.transfer_state(&id) == "done" && b.transfer_state(&id) == "done").await;
    assert_eq!(std::fs::read(b.dir.join("inbox/holiday.jpg")).unwrap(), data);

    // The same stand-in works on the shared board, twice over.
    a.call("board_put", json!({ "paths": [stand_in] })).await;
    until("listed", || b.board("holiday.jpg")["sources"] == json!(["alpha"])).await;
    b.call("board_get", json!({ "id": b.board("holiday.jpg")["id"] })).await;
    until("fetched", || b.board("holiday.jpg")["state"] == "here").await;
    assert_eq!(std::fs::read(b.dir.join("board/holiday.jpg")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread")]
async fn phone_through_the_browser_link() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // alpha serves the phone; bravo is another device of the same person.
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    b.call("set_accept", json!({ "mode": "auto" })).await;
    a.call("guest_start", json!({})).await;
    let url = a.snap()["guest"]["url"].as_str().unwrap().to_string();
    let (host, path) = url.strip_prefix("http://").unwrap().split_once('/').unwrap();
    let port: u16 = host.rsplit_once(':').unwrap().1.parse().unwrap();
    async fn http(port: u16, req: String, body: &[u8]) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        s.write_all(body).await.unwrap();
        let mut out = vec![];
        s.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }
    let get = |what: &str| format!("GET /{path}/{what} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");

    // The phone sees the other device and sends it a file; alpha passes it on.
    let state = http(port, get("state"), b"").await;
    assert!(state.contains("\"name\":\"bravo\"") && state.contains("\"online\":true"), "{state}");
    let data = random(900_000);
    let put = format!("PUT /{path}/to/{}/clip.mov HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.core.id, data.len());
    assert!(http(port, put, &data).await.starts_with("HTTP/1.1 200"));
    until("file passed on to bravo", || b.dir.join("inbox/clip.mov").exists() && !b.snap()["inbox"].as_array().unwrap().is_empty()).await;
    assert_eq!(std::fs::read(b.dir.join("inbox/clip.mov")).unwrap(), data);
    assert!(a.snap()["inbox"].as_array().unwrap().is_empty(), "a passed-on file does not stay with the device in between");
    let state = http(port, get("state"), b"").await;
    assert!(state.contains("\"state\":\"done\"") && state.contains("clip.mov"), "{state}");
    // A device the phone's host is not paired with cannot be reached this way.
    let put = format!("PUT /{path}/to/0000/x.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert!(http(port, put, b"").await.starts_with("HTTP/1.1 404"));

    // What another device puts on the board is fetched by alpha by itself, so the phone can download it.
    let photo = random(400_000);
    b.call("board_put", json!({ "paths": [b.file("photo.png", &photo)] })).await;
    until("alpha fetched it for the phone", || a.board("photo.png")["state"] == "here").await;
    let id = a.board("photo.png")["id"].as_str().unwrap().to_string();
    let resp = http(port, get(&format!("board/dl/{id}")), b"").await;
    assert!(resp.starts_with("HTTP/1.1 200"));

    // Files put on the phone's card wait for it, and can be taken back.
    a.call("guest_share", json!({ "paths": [a.file("for-phone.pdf", b"pdf")] })).await;
    assert!(http(port, get("state"), b"").await.contains("for-phone.pdf"));
    a.call("guest_clear", json!({})).await;
    assert!(!http(port, get("state"), b"").await.contains("for-phone.pdf"));
    a.call("guest_stop", json!({})).await;
}
