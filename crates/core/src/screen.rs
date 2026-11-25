//! What the remote needs from the controlled computer besides keys: a small picture of its
//! screen, and the few system actions a phone may ask for (lock, screen off, sleep).
//!
//! The picture is taken with each system's own interface, so no capture library and no
//! system package is needed: X11 on Linux, GDI on Windows, Core Graphics on macOS.

use std::process::Command;

/// Width of the preview picture; enough to read a slide title on a phone.
const PREVIEW_W: usize = 720;

/// The screen as a small JPEG.
#[cfg(all(feature = "remote", not(target_os = "android")))]
pub fn preview() -> Result<Vec<u8>, String> {
    let (w, h, stride, bgra) = grab()?;
    let (tw, th, rgb) = shrink(w, h, stride, &bgra);
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 62).encode(&rgb, tw as u16, th as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(not(all(feature = "remote", not(target_os = "android"))))]
pub fn preview() -> Result<Vec<u8>, String> {
    Err(crate::i18n::remote("remote_none"))
}

/// [`preview`] as base64, the way it travels to another device.
pub fn preview_base64() -> Result<String, String> {
    #[cfg(all(feature = "remote", not(target_os = "android")))]
    {
        use base64::Engine;
        preview().map(|j| base64::engine::general_purpose::STANDARD.encode(j))
    }
    #[cfg(not(all(feature = "remote", not(target_os = "android"))))]
    preview().map(|_| String::new())
}

/// Shrinks a BGRA (or BGRX) picture to [`PREVIEW_W`] wide, averaging the pixels each one covers,
/// and returns it as RGB.
#[allow(dead_code)]
fn shrink(w: usize, h: usize, stride: usize, bgra: &[u8]) -> (usize, usize, Vec<u8>) {
    let tw = w.clamp(1, PREVIEW_W);
    let th = (h * tw / w.max(1)).max(1);
    let mut out = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1).min(h));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1).min(w));
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            // Every second row and column is plenty for a picture this small, and twice as fast.
            for y in (y0..y1).step_by(2) {
                let row = y * stride;
                for x in (x0..x1).step_by(2) {
                    let i = row + x * 4;
                    if i + 2 < bgra.len() {
                        b += bgra[i] as u32;
                        g += bgra[i + 1] as u32;
                        r += bgra[i + 2] as u32;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    (tw, th, out)
}

/// Width, height, bytes per row, and the pixels as BGRA.
#[cfg(all(feature = "remote", unix, not(target_os = "macos"), not(target_os = "android")))]
fn grab() -> Result<(usize, usize, usize, Vec<u8>), String> {
    use x11rb::{connection::Connection, protocol::xproto::{ConnectionExt, ImageFormat}};
    let (conn, n) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let screen = &conn.setup().roots[n];
    let (w, h) = (screen.width_in_pixels, screen.height_in_pixels);
    let img = conn.get_image(ImageFormat::Z_PIXMAP, screen.root, 0, 0, w, h, !0).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
    let (w, h) = (w as usize, h as usize);
    if img.depth < 24 || img.data.len() < w * h * 4 {
        return Err(format!("unsupported screen depth {}", img.depth));
    }
    Ok((w, h, img.data.len() / h, img.data))
}

#[cfg(all(feature = "remote", windows))]
fn grab() -> Result<(usize, usize, usize, Vec<u8>), String> {
    use windows_sys::Win32::{
        Graphics::Gdi::*,
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };
    // GDI shrinks while it copies, so only the small picture is ever held here.
    unsafe {
        let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        if w <= 0 || h <= 0 {
            return Err("no screen".into());
        }
        let tw = (w as usize).min(PREVIEW_W) as i32;
        let th = ((h as i64 * tw as i64) / w as i64).max(1) as i32;
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, tw, th);
        let old = SelectObject(dc, bmp as _);
        SetStretchBltMode(dc, HALFTONE);
        let copied = StretchBlt(dc, 0, 0, tw, th, screen, 0, 0, w, h, SRCCOPY) != 0;
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = tw;
        info.bmiHeader.biHeight = -th; // top row first
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut buf = vec![0u8; tw as usize * th as usize * 4];
        let lines = GetDIBits(dc, bmp, 0, th as u32, buf.as_mut_ptr() as _, &mut info, DIB_RGB_COLORS);
        SelectObject(dc, old);
        DeleteObject(bmp as _);
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);
        if !copied || lines == 0 {
            return Err("the screen could not be copied".into());
        }
        Ok((tw as usize, th as usize, tw as usize * 4, buf))
    }
}

#[cfg(all(feature = "remote", target_os = "macos"))]
fn grab() -> Result<(usize, usize, usize, Vec<u8>), String> {
    use core_graphics::display::CGDisplay;
    // Without the "Screen Recording" permission macOS hands out only the wallpaper.
    let img = CGDisplay::main().image().ok_or("the screen could not be copied")?;
    if img.bits_per_pixel() != 32 {
        return Err(format!("unsupported screen format ({} bits)", img.bits_per_pixel()));
    }
    let data = img.data();
    Ok((img.width(), img.height(), img.bytes_per_row(), data.bytes().to_vec()))
}

/// Locks the screen, turns the display off, or puts the computer to sleep.
pub fn system(act: &str) -> Result<(), String> {
    #[cfg(all(feature = "remote", windows))]
    {
        use windows_sys::Win32::{
            System::{Power::SetSuspendState, Shutdown::LockWorkStation},
            UI::WindowsAndMessaging::{PostMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND},
        };
        let ok = unsafe {
            match act {
                "lock" => LockWorkStation() != 0,
                // 2 means "off"; any key or mouse movement turns it back on.
                "screen_off" => PostMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2) != 0,
                "sleep" => SetSuspendState(false, false, false),
                _ => return Ok(()),
            }
        };
        return if ok { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) };
    }
    #[allow(unreachable_code)]
    {
        let tries: &[&[&str]] = if cfg!(target_os = "macos") {
            match act {
                "lock" => &[&["osascript", "-e", "tell application \"System Events\" to keystroke \"q\" using {control down, command down}"], &["pmset", "displaysleepnow"]],
                "screen_off" => &[&["pmset", "displaysleepnow"]],
                "sleep" => &[&["pmset", "sleepnow"]],
                _ => return Ok(()),
            }
        } else {
            match act {
                "lock" => &[&["loginctl", "lock-session"], &["xdg-screensaver", "lock"]],
                "screen_off" => &[
                    &["xset", "dpms", "force", "off"],
                    &["busctl", "--user", "set-property", "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig", "org.gnome.Mutter.DisplayConfig", "PowerSaveMode", "i", "1"],
                ],
                "sleep" => &[&["systemctl", "suspend"]],
                _ => return Ok(()),
            }
        };
        let mut last = String::new();
        for t in tries {
            match Command::new(t[0]).args(&t[1..]).output() {
                Ok(o) if o.status.success() => return Ok(()),
                Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => last = format!("{}: {e}", t[0]),
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shrink_averages() {
        // 4×2 picture: left half black, right half white.
        let mut px = vec![];
        for _ in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (w, h, rgb) = super::shrink(4, 2, 16, &px);
        assert_eq!((w, h), (4, 2));
        assert_eq!(&rgb[..3], &[0, 0, 0]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }
}
