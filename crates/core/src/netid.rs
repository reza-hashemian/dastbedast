//! Recognising which network the device is on.
//!
//! A network is identified by the hardware address of its gateway, so "home"
//! stays "home" even when the DHCP lease or the Wi-Fi name changes.

use std::net::{Ipv4Addr, UdpSocket};

#[derive(Clone, Debug, PartialEq)]
pub struct NetInfo {
    pub id: String,
    pub local_ip: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub iface: String,
}

fn local_ip_towards(target: Ipv4Addr) -> Option<Ipv4Addr> {
    // Connecting a UDP socket sends nothing; it only makes the OS pick the outgoing address.
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect((target, 9)).ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
pub fn detect() -> Option<NetInfo> {
    fn hex_ip(s: &str) -> Option<Ipv4Addr> {
        u32::from_str_radix(s, 16).ok().map(|v| Ipv4Addr::from(v.to_le_bytes()))
    }
    let routes = std::fs::read_to_string("/proc/net/route").ok()?;
    let rows: Vec<Vec<&str>> = routes.lines().skip(1).map(|l| l.split_whitespace().collect()).filter(|r: &Vec<&str>| r.len() >= 8).collect();
    // Default route with the lowest metric.
    let default = rows
        .iter()
        .filter(|r| r[1] == "00000000" && r[7] == "00000000")
        .min_by_key(|r| r[6].parse::<u32>().unwrap_or(u32::MAX))?;
    let iface = default[0].to_string();
    let gateway = hex_ip(default[2]).filter(|g| !g.is_unspecified());
    let mask = rows
        .iter()
        .find(|r| r[0] == iface && r[1] != "00000000" && r[2] == "00000000")
        .and_then(|r| hex_ip(r[7]))
        .unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
    let local_ip = local_ip_towards(gateway.unwrap_or(Ipv4Addr::new(8, 8, 8, 8)))?;

    let mac = gateway.and_then(|gw| {
        let arp = std::fs::read_to_string("/proc/net/arp").ok()?;
        arp.lines().skip(1).find_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (c.len() >= 6 && c[0] == gw.to_string() && c[5] == iface && c[3] != "00:00:00:00:00:00").then(|| c[3].to_lowercase())
        })
    });
    let subnet = Ipv4Addr::from(u32::from(local_ip) & u32::from(mask));
    let id = match (mac, gateway) {
        (Some(mac), _) => format!("mac:{mac}"),
        (None, Some(gw)) => format!("gw:{gw}/{subnet}"),
        (None, None) => format!("if:{iface}/{subnet}"),
    };
    Some(NetInfo { id, local_ip, mask, gateway, iface })
}

/// The default route in the output of Windows `route print -4`: gateway, this device's address and its netmask.
/// Only the numeric columns are read, so the language of the system does not matter.
#[cfg(any(windows, test))]
fn parse_route_print(out: &str) -> Option<(Ipv4Addr, Ipv4Addr, Ipv4Addr)> {
    let ip = |s: &str| s.parse::<Ipv4Addr>().ok();
    // destination, netmask, ..., interface address, metric
    let rows: Vec<(Ipv4Addr, Ipv4Addr, Vec<&str>)> = out
        .lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (c.len() >= 5).then(|| Some((ip(c[0])?, ip(c[1])?, c))).flatten()
        })
        .collect();
    let (gateway, local) = rows
        .iter()
        .filter(|(dest, mask, _)| dest.is_unspecified() && mask.is_unspecified())
        .filter_map(|(_, _, c)| Some((ip(c[2])?, ip(c[c.len() - 2])?, c[c.len() - 1].parse::<u32>().ok()?)))
        .min_by_key(|r| r.2)
        .map(|r| (r.0, r.1))?;
    // The route to the device's own subnet carries the netmask.
    let mask = rows
        .iter()
        .find(|(dest, mask, c)| {
            let m = u32::from(*mask);
            m != 0 && m != u32::MAX && ip(c[c.len() - 2]) == Some(local) && u32::from(*dest) == u32::from(local) & m && !dest.is_loopback() && !dest.is_multicast()
        })
        .map(|r| r.1)
        .unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
    Some((gateway, local, mask))
}

/// The hardware address of `ip` in the output of Windows `arp -a`.
#[cfg(any(windows, test))]
fn parse_arp(out: &str, ip: Ipv4Addr) -> Option<String> {
    let ip = ip.to_string();
    out.lines().find_map(|l| {
        let c: Vec<&str> = l.split_whitespace().collect();
        let mac = c.get(1)?.to_lowercase().replace('-', ":");
        let shaped = mac.len() == 17 && mac.split(':').all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()));
        (c[0] == ip && shaped && mac != "00:00:00:00:00:00" && mac != "ff:ff:ff:ff:ff:ff").then_some(mac)
    })
}

/// Runs a system tool without flashing a console window.
#[cfg(windows)]
fn tool(program: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(windows)]
pub fn detect() -> Option<NetInfo> {
    use std::{sync::Mutex, time::Instant};
    // Asking the system tools costs two processes, so the answer is reused while the address stays the same.
    static LAST: Mutex<Option<(Instant, NetInfo)>> = Mutex::new(None);
    let quick = local_ip_towards(Ipv4Addr::new(8, 8, 8, 8));
    if let Some((at, info)) = LAST.lock().unwrap().as_ref() {
        // Without the gateway's hardware address the answer is provisional; look again soon.
        let fresh = if info.id.starts_with("mac:") { 60 } else { 8 };
        if Some(info.local_ip) == quick && at.elapsed().as_secs() < fresh {
            return Some(info.clone());
        }
    }
    let found = (|| {
        let (gateway, local_ip, mask) = parse_route_print(&tool("route", &["print", "-4"])?)?;
        let mac = tool("arp", &["-a"]).and_then(|out| parse_arp(&out, gateway));
        let subnet = Ipv4Addr::from(u32::from(local_ip) & u32::from(mask));
        let id = match mac {
            Some(mac) => format!("mac:{mac}"),
            None => format!("gw:{gateway}/{subnet}"),
        };
        Some(NetInfo { id, local_ip, mask, gateway: Some(gateway), iface: "wlan".into() })
    })();
    *LAST.lock().unwrap() = found.clone().map(|info| (Instant::now(), info));
    found
}

// Android gets gateway-based detection in its own phase.
// Until then the subnet stands in for the network identity.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn detect() -> Option<NetInfo> {
    let local_ip = local_ip_towards(Ipv4Addr::new(8, 8, 8, 8))?;
    let mask = Ipv4Addr::new(255, 255, 255, 0);
    let subnet = Ipv4Addr::from(u32::from(local_ip) & u32::from(mask));
    Some(NetInfo { id: format!("subnet:{subnet}"), local_ip, mask, gateway: None, iface: String::new() })
}

/// A human name to offer when the user saves this network (the NetworkManager connection name).
#[cfg(target_os = "linux")]
pub async fn suggest_name(iface: &str) -> Option<String> {
    if iface.is_empty() {
        return None;
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::process::Command::new("nmcli").args(["-t", "-f", "NAME,DEVICE", "connection", "show", "--active"]).output(),
    )
    .await
    .ok()?
    .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let (name, dev) = l.rsplit_once(':')?;
        (dev == iface && !name.is_empty()).then(|| name.replace("\\:", ":"))
    })
}

/// The Wi-Fi name in the output of Windows `netsh wlan show interfaces`.
#[cfg(any(windows, test))]
fn parse_ssid(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let (key, value) = l.split_once(':')?;
        (key.trim() == "SSID" && !value.trim().is_empty()).then(|| value.trim().to_string())
    })
}

/// A human name to offer when the user saves this network (the Wi-Fi name, when on Wi-Fi).
#[cfg(windows)]
pub async fn suggest_name(_iface: &str) -> Option<String> {
    tokio::task::spawn_blocking(|| tool("netsh", &["wlan", "show", "interfaces"]).and_then(|out| parse_ssid(&out))).await.ok().flatten()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub async fn suggest_name(_iface: &str) -> Option<String> {
    None
}

/// Host addresses of the local subnet, capped so a huge corporate range is never swept.
pub fn subnet_hosts(net: &NetInfo) -> Vec<Ipv4Addr> {
    let ip = u32::from(net.local_ip);
    let mut mask = u32::from(net.mask);
    if mask.count_zeros() > 10 {
        mask = 0xFFFF_FF00;
    }
    let base = ip & mask;
    let size = !mask;
    (1..size).map(|i| Ipv4Addr::from(base + i)).filter(|a| *a != net.local_ip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str = "\
===========================================================================
Interface List
 12...a4 b1 c1 00 11 22 ......Intel(R) Wi-Fi 6 AX201 160MHz
  1...........................Software Loopback Interface 1
===========================================================================

IPv4 Route Table
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      192.168.1.1    192.168.1.34     35
          0.0.0.0          0.0.0.0       10.8.0.1        10.8.0.6     290
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
      192.168.0.0    255.255.254.0         On-link      192.168.1.34    291
     192.168.1.34  255.255.255.255         On-link      192.168.1.34    291
        224.0.0.0        240.0.0.0         On-link      192.168.1.34    291
===========================================================================
";

    #[test]
    fn windows_default_route() {
        let (gw, local, mask) = parse_route_print(ROUTE).unwrap();
        assert_eq!((gw.to_string(), local.to_string(), mask.to_string()), ("192.168.1.1".into(), "192.168.1.34".into(), "255.255.254.0".into()));
        // A translated "On-link" made of two words must not shift the columns.
        assert_eq!(parse_route_print(&ROUTE.replace("On-link", "Auf Verbindung")).unwrap().2.to_string(), "255.255.254.0");
        assert!(parse_route_print("nothing here").is_none());
    }

    #[test]
    fn windows_arp_and_ssid() {
        let arp = "\nInterface: 192.168.1.34 --- 0xc\n  Internet Address      Physical Address      Type\n  192.168.1.1           3C-84-6A-0B-11-F2     dynamic\n  192.168.1.255         ff-ff-ff-ff-ff-ff     static\n";
        assert_eq!(parse_arp(arp, Ipv4Addr::new(192, 168, 1, 1)).as_deref(), Some("3c:84:6a:0b:11:f2"));
        assert_eq!(parse_arp(arp, Ipv4Addr::new(192, 168, 1, 255)), None);
        assert_eq!(parse_arp(arp, Ipv4Addr::new(192, 168, 1, 9)), None);
        let wlan = "    Name                   : Wi-Fi\n    State                  : connected\n    SSID                   : Home 5G\n    BSSID                  : 3c:84:6a:0b:11:f3\n";
        assert_eq!(parse_ssid(wlan).as_deref(), Some("Home 5G"));
    }
}
