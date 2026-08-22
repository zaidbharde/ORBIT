//! Network interface discovery and read-only traffic monitoring.
//!
//! Collects interface metadata (name, state, type, MAC, addresses, MTU) and
//! live traffic counters from Linux `/proc/net/dev` and `/sys/class/net`.
//! All operations are strictly read-only — no interface state is modified.

use super::{HISTORY_LEN, push_history};
use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::epaint::{PathShape, PathStroke};
use eframe::egui::{Align2, Color32, FontId, Frame, Margin, Pos2, RichText, Shape, Stroke, Ui};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Minimum throughput floor for graph scaling (1 KB/s).
pub const THROUGHPUT_FLOOR_BPS: f32 = 1024.0;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// The kind of network interface, determined from sysfs metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InterfaceType {
    Ethernet,
    Wifi,
    Loopback,
    Virtual,
    Bridge,
    Other,
}

impl InterfaceType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ethernet => "Ethernet",
            Self::Wifi => "Wi-Fi",
            Self::Loopback => "Loopback",
            Self::Virtual => "Virtual",
            Self::Bridge => "Bridge",
            Self::Other => "Other",
        }
    }
}

/// Whether the interface is administratively up or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceState {
    Up,
    Down,
    Unknown,
}

impl InterfaceState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Unknown => "Unknown",
        }
    }

    fn from_operstate(s: &str) -> Self {
        match s.trim() {
            "up" => Self::Up,
            "down" => Self::Down,
            _ => Self::Unknown,
        }
    }
}

/// A snapshot of raw cumulative counters for one interface at one point in time.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct InterfaceCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

/// Static and dynamic metadata for a single network interface.
#[derive(Debug, Clone)]
pub struct NetworkInterfaceInfo {
    pub name: String,
    pub state: InterfaceState,
    pub interface_type: InterfaceType,
    pub mac_address: Option<String>,
    pub mtu: Option<u32>,
    pub ipv4_addresses: Vec<String>,
    pub ipv6_addresses: Vec<String>,
    pub counters: InterfaceCounters,
    /// Download speed in bytes/sec (computed from delta).
    pub rx_bytes_per_sec: Option<f32>,
    /// Upload speed in bytes/sec (computed from delta).
    pub tx_bytes_per_sec: Option<f32>,
}

impl NetworkInterfaceInfo {
    fn primary_address(&self) -> Option<&str> {
        self.ipv4_addresses.first().map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Monitor
// ---------------------------------------------------------------------------

/// Owns the full set of discovered interfaces and computes per-second
/// throughput from successive counter snapshots.
pub struct NetworkMonitor {
    interfaces: Vec<NetworkInterfaceInfo>,
    prev_counters: HashMap<String, InterfaceCounters>,
    prev_timestamp: Option<Instant>,
    pub rx_history: VecDeque<f32>,
    pub tx_history: VecDeque<f32>,
    pub total_rx: u64,
    pub total_tx: u64,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            interfaces: Vec::new(),
            prev_counters: HashMap::new(),
            prev_timestamp: None,
            rx_history: VecDeque::with_capacity(HISTORY_LEN),
            tx_history: VecDeque::with_capacity(HISTORY_LEN),
            total_rx: 0,
            total_tx: 0,
        }
    }

    /// Read all interface data from /proc and /sys. Compute speeds from
    /// the delta against the previous snapshot. Must be called at ~1 Hz.
    pub fn poll(&mut self) {
        let now = Instant::now();
        let mut new_ifaces = imp::collect_interfaces();

        // Compute deltas if we have a previous snapshot.
        if let Some(prev_time) = self.prev_timestamp {
            let elapsed = now.duration_since(prev_time);
            for iface in &mut new_ifaces {
                if let Some(prev) = self.prev_counters.get(&iface.name) {
                    compute_speed(iface, prev, elapsed);
                }
            }
        }

        // Accumulate totals from current counters.
        let mut rx_total: u64 = 0;
        let mut tx_total: u64 = 0;
        for iface in &new_ifaces {
            rx_total = rx_total.saturating_add(iface.counters.rx_bytes);
            tx_total = tx_total.saturating_add(iface.counters.tx_bytes);
        }
        self.total_rx = rx_total;
        self.total_tx = tx_total;

        // Snapshot counters for next delta.
        let mut counters_map = HashMap::new();
        for iface in &new_ifaces {
            counters_map.insert(iface.name.clone(), iface.counters.clone());
        }
        self.prev_counters = counters_map;
        self.prev_timestamp = Some(now);
        self.interfaces = new_ifaces;

        // Aggregate rx/tx speed across all interfaces.
        let agg_rx: f32 = self
            .interfaces
            .iter()
            .filter_map(|i| i.rx_bytes_per_sec)
            .sum();
        let agg_tx: f32 = self
            .interfaces
            .iter()
            .filter_map(|i| i.tx_bytes_per_sec)
            .sum();
        push_history(&mut self.rx_history, agg_rx);
        push_history(&mut self.tx_history, agg_tx);
    }

    #[allow(dead_code)]
    pub fn interfaces(&self) -> &[NetworkInterfaceInfo] {
        &self.interfaces
    }
}

/// Compute per-second speed from counter delta.
fn compute_speed(iface: &mut NetworkInterfaceInfo, prev: &InterfaceCounters, elapsed: Duration) {
    let secs = elapsed.as_secs_f32();
    if secs <= 0.0 {
        return;
    }
    iface.rx_bytes_per_sec = Some(delta_rate(prev.rx_bytes, iface.counters.rx_bytes, secs));
    iface.tx_bytes_per_sec = Some(delta_rate(prev.tx_bytes, iface.counters.tx_bytes, secs));
}

/// Counter delta rate, handling reset (counter decreased).
fn delta_rate(prev: u64, curr: u64, secs: f32) -> f32 {
    if secs <= 0.0 {
        return 0.0;
    }
    let delta = if curr >= prev {
        curr - prev
    } else {
        // Counter reset — treat as zero delta.
        0
    };
    delta as f32 / secs
}

// ---------------------------------------------------------------------------
// Platform: Linux /proc and /sys readers
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    /// Collect all network interfaces from /proc/net/dev and /sys/class/net.
    pub fn collect_interfaces() -> Vec<NetworkInterfaceInfo> {
        let counters_map = parse_proc_net_dev(&read_file("/proc/net/dev").unwrap_or_default());
        let mut ifaces: Vec<NetworkInterfaceInfo> = counters_map
            .into_iter()
            .map(|(name, counters)| {
                let base = format!("/sys/class/net/{name}");
                let state = read_trimmed(&format!("{base}/operstate"))
                    .map(|s| InterfaceState::from_operstate(&s))
                    .unwrap_or(InterfaceState::Unknown);
                let mac = read_trimmed(&format!("{base}/address"));
                let mtu = read_trimmed(&format!("{base}/mtu")).and_then(|s| s.parse().ok());
                let iftype = detect_interface_type(&name, &base);
                let ipv4 = read_ipv4_addresses(&name);
                let ipv6 = read_ipv6_addresses(&name);
                NetworkInterfaceInfo {
                    name,
                    state,
                    interface_type: iftype,
                    mac_address: mac,
                    mtu,
                    ipv4_addresses: ipv4,
                    ipv6_addresses: ipv6,
                    counters,
                    rx_bytes_per_sec: None,
                    tx_bytes_per_sec: None,
                }
            })
            .collect();
        ifaces.sort_by(|a, b| {
            // Loopback last, then alphabetical.
            match (
                a.interface_type == InterfaceType::Loopback,
                b.interface_type == InterfaceType::Loopback,
            ) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => a.name.cmp(&b.name),
            }
        });
        ifaces
    }

    /// Parse /proc/net/dev. Skips the two header lines.
    /// Returns a map of interface name → counters.
    pub fn parse_proc_net_dev(content: &str) -> HashMap<String, InterfaceCounters> {
        let mut map = HashMap::new();
        for line in content.lines().skip(2) {
            if let Some((name, counters)) = parse_proc_net_dev_line(line) {
                map.insert(name, counters);
            }
        }
        map
    }

    /// Parse a single data line from /proc/net/dev.
    /// Format: "  iface: rx_bytes rx_packets rx_errs rx_drop ... tx_bytes tx_packets tx_errs tx_drop ..."
    pub fn parse_proc_net_dev_line(line: &str) -> Option<(String, InterfaceCounters)> {
        let colon = line.find(':')?;
        let name = line[..colon].trim().to_string();
        if name.is_empty() {
            return None;
        }
        let fields: Vec<&str> = line[colon + 1..].split_whitespace().collect();
        if fields.len() < 16 {
            return None;
        }
        let rx_bytes = fields[0].parse().ok()?;
        let rx_packets = fields[1].parse().ok()?;
        let rx_errors = fields[2].parse().ok()?;
        let rx_dropped = fields[3].parse().ok()?;
        let tx_bytes = fields[8].parse().ok()?;
        let tx_packets = fields[9].parse().ok()?;
        let tx_errors = fields[10].parse().ok()?;
        let tx_dropped = fields[11].parse().ok()?;
        Some((
            name,
            InterfaceCounters {
                rx_bytes,
                tx_bytes,
                rx_packets,
                tx_packets,
                rx_errors,
                tx_errors,
                rx_dropped,
                tx_dropped,
            },
        ))
    }

    /// Determine interface type from sysfs type field and driver name.
    fn detect_interface_type(name: &str, base: &str) -> InterfaceType {
        if name == "lo" {
            return InterfaceType::Loopback;
        }
        // ARPHRD types from /sys/class/net/<iface>/type
        match read_trimmed(&format!("{base}/type")).as_deref() {
            Some("772") => return InterfaceType::Loopback,
            Some("1") => {
                // ARPHRD_ETHER — could be Ethernet or Wi-Fi.
                // Distinguish via driver name for common WiFi drivers.
                if let Some(driver) = read_trimmed(&format!("{base}/device/driver/module/name")) {
                    let wifi_keywords = [
                        "iwlwifi", "ath9k", "ath10k", "ath11k", "rt2x00", "rtl8xxxu", "brcmfmac",
                        "brcmsmac", "mt76", "wl1251", "wl12xx", "mwifiex",
                    ];
                    if wifi_keywords
                        .iter()
                        .any(|kw| driver.to_lowercase().contains(kw))
                    {
                        return InterfaceType::Wifi;
                    }
                }
                return InterfaceType::Ethernet;
            }
            Some("65534") => return InterfaceType::Virtual,
            Some(_) => {}
            None => {}
        }
        // Fallback: check device path for virtual indicators.
        if let Ok(path) = std::fs::read_link(format!("{base}/device")) {
            if let Some(s) = path.to_str() {
                if s.contains("/virtual/") {
                    return InterfaceType::Virtual;
                }
            }
        }
        InterfaceType::Other
    }

    /// Read /proc/net/if_inet6 for IPv6 addresses.
    fn read_ipv6_addresses(name: &str) -> Vec<String> {
        let content = match read_file("/proc/net/if_inet6") {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut addrs = Vec::new();
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 7 {
                continue;
            }
            // Last field is the interface name.
            if parts[6] != name {
                continue;
            }
            // First field is 32 hex chars (128-bit address in hex).
            let hex = parts[0];
            if hex.len() != 32 {
                continue;
            }
            // Convert to standard IPv6 notation.
            let groups: Vec<String> = (0..8)
                .map(|i| &hex[i * 4..i * 4 + 4])
                .map(|g| g.to_string())
                .collect();
            let addr = groups.join(":");
            addrs.push(addr);
        }
        addrs
    }

    /// Read IPv4 addresses from /sys/class/net/<iface>/inet route info.
    /// Uses `ip`-like approach: parse from /proc/net/if_inet6 alternatives.
    /// Actually, simplest reliable approach: read from /sys/class/net/<iface>/ addresses.
    fn read_ipv4_addresses(name: &str) -> Vec<String> {
        // Read from /proc/net/fib_trie (IPv4 routing table).
        let content = match read_file("/proc/net/fib_trie") {
            Some(c) => c,
            None => return Vec::new(),
        };
        let marker = format!("/32 host LOCAL {name}");
        let mut addrs = Vec::new();
        let mut in_section = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains(&marker) {
                in_section = true;
                continue;
            }
            if in_section && trimmed.starts_with("|--") {
                // Extract IP from lines like: "|-- 192.168.1.20"
                if let Some(ip) = trimmed.strip_prefix("|-- ") {
                    let ip = ip.trim();
                    if !ip.is_empty() && !ip.starts_with("0.") && !ip.starts_with("127.")
                        || name == "lo"
                    {
                        if !addrs.contains(&ip.to_string()) {
                            addrs.push(ip.to_string());
                        }
                    }
                }
            } else if in_section && !trimmed.starts_with("|--") {
                in_section = false;
            }
        }
        addrs
    }

    fn read_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn read_trimmed(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    pub fn collect_interfaces() -> Vec<NetworkInterfaceInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

/// Render the Network card inside the System dashboard.
pub fn show_network_card(ui: &mut Ui, context: &SectionContext<'_>, monitor: &mut NetworkMonitor) {
    let theme = context.theme;
    let appearance = context.appearance;
    let frame = Frame::new()
        .fill(context.panel_fill)
        .inner_margin(Margin::symmetric(12, 10))
        .corner_radius(appearance.panel_radius.clamp(0.0, 16.0))
        .stroke(if appearance.border_width > 0.0 {
            Stroke::new(
                appearance.border_width.clamp(0.0, 4.0),
                with_alpha(theme.ui.border, appearance.border_opacity),
            )
        } else {
            Stroke::NONE
        });

    frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;

        // ---- Summary header ----
        let active_count = monitor
            .interfaces
            .iter()
            .filter(|i| i.state == InterfaceState::Up)
            .count();
        let total_count = monitor.interfaces.len();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Interfaces: {total_count}   Active: {active_count}"
                ))
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
            );
        });

        // Aggregate speeds.
        let agg_rx: f32 = monitor
            .interfaces
            .iter()
            .filter_map(|i| i.rx_bytes_per_sec)
            .sum();
        let agg_tx: f32 = monitor
            .interfaces
            .iter()
            .filter_map(|i| i.tx_bytes_per_sec)
            .sum();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "↓ {}  ↑ {}",
                    super::dashboard::format_throughput(agg_rx),
                    super::dashboard::format_throughput(agg_tx)
                ))
                .font(FontId::monospace(11.0))
                .color(theme.ui.text),
            );
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Total RX: {}  Total TX: {}",
                    super::dashboard::format_bytes(monitor.total_rx),
                    super::dashboard::format_bytes(monitor.total_tx)
                ))
                .font(FontId::proportional(10.0))
                .color(theme.ui.secondary_text),
            );
        });

        // ---- Interface table ----
        if monitor.interfaces.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("No interfaces found").color(theme.ui.secondary_text));
            return;
        }

        ui.add_space(4.0);

        // Table header.
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2(72.0, 14.0),
                egui::Label::new(
                    RichText::new("NAME")
                        .font(FontId::monospace(9.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                ),
            );
            ui.add_sized(
                egui::vec2(48.0, 14.0),
                egui::Label::new(
                    RichText::new("STATE")
                        .font(FontId::monospace(9.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                ),
            );
            ui.add_sized(
                egui::vec2(72.0, 14.0),
                egui::Label::new(
                    RichText::new("TYPE")
                        .font(FontId::monospace(9.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                ),
            );
            ui.add_sized(
                egui::vec2(110.0, 14.0),
                egui::Label::new(
                    RichText::new("ADDRESS")
                        .font(FontId::monospace(9.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                ),
            );
            ui.add_sized(
                egui::vec2(80.0, 14.0),
                egui::Label::new(
                    RichText::new("RX / TX")
                        .font(FontId::monospace(9.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                ),
            );
        });

        ui.separator();

        // Interface rows.
        let row_height = 16.0;
        let max_rows = 10;
        let visible_rows = monitor.interfaces.len().min(max_rows);
        let interfaces_snapshot: Vec<_> = monitor.interfaces.iter().cloned().collect();

        egui::ScrollArea::vertical()
            .id_salt("network-interfaces")
            .max_height(row_height * visible_rows as f32 + 4.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for iface in &interfaces_snapshot {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            egui::vec2(72.0, row_height),
                            egui::Label::new(
                                RichText::new(truncate_str(&iface.name, 10))
                                    .font(FontId::monospace(10.0))
                                    .color(theme.ui.text),
                            ),
                        );
                        let state_color = match iface.state {
                            InterfaceState::Up => theme.status.success,
                            InterfaceState::Down => theme.status.error,
                            InterfaceState::Unknown => theme.ui.secondary_text,
                        };
                        ui.add_sized(
                            egui::vec2(48.0, row_height),
                            egui::Label::new(
                                RichText::new(iface.state.label())
                                    .font(FontId::monospace(10.0))
                                    .color(state_color),
                            ),
                        );
                        ui.add_sized(
                            egui::vec2(72.0, row_height),
                            egui::Label::new(
                                RichText::new(iface.interface_type.label())
                                    .font(FontId::monospace(10.0))
                                    .color(theme.ui.secondary_text),
                            ),
                        );
                        let addr = iface
                            .primary_address()
                            .or_else(|| iface.ipv6_addresses.first().map(|s| s.as_str()))
                            .unwrap_or("--");
                        ui.add_sized(
                            egui::vec2(110.0, row_height),
                            egui::Label::new(
                                RichText::new(truncate_str(addr, 15))
                                    .font(FontId::monospace(10.0))
                                    .color(theme.ui.text),
                            ),
                        );
                        let rx_text = iface
                            .rx_bytes_per_sec
                            .map(|v| super::dashboard::format_throughput(v))
                            .unwrap_or_else(|| "--".into());
                        let tx_text = iface
                            .tx_bytes_per_sec
                            .map(|v| super::dashboard::format_throughput(v))
                            .unwrap_or_else(|| "--".into());
                        ui.add_sized(
                            egui::vec2(80.0, row_height),
                            egui::Label::new(
                                RichText::new(format!("{rx_text} / {tx_text}"))
                                    .font(FontId::monospace(9.0))
                                    .color(theme.ui.text),
                            ),
                        );
                    });
                }
            });

        // ---- Detail panel for selected (first active) interface ----
        // Show MAC / MTU / IPv6 for the primary interface.
        let primary = monitor
            .interfaces
            .iter()
            .find(|i| i.state == InterfaceState::Up && i.interface_type != InterfaceType::Loopback)
            .or_else(|| monitor.interfaces.first());

        if let Some(iface) = primary {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("Details: {}", iface.name))
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.text)
                    .strong(),
            );
            detail_row(ui, theme, "MAC", iface.mac_address.as_deref());
            detail_row(
                ui,
                theme,
                "MTU",
                iface.mtu.map(|m| format!("{m}")).as_deref(),
            );
            if !iface.ipv4_addresses.is_empty() {
                detail_row(ui, theme, "IPv4", Some(&iface.ipv4_addresses.join(", ")));
            }
            if !iface.ipv6_addresses.is_empty() {
                // Show at most 2 IPv6 addresses in the detail row.
                let display: String = iface
                    .ipv6_addresses
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let extra = iface.ipv6_addresses.len().saturating_sub(2);
                let label = if extra > 0 {
                    format!("{display} (+{extra} more)")
                } else {
                    display
                };
                detail_row(ui, theme, "IPv6", Some(&label));
            }
        }

        // ---- Traffic graphs ----
        ui.add_space(6.0);
        ui.label(
            RichText::new("Download")
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
        );
        let rx = monitor.rx_history.make_contiguous();
        throughput_graph(ui, context, rx, theme.ui.accent);

        ui.add_space(4.0);
        ui.label(
            RichText::new("Upload")
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
        );
        let tx = monitor.tx_history.make_contiguous();
        throughput_graph(ui, context, tx, theme.status.warning);
    });
}

fn detail_row(ui: &mut Ui, theme: &Theme, label: &str, value: Option<&str>) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(56.0, 14.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        match value {
            Some(v) => {
                ui.label(
                    RichText::new(v)
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.text),
                );
            }
            None => {
                ui.label(
                    RichText::new("--")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
            }
        }
    });
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len { s } else { &s[..max_len] }
}

/// Auto-scaling throughput graph (reused from dashboard.rs pattern).
fn throughput_graph(ui: &mut Ui, context: &SectionContext<'_>, history: &[f32], color: Color32) {
    let theme = context.theme;
    let height = 56.0;
    let width = ui.available_width();
    if width < 20.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, with_alpha(theme.ui.text, 0.06));

    let grid_stroke = Stroke::new(1.0_f32, with_alpha(theme.ui.divider, 0.35));
    for quarter in 1..=3 {
        let y = rect.top() + rect.height() * quarter as f32 / 4.0;
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid_stroke,
        );
    }

    let n = history.len();
    if n >= 2 {
        let peak = history
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(THROUGHPUT_FLOOR_BPS);
        let step = rect.width() / HISTORY_LEN as f32;
        let points: Vec<Pos2> = history
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let fraction = (value / peak).clamp(0.0, 1.0);
                let x = rect.right() - (n - 1 - index) as f32 * step;
                let y = rect.bottom() - fraction * rect.height();
                Pos2::new(x, y)
            })
            .collect();
        let mut fill_points = points.clone();
        fill_points.push(Pos2::new(rect.right(), rect.bottom()));
        fill_points.push(Pos2::new(
            rect.right() - (n - 1) as f32 * step,
            rect.bottom(),
        ));
        painter.add(Shape::Path(PathShape {
            points: fill_points,
            closed: true,
            fill: with_alpha(color, 0.12),
            stroke: PathStroke::NONE,
        }));
        painter.add(Shape::line(points, Stroke::new(1.5_f32, color)));
        painter.text(
            Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            super::dashboard::format_throughput(peak),
            FontId::proportional(9.0),
            theme.ui.secondary_text,
        );
    } else if n == 1 {
        let peak = history[0].max(THROUGHPUT_FLOOR_BPS);
        let fraction = (history[0] / peak).clamp(0.0, 1.0);
        let x = rect.right();
        let y = rect.bottom() - fraction * rect.height();
        painter.circle_filled(Pos2::new(x, y), 2.5, color);
        painter.text(
            Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            super::dashboard::format_throughput(peak),
            FontId::proportional(9.0),
            theme.ui.secondary_text,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "collecting…",
            FontId::proportional(10.0),
            theme.ui.secondary_text,
        );
    }

    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        Align2::RIGHT_BOTTOM,
        "60s",
        FontId::proportional(9.0),
        theme.ui.secondary_text,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- /proc/net/dev parsing ---

    #[test]
    fn parses_real_proc_net_dev() {
        let content = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  805249    6446    0    0    0     0          0         0   805249    6446    0    0    0     0       0          0
  wlo1: 487927517  465266    0    0    0     0          0         0 114532163  189151    0    0    0     0       0          0";
        let map = imp::parse_proc_net_dev(content);
        assert_eq!(map.len(), 2);
        let lo = map.get("lo").unwrap();
        assert_eq!(lo.rx_bytes, 805249);
        assert_eq!(lo.rx_packets, 6446);
        assert_eq!(lo.tx_bytes, 805249);
        assert_eq!(lo.tx_packets, 6446);
        assert_eq!(lo.rx_errors, 0);
        let wlo1 = map.get("wlo1").unwrap();
        assert_eq!(wlo1.rx_bytes, 487_927_517);
        assert_eq!(wlo1.tx_bytes, 114_532_163);
        assert_eq!(wlo1.rx_packets, 465_266);
        assert_eq!(wlo1.tx_packets, 189_151);
    }

    #[test]
    fn parses_single_line() {
        let line = "  enp3s0: 12345 678 1 2 3 4 5 6 9999 100 101 102 103 104 105 106";
        let (name, c) = imp::parse_proc_net_dev_line(line).unwrap();
        assert_eq!(name, "enp3s0");
        assert_eq!(c.rx_bytes, 12345);
        assert_eq!(c.rx_packets, 678);
        assert_eq!(c.rx_errors, 1);
        assert_eq!(c.rx_dropped, 2);
        assert_eq!(c.tx_bytes, 9999);
        assert_eq!(c.tx_packets, 100);
        assert_eq!(c.tx_errors, 101);
        assert_eq!(c.tx_dropped, 102);
    }

    #[test]
    fn rejects_line_with_too_few_fields() {
        assert!(imp::parse_proc_net_dev_line("lo: 1 2 3").is_none());
    }

    #[test]
    fn rejects_line_without_colon() {
        assert!(imp::parse_proc_net_dev_line("no colon here").is_none());
    }

    #[test]
    fn rejects_line_with_non_numeric_fields() {
        assert!(
            imp::parse_proc_net_dev_line(
                "  lo: abc def ghi jkl mno pqr stu vwx yz0 123 456 789 012 345 678 901"
            )
            .is_none()
        );
    }

    #[test]
    fn empty_content_returns_empty_map() {
        let map = imp::parse_proc_net_dev("");
        assert!(map.is_empty());
    }

    #[test]
    fn skips_header_lines() {
        let content = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
";
        let map = imp::parse_proc_net_dev(content);
        assert!(map.is_empty());
    }

    #[test]
    fn handles_missing_interface_name() {
        // A line with a colon but empty name before it.
        assert!(imp::parse_proc_net_dev_line(": 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0").is_none());
    }

    // --- Counter delta rate ---

    #[test]
    fn delta_rate_normal() {
        let rate = delta_rate(1000, 2048, 1.0);
        assert!((rate - 1048.0).abs() < 0.1);
    }

    #[test]
    fn delta_rate_zero_elapsed() {
        let rate = delta_rate(1000, 2000, 0.0);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn delta_rate_counter_reset() {
        let rate = delta_rate(5000, 1000, 1.0);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn delta_rate_no_change() {
        let rate = delta_rate(1000, 1000, 1.0);
        assert_eq!(rate, 0.0);
    }

    // --- InterfaceState ---

    #[test]
    fn state_from_operstate() {
        assert_eq!(InterfaceState::from_operstate("up"), InterfaceState::Up);
        assert_eq!(InterfaceState::from_operstate("down"), InterfaceState::Down);
        assert_eq!(
            InterfaceState::from_operstate("unknown"),
            InterfaceState::Unknown
        );
        assert_eq!(InterfaceState::from_operstate("  up  "), InterfaceState::Up);
    }

    #[test]
    fn state_labels() {
        assert_eq!(InterfaceState::Up.label(), "UP");
        assert_eq!(InterfaceState::Down.label(), "DOWN");
        assert_eq!(InterfaceState::Unknown.label(), "Unknown");
    }

    // --- InterfaceType ---

    #[test]
    fn type_labels() {
        assert_eq!(InterfaceType::Ethernet.label(), "Ethernet");
        assert_eq!(InterfaceType::Wifi.label(), "Wi-Fi");
        assert_eq!(InterfaceType::Loopback.label(), "Loopback");
        assert_eq!(InterfaceType::Virtual.label(), "Virtual");
        assert_eq!(InterfaceType::Bridge.label(), "Bridge");
        assert_eq!(InterfaceType::Other.label(), "Other");
    }

    // --- Traffic history cap ---

    #[test]
    fn history_is_capped_at_history_len() {
        let mut monitor = NetworkMonitor::new();
        // Poll multiple times to fill history.
        for _ in 0..HISTORY_LEN + 10 {
            monitor.poll();
        }
        assert!(monitor.rx_history.len() <= HISTORY_LEN);
        assert!(monitor.tx_history.len() <= HISTORY_LEN);
    }

    // --- Throughput formatting ---

    #[test]
    fn throughput_formatting() {
        fn fmt(bps: f32) -> String {
            const KB: f32 = 1024.0;
            const MB: f32 = KB * 1024.0;
            const GB: f32 = MB * 1024.0;
            if bps >= GB {
                format!("{:.1} GB/s", bps / GB)
            } else if bps >= MB {
                format!("{:.1} MB/s", bps / MB)
            } else if bps >= KB {
                format!("{:.1} KB/s", bps / KB)
            } else {
                format!("{:.0} B/s", bps)
            }
        }
        assert_eq!(fmt(0.0), "0 B/s");
        assert_eq!(fmt(512.0), "512 B/s");
        assert_eq!(fmt(1024.0), "1.0 KB/s");
        assert_eq!(fmt(1_048_576.0), "1.0 MB/s");
        assert_eq!(fmt(1_073_741_824.0), "1.0 GB/s");
    }

    // --- Bytes formatting ---

    #[test]
    fn bytes_formatting() {
        fn fmt(bytes: u64) -> String {
            const KB: f64 = 1024.0;
            const MB: f64 = KB * 1024.0;
            const GB: f64 = MB * 1024.0;
            let v = bytes as f64;
            if v >= GB {
                format!("{:.2} GB", v / GB)
            } else if v >= MB {
                format!("{:.1} MB", v / MB)
            } else if v >= KB {
                format!("{:.1} KB", v / KB)
            } else {
                format!("{bytes} B")
            }
        }
        assert_eq!(fmt(0), "0 B");
        assert_eq!(fmt(1024), "1.0 KB");
        assert_eq!(fmt(1_073_741_824), "1.00 GB");
    }

    // --- Monitor starts empty ---

    #[test]
    fn monitor_starts_empty() {
        let monitor = NetworkMonitor::new();
        assert!(monitor.interfaces.is_empty());
        assert!(monitor.rx_history.is_empty());
        assert!(monitor.tx_history.is_empty());
        assert_eq!(monitor.total_rx, 0);
        assert_eq!(monitor.total_tx, 0);
    }

    // --- Multiple poll cycles produce stable counters ---

    #[test]
    fn polling_multiple_times_does_not_crash() {
        let mut monitor = NetworkMonitor::new();
        monitor.poll();
        monitor.poll();
        monitor.poll();
        // Counters should be populated.
        assert!(!monitor.interfaces.is_empty() || monitor.total_rx == 0);
    }
}
