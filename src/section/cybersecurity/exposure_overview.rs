//! Read-only local security exposure overview.
//!
//! Classifies observable local listening sockets by exposure scope and
//! provides a conservative risk indicator. Never connects to, probes, or
//! scans ports. Only reads cached data collected by P7 networking.

use crate::section::networking::connections::{ConnectionSnapshot, ConnectionState, Protocol};

/// Maximum number of listeners to retain in the bounded list.
const MAX_LISTENERS: usize = 512;

/// Maximum number of displayed rows.
pub const MAX_DISPLAYED_LISTENERS: usize = 100;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Exposure scope for a listening socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExposureScope {
    /// Bound only to 127.x.x.x / ::1 — loopback.
    Loopback,
    /// Bound to a specific local interface/address.
    Local,
    /// Bound to 0.0.0.0 / :: — all interfaces.
    AllInterfaces,
    /// Cannot determine binding scope.
    Unknown,
}

impl ExposureScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loopback => "Loopback",
            Self::Local => "Local",
            Self::AllInterfaces => "All interfaces",
            Self::Unknown => "Unknown",
        }
    }

    pub const ALL: [ExposureScope; 4] = [
        ExposureScope::Loopback,
        ExposureScope::Local,
        ExposureScope::AllInterfaces,
        ExposureScope::Unknown,
    ];
}

/// Overall risk indicator based on observable facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RiskLevel {
    /// No non-loopback listeners detected.
    Low,
    /// Limited non-loopback listeners detected.
    Moderate,
    /// Multiple all-interface listeners AND firewall posture unavailable/disabled.
    Elevated,
    /// Required data unavailable.
    Unknown,
}

impl Default for RiskLevel {
    fn default() -> Self {
        Self::Unknown
    }
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Moderate => "MODERATE",
            Self::Elevated => "ELEVATED",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Low => "No non-loopback listeners detected",
            Self::Moderate => "Limited non-loopback listeners detected",
            Self::Elevated => {
                "Multiple all-interface listeners detected AND firewall posture unavailable"
            }
            Self::Unknown => "Required data unavailable",
        }
    }
}

/// A classified listener for display.
#[derive(Debug, Clone)]
pub struct Listener {
    pub protocol: Protocol,
    pub local_addr: String,
    pub local_port: u16,
    pub state: ConnectionState,
    pub address_family: String,
    pub exposure: ExposureScope,
    pub explanation: String,
    pub process_name: Option<String>,
    pub process_pid: Option<u32>,
}

/// Filter for listeners.
#[derive(Debug, Clone, Default)]
pub struct ListenerFilter {
    pub search: String,
    pub protocol: Option<Protocol>,
    pub exposure: Option<ExposureScope>,
    pub state_filter: ListenerStateFilter,
}

/// State filter variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerStateFilter {
    All,
    Listening,
    Other,
}

impl Default for ListenerStateFilter {
    fn default() -> Self {
        Self::All
    }
}

impl ListenerStateFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Listening => "Listening",
            Self::Other => "Other",
        }
    }

    pub const ALL: [ListenerStateFilter; 3] = [
        ListenerStateFilter::All,
        ListenerStateFilter::Listening,
        ListenerStateFilter::Other,
    ];
}

impl ListenerFilter {
    pub fn matches(&self, listener: &Listener) -> bool {
        // Protocol filter
        if let Some(proto) = self.protocol {
            if listener.protocol != proto {
                return false;
            }
        }
        // Exposure filter
        if let Some(exp) = self.exposure {
            if listener.exposure != exp {
                return false;
            }
        }
        // State filter
        match self.state_filter {
            ListenerStateFilter::All => {}
            ListenerStateFilter::Listening => {
                if listener.state != ConnectionState::Listen {
                    return false;
                }
            }
            ListenerStateFilter::Other => {
                if listener.state == ConnectionState::Listen {
                    return false;
                }
            }
        }
        // Search
        let search_lower = self.search.to_lowercase();
        if !search_lower.is_empty() {
            let port_str = listener.local_port.to_string();
            let fields = [
                listener.protocol.label().to_lowercase(),
                listener.local_addr.to_lowercase(),
                port_str.to_lowercase(),
                listener.state.label().to_lowercase(),
                listener.exposure.label().to_lowercase(),
                listener.address_family.to_lowercase(),
            ];
            if !fields.iter().any(|f| f.contains(&search_lower)) {
                return false;
            }
        }
        true
    }
}

/// Summary counts for the exposure overview.
#[derive(Debug, Clone, Default)]
pub struct ExposureSummary {
    pub total_listeners: u32,
    pub tcp_listeners: u32,
    pub udp_listeners: u32,
    pub loopback_listeners: u32,
    pub non_loopback_listeners: u32,
    pub all_interface_listeners: u32,
    pub risk_level: RiskLevel,
    pub firewall_active: Option<bool>,
}

/// Complete exposure overview state.
#[derive(Debug, Clone)]
pub struct ExposureOverview {
    pub listeners: Vec<Listener>,
    pub summary: ExposureSummary,
    pub data_available: bool,
    pub firewall_message: String,
    pub platform_message: Option<String>,
}

impl Default for ExposureOverview {
    fn default() -> Self {
        Self {
            listeners: Vec::new(),
            summary: ExposureSummary::default(),
            data_available: false,
            firewall_message: "Firewall posture unknown".into(),
            platform_message: None,
        }
    }
}

impl ExposureOverview {
    pub fn len(&self) -> usize {
        self.listeners.len()
    }

    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Listener> {
        self.listeners.get(index)
    }

    pub fn filter(&self, filter: &ListenerFilter) -> Vec<usize> {
        self.listeners
            .iter()
            .enumerate()
            .filter(|(_, l)| filter.matches(l))
            .map(|(i, _)| i)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classify a local address into an exposure scope.
pub fn classify_exposure(local_addr: &str) -> ExposureScope {
    // IPv4 loopback
    if local_addr.starts_with("127.") {
        return ExposureScope::Loopback;
    }
    // IPv4 all-interfaces
    if local_addr == "0.0.0.0" {
        return ExposureScope::AllInterfaces;
    }
    // IPv6 loopback (::1)
    if local_addr == "0000:0000:0000:0000:0000:0000:0000:0001" || local_addr == "::1" {
        return ExposureScope::Loopback;
    }
    // IPv6 all-interfaces (::)
    if local_addr == "0000:0000:0000:0000:0000:0000:0000:0000"
        || local_addr == "::"
        || local_addr == "0000:0000:0000:0000:0000:0000:0000:0000"
    {
        return ExposureScope::AllInterfaces;
    }
    // Non-loopback IPv6 (fe80::, fd00::, etc.) — link-local or ULA
    if local_addr.contains(':') {
        // Check for link-local (fe80) or ULA (fd00)
        if local_addr.starts_with("fe80:")
            || local_addr.starts_with("fe80:0000")
            || local_addr.starts_with("fd")
        {
            return ExposureScope::Local;
        }
        // Any other non-zero IPv6
        return ExposureScope::Local;
    }
    // Specific IPv4 address — not loopback, not all-interfaces
    if !local_addr.is_empty() && local_addr != "0.0.0.0" && !local_addr.starts_with("127.") {
        return ExposureScope::Local;
    }
    ExposureScope::Unknown
}

/// Generate explanation for an exposure classification.
pub fn explain_exposure(addr: &str, exposure: ExposureScope) -> String {
    match exposure {
        ExposureScope::Loopback => {
            format!("Bound to {addr}, so the service is accessible only from the local machine.")
        }
        ExposureScope::Local => {
            format!(
                "Bound to {addr}, so the service is accessible from the local network interface."
            )
        }
        ExposureScope::AllInterfaces => {
            format!("Bound to {addr}, so the service is listening on all interfaces.")
        }
        ExposureScope::Unknown => "Unable to determine the binding scope of this service.".into(),
    }
}

/// Determine the address family label.
pub fn address_family(addr: &str) -> &'static str {
    if addr.contains(':') { "IPv6" } else { "IPv4" }
}

// ---------------------------------------------------------------------------
// Risk classification
// ---------------------------------------------------------------------------

/// Compute the risk level from observable facts.
///
/// Classification rules:
/// - LOW: no non-loopback listeners
/// - MODERATE: limited non-loopback listeners (1-5)
/// - ELEVATED: multiple all-interface listeners (>= 3) AND firewall unavailable
/// - UNKNOWN: data unavailable
pub fn compute_risk_level(
    all_interface_count: usize,
    non_loopback_count: usize,
    firewall_active: Option<bool>,
    data_available: bool,
) -> RiskLevel {
    if !data_available {
        return RiskLevel::Unknown;
    }
    if non_loopback_count == 0 {
        return RiskLevel::Low;
    }
    if all_interface_count >= 3 && firewall_active != Some(true) {
        return RiskLevel::Elevated;
    }
    if non_loopback_count <= 5 {
        return RiskLevel::Moderate;
    }
    // More than 5 non-loopback but not all-interface heavy or no firewall issue
    RiskLevel::Moderate
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// Collect exposure overview from an existing connection snapshot.
/// Does NOT re-read /proc — uses the cached snapshot from P7.
pub fn collect_exposure(
    snapshot: &ConnectionSnapshot,
    firewall_active: Option<bool>,
) -> ExposureOverview {
    if !snapshot.available {
        return ExposureOverview {
            data_available: false,
            firewall_message: firewall_status_message(firewall_active),
            platform_message: None,
            ..Default::default()
        };
    }

    let mut listeners: Vec<Listener> = Vec::with_capacity(MAX_LISTENERS);

    for conn in &snapshot.connections {
        // Only include listeners and unconnected UDP
        let is_listener = conn.state == ConnectionState::Listen
            || (conn.protocol == Protocol::Udp && conn.state == ConnectionState::UdpUnconn);

        if !is_listener {
            continue;
        }

        let exposure = classify_exposure(&conn.local_addr);
        let explanation = explain_exposure(&conn.local_addr, exposure);
        let af = address_family(&conn.local_addr);

        listeners.push(Listener {
            protocol: conn.protocol,
            local_addr: conn.local_addr.clone(),
            local_port: conn.local_port,
            state: conn.state,
            address_family: af.into(),
            exposure,
            explanation,
            process_name: conn.process.as_ref().map(|p| p.name.clone()),
            process_pid: conn.process.as_ref().map(|p| p.pid),
        });
    }

    // Enforce bound
    if listeners.len() > MAX_LISTENERS {
        listeners.truncate(MAX_LISTENERS);
    }

    let summary = compute_summary(&listeners, firewall_active);

    ExposureOverview {
        listeners,
        summary,
        data_available: true,
        firewall_message: firewall_status_message(firewall_active),
        platform_message: None,
    }
}

fn compute_summary(listeners: &[Listener], firewall_active: Option<bool>) -> ExposureSummary {
    let total_listeners = listeners.len() as u32;
    let tcp_listeners = listeners
        .iter()
        .filter(|l| l.protocol == Protocol::Tcp)
        .count() as u32;
    let udp_listeners = listeners
        .iter()
        .filter(|l| l.protocol == Protocol::Udp)
        .count() as u32;
    let loopback_listeners = listeners
        .iter()
        .filter(|l| l.exposure == ExposureScope::Loopback)
        .count() as u32;
    let non_loopback_listeners = listeners
        .iter()
        .filter(|l| l.exposure != ExposureScope::Loopback)
        .count() as u32;
    let all_interface_listeners = listeners
        .iter()
        .filter(|l| l.exposure == ExposureScope::AllInterfaces)
        .count() as u32;

    let risk_level = compute_risk_level(
        all_interface_listeners as usize,
        non_loopback_listeners as usize,
        firewall_active,
        true,
    );

    ExposureSummary {
        total_listeners,
        tcp_listeners,
        udp_listeners,
        loopback_listeners,
        non_loopback_listeners,
        all_interface_listeners,
        risk_level,
        firewall_active,
    }
}

fn firewall_status_message(active: Option<bool>) -> String {
    match active {
        Some(true) => "Firewall: active".into(),
        Some(false) => "Firewall: inactive".into(),
        None => "Firewall: unavailable".into(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::section::networking::connections::Connection;

    // --- IPv4 exposure classification ---

    #[test]
    fn ipv4_loopback_classification() {
        assert_eq!(classify_exposure("127.0.0.1"), ExposureScope::Loopback);
        assert_eq!(classify_exposure("127.0.0.53"), ExposureScope::Loopback);
        assert_eq!(
            classify_exposure("127.255.255.255"),
            ExposureScope::Loopback
        );
    }

    #[test]
    fn ipv4_all_interfaces_classification() {
        assert_eq!(classify_exposure("0.0.0.0"), ExposureScope::AllInterfaces);
    }

    #[test]
    fn ipv4_local_address_classification() {
        assert_eq!(classify_exposure("192.168.1.1"), ExposureScope::Local);
        assert_eq!(classify_exposure("10.0.0.1"), ExposureScope::Local);
        assert_eq!(classify_exposure("172.16.0.1"), ExposureScope::Local);
    }

    // --- IPv6 exposure classification ---

    #[test]
    fn ipv6_loopback_classification() {
        assert_eq!(
            classify_exposure("0000:0000:0000:0000:0000:0000:0000:0001"),
            ExposureScope::Loopback
        );
        assert_eq!(classify_exposure("::1"), ExposureScope::Loopback);
    }

    #[test]
    fn ipv6_all_interfaces_classification() {
        assert_eq!(
            classify_exposure("0000:0000:0000:0000:0000:0000:0000:0000"),
            ExposureScope::AllInterfaces
        );
        assert_eq!(classify_exposure("::"), ExposureScope::AllInterfaces);
    }

    #[test]
    fn ipv6_local_classification() {
        assert_eq!(classify_exposure("fe80::1"), ExposureScope::Local);
        assert_eq!(classify_exposure("fd00::1"), ExposureScope::Local);
    }

    #[test]
    fn unknown_address_classification() {
        assert_eq!(classify_exposure(""), ExposureScope::Unknown);
    }

    // --- TCP / UDP listener detection ---

    #[test]
    fn tcp_listener_detected() {
        let snapshot = ConnectionSnapshot {
            connections: vec![Connection {
                protocol: Protocol::Tcp,
                local_addr: "0.0.0.0".into(),
                local_port: 22,
                remote_addr: "0.0.0.0".into(),
                remote_port: 0,
                state: ConnectionState::Listen,
                inode: 100,
                process: None,
            }],
            tcp_count: 1,
            udp_count: 0,
            listen_count: 1,
            established_count: 0,
            available: true,
        };
        let overview = collect_exposure(&snapshot, Some(true));
        assert_eq!(overview.listeners.len(), 1);
        assert_eq!(overview.listeners[0].protocol, Protocol::Tcp);
        assert_eq!(overview.listeners[0].exposure, ExposureScope::AllInterfaces);
    }

    #[test]
    fn udp_listener_detected() {
        let snapshot = ConnectionSnapshot {
            connections: vec![Connection {
                protocol: Protocol::Udp,
                local_addr: "0.0.0.0".into(),
                local_port: 5353,
                remote_addr: "0.0.0.0".into(),
                remote_port: 0,
                state: ConnectionState::UdpUnconn,
                inode: 200,
                process: None,
            }],
            tcp_count: 0,
            udp_count: 1,
            listen_count: 0,
            established_count: 0,
            available: true,
        };
        let overview = collect_exposure(&snapshot, Some(true));
        assert_eq!(overview.listeners.len(), 1);
        assert_eq!(overview.listeners[0].protocol, Protocol::Udp);
    }

    #[test]
    fn established_connections_not_shown() {
        let snapshot = ConnectionSnapshot {
            connections: vec![Connection {
                protocol: Protocol::Tcp,
                local_addr: "10.0.0.1".into(),
                local_port: 443,
                remote_addr: "1.2.3.4".into(),
                remote_port: 8080,
                state: ConnectionState::Established,
                inode: 300,
                process: None,
            }],
            tcp_count: 1,
            udp_count: 0,
            listen_count: 0,
            established_count: 1,
            available: true,
        };
        let overview = collect_exposure(&snapshot, Some(true));
        assert!(overview.listeners.is_empty());
    }

    // --- Filtering ---

    #[test]
    fn filter_by_protocol() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            protocol: Some(Protocol::Tcp),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        for &i in &indices {
            assert_eq!(overview.listeners[i].protocol, Protocol::Tcp);
        }
    }

    #[test]
    fn filter_by_exposure() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            exposure: Some(ExposureScope::Loopback),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        for &i in &indices {
            assert_eq!(overview.listeners[i].exposure, ExposureScope::Loopback);
        }
    }

    #[test]
    fn filter_by_state_listening() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            state_filter: ListenerStateFilter::Listening,
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        for &i in &indices {
            assert_eq!(overview.listeners[i].state, ConnectionState::Listen);
        }
    }

    #[test]
    fn filter_combined() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            protocol: Some(Protocol::Tcp),
            exposure: Some(ExposureScope::AllInterfaces),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        for &i in &indices {
            assert_eq!(overview.listeners[i].protocol, Protocol::Tcp);
            assert_eq!(overview.listeners[i].exposure, ExposureScope::AllInterfaces);
        }
    }

    // --- Search ---

    #[test]
    fn search_case_insensitive() {
        let overview = make_test_overview();
        let filter_upper = ListenerFilter {
            search: "TCP".into(),
            ..Default::default()
        };
        let filter_lower = ListenerFilter {
            search: "tcp".into(),
            ..Default::default()
        };
        let upper = overview.filter(&filter_upper);
        let lower = overview.filter(&filter_lower);
        assert_eq!(upper, lower);
    }

    #[test]
    fn search_matches_port() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            search: "22".into(),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        assert!(!indices.is_empty());
        assert!(
            indices
                .iter()
                .all(|&i| overview.listeners[i].local_port.to_string().contains("22"))
        );
    }

    #[test]
    fn search_matches_address() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            search: "127.0.0.1".into(),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        assert!(!indices.is_empty());
    }

    #[test]
    fn combined_search_and_filter() {
        let overview = make_test_overview();
        let filter = ListenerFilter {
            search: "22".into(),
            protocol: Some(Protocol::Tcp),
            ..Default::default()
        };
        let indices = overview.filter(&filter);
        for &i in &indices {
            assert_eq!(overview.listeners[i].protocol, Protocol::Tcp);
        }
    }

    // --- Summary counts ---

    #[test]
    fn summary_counts_correct() {
        let overview = make_test_overview();
        assert_eq!(overview.summary.total_listeners, 4);
        assert_eq!(overview.summary.tcp_listeners, 3);
        assert_eq!(overview.summary.udp_listeners, 1);
        assert_eq!(overview.summary.loopback_listeners, 1);
        assert_eq!(overview.summary.non_loopback_listeners, 3);
        assert_eq!(overview.summary.all_interface_listeners, 2);
    }

    // --- Risk classification ---

    #[test]
    fn risk_low_no_non_loopback() {
        assert_eq!(compute_risk_level(0, 0, Some(true), true), RiskLevel::Low);
    }

    #[test]
    fn risk_moderate_limited_non_loopback() {
        assert_eq!(
            compute_risk_level(1, 2, Some(true), true),
            RiskLevel::Moderate
        );
    }

    #[test]
    fn risk_elevated_all_interfaces_and_no_firewall() {
        assert_eq!(
            compute_risk_level(3, 5, Some(false), true),
            RiskLevel::Elevated
        );
        assert_eq!(compute_risk_level(3, 5, None, true), RiskLevel::Elevated);
    }

    #[test]
    fn risk_unknown_data_unavailable() {
        assert_eq!(compute_risk_level(0, 0, None, false), RiskLevel::Unknown);
    }

    #[test]
    fn risk_moderate_many_non_loopback_but_firewall_active() {
        // Even with many non-loopback, if firewall is active and all-interface count < 3
        assert_eq!(
            compute_risk_level(2, 10, Some(true), true),
            RiskLevel::Moderate
        );
    }

    // --- Empty state ---

    #[test]
    fn empty_snapshot_no_listeners() {
        let snapshot = ConnectionSnapshot::default();
        let overview = collect_exposure(&snapshot, None);
        assert!(overview.listeners.is_empty());
        assert!(!overview.data_available);
    }

    #[test]
    fn unavailable_data() {
        let snapshot = ConnectionSnapshot {
            available: false,
            ..Default::default()
        };
        let overview = collect_exposure(&snapshot, None);
        assert!(!overview.data_available);
        assert!(overview.listeners.is_empty());
    }

    // --- Exposure explanation ---

    #[test]
    fn explain_loopback() {
        let exp = explain_exposure("127.0.0.1", ExposureScope::Loopback);
        assert!(exp.contains("local machine"));
    }

    #[test]
    fn explain_all_interfaces() {
        let exp = explain_exposure("0.0.0.0", ExposureScope::AllInterfaces);
        assert!(exp.contains("all interfaces"));
    }

    #[test]
    fn explain_local() {
        let exp = explain_exposure("192.168.1.1", ExposureScope::Local);
        assert!(exp.contains("local network"));
    }

    // --- Address family ---

    #[test]
    fn ipv4_family() {
        assert_eq!(address_family("127.0.0.1"), "IPv4");
        assert_eq!(address_family("0.0.0.0"), "IPv4");
    }

    #[test]
    fn ipv6_family() {
        assert_eq!(
            address_family("0000:0000:0000:0000:0000:0000:0000:0001"),
            "IPv6"
        );
        assert_eq!(address_family("::1"), "IPv6");
    }

    // --- Firewall context ---

    #[test]
    fn firewall_active_message() {
        let overview = collect_exposure(&ConnectionSnapshot::default(), Some(true));
        assert_eq!(overview.firewall_message, "Firewall: active");
    }

    #[test]
    fn firewall_inactive_message() {
        let overview = collect_exposure(&ConnectionSnapshot::default(), Some(false));
        assert_eq!(overview.firewall_message, "Firewall: inactive");
    }

    #[test]
    fn firewall_unavailable_message() {
        let overview = collect_exposure(&ConnectionSnapshot::default(), None);
        assert_eq!(overview.firewall_message, "Firewall: unavailable");
    }

    // --- Bounded display ---

    #[test]
    fn listeners_bounded() {
        let mut connections = Vec::new();
        for i in 0..MAX_LISTENERS + 10 {
            connections.push(Connection {
                protocol: Protocol::Tcp,
                local_addr: "0.0.0.0".into(),
                local_port: (i % 65535) as u16,
                remote_addr: "0.0.0.0".into(),
                remote_port: 0,
                state: ConnectionState::Listen,
                inode: i as u64,
                process: None,
            });
        }
        let snapshot = ConnectionSnapshot {
            connections,
            available: true,
            ..Default::default()
        };
        let overview = collect_exposure(&snapshot, None);
        assert!(overview.listeners.len() <= MAX_LISTENERS);
    }

    // --- Fixture builder ---

    fn make_test_overview() -> ExposureOverview {
        let mut connections = Vec::new();
        // TCP LISTEN on 0.0.0.0:22 — all interfaces
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "0.0.0.0".into(),
            local_port: 22,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::Listen,
            inode: 100,
            process: None,
        });
        // TCP LISTEN on 127.0.0.1:631 — loopback
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "127.0.0.1".into(),
            local_port: 631,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::Listen,
            inode: 200,
            process: None,
        });
        // UDP UNCONN on 0.0.0.0:5353 — all interfaces
        connections.push(Connection {
            protocol: Protocol::Udp,
            local_addr: "0.0.0.0".into(),
            local_port: 5353,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::UdpUnconn,
            inode: 300,
            process: None,
        });
        // TCP LISTEN on 192.168.1.100:8080 — local
        connections.push(Connection {
            protocol: Protocol::Tcp,
            local_addr: "192.168.1.100".into(),
            local_port: 8080,
            remote_addr: "0.0.0.0".into(),
            remote_port: 0,
            state: ConnectionState::Listen,
            inode: 400,
            process: None,
        });

        let snapshot = ConnectionSnapshot {
            connections,
            tcp_count: 3,
            udp_count: 1,
            listen_count: 3,
            established_count: 0,
            available: true,
        };

        collect_exposure(&snapshot, Some(true))
    }
}
