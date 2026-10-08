//! Messages in English. Kept in its own test binary because the language is a process-wide setting.

mod common;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn english_messages() {
    let a = Device::start("alpha").await;
    let b = Device::start("bravo").await;
    a.call("set_lang", json!({ "lang": "en" })).await;
    assert_eq!(a.snap()["settings"]["lang"], "en");
    // The other device answers with a code; the wording is chosen here.
    let err = a.core.call("pair_addr", json!({ "host": "127.0.0.1", "port": b.core.port })).await.unwrap_err();
    assert!(err.to_string().contains("New connection"), "{err}");
    let err = a.core.call("board_text", json!({ "text": "   " })).await.unwrap_err();
    assert_eq!(err.to_string(), "The text is empty");
}
