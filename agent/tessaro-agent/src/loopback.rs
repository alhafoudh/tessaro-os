//! Who else is connected to a port on this device's loopback: a technician's
//! DevTools on 9222 (`cdp/clients.rs`), a VNC viewer on 5900
//! (`control/vnc.rs`), both through an SSH tunnel.
//!
//! The servers do not say, so the kernel is asked instead: every established
//! connection to the port has a client end whose *remote* port is that port,
//! the agent's own client ends are the sockets in `/proc/self/fd`, and
//! whatever is left belongs to someone else. dropbear holds the client end of
//! a forward, so a tunnel counts only while something is connected through
//! it, not for as long as ssh is up.

use std::collections::HashSet;
use std::fs;

/// `TCP_ESTABLISHED` in `/proc/net/tcp`'s `st` column.
const ESTABLISHED: &str = "01";

/// `TCP_LISTEN`, the same column.
const LISTEN: &str = "0A";

/// Blocking (`/proc` reads): something listens on `port`. Read from the
/// tables rather than by connecting, which a server would see as a client.
pub fn listening(port: u16) -> bool {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .any(|table| fs::read_to_string(table).is_ok_and(|text| listens(&text, port)))
}

/// A socket in one `/proc/net/tcp` or `tcp6` table listens on `port`.
fn listens(table: &str, port: u16) -> bool {
    let wanted = format!("{port:04X}");
    table.lines().skip(1).any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let local_port = fields.get(1).and_then(|local| local.rsplit_once(':'));
        local_port.is_some_and(|(_, local_port)| local_port == wanted)
            && fields.get(3) == Some(&LISTEN)
    })
}

/// Blocking (`/proc` reads): the connections to `port` on the loopback that
/// are not the agent's own. 0 when the kernel will not say.
///
/// The agent's sockets are read on both sides of the tables, so one it opens
/// or closes while they are being read is still recognised as its own.
pub fn foreign(port: u16) -> usize {
    let Some(before) = own_sockets() else {
        return 0;
    };
    let mut clients = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = fs::read_to_string(table) {
            clients.extend(client_ends(&text, port));
        }
    }
    let after = own_sockets().unwrap_or_default();
    clients
        .iter()
        .filter(|inode| !before.contains(inode) && !after.contains(inode))
        .count()
}

/// The inodes of the sockets this process holds.
fn own_sockets() -> Option<HashSet<u64>> {
    let dir = fs::read_dir("/proc/self/fd").ok()?;
    Some(
        dir.filter_map(|entry| {
            let link = fs::read_link(entry.ok()?.path()).ok()?;
            link.to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect(),
    )
}

/// The inodes of the established loopback connections *to* `port` in one
/// `/proc/net/tcp` or `tcp6` table: the client ends, never the server's.
fn client_ends(table: &str, port: u16) -> Vec<u64> {
    let wanted = format!("{port:04X}");
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (address, remote_port) = fields.get(2)?.rsplit_once(':')?;
            let inode: u64 = fields.get(9)?.parse().ok()?;
            let wanted = *fields.get(3)? == ESTABLISHED
                && remote_port == wanted
                && loopback(address)
                && inode != 0;
            wanted.then_some(inode)
        })
        .collect()
}

/// A `/proc/net/tcp` address on the loopback: 127/8, `::1`, or 127/8 mapped
/// into IPv6. The words are in host byte order, so on every little-endian
/// board 127.0.0.1 reads `0100007F`.
fn loopback(address: &str) -> bool {
    match address.len() {
        8 => address.ends_with("7F"),
        32 => {
            address == "00000000000000000000000001000000"
                || (address.starts_with("0000000000000000FFFF0000") && address.ends_with("7F"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:2406 00000000:0000 0A 00000000:00000000 00:00000000 00000000   998        0 1001 1 0000000000000000 100 0 0 10 0
   1: 0100007F:2406 0100007F:A1B2 01 00000000:00000000 00:00000000 00000000   998        0 1002 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:A1B2 0100007F:2406 01 00000000:00000000 00:00000000 00000000     0        0 1003 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:C3D4 0100007F:2406 06 00000000:00000000 03:00000000 00000000     0        0 0 3 0000000000000000
   4: 0A00020F:E5F6 5DB8D822:2406 01 00000000:00000000 00:00000000 00000000     0        0 1004 1 0000000000000000 20 4 30 10 -1
   5: 0100007F:B7C8 0100007F:0050 01 00000000:00000000 00:00000000 00000000     0        0 1005 1 0000000000000000 20 4 30 10 -1
   6: 0100007F:170C 0100007F:D5E6 01 00000000:00000000 00:00000000 00000000   998        0 1006 1 0000000000000000 20 4 30 10 -1
   7: 0100007F:D5E6 0100007F:170C 01 00000000:00000000 00:00000000 00000000     0        0 1007 1 0000000000000000 20 4 30 10 -1
";

    #[test]
    fn only_established_loopback_client_ends_are_found() {
        // Not the listener, not the server's end, not a closing socket, not a
        // connection to port 9222 somewhere else, not another port.
        assert_eq!(client_ends(TCP, 9222), vec![1003]);
        assert_eq!(client_ends(TCP, 80), vec![1005]);
        // A VNC viewer through the tunnel, the same way.
        assert_eq!(client_ends(TCP, 5900), vec![1007]);
        assert!(client_ends("", 9222).is_empty());
    }

    #[test]
    fn a_listener_is_found_by_its_port_and_state() {
        assert!(listens(TCP, 9222));
        // Connected on 5900 and 80, listening on neither.
        assert!(!listens(TCP, 5900));
        assert!(!listens(TCP, 80));
        assert!(!listens("", 9222));
    }

    #[test]
    fn ipv6_loopback_and_mapped_ipv4_count() {
        let tcp6 = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:D001 00000000000000000000000001000000:2406 01 00000000:00000000 00:00000000 00000000     0        0 2001 1 0
   1: 0000000000000000FFFF00000100007F:D002 0000000000000000FFFF00000100007F:2406 01 00000000:00000000 00:00000000 00000000     0        0 2002 1 0
   2: 20010DB8000000000000000000000001:D003 20010DB8000000000000000000000002:2406 01 00000000:00000000 00:00000000 00000000     0        0 2003 1 0
";
        assert_eq!(client_ends(tcp6, 9222), vec![2001, 2002]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    // A plain test thread, not the runtime: a blocking connect is fine here.
    #[allow(clippy::disallowed_methods)]
    fn a_connection_of_our_own_is_not_foreign() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _server = listener.accept().unwrap();
        assert_eq!(foreign(port), 0);
    }
}
