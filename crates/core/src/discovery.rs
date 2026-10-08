//! Finding devices on the local network with a small UDP beacon.
//!
//! A beacon only says "a device with this id is at this address". It is never trusted:
//! the id is checked against the TLS certificate when the address is actually dialed.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use crate::{
    proto::{Beacon, BEACON_PORT},
    Core,
};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 47, 80);

pub(crate) fn spawn(core: Core) -> Result<()> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.set_broadcast(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, BEACON_PORT)).into())?;
    let udp = Arc::new(UdpSocket::from_std(sock.into())?);
    let _ = udp.join_multicast_v4(GROUP, Ipv4Addr::UNSPECIFIED);

    let (c, u) = (core.clone(), udp.clone());
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let Ok((n, from)) = u.recv_from(&mut buf).await else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };
            let Ok(b) = serde_json::from_slice::<Beacon>(&buf[..n]) else { continue };
            if b.app != "dbd" || b.id == c.id || !c.discovery_active() {
                continue;
            }
            let addr = SocketAddr::new(from.ip(), b.info.port);
            c.saw_device(b.id, b.info, addr, Duration::from_secs(12));
        }
    });

    tokio::spawn(async move {
        loop {
            if core.discovery_active() {
                let beacon = Beacon { app: "dbd".into(), id: core.id.clone(), info: core.info() };
                if let Ok(body) = serde_json::to_vec(&beacon) {
                    let directed = core.rt.lock().unwrap().net.as_ref().map(|n| Ipv4Addr::from(u32::from(n.local_ip) | !u32::from(n.mask)));
                    for ip in [Some(Ipv4Addr::BROADCAST), directed, Some(GROUP)].into_iter().flatten() {
                        let _ = udp.send_to(&body, (ip, BEACON_PORT)).await;
                    }
                }
            }
            let dropped = {
                let mut rt = core.rt.lock().unwrap();
                let before = rt.nearby.len();
                rt.nearby.retain(|_, n| n.seen.elapsed() < n.ttl);
                before != rt.nearby.len()
            };
            if dropped {
                core.changed();
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
    Ok(())
}
