//! Online detection. DS3 Pause is offline only.
//!
//! Measured on a real install: "Start offline" Dark Souls III opens no network
//! sockets at all, while online DS3 opens UDP ports once it connects to
//! FromSoftware's servers (it talks over UDP, not TCP). So: if the game's
//! process owns any non-loopback UDP port or any active TCP connection, it's
//! online. We read Windows' connection tables (what `netstat -ano` shows); no
//! game memory is read.

use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const STATE_SYN_SENT: u32 = 3;
const STATE_ESTABLISHED: u32 = 5;

#[derive(Clone, Copy)]
enum Kind {
    Tcp,
    Udp,
}

fn table(kind: Kind, af: u32) -> Option<Vec<u8>> {
    unsafe {
        let get = |buf: *mut u8, sz: &mut u32| match kind {
            Kind::Tcp => GetExtendedTcpTable(buf as *mut _, sz, 0, af, TCP_TABLE_OWNER_PID_ALL, 0),
            Kind::Udp => GetExtendedUdpTable(buf as *mut _, sz, 0, af, UDP_TABLE_OWNER_PID, 0),
        };
        let mut size = 0u32;
        get(std::ptr::null_mut(), &mut size);
        for _ in 0..4 {
            let mut buf = vec![0u8; size as usize + 4096];
            let mut sz = buf.len() as u32;
            if get(buf.as_mut_ptr(), &mut sz) == 0 {
                buf.truncate(sz as usize);
                return Some(buf);
            }
            size = sz; // table grew, retry
        }
        None
    }
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn loopback4(addr: u32) -> bool {
    addr.to_le_bytes()[0] == 127
}

fn loopback6(a: &[u8]) -> bool {
    let plain = a[..15].iter().all(|&x| x == 0) && a[15] == 1;
    let mapped = a[..10].iter().all(|&x| x == 0) && a[10] == 0xFF && a[11] == 0xFF && a[12] == 127;
    plain || mapped
}

/// Network endpoints that show the game is online (empty = offline).
/// Errors if Windows won't give us the tables (treated as "can't confirm").
pub fn online_signs(pid: u32) -> Result<Vec<String>, ()> {
    let mut signs = Vec::new();

    // UDP, IPv4: MIB_UDPROW_OWNER_PID = local addr, local port, pid (12 bytes)
    let u4 = table(Kind::Udp, AF_INET).ok_or(())?;
    for i in 0..u32_at(&u4, 0) as usize {
        let o = 4 + i * 12;
        if o + 12 > u4.len() {
            break;
        }
        let (addr, port, owner) = (u32_at(&u4, o), u32_at(&u4, o + 4), u32_at(&u4, o + 8));
        if owner == pid && !loopback4(addr) {
            signs.push(format!("UDP port {}", u16::from_be(port as u16)));
        }
    }
    // UDP, IPv6: MIB_UDP6ROW_OWNER_PID = addr[16], scope, port, pid (28 bytes)
    if let Some(u6) = table(Kind::Udp, AF_INET6) {
        for i in 0..u32_at(&u6, 0) as usize {
            let o = 4 + i * 28;
            if o + 28 > u6.len() {
                break;
            }
            let (port, owner) = (u32_at(&u6, o + 20), u32_at(&u6, o + 24));
            if owner == pid && !loopback6(&u6[o..o + 16]) {
                signs.push(format!("UDP6 port {}", u16::from_be(port as u16)));
            }
        }
    }

    // TCP, IPv4: MIB_TCPROW_OWNER_PID = state, laddr, lport, raddr, rport, pid (24 bytes)
    let t4 = table(Kind::Tcp, AF_INET).ok_or(())?;
    for i in 0..u32_at(&t4, 0) as usize {
        let o = 4 + i * 24;
        if o + 24 > t4.len() {
            break;
        }
        let (state, raddr, rport, owner) = (u32_at(&t4, o), u32_at(&t4, o + 12), u32_at(&t4, o + 16), u32_at(&t4, o + 20));
        if owner == pid && (state == STATE_ESTABLISHED || state == STATE_SYN_SENT) && raddr != 0 && !loopback4(raddr) {
            let ip = raddr.to_le_bytes();
            signs.push(format!("TCP {}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], u16::from_be(rport as u16)));
        }
    }
    // TCP, IPv6: MIB_TCP6ROW_OWNER_PID (56 bytes)
    if let Some(t6) = table(Kind::Tcp, AF_INET6) {
        for i in 0..u32_at(&t6, 0) as usize {
            let o = 4 + i * 56;
            if o + 56 > t6.len() {
                break;
            }
            let remote = &t6[o + 24..o + 40];
            let (rport, state, owner) = (u32_at(&t6, o + 44), u32_at(&t6, o + 48), u32_at(&t6, o + 52));
            let active = state == STATE_ESTABLISHED || state == STATE_SYN_SENT;
            if owner == pid && active && !remote.iter().all(|&x| x == 0) && !loopback6(remote) {
                signs.push(format!("TCP6 port {}", u16::from_be(rport as u16)));
            }
        }
    }
    Ok(signs)
}
