//! Read-only local socket/connection monitoring.
//!
//! Parses `/proc/net/tcp`, `/proc/net/tcp6`, `/proc/net/udp`, and
//! `/proc/net/udp6` to enumerate active local connections. All operations
//! are strictly read-only — no connections are opened, closed, or probed.

/// Hard limit on stored connections to prevent unbounded memory growth.
const MAX_CONNECTIONS: usize = 2048;

/// Maximum number of data lines parsed per proc file (safety bound).
const MAX_LINES_PER_FILE: usize = 4096;

/// Parsed connection entry from `/proc/net/tcp[6]` or `/proc/net/udp[6]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub protocol: Protocol,
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
    pub state: ConnectionState,
}

/// Network protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// TCP connection state decoded from the kernel `st` field.
/// UDP entries use `UdpUnconn` for state code 07.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    UdpUnconn,
    Unknown(u32),
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Established => "ESTABLISHED",
            Self::SynSent => "SYN_SENT",
            Self::SynRecv => "SYN_RECV",
            Self::FinWait1 => "FIN_WAIT1",
            Self::FinWait2 => "FIN_WAIT2",
            Self::TimeWait => "TIME_WAIT",
            Self::Close => "CLOSE",
            Self::CloseWait => "CLOSE_WAIT",
            Self::LastAck => "LAST_ACK",
            Self::Listen => "LISTEN",
            Self::Closing => "CLOSING",
            Self::UdpUnconn => "UNCONN",
            Self::Unknown(_) => "UNKNOWN",
        }
    }
}

/// Snapshot of all parsed connections at a point in time.
#[derive(Debug, Clone, Default)]
pub struct ConnectionSnapshot {
    pub connections: Vec<Connection>,
    pub tcp_count: usize,
    pub udp_count: usize,
    pub listen_count: usize,
    pub established_count: usize,
    pub available: bool,
}

impl ConnectionSnapshot {
    /// Build a new snapshot by reading kernel proc files.
    pub fn collect() -> Self {
        let mut connections = Vec::with_capacity(256);

        // TCP IPv4
        if let Some(content) = read_file("/proc/net/tcp") {
            connections.extend(parse_tcp_table(&content));
        }
        // TCP IPv6
        if let Some(content) = read_file("/proc/net/tcp6") {
            connections.extend(parse_tcp6_table(&content));
        }
        // UDP IPv4
        if let Some(content) = read_file("/proc/net/udp") {
            connections.extend(parse_udp_table(&content));
        }
        // UDP IPv6
        if let Some(content) = read_file("/proc/net/udp6") {
            connections.extend(parse_udp6_table(&content));
        }

        let available = !connections.is_empty() || has_any_proc_net_file();

        // Enforce bound.
        if connections.len() > MAX_CONNECTIONS {
            connections.truncate(MAX_CONNECTIONS);
        }

        let tcp_count = connections
            .iter()
            .filter(|c| c.protocol == Protocol::Tcp)
            .count();
        let udp_count = connections
            .iter()
            .filter(|c| c.protocol == Protocol::Udp)
            .count();
        let listen_count = connections
            .iter()
            .filter(|c| c.state == ConnectionState::Listen)
            .count();
        let established_count = connections
            .iter()
            .filter(|c| c.state == ConnectionState::Established)
            .count();

        ConnectionSnapshot {
            connections,
            tcp_count,
            udp_count,
            listen_count,
            established_count,
            available,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a `/proc/net/tcp` table (IPv4).
fn parse_tcp_table(content: &str) -> Vec<Connection> {
    parse_table(content, Protocol::Tcp, false)
}

/// Parse a `/proc/net/tcp6` table (IPv6).
fn parse_tcp6_table(content: &str) -> Vec<Connection> {
    parse_table(content, Protocol::Tcp, true)
}

/// Parse a `/proc/net/udp` table (IPv4).
fn parse_udp_table(content: &str) -> Vec<Connection> {
    parse_table(content, Protocol::Udp, false)
}

/// Parse a `/proc/net/udp6` table (IPv6).
fn parse_udp6_table(content: &str) -> Vec<Connection> {
    parse_table(content, Protocol::Udp, true)
}

/// Generic parser for any of the four proc files.
/// Skips the header line, then parses each data line.
fn parse_table(content: &str, protocol: Protocol, ipv6: bool) -> Vec<Connection> {
    let mut connections = Vec::with_capacity(128);
    let mut lines_parsed = 0usize;

    for line in content.lines().skip(1) {
        lines_parsed += 1;
        if lines_parsed > MAX_LINES_PER_FILE {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(conn) = parse_table_line(line, protocol, ipv6) {
            connections.push(conn);
        }
    }
    connections
}

/// Parse a single data line from `/proc/net/tcp[6]` or `/proc/net/udp[6]`.
///
/// Format (IPv4):
/// ```text
///  sl  local_address rem_address   st tx_queue rx_queue ...
///  0: 3500007F:0035 00000000:0000 0A ...
/// ```
///
/// Format (IPv6):
/// ```text
///  0: 00000000000000000000000001000000:0277 00000000000000000000000000000000:0000 0A ...
/// ```
pub fn parse_table_line(line: &str, protocol: Protocol, ipv6: bool) -> Option<Connection> {
    // Split on whitespace. Fields: sl, local_address, rem_address, st, ...
    let mut parts = line.split_whitespace();
    let _sl = parts.next()?; // "0:"
    let local_hex = parts.next()?;
    let remote_hex = parts.next()?;
    let state_hex = parts.next()?;

    let state_code = parse_hex_u32(state_hex)?;
    let state = decode_state(state_code, protocol);

    let (local_addr, local_port) = if ipv6 {
        parse_ipv6_hex_address(local_hex)?
    } else {
        parse_ipv4_hex_address(local_hex)?
    };
    let (remote_addr, remote_port) = if ipv6 {
        parse_ipv6_hex_address(remote_hex)?
    } else {
        parse_ipv4_hex_address(remote_hex)?
    };

    Some(Connection {
        protocol,
        local_addr,
        local_port,
        remote_addr,
        remote_port,
        state,
    })
}

/// Parse a hex-encoded IPv4 address:port like `"3500007F:0035"`.
/// IPv4 addresses in /proc/net/tcp are stored as 32-bit hex in host byte
/// order (little-endian on x86), so `3500007F` → 127.0.0.53.
pub fn parse_ipv4_hex_address(hex: &str) -> Option<(String, u16)> {
    let colon = hex.find(':')?;
    let addr_hex = &hex[..colon];
    let port_hex = &hex[colon + 1..];

    let addr_val = parse_hex_u32(addr_hex)?;
    let port = parse_hex_u16(port_hex)?;

    // Decode little-endian: byte 0 is addr[3], byte 1 is addr[2], etc.
    let a = (addr_val >> 24) & 0xFF;
    let b = (addr_val >> 16) & 0xFF;
    let c = (addr_val >> 8) & 0xFF;
    let d = addr_val & 0xFF;
    let addr = format!("{d}.{c}.{b}.{a}");

    Some((addr, port))
}

/// Parse a hex-encoded IPv6 address:port like
/// `"00000000000000000000000001000000:0277"`.
/// IPv6 addresses in /proc/net/tcp6 are stored as four `__be32` words
/// (32 hex chars total, 8 per word). On little-endian hosts, byte-swap
/// each 32-bit word to recover the correct address bytes.
pub fn parse_ipv6_hex_address(hex: &str) -> Option<(String, u16)> {
    let colon = hex.find(':')?;
    let addr_hex = &hex[..colon];
    let port_hex = &hex[colon + 1..];

    if addr_hex.len() != 32 {
        return None;
    }

    let port = parse_hex_u16(port_hex)?;

    // Parse as four 32-bit words, byte-swap each, then assemble 16 bytes.
    let mut bytes = [0u8; 16];
    for i in 0..4 {
        let word_hex = &addr_hex[i * 8..i * 8 + 8];
        if let Ok(word) = u32::from_str_radix(word_hex, 16) {
            let swapped = word.swap_bytes();
            bytes[i * 4] = ((swapped >> 24) & 0xFF) as u8;
            bytes[i * 4 + 1] = ((swapped >> 16) & 0xFF) as u8;
            bytes[i * 4 + 2] = ((swapped >> 8) & 0xFF) as u8;
            bytes[i * 4 + 3] = (swapped & 0xFF) as u8;
        }
    }

    // Format as eight 16-bit groups in standard IPv6 notation.
    let groups: Vec<String> = (0..8)
        .map(|i| {
            let val = u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
            format!("{val:04x}")
        })
        .collect();

    let addr = groups.join(":");
    Some((addr, port))
}

/// Parse a hex string to `u32`, returning `None` on failure.
pub fn parse_hex_u32(hex: &str) -> Option<u32> {
    u32::from_str_radix(hex, 16).ok()
}

/// Parse a hex string to `u16`, returning `None` on failure.
pub fn parse_hex_u16(hex: &str) -> Option<u16> {
    u16::from_str_radix(hex, 16).ok()
}

/// Decode the kernel TCP/UDP state code into a human-readable enum.
pub fn decode_state(code: u32, protocol: Protocol) -> ConnectionState {
    match code {
        0x01 => ConnectionState::Established,
        0x02 => ConnectionState::SynSent,
        0x03 => ConnectionState::SynRecv,
        0x04 => ConnectionState::FinWait1,
        0x05 => ConnectionState::FinWait2,
        0x06 => ConnectionState::TimeWait,
        0x07 => {
            if protocol == Protocol::Udp {
                ConnectionState::UdpUnconn
            } else {
                ConnectionState::Close
            }
        }
        0x08 => ConnectionState::CloseWait,
        0x09 => ConnectionState::LastAck,
        0x0A => ConnectionState::Listen,
        0x0B => ConnectionState::Closing,
        _ => ConnectionState::Unknown(code),
    }
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Preset filter for quick protocol/state selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionFilter {
    All,
    Tcp,
    Udp,
    Listen,
    Established,
}

impl ConnectionFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
            Self::Listen => "LISTEN",
            Self::Established => "ESTABLISHED",
        }
    }

    pub const ALL: [ConnectionFilter; 5] = [
        ConnectionFilter::All,
        ConnectionFilter::Tcp,
        ConnectionFilter::Udp,
        ConnectionFilter::Listen,
        ConnectionFilter::Established,
    ];
}

/// Filter a snapshot by the given preset filter and optional text search.
/// Returns indices into `snapshot.connections`.
pub fn filter_connections(
    snapshot: &ConnectionSnapshot,
    filter: ConnectionFilter,
    search: &str,
) -> Vec<usize> {
    let search_lower = search.to_lowercase();
    let has_search = !search_lower.is_empty();

    snapshot
        .connections
        .iter()
        .enumerate()
        .filter(|(_, conn)| {
            // Protocol / state filter.
            let matches_filter = match filter {
                ConnectionFilter::All => true,
                ConnectionFilter::Tcp => conn.protocol == Protocol::Tcp,
                ConnectionFilter::Udp => conn.protocol == Protocol::Udp,
                ConnectionFilter::Listen => conn.state == ConnectionState::Listen,
                ConnectionFilter::Established => conn.state == ConnectionState::Established,
            };

            if !matches_filter {
                return false;
            }

            if !has_search {
                return true;
            }

            // Text search against all fields.
            let fields = [
                conn.local_addr.as_str(),
                &conn.local_port.to_string(),
                conn.remote_addr.as_str(),
                &conn.remote_port.to_string(),
                conn.protocol.label(),
                conn.state.label(),
            ];
            fields
                .iter()
                .any(|f| f.to_lowercase().contains(&search_lower))
        })
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn has_any_proc_net_file() -> bool {
    std::path::Path::new("/proc/net/tcp").exists()
        || std::path::Path::new("/proc/net/tcp6").exists()
        || std::path::Path::new("/proc/net/udp").exists()
        || std::path::Path::new("/proc/net/udp6").exists()
}

#[cfg(not(target_os = "linux"))]
fn read_file(_path: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
fn has_any_proc_net_file() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- /proc/net/tcp format fixtures ---

    const PROC_NET_TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 3500007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   989        0 8035 1 0000000000000000 100 0 0 10 5
   1: 0100007F:B6E3 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 50959 1 0000000000000000 100 0 0 10 0
   2: 0100007F:0019 8EA1B90A:0050 01 00000000:00000000 00:00000000 00000000  1000        0 60589 2 0000000000000000 22 4 0 10 -1";

    const PROC_NET_TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:0277 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 22096 1 0000000000000000 100 0 0 10 0";

    const PROC_NET_UDP: &str = "\
   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  675: 3600007F:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000   989        0 8036 2 0000000000000000 0";

    const PROC_NET_UDP6: &str = "\
   sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  651: 803A0224657485136E5048EA61747843:801D 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 45973 2 0000000000000000 0";

    // --- TCP row parsing ---

    #[test]
    fn parses_tcp_ipv4_row() {
        let conns = parse_tcp_table(PROC_NET_TCP);
        assert_eq!(conns.len(), 3);
        let first = &conns[0];
        assert_eq!(first.protocol, Protocol::Tcp);
        assert_eq!(first.local_addr, "127.0.0.53");
        assert_eq!(first.local_port, 53);
        assert_eq!(first.remote_addr, "0.0.0.0");
        assert_eq!(first.remote_port, 0);
        assert_eq!(first.state, ConnectionState::Listen);
    }

    #[test]
    fn parses_tcp_ipv4_established() {
        let conns = parse_tcp_table(PROC_NET_TCP);
        let est = conns
            .iter()
            .find(|c| c.state == ConnectionState::Established);
        assert!(est.is_some());
        let est = est.unwrap();
        assert_eq!(est.local_addr, "127.0.0.1");
        assert_eq!(est.local_port, 25);
        assert_eq!(est.remote_addr, "10.185.161.142");
        assert_eq!(est.remote_port, 80);
    }

    #[test]
    fn parses_tcp_ipv6_row() {
        let conns = parse_tcp6_table(PROC_NET_TCP6);
        assert_eq!(conns.len(), 1);
        let first = &conns[0];
        assert_eq!(first.protocol, Protocol::Tcp);
        assert_eq!(first.local_port, 631);
        assert_eq!(first.state, ConnectionState::Listen);
        // IPv6 address should contain colons.
        assert!(first.local_addr.contains(':'));
    }

    // --- UDP row parsing ---

    #[test]
    fn parses_udp_ipv4_row() {
        let conns = parse_udp_table(PROC_NET_UDP);
        assert_eq!(conns.len(), 1);
        let first = &conns[0];
        assert_eq!(first.protocol, Protocol::Udp);
        assert_eq!(first.local_addr, "127.0.0.54");
        assert_eq!(first.local_port, 53);
        assert_eq!(first.state, ConnectionState::UdpUnconn);
    }

    #[test]
    fn parses_udp_ipv6_row() {
        let conns = parse_udp6_table(PROC_NET_UDP6);
        assert_eq!(conns.len(), 1);
        let first = &conns[0];
        assert_eq!(first.protocol, Protocol::Udp);
        assert_eq!(first.local_port, 0x801D);
        assert_eq!(first.state, ConnectionState::UdpUnconn);
    }

    // --- IPv4 hex address decoding ---

    #[test]
    fn decodes_ipv4_loopback_53() {
        let (addr, port) = parse_ipv4_hex_address("3500007F:0035").unwrap();
        assert_eq!(addr, "127.0.0.53");
        assert_eq!(port, 53);
    }

    #[test]
    fn decodes_ipv4_loopback_1() {
        let (addr, port) = parse_ipv4_hex_address("0100007F:B6E3").unwrap();
        assert_eq!(addr, "127.0.0.1");
        assert_eq!(port, 0xB6E3);
    }

    #[test]
    fn decodes_ipv4_zeros() {
        let (addr, port) = parse_ipv4_hex_address("00000000:0000").unwrap();
        assert_eq!(addr, "0.0.0.0");
        assert_eq!(port, 0);
    }

    #[test]
    fn decodes_ipv4_real_ip() {
        let (addr, port) = parse_ipv4_hex_address("8EA1B90A:0050").unwrap();
        assert_eq!(addr, "10.185.161.142");
        assert_eq!(port, 80);
    }

    // --- Hex port decoding ---

    #[test]
    fn decodes_hex_port_ff() {
        assert_eq!(parse_hex_u16("00FF").unwrap(), 255);
    }

    #[test]
    fn decodes_hex_port_max() {
        assert_eq!(parse_hex_u16("FFFF").unwrap(), 65535);
    }

    #[test]
    fn decodes_hex_port_zero() {
        assert_eq!(parse_hex_u16("0000").unwrap(), 0);
    }

    // --- TCP state mapping ---

    #[test]
    fn maps_all_tcp_states() {
        assert_eq!(
            decode_state(0x01, Protocol::Tcp),
            ConnectionState::Established
        );
        assert_eq!(decode_state(0x02, Protocol::Tcp), ConnectionState::SynSent);
        assert_eq!(decode_state(0x03, Protocol::Tcp), ConnectionState::SynRecv);
        assert_eq!(decode_state(0x04, Protocol::Tcp), ConnectionState::FinWait1);
        assert_eq!(decode_state(0x05, Protocol::Tcp), ConnectionState::FinWait2);
        assert_eq!(decode_state(0x06, Protocol::Tcp), ConnectionState::TimeWait);
        assert_eq!(decode_state(0x07, Protocol::Tcp), ConnectionState::Close);
        assert_eq!(
            decode_state(0x08, Protocol::Tcp),
            ConnectionState::CloseWait
        );
        assert_eq!(decode_state(0x09, Protocol::Tcp), ConnectionState::LastAck);
        assert_eq!(decode_state(0x0A, Protocol::Tcp), ConnectionState::Listen);
        assert_eq!(decode_state(0x0B, Protocol::Tcp), ConnectionState::Closing);
    }

    #[test]
    fn udp_state_07_is_unconn() {
        assert_eq!(
            decode_state(0x07, Protocol::Udp),
            ConnectionState::UdpUnconn
        );
    }

    #[test]
    fn unknown_state_code() {
        match decode_state(0xFF, Protocol::Tcp) {
            ConnectionState::Unknown(0xFF) => {}
            other => panic!("expected Unknown(0xFF), got {other:?}"),
        }
    }

    #[test]
    fn state_labels_are_correct() {
        assert_eq!(ConnectionState::Established.label(), "ESTABLISHED");
        assert_eq!(ConnectionState::SynSent.label(), "SYN_SENT");
        assert_eq!(ConnectionState::SynRecv.label(), "SYN_RECV");
        assert_eq!(ConnectionState::FinWait1.label(), "FIN_WAIT1");
        assert_eq!(ConnectionState::FinWait2.label(), "FIN_WAIT2");
        assert_eq!(ConnectionState::TimeWait.label(), "TIME_WAIT");
        assert_eq!(ConnectionState::Close.label(), "CLOSE");
        assert_eq!(ConnectionState::CloseWait.label(), "CLOSE_WAIT");
        assert_eq!(ConnectionState::LastAck.label(), "LAST_ACK");
        assert_eq!(ConnectionState::Listen.label(), "LISTEN");
        assert_eq!(ConnectionState::Closing.label(), "CLOSING");
        assert_eq!(ConnectionState::UdpUnconn.label(), "UNCONN");
        assert_eq!(ConnectionState::Unknown(42).label(), "UNKNOWN");
    }

    // --- Malformed rows ---

    #[test]
    fn skips_empty_line() {
        assert!(parse_table_line("", Protocol::Tcp, false).is_none());
    }

    #[test]
    fn skips_whitespace_only_line() {
        assert!(parse_table_line("   ", Protocol::Tcp, false).is_none());
    }

    #[test]
    fn rejects_line_with_too_few_fields() {
        assert!(parse_table_line("0: 3500007F:0035", Protocol::Tcp, false).is_none());
    }

    #[test]
    fn rejects_invalid_hex_state() {
        assert!(
            parse_table_line(
                "0: 0100007F:0050 0100007F:0050 ZZZ 0 0",
                Protocol::Tcp,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_invalid_hex_addr_port() {
        assert!(
            parse_table_line("0: ZZZZ:0035 00000000:0000 0A 0 0", Protocol::Tcp, false).is_none()
        );
    }

    // --- Malformed addresses ---

    #[test]
    fn rejects_ipv6_wrong_length() {
        assert!(parse_ipv6_hex_address("000000000000000000000000:0277").is_none());
    }

    // --- Filter matching ---

    #[test]
    fn filter_all_returns_all() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "");
        assert_eq!(indices.len(), snapshot.connections.len());
    }

    #[test]
    fn filter_tcp_returns_only_tcp() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::Tcp, "");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].protocol, Protocol::Tcp);
        }
        assert!(!indices.is_empty());
    }

    #[test]
    fn filter_udp_returns_only_udp() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::Udp, "");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].protocol, Protocol::Udp);
        }
    }

    #[test]
    fn filter_listen_returns_only_listen() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::Listen, "");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].state, ConnectionState::Listen);
        }
        assert!(!indices.is_empty());
    }

    #[test]
    fn filter_established_returns_only_established() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::Established, "");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].state, ConnectionState::Established);
        }
        assert!(!indices.is_empty());
    }

    #[test]
    fn search_matches_local_addr() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "127.0.0.53");
        assert_eq!(indices.len(), 1);
        assert_eq!(snapshot.connections[indices[0]].local_port, 53);
    }

    #[test]
    fn search_matches_port() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "53");
        assert!(!indices.is_empty());
    }

    #[test]
    fn search_matches_protocol() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "UDP");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].protocol, Protocol::Udp);
        }
    }

    #[test]
    fn search_matches_state() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "LISTEN");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].state, ConnectionState::Listen);
        }
    }

    #[test]
    fn search_is_case_insensitive() {
        let snapshot = make_test_snapshot();
        let upper = filter_connections(&snapshot, ConnectionFilter::All, "LISTEN");
        let lower = filter_connections(&snapshot, ConnectionFilter::All, "listen");
        assert_eq!(upper, lower);
    }

    #[test]
    fn combined_filter_and_search() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::Tcp, "LISTEN");
        for &i in &indices {
            assert_eq!(snapshot.connections[i].protocol, Protocol::Tcp);
            assert_eq!(snapshot.connections[i].state, ConnectionState::Listen);
        }
    }

    #[test]
    fn no_match_returns_empty() {
        let snapshot = make_test_snapshot();
        let indices = filter_connections(&snapshot, ConnectionFilter::All, "nonexistent_xyz");
        assert!(indices.is_empty());
    }

    // --- Summary counts ---

    #[test]
    fn snapshot_counts_correct() {
        let snapshot = make_test_snapshot();
        assert_eq!(snapshot.tcp_count, 3);
        assert_eq!(snapshot.udp_count, 1);
        assert_eq!(snapshot.listen_count, 2);
        assert_eq!(snapshot.established_count, 1);
    }

    #[test]
    fn empty_snapshot_counts_zero() {
        let snapshot = ConnectionSnapshot::default();
        assert_eq!(snapshot.tcp_count, 0);
        assert_eq!(snapshot.udp_count, 0);
        assert_eq!(snapshot.listen_count, 0);
        assert_eq!(snapshot.established_count, 0);
    }

    // --- parse_table_line basic ---

    #[test]
    fn parse_table_line_ipv4() {
        let line =
            "   0: 3500007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   989";
        let conn = parse_table_line(line, Protocol::Tcp, false).unwrap();
        assert_eq!(conn.protocol, Protocol::Tcp);
        assert_eq!(conn.local_addr, "127.0.0.53");
        assert_eq!(conn.local_port, 53);
        assert_eq!(conn.state, ConnectionState::Listen);
    }

    #[test]
    fn parse_table_line_ipv6() {
        let line = "   0: 00000000000000000000000001000000:0277 00000000000000000000000000000000:0000 0A 0";
        let conn = parse_table_line(line, Protocol::Tcp, true).unwrap();
        assert_eq!(conn.local_port, 631);
        assert_eq!(conn.state, ConnectionState::Listen);
        assert!(conn.local_addr.contains(':'));
    }

    // --- Protocol labels ---

    #[test]
    fn protocol_labels() {
        assert_eq!(Protocol::Tcp.label(), "TCP");
        assert_eq!(Protocol::Udp.label(), "UDP");
    }

    // --- Filter labels ---

    #[test]
    fn filter_labels() {
        assert_eq!(ConnectionFilter::All.label(), "All");
        assert_eq!(ConnectionFilter::Tcp.label(), "TCP");
        assert_eq!(ConnectionFilter::Udp.label(), "UDP");
        assert_eq!(ConnectionFilter::Listen.label(), "LISTEN");
        assert_eq!(ConnectionFilter::Established.label(), "ESTABLISHED");
    }

    // --- IPv6 hex address decoding ---

    #[test]
    fn decodes_ipv6_all_zeros() {
        let (addr, port) = parse_ipv6_hex_address("00000000000000000000000000000000:0277").unwrap();
        assert_eq!(addr, "0000:0000:0000:0000:0000:0000:0000:0000");
        assert_eq!(port, 631);
    }

    #[test]
    fn decodes_ipv6_loopback() {
        // ::1 = bytes [00,00,00,00, 00,00,00,00, 00,00,00,00, 00,00,00,01]
        // As four __be32 on LE: 00000000 00000000 00000000 01000000
        let (addr, port) = parse_ipv6_hex_address("00000000000000000000000001000000:0036").unwrap();
        assert_eq!(addr, "0000:0000:0000:0000:0000:0000:0000:0001");
        assert_eq!(port, 54);
    }

    // --- Integration: parse a realistic full table ---

    #[test]
    fn parses_full_tcp_table() {
        let conns = parse_tcp_table(PROC_NET_TCP);
        assert_eq!(conns.len(), 3);
        let listen_count = conns
            .iter()
            .filter(|c| c.state == ConnectionState::Listen)
            .count();
        assert_eq!(listen_count, 2);
        let est_count = conns
            .iter()
            .filter(|c| c.state == ConnectionState::Established)
            .count();
        assert_eq!(est_count, 1);
    }

    // --- Fixture builder ---

    fn make_test_snapshot() -> ConnectionSnapshot {
        let mut connections = Vec::new();
        // TCP LISTEN on 127.0.0.53:53
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "127.0.0.53".into(),
            local_port: 53,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::Listen,
        });
        // TCP LISTEN on 127.0.0.1:46819
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "127.0.0.1".into(),
            local_port: 46819,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::Listen,
        });
        // TCP ESTABLISHED 127.0.0.1:25 -> 10.185.161.142:80
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "127.0.0.1".into(),
            local_port: 25,
            remote_addr: "10.185.161.142".into(),
            remote_port: 80,
            state: ConnectionState::Established,
        });
        // UDP UNCONN 127.0.0.54:53
        connections.push(Connection {
            protocol: Protocol::Udp,
            local_addr: "127.0.0.54".into(),
            local_port: 53,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::UdpUnconn,
        });

        let tcp_count = connections
            .iter()
            .filter(|c| c.protocol == Protocol::Tcp)
            .count();
        let udp_count = connections
            .iter()
            .filter(|c| c.protocol == Protocol::Udp)
            .count();
        let listen_count = connections
            .iter()
            .filter(|c| c.state == ConnectionState::Listen)
            .count();
        let established_count = connections
            .iter()
            .filter(|c| c.state == ConnectionState::Established)
            .count();

        ConnectionSnapshot {
            connections,
            tcp_count,
            udp_count,
            listen_count,
            established_count,
            available: true,
        }
    }
}
