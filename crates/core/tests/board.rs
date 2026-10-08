//! The shared board across three devices: alpha and charlie are each paired with bravo only.

mod common;

use common::*;
use serde_json::json;

fn live(d: &Device) -> usize {
    d.snap()["board"].as_array().unwrap().len()
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_board() {
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    let c = Device::start("charlie").await;
    pair(&a, &b).await;
    pair(&b, &c).await;

    // --- text reaches every device, also the one that is not paired with its author
    a.call("board_text", json!({ "text": "  https://example.com/page  " })).await;
    until("text on all boards", || !b.board("https://example.com/page").is_null() && !c.board("https://example.com/page").is_null()).await;
    let note = c.board("https://example.com/page");
    assert_eq!(note["from"], "alpha");
    assert_eq!(note["state"], "here");
    assert_eq!(note["mine"], false);
    assert_eq!(a.board("https://example.com/page")["mine"], true);

    // --- a file is listed everywhere, but its content moves only when asked for
    let data = random(2 * 1024 * 1024 + 11);
    let src = a.file("plan.pdf", &data);
    a.call("board_put", json!({ "paths": [src] })).await;
    until("file listed", || b.board("plan.pdf")["sources"] == json!(["alpha"]) && !c.board("plan.pdf").is_null()).await;
    assert_eq!(a.board("plan.pdf")["state"], "here");
    assert_eq!(a.board("plan.pdf")["own"], true);
    assert_eq!(b.board("plan.pdf")["state"], "away");
    assert_eq!(b.board("plan.pdf")["size"], data.len());
    assert!(!b.dir.join("board/plan.pdf").exists());
    // Putting the same file out twice does not list it twice.
    a.call("board_put", json!({ "paths": [a.dir.join("src/plan.pdf")] })).await;
    assert_eq!(live(&a), 2);

    // charlie knows about the file but no device it is connected to has it yet.
    let id = c.board("plan.pdf")["id"].clone();
    assert_eq!(c.board("plan.pdf")["sources"], json!([]));
    assert!(c.core.call("board_get", json!({ "id": id })).await.is_err());

    // bravo fetches it without being asked to confirm, although it normally asks.
    b.call("board_get", json!({ "id": id })).await;
    until("bravo has it", || b.board("plan.pdf")["state"] == "here").await;
    assert_eq!(std::fs::read(b.dir.join("board/plan.pdf")).unwrap(), data);
    assert!(b.snap()["offers"].as_array().unwrap().is_empty());
    assert!(b.snap()["inbox"].as_array().unwrap().is_empty(), "board content does not go through the inbox");

    // Now charlie can get it from bravo, a device alpha never met.
    until("bravo listed as source", || c.board("plan.pdf")["sources"] == json!(["bravo"])).await;
    c.call("board_get", json!({ "id": id })).await;
    until("charlie has it", || c.board("plan.pdf")["state"] == "here").await;
    assert_eq!(std::fs::read(c.dir.join("board/plan.pdf")).unwrap(), data);

    // --- keeping a copy moves it out of the board folder for good
    c.call("board_keep", json!({ "id": id })).await;
    assert_eq!(std::fs::read(c.dir.join("keep/plan.pdf")).unwrap(), data);
    assert!(!c.dir.join("board/plan.pdf").exists());
    assert_eq!(c.board("plan.pdf")["kept"], true);

    // --- removing on one device removes everywhere; only un-kept fetched copies are deleted
    c.call("board_remove", json!({ "id": id })).await;
    until("removed everywhere", || a.board("plan.pdf").is_null() && b.board("plan.pdf").is_null()).await;
    until("bravo's copy deleted", || !b.dir.join("board/plan.pdf").exists()).await;
    assert!(c.dir.join("keep/plan.pdf").exists(), "a kept copy stays");
    assert_eq!(std::fs::read(a.dir.join("src/plan.pdf")).unwrap(), data, "the owner's file is never touched");
    assert_eq!(live(&a), 1);

    // --- an always-on device fetches whatever appears, folders included
    b.call("set_always_on", json!({ "on": true })).await;
    a.file("trip/a.jpg", &random(300_000));
    a.file("trip/day2/b.jpg", &random(200_000));
    a.call("board_put", json!({ "paths": [a.dir.join("src/trip")] })).await;
    until("always-on device fetched the folder", || b.board("trip")["state"] == "here").await;
    assert_eq!(b.board("trip")["files"], 2);
    assert_eq!(std::fs::read(b.dir.join("board/trip/day2/b.jpg")).unwrap(), std::fs::read(a.dir.join("src/trip/day2/b.jpg")).unwrap());
    until("charlie sees bravo as source", || c.board("trip")["sources"] == json!(["bravo"])).await;

    // --- a device that joins later gets the whole board when it connects
    let d = Device::start("delta").await;
    pair(&c, &d).await;
    until("late device caught up", || live(&d) == 2).await;
    assert_eq!(d.board("trip")["from"], "alpha");
    // ... and what it removes is removed for everyone, including devices two hops away.
    d.call("board_remove", json!({ "id": d.board("https://example.com/page")["id"] })).await;
    until("removal travelled back", || a.board("https://example.com/page").is_null()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unrequested_board_content_is_an_ordinary_offer() {
    use dbd_core::proto::{write_msg, FileEntry, Offer, Req};
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    pair(&a, &b).await;
    // A paired device cannot slip a file past the confirmation by labelling it as board content.
    let mut stream = a.core.raw_stream(&b.core.id).await.unwrap();
    let offer = Offer { id: "feedface".into(), files: vec![FileEntry { rel: "sneaky.bin".into(), size: 4 }], board: Some("whatever".into()) };
    write_msg(&mut stream, &Req::Offer { offer }).await.unwrap();
    until("asks the user", || b.snap()["offers"].as_array().unwrap().len() == 1).await;
    assert!(!b.dir.join("board").join("sneaky.bin").exists());
}
