# DastBeDast · دست‌به‌دست

Send files between your own devices — Windows, Linux and Android — over the network they share.
Pair two devices once and they find each other from then on. No account, no cloud, no server.

*«دست‌به‌دست» means “hand to hand”. The app speaks Persian and English.*

## What it does

- **Pair once.** Two devices compare a 6-digit code; after that they recognise each other on any saved network.
- **Networks are remembered.** Home, office and so on are recognised by themselves when you join them.
- **Direct send.** Pick a device, pick files or folders. The receiver asks first or accepts by itself — you choose per device, per network, or as a default.
- **Inbox.** Received files wait in one folder until you keep or delete them.
- **Shared board.** Put a file, a note or a link out once and every device of yours sees it. Files move only when a device asks for them.
- **Always-on device.** One device can keep a copy of everything on the board, so the others can get it while its owner is off.
- **Static address.** A device with a public IP and a forwarded port can be reached from anywhere; devices behind NAT are called back over the same link.
- **Browser guest.** A device without the app sends and receives through a link and a QR code.
- **Interrupted transfers continue** from where they stopped, and every file is verified with BLAKE3.

## Status

| Platform | State |
|---|---|
| Linux | works; `.deb` package |
| Windows | builds in CI; not yet tried on real hardware |
| Android | in progress |

## Install

Packages are on the [releases page](../../releases).

- **Linux:** `sudo apt install ./DastBeDast_*_amd64.deb`
- **Windows:** run `DastBeDast_*_x64-setup.exe`. Windows asks once whether to allow it through the firewall; say yes for private networks.
- **Headless (server, always-on box):** run `dbd` and open the printed local address in a browser.

Devices talk on TCP port 47800 and find each other with a UDP beacon on 47801.

## How it is built

- `crates/core` — everything that matters, in Rust: identity, pairing, presence, transfers, the shared board. Each device has a self-signed certificate; its fingerprint is its identity, and every connection is mutual TLS 1.3 pinned to the paired fingerprints.
- `crates/daemon` — `dbd`, the core without a window, serving the same UI on localhost.
- `app` — the desktop and mobile shell (Tauri 2).
- `ui` — plain HTML, CSS and JavaScript. No bundler, no npm.

```bash
cargo test                         # core tests: several devices on loopback
cargo run -p dbd                   # headless device with the UI in your browser
cd app && cargo tauri build        # desktop package (needs tauri-cli 2)
```

## License

MIT

The bundled [Vazirmatn](https://github.com/rastikerdar/vazirmatn) font is under the SIL Open Font License.
