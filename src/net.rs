use std::net::{IpAddr, UdpSocket};

/// Best-effort discovery of the address other devices on the LAN can reach
/// this machine at. Connecting a UDP socket sends no packets — it just asks
/// the routing table which local interface would be used.
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}
