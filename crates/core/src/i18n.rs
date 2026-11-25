//! Two languages, Persian and English.
//!
//! The core words its own messages in the language chosen on this device.
//! What another device tells us arrives as a short code and is worded here, so each side reads its own language.

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

pub(crate) fn set(lang: &str) {
    EN.store(lang == "en", Ordering::Relaxed);
}

pub(crate) fn en() -> bool {
    EN.load(Ordering::Relaxed)
}

/// `tr!("فارسی {}", "English {}", arg)`: the same arguments go into whichever wording is active.
macro_rules! tr {
    ($fa:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::i18n::en() { format!($en $(, $a)*) } else { format!($fa $(, $a)*) }
    };
}
pub(crate) use tr;

/// Wording for a code sent by another device. Anything unknown is shown as it came.
pub(crate) fn remote(code: &str) -> String {
    match code {
        "not_paired" => tr!("این دستگاه جفت نشده است", "This device is not paired"),
        "pairing_closed" => tr!("روی دستگاه مقصد «اتصال جدید» باز نیست", "“New connection” is not open on the other device"),
        "bad_list" => tr!("فهرست فایل‌ها نامعتبر است", "The file list is not valid"),
        "cancelled" => tr!("گیرنده لغو کرد", "The receiver cancelled"),
        "declined" => tr!("گیرنده نپذیرفت", "The receiver declined"),
        "board_gone" => tr!("این فایل دیگر روی آن دستگاه نیست", "That device no longer has this file"),
        "remote_off" => tr!("کنترل از راه دور روی آن دستگاه خاموش است", "The remote is switched off on that device"),
        "remote_none" => tr!("آن دستگاه را نمی‌شود از راه دور کنترل کرد", "That device cannot be controlled remotely"),
        "clip_none" => tr!("آن دستگاه به کلیپ‌بورد دسترسی ندارد", "That device has no clipboard to use"),
        "clip_empty" => tr!("کلیپ‌بورد خالی است یا متن ندارد", "The clipboard is empty or holds no text"),
        other => other.to_string(),
    }
}
