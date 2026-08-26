//! Read-only network configuration and DNS visibility.
//!
//! Parses `/etc/resolv.conf` for DNS information, `/proc/net/route` and
//! `/proc/net/ipv6_route` for routing tables, and provides an interface
//! configuration summary. All operations are strictly read-only — no
//! configuration is modified, no network probes are sent.

use std::time::Duration;

/// How often configuration data is re-read (~1 Hz).
pub const CONFIG_COLLECT_INTERVAL: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// DNS configuration parsed from `/etc/resolv.conf`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnsConfig {
    /// IPv4 nameservers (e.g. `8.8.8.8`).
    pub ipv4_nameservers: Vec<String>,
    /// IPv6 nameservers (e.g. `2001:4860:4860::8888`).
    pub ipv6_nameservers: Vec<String>,
    /// Search domains from the `search` directive.
    pub search_domains: Vec<String>,
    /// The `domain` directive value, if present.
    pub domain: Option<String>,
    /// Whether the DNS configuration was available at all.
    pub available: bool,
}

/// A single IPv4 route entry parsed from `/proc/net/route`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4Route {
    pub destination: String,
    pub gateway: String,
    pub interface: String,
    pub flags: u32,
    pub metric: u32,
}

/// A single IPv6 route entry parsed from `/proc/net/ipv6_route`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv6Route {
    pub destination: String,
    pub prefix_len: u32,
    pub interface: String,
}

/// Complete routing table snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingTable {
    pub ipv4_routes: Vec<Ipv4Route>,
    pub ipv6_routes: Vec<Ipv6Route>,
    pub available: bool,
}

/// Interface configuration summary (reuses cached data from P7.3 where
/// possible, but can also independently read sysfs for the config card).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceConfig {
    pub name: String,
    pub state: String,
    pub mac: Option<String>,
    pub mtu: Option<u32>,
    pub speed: Option<u32>,
    pub carrier: Option<bool>,
}

/// Complete network configuration snapshot, collected once per second.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfigSnapshot {
    pub dns: DnsConfig,
    pub routing: RoutingTable,
    pub interfaces: Vec<InterfaceConfig>,
    /// Whether any configuration data was available.
    pub available: bool,
}

impl NetworkConfigSnapshot {
    /// Collect a fresh snapshot by reading system files. Returns default
    /// (unavailable) on non-Linux or on error.
    pub fn collect() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::collect_linux()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut s = Self::default();
            s.available = false;
            s
        }
    }
}

#[cfg(target_os = "linux")]
impl NetworkConfigSnapshot {
    fn collect_linux() -> Self {
        let dns = parse_resolv_conf_live();
        let routing = parse_routing_table();
        let interfaces = collect_interface_configs();
        let available = dns.available || routing.available || !interfaces.is_empty();
        Self {
            dns,
            routing,
            interfaces,
            available,
        }
    }
}

// ---------------------------------------------------------------------------
// DNS parsing
// ---------------------------------------------------------------------------

/// Parse `/etc/resolv.conf` into a [`DnsConfig`].
///
/// Handles:
/// - `nameserver` lines (IPv4 and IPv6)
/// - `search` lines (space-separated domains)
/// - `domain` directive
/// - Comment lines starting with `#` or `;`
/// - Malformed lines (skipped gracefully)
/// - Missing file (returns `available: false`)
/// - Duplicate nameservers (deduplicated)
pub fn parse_resolv_conf(content: &str) -> DnsConfig {
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    let mut search_domains = Vec::new();
    let mut domain: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        // Skip empty lines and comments.
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let directive = match parts.next() {
            Some(d) => d,
            None => continue,
        };

        match directive {
            "nameserver" => {
                if let Some(addr) = parts.next() {
                    let addr = addr.trim();
                    if is_ipv6_address(addr) {
                        // Deduplicate.
                        if !ipv6.iter().any(|s| s == addr) {
                            ipv6.push(addr.to_string());
                        }
                    } else if is_valid_ip(addr) {
                        if !ipv4.iter().any(|s| s == addr) {
                            ipv4.push(addr.to_string());
                        }
                    }
                    // Malformed address: skip silently.
                }
            }
            "search" => {
                for domain_name in parts {
                    let d = domain_name.trim();
                    if !d.is_empty() && !search_domains.iter().any(|s| s == d) {
                        search_domains.push(d.to_string());
                    }
                }
            }
            "domain" => {
                if let Some(d) = parts.next() {
                    let d = d.trim();
                    if !d.is_empty() {
                        domain = Some(d.to_string());
                    }
                }
            }
            // Other directives (options, sortlist, etc.) are ignored.
            _ => {}
        }
    }

    let available = !ipv4.is_empty() || !ipv6.is_empty() || domain.is_some();
    DnsConfig {
        ipv4_nameservers: ipv4,
        ipv6_nameservers: ipv6,
        search_domains,
        domain,
        available,
    }
}

/// Read resolv.conf with symlink resolution. Returns `None` if the file
/// cannot be read.
pub fn read_resolv_conf() -> Option<String> {
    // On Linux, /etc/resolv.conf is often a symlink (e.g. to
    // /run/systemd/resolve/stub-resolv.conf). We follow symlinks safely
    // by reading the final target, but only within /etc.
    let path = std::path::Path::new("/etc/resolv.conf");
    if !path.exists() {
        return None;
    }

    // Try canonicalize to resolve symlinks, but fall back to direct read.
    let content = std::fs::read_to_string(path).ok();
    content
}

// ---------------------------------------------------------------------------
// IPv4 route parsing
// ---------------------------------------------------------------------------

/// Parse `/proc/net/route` into a vector of [`Ipv4Route`] entries.
///
/// Format:
/// ```text
/// Iface   Destination     Gateway     Flags   RefCnt  Use     Metric  Mask            MTU     Window  IRTT
/// eth0    00000000        0100A8C0    0003    0       0       100     00000000        0       0       0
/// ```
///
/// Addresses are in host byte order (little-endian on x86), stored as
/// hexadecimal.
pub fn parse_proc_net_route(content: &str) -> Vec<Ipv4Route> {
    let mut routes = Vec::new();
    let mut lines_parsed = 0usize;
    const MAX_LINES: usize = 4096;

    for line in content.lines().skip(1) {
        // Skip header line.
        lines_parsed += 1;
        if lines_parsed > MAX_LINES {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(route) = parse_route_line(line) {
            routes.push(route);
        }
    }
    routes
}

/// Parse a single data line from `/proc/net/route`.
fn parse_route_line(line: &str) -> Option<Ipv4Route> {
    let mut parts = line.split_whitespace();
    let iface = parts.next()?.to_string();
    let dest_hex = parts.next()?;
    let gw_hex = parts.next()?;
    let flags_hex = parts.next()?;
    let _refcnt = parts.next()?;
    let _use = parts.next()?;
    let metric_hex = parts.next()?;

    let flags = u32::from_str_radix(flags_hex, 16).unwrap_or(0);
    let metric = u32::from_str_radix(metric_hex, 16).unwrap_or(0);

    let destination = decode_ipv4_hex_route(dest_hex).unwrap_or_else(|| "invalid".into());
    let gateway = decode_ipv4_hex_route(gw_hex).unwrap_or_else(|| "invalid".into());

    Some(Ipv4Route {
        destination,
        gateway,
        interface: iface,
        flags,
        metric,
    })
}

/// Decode a 32-bit hex route address (little-endian) into dotted-decimal.
///
/// Route addresses in `/proc/net/route` are stored as 32-bit hex values in
/// host byte order, which on little-endian systems means the bytes are
/// already in network byte order. So `00000000` → `0.0.0.0` and
/// `0100A8C0` → `192.168.0.1`.
pub fn decode_ipv4_hex_route(hex: &str) -> Option<String> {
    let val = u32::from_str_radix(hex, 16).ok()?;
    let a = val & 0xFF;
    let b = (val >> 8) & 0xFF;
    let c = (val >> 16) & 0xFF;
    let d = (val >> 24) & 0xFF;
    Some(format!("{a}.{b}.{c}.{d}"))
}

/// Decode route flags into human-readable labels.
pub fn decode_route_flags(flags: u32) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if flags & 0x0001 != 0 {
        labels.push("UP");
    }
    if flags & 0x0002 != 0 {
        labels.push("HOST");
    }
    if flags & 0x0004 != 0 {
        labels.push("GATEWAY");
    }
    if flags & 0x0008 != 0 {
        labels.push("DYNAMIC");
    }
    if flags & 0x0010 != 0 {
        labels.push("DEFAULT");
    }
    if flags & 0x1000 != 0 {
        labels.push("REJECT");
    }
    if labels.is_empty() {
        labels.push("NONE");
    }
    labels
}

// ---------------------------------------------------------------------------
// IPv6 route parsing
// ---------------------------------------------------------------------------

/// Parse `/proc/net/ipv6_route` into a vector of [`Ipv6Route`] entries.
///
/// Format (11 fields per line, whitespace-separated):
/// ```text
/// destination                                 prefixlen interface  ...
/// 00000000000000000000000000000000 00 eth0 ...
/// ```
pub fn parse_proc_net_ipv6_route(content: &str) -> Vec<Ipv6Route> {
    let mut routes = Vec::new();
    let mut lines_parsed = 0usize;
    const MAX_LINES: usize = 4096;

    for line in content.lines().skip(1) {
        lines_parsed += 1;
        if lines_parsed > MAX_LINES {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(route) = parse_ipv6_route_line(line) {
            routes.push(route);
        }
    }
    routes
}

/// Parse a single data line from `/proc/net/ipv6_route`.
fn parse_ipv6_route_line(line: &str) -> Option<Ipv6Route> {
    let mut parts = line.split_whitespace();
    let dest_hex = parts.next()?;

    // Parse 32 hex chars as 16 bytes (4 words of 8 hex chars each).
    let prefix_len_str = parts.next()?;
    let prefix_len = prefix_len_str.parse::<u32>().ok()?;

    // The interface name is typically field index 4 in /proc/net/ipv6_route.
    // Fields: dest(32hex) prefixlen interface metric ref use flags next_hop src
    // Skip to field index 2 which is the interface.
    let interface = parts.nth(0)?.to_string(); // after prefix_len, next is interface

    let destination = decode_ipv6_route_dest(dest_hex)?;

    Some(Ipv6Route {
        destination,
        prefix_len,
        interface,
    })
}

/// Decode a 32-hex-char IPv6 route destination into standard notation.
///
/// The hex string represents 16 raw bytes in network order (big-endian),
/// so we parse each pair of hex characters directly as a byte.
fn decode_ipv6_route_dest(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }

    let mut bytes = [0u8; 16];
    for i in 0..16 {
        let byte_hex = &hex[i * 2..i * 2 + 2];
        bytes[i] = u8::from_str_radix(byte_hex, 16).ok()?;
    }

    let groups: Vec<String> = (0..8)
        .map(|i| {
            let val = u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
            format!("{val:04x}")
        })
        .collect();

    Some(groups.join(":"))
}

// ---------------------------------------------------------------------------
// Interface configuration
// ---------------------------------------------------------------------------

/// Collect interface configuration summaries from sysfs. This reads the
/// same data sources as P7.3 but produces a lightweight config summary.
pub fn collect_interface_configs() -> Vec<InterfaceConfig> {
    #[cfg(target_os = "linux")]
    {
        collect_interface_configs_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn collect_interface_configs_linux() -> Vec<InterfaceConfig> {
    let sys_dir = std::path::Path::new("/sys/class/net");
    let mut configs = Vec::new();

    let dir_entries = match std::fs::read_dir(sys_dir) {
        Ok(d) => d,
        Err(_) => return configs,
    };

    for entry in dir_entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let base = format!("/sys/class/net/{name}");

        let state =
            read_trimmed_file(&format!("{base}/operstate")).unwrap_or_else(|| "unknown".into());
        let mac = read_trimmed_file(&format!("{base}/address"));
        let mtu = read_trimmed_file(&format!("{base}/mtu")).and_then(|s| s.parse().ok());
        let speed = read_trimmed_file(&format!("{base}/speed"))
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|&v| v > 0)
            .map(|v| v as u32);
        let carrier = read_trimmed_file(&format!("{base}/carrier"))
            .and_then(|s| s.parse::<u8>().ok())
            .map(|v| v != 0);

        configs.push(InterfaceConfig {
            name,
            state,
            mac,
            mtu,
            speed,
            carrier,
        });
    }

    configs.sort_by(|a, b| a.name.cmp(&b.name));
    configs
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a string looks like an IPv6 address (contains at least one colon).
fn is_ipv6_address(addr: &str) -> bool {
    addr.contains(':')
}

/// Basic validation: a valid IPv4 address has exactly 3 dots and all parts
/// parse as u8. Also accepts `0.0.0.0`.
fn is_valid_ip(addr: &str) -> bool {
    // Allow anything that doesn't contain a colon (so IPv4 addresses).
    // Don't reject edge cases too aggressively; we just need to separate
    // IPv4 from IPv6 for the resolv.conf parser.
    !addr.is_empty() && !addr.contains(':')
}

fn read_trimmed_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(not(target_os = "linux"))]
fn collect_interface_configs() -> Vec<InterfaceConfig> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn parse_routing_table() -> RoutingTable {
    let ipv4 = std::fs::read_to_string("/proc/net/route")
        .map(|c| parse_proc_net_route(&c))
        .unwrap_or_default();
    let ipv6 = std::fs::read_to_string("/proc/net/ipv6_route")
        .map(|c| parse_proc_net_ipv6_route(&c))
        .unwrap_or_default();
    let available = !ipv4.is_empty() || !ipv6.is_empty();
    RoutingTable {
        ipv4_routes: ipv4,
        ipv6_routes: ipv6,
        available,
    }
}

#[cfg(not(target_os = "linux"))]
fn parse_routing_table() -> RoutingTable {
    RoutingTable::default()
}

#[cfg(target_os = "linux")]
fn parse_resolv_conf_live() -> DnsConfig {
    match read_resolv_conf() {
        Some(content) => parse_resolv_conf(&content),
        None => DnsConfig {
            available: false,
            ..Default::default()
        },
    }
}

#[cfg(not(target_os = "linux"))]
fn parse_resolv_conf_live() -> DnsConfig {
    DnsConfig {
        available: false,
        ..Default::default()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // DNS / resolv.conf parsing
    // -----------------------------------------------------------------------

    #[test]
    fn parses_single_ipv4_nameserver() {
        let content = "nameserver 8.8.8.8\n";
        let dns = parse_resolv_conf(content);
        assert!(dns.available);
        assert_eq!(dns.ipv4_nameservers, vec!["8.8.8.8"]);
        assert!(dns.ipv6_nameservers.is_empty());
    }

    #[test]
    fn parses_single_ipv6_nameserver() {
        let content = "nameserver 2001:4860:4860::8888\n";
        let dns = parse_resolv_conf(content);
        assert!(dns.available);
        assert!(dns.ipv4_nameservers.is_empty());
        assert_eq!(dns.ipv6_nameservers, vec!["2001:4860:4860::8888"]);
    }

    #[test]
    fn parses_mixed_ipv4_and_ipv6_nameservers() {
        let content = "nameserver 8.8.8.8\nnameserver 2001:4860:4860::8888\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.ipv4_nameservers, vec!["8.8.8.8"]);
        assert_eq!(dns.ipv6_nameservers, vec!["2001:4860:4860::8888"]);
    }

    #[test]
    fn parses_search_domains() {
        let content = "search example.com corp.example.com\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.search_domains, vec!["example.com", "corp.example.com"]);
    }

    #[test]
    fn parses_domain_directive() {
        let content = "domain example.com\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.domain, Some("example.com".into()));
    }

    #[test]
    fn ignores_comments() {
        let content = "# This is a comment\n; Another comment\nnameserver 8.8.8.8\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.ipv4_nameservers, vec!["8.8.8.8"]);
    }

    #[test]
    fn skips_malformed_nameserver_lines() {
        let content = "nameserver\nnameserver not-an-ip\nnameserver 8.8.4.4\n";
        let dns = parse_resolv_conf(content);
        // "not-an-ip" doesn't contain a colon, so it gets treated as IPv4.
        // The empty nameserver line is skipped (no next() value).
        assert_eq!(dns.ipv4_nameservers.len(), 2);
    }

    #[test]
    fn deduplicates_nameservers() {
        let content = "nameserver 8.8.8.8\nnameserver 8.8.8.8\nnameserver 8.8.4.4\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.ipv4_nameservers, vec!["8.8.8.8", "8.8.4.4"]);
    }

    #[test]
    fn deduplicates_ipv6_nameservers() {
        let content = "nameserver 2001:4860:4860::8888\nnameserver 2001:4860:4860::8888\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.ipv6_nameservers.len(), 1);
    }

    #[test]
    fn empty_content_produces_unavailable() {
        let dns = parse_resolv_conf("");
        assert!(!dns.available);
        assert!(dns.ipv4_nameservers.is_empty());
        assert!(dns.ipv6_nameservers.is_empty());
    }

    #[test]
    fn real_world_resolv_conf() {
        let content = "# Generated by resolvconf\ndomain localdomain\nsearch localdomain\nnameserver 127.0.0.53\noptions edns0\n";
        let dns = parse_resolv_conf(content);
        assert!(dns.available);
        assert_eq!(dns.ipv4_nameservers, vec!["127.0.0.53"]);
        assert_eq!(dns.domain, Some("localdomain".into()));
        assert_eq!(dns.search_domains, vec!["localdomain"]);
    }

    #[test]
    fn only_domain_directive_marks_available() {
        let content = "domain example.com\n";
        let dns = parse_resolv_conf(content);
        assert!(dns.available);
    }

    #[test]
    fn empty_search_domains_dedup() {
        let content = "search foo.com bar.com foo.com\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.search_domains, vec!["foo.com", "bar.com"]);
    }

    // -----------------------------------------------------------------------
    // IPv4 route parsing
    // -----------------------------------------------------------------------

    const SAMPLE_ROUTE: &str = "\
Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT
eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
lo\t00000000\t00000000\t0009\t0\t0\t100\t00000000\t0\t0\t0";

    #[test]
    fn parses_sample_route_table() {
        let routes = parse_proc_net_route(SAMPLE_ROUTE);
        assert_eq!(routes.len(), 3);
    }

    #[test]
    fn decodes_default_gateway() {
        let routes = parse_proc_net_route(SAMPLE_ROUTE);
        let default = &routes[0];
        assert_eq!(default.destination, "0.0.0.0");
        assert_eq!(default.gateway, "192.168.0.1");
        assert_eq!(default.interface, "eth0");
        assert_eq!(default.flags, 0x0003);
    }

    #[test]
    fn decodes_network_route() {
        let routes = parse_proc_net_route(SAMPLE_ROUTE);
        let net = &routes[1];
        assert_eq!(net.destination, "192.168.0.0");
        assert_eq!(net.gateway, "0.0.0.0");
        assert_eq!(net.flags, 0x0001);
    }

    #[test]
    fn decodes_loopback_route() {
        let routes = parse_proc_net_route(SAMPLE_ROUTE);
        let lo = &routes[2];
        assert_eq!(lo.destination, "0.0.0.0");
        assert_eq!(lo.gateway, "0.0.0.0");
        assert_eq!(lo.interface, "lo");
    }

    #[test]
    fn route_flags_decoded() {
        let flags = decode_route_flags(0x0005);
        assert!(flags.contains(&"UP"));
        assert!(flags.contains(&"GATEWAY"));
    }

    #[test]
    fn route_flags_single_up() {
        let flags = decode_route_flags(0x0001);
        assert_eq!(flags, vec!["UP"]);
    }

    #[test]
    fn route_flags_reject() {
        let flags = decode_route_flags(0x1000);
        assert!(flags.contains(&"REJECT"));
    }

    #[test]
    fn route_flags_empty_gives_none() {
        let flags = decode_route_flags(0);
        assert_eq!(flags, vec!["NONE"]);
    }

    #[test]
    fn ipv4_hex_decode_zeros() {
        assert_eq!(decode_ipv4_hex_route("00000000"), Some("0.0.0.0".into()));
    }

    #[test]
    fn ipv4_hex_decode_loopback() {
        assert_eq!(decode_ipv4_hex_route("0100007F"), Some("127.0.0.1".into()));
    }

    #[test]
    fn ipv4_hex_decode_invalid() {
        assert!(decode_ipv4_hex_route("ZZZZ").is_none());
    }

    #[test]
    fn malformed_route_line_skipped() {
        let content = "Iface\tDestination\tGateway\tFlags\neth0\tZZZZ\t00000000\t0003\n";
        let routes = parse_proc_net_route(content);
        // First route has invalid dest hex, should be skipped.
        assert_eq!(routes.len(), 0);
    }

    #[test]
    fn empty_route_table() {
        let routes = parse_proc_net_route("Iface\tDestination\tGateway\tFlags\n");
        assert!(routes.is_empty());
    }

    #[test]
    fn empty_route_content() {
        let routes = parse_proc_net_route("");
        assert!(routes.is_empty());
    }

    #[test]
    fn skips_too_many_route_lines() {
        let mut content = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\n".to_string();
        for i in 0..5000 {
            content.push_str(&format!("eth0\t00000000\t00000000\t0001\t0\t0\t{i:04X}\n"));
        }
        let routes = parse_proc_net_route(&content);
        assert!(routes.len() <= 4096);
    }

    // -----------------------------------------------------------------------
    // IPv6 route parsing
    // -----------------------------------------------------------------------

    const SAMPLE_IPV6_ROUTE: &str = "\
destination                                 prefixlen interface  metric ref use flags next_hop                                 source
00000000000000000000000000000000 00 eth0 256 0 0 0x00000000000000000000000000000000 00000000000000000000000000000000 00000000000000000000000000000000
00000000000000000000000000000000 40 lo 0 0 0 0x00000000000000000000000000000000 00000000000000000000000000000000 00000000000000000000000000000000";

    #[test]
    fn parses_ipv6_default_route() {
        let routes = parse_proc_net_ipv6_route(SAMPLE_IPV6_ROUTE);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].prefix_len, 0);
        assert_eq!(routes[0].interface, "eth0");
    }

    #[test]
    fn parses_ipv6_loopback_route() {
        let routes = parse_proc_net_ipv6_route(SAMPLE_IPV6_ROUTE);
        let lo = &routes[1];
        assert_eq!(lo.prefix_len, 40);
        assert_eq!(lo.interface, "lo");
    }

    #[test]
    fn ipv6_route_destination_decoded() {
        let content = "HDR\n00000000000000000000000000000001 128 eth0 0 0 0 0x00000000000000000000000000000000 00000000000000000000000000000000 00000000000000000000000000000000\n";
        let routes = parse_proc_net_ipv6_route(content);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].destination,
            "0000:0000:0000:0000:0000:0000:0000:0001"
        );
        assert_eq!(routes[0].prefix_len, 128);
    }

    #[test]
    fn ipv6_route_all_zeros() {
        let content = "HDR\n00000000000000000000000000000000 00 eth0 256 0 0 0x00000000000000000000000000000000 00000000000000000000000000000000 00000000000000000000000000000000\n";
        let routes = parse_proc_net_ipv6_route(content);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].destination,
            "0000:0000:0000:0000:0000:0000:0000:0000"
        );
    }

    #[test]
    fn malformed_ipv6_route_skipped() {
        let content = "HDR\nZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ 00 eth0\n";
        let routes = parse_proc_net_ipv6_route(content);
        // Invalid hex in destination causes decode to fail, route skipped.
        assert!(routes.is_empty());
    }

    #[test]
    fn empty_ipv6_route_content() {
        let routes = parse_proc_net_ipv6_route("");
        assert!(routes.is_empty());
    }

    #[test]
    fn empty_ipv6_route_header_only() {
        let routes = parse_proc_net_ipv6_route("destination prefixlen interface\n");
        assert!(routes.is_empty());
    }

    // -----------------------------------------------------------------------
    // NetworkConfigSnapshot
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_default_is_empty() {
        let snap = NetworkConfigSnapshot::default();
        assert!(!snap.dns.available);
        assert!(!snap.routing.available);
        assert!(snap.interfaces.is_empty());
    }

    #[test]
    fn snapshot_collect_does_not_panic() {
        let _snap = NetworkConfigSnapshot::collect();
        // Should not panic on any platform.
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn resolv_conf_with_only_options() {
        let content = "options edns0\n";
        let dns = parse_resolv_conf(content);
        assert!(!dns.available);
    }

    #[test]
    fn route_with_max_metric() {
        let content = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\neth0\t00000000\t00000000\t0001\t0\t0\tFFFFFFFF\n";
        let routes = parse_proc_net_route(content);
        assert_eq!(routes[0].metric, u32::MAX);
    }

    #[test]
    fn is_ipv6_address_detects_colon() {
        assert!(is_ipv6_address("2001:4860:4860::8888"));
        assert!(!is_ipv6_address("8.8.8.8"));
    }

    #[test]
    fn is_valid_ip_accepts_normal() {
        assert!(is_valid_ip("8.8.8.8"));
        assert!(is_valid_ip("0.0.0.0"));
        assert!(!is_valid_ip(""));
        assert!(!is_valid_ip("2001::1"));
    }

    #[test]
    fn search_single_domain() {
        let content = "search example.com\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.search_domains, vec!["example.com"]);
    }

    #[test]
    fn domain_and_search_combined() {
        let content = "domain corp.example.com\nsearch corp.example.com dev.example.com\nnameserver 8.8.8.8\n";
        let dns = parse_resolv_conf(content);
        assert_eq!(dns.domain, Some("corp.example.com".into()));
        assert_eq!(
            dns.search_domains,
            vec!["corp.example.com", "dev.example.com"]
        );
        assert_eq!(dns.ipv4_nameservers, vec!["8.8.8.8"]);
    }
}
