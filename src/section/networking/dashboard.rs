//! Themed dashboard rendering for the Networking section.
//!
//! Pure presentation: reads cached interface data and paints themed cards,
//! status indicators, detail rows and rolling throughput graphs. Data is
//! collected by the section's update loop, never here.

use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::section::networking::connections::{
    ConnectionFilter, ConnectionSnapshot, filter_connections,
};
use crate::section::system::network::{
    InterfaceState, InterfaceType, NetworkInterfaceInfo, THROUGHPUT_FLOOR_BPS,
};
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::epaint::{PathShape, PathStroke};
use eframe::egui::{
    Align2, Color32, FontId, Frame, Grid, Margin, Pos2, RichText, Shape, Stroke, Ui,
};
use std::collections::VecDeque;

/// Maximum number of connection rows to render to avoid layout blowup.
const MAX_RENDERED_ROWS: usize = 200;

/// Render the full Networking dashboard.
pub fn show(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    interfaces: &[NetworkInterfaceInfo],
    agg_rx: f32,
    agg_tx: f32,
    total_rx: u64,
    total_tx: u64,
    rx_history: &VecDeque<f32>,
    tx_history: &VecDeque<f32>,
    conn_snapshot: &ConnectionSnapshot,
) {
    let theme = context.theme;
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);

    summary_card(ui, context, interfaces, agg_rx, agg_tx, total_rx, total_tx);

    if interfaces.is_empty() {
        empty_state(ui, context);
        return;
    }

    ui.columns(2, |columns| {
        throughput_card(
            &mut columns[0],
            context,
            "Download",
            agg_rx,
            rx_history,
            theme.ui.accent,
        );
        throughput_card(
            &mut columns[1],
            context,
            "Upload",
            agg_tx,
            tx_history,
            theme.status.warning,
        );
    });

    interfaces_card(ui, context, interfaces);

    connections_card(ui, context, conn_snapshot);
}

/// Compact summary row at the top.
fn summary_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    interfaces: &[NetworkInterfaceInfo],
    agg_rx: f32,
    agg_tx: f32,
    total_rx: u64,
    total_tx: u64,
) {
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

        let total = interfaces.len();
        let up = interfaces
            .iter()
            .filter(|i| i.state == InterfaceState::Up)
            .count();
        let down = interfaces
            .iter()
            .filter(|i| i.state == InterfaceState::Down)
            .count();
        let active = interfaces
            .iter()
            .filter(|i| {
                i.state == InterfaceState::Up && i.interface_type != InterfaceType::Loopback
            })
            .count();

        ui.label(
            RichText::new("Network Summary")
                .font(FontId::proportional(13.0))
                .color(theme.ui.text)
                .strong(),
        );

        Grid::new("network-summary-grid")
            .num_columns(4)
            .spacing(egui::vec2(24.0, 6.0))
            .min_col_width(80.0)
            .show(ui, |ui| {
                summary_item(ui, theme, "Total", &total.to_string());
                summary_item_colored(ui, theme, "UP", &up.to_string(), theme.status.success);
                summary_item_colored(ui, theme, "DOWN", &down.to_string(), theme.status.error);
                summary_item(ui, theme, "Active", &active.to_string());
                ui.end_row();
            });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Throughput")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
            ui.label(
                RichText::new(format!(
                    "↓ {}  ↑ {}",
                    format_throughput(agg_rx),
                    format_throughput(agg_tx)
                ))
                .font(FontId::monospace(11.0))
                .color(theme.ui.text),
            );
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Total")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
            ui.label(
                RichText::new(format!(
                    "RX: {}  TX: {}",
                    format_bytes(total_rx),
                    format_bytes(total_tx)
                ))
                .font(FontId::monospace(11.0))
                .color(theme.ui.text),
            );
        });
    });
}

fn summary_item(ui: &mut Ui, theme: &Theme, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(FontId::proportional(10.0))
                .color(theme.ui.secondary_text),
        );
        ui.label(
            RichText::new(value)
                .font(FontId::monospace(13.0))
                .color(theme.ui.text)
                .strong(),
        );
    });
}

fn summary_item_colored(ui: &mut Ui, theme: &Theme, label: &str, value: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(FontId::proportional(10.0))
                .color(theme.ui.secondary_text),
        );
        ui.label(
            RichText::new(value)
                .font(FontId::monospace(13.0))
                .color(color)
                .strong(),
        );
    });
}

fn empty_state(ui: &mut Ui, context: &SectionContext<'_>) {
    let theme = context.theme;
    let appearance = context.appearance;
    let frame = Frame::new()
        .fill(context.panel_fill)
        .inner_margin(Margin::symmetric(12, 24))
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
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No network interfaces detected")
                    .font(FontId::proportional(13.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Interfaces will appear when available.")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
        });
    });
}

/// Rolling throughput graph card.
fn throughput_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    title: &str,
    current: f32,
    history: &VecDeque<f32>,
    color: Color32,
) {
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
        ui.label(
            RichText::new(title)
                .font(FontId::proportional(12.0))
                .color(theme.ui.secondary_text)
                .strong(),
        );
        ui.label(
            RichText::new(format_throughput(current))
                .font(FontId::monospace(18.0))
                .color(theme.ui.text),
        );
        let data: Vec<f32> = history.iter().copied().collect();
        throughput_graph(ui, context, &data, color);
    });
}

/// Detailed interface list card.
fn interfaces_card(ui: &mut Ui, context: &SectionContext<'_>, interfaces: &[NetworkInterfaceInfo]) {
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
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(
            RichText::new("Interfaces")
                .font(FontId::proportional(12.0))
                .color(theme.ui.secondary_text)
                .strong(),
        );
        ui.add_space(2.0);

        // Table header.
        ui.horizontal(|ui| {
            header_cell(ui, theme, "NAME", 90.0);
            header_cell(ui, theme, "STATE", 56.0);
            header_cell(ui, theme, "TYPE", 72.0);
            header_cell(ui, theme, "MAC", 120.0);
            header_cell(ui, theme, "MTU", 48.0);
            header_cell(ui, theme, "ADDRESS", 140.0);
            header_cell(ui, theme, "RX / TX", 100.0);
        });
        ui.separator();

        let row_height = 18.0;
        let max_rows = 12;
        let visible_rows = interfaces.len().min(max_rows);

        egui::ScrollArea::vertical()
            .id_salt("networking-interfaces-list")
            .max_height(row_height * visible_rows as f32 + 8.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for iface in interfaces {
                    interface_row(ui, context, iface, row_height);
                }
            });

        // Detail panels for each interface.
        for iface in interfaces {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);
            interface_detail(ui, context, iface);
        }
    });
}

fn header_cell(ui: &mut Ui, theme: &Theme, label: &str, width: f32) {
    ui.add_sized(
        egui::vec2(width, 14.0),
        egui::Label::new(
            RichText::new(label)
                .font(FontId::monospace(9.0))
                .color(theme.ui.secondary_text)
                .strong(),
        ),
    );
}

fn interface_row(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    iface: &NetworkInterfaceInfo,
    height: f32,
) {
    let theme = context.theme;
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(90.0, height),
            egui::Label::new(
                RichText::new(truncate_str(&iface.name, 12))
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
            egui::vec2(56.0, height),
            egui::Label::new(
                RichText::new(iface.state.label())
                    .font(FontId::monospace(10.0))
                    .color(state_color),
            ),
        );
        ui.add_sized(
            egui::vec2(72.0, height),
            egui::Label::new(
                RichText::new(iface.interface_type.label())
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        let mac = iface.mac_address.as_deref().unwrap_or("--");
        ui.add_sized(
            egui::vec2(120.0, height),
            egui::Label::new(
                RichText::new(truncate_str(mac, 17))
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        let mtu = iface
            .mtu
            .map(|m| m.to_string())
            .unwrap_or_else(|| "--".into());
        ui.add_sized(
            egui::vec2(48.0, height),
            egui::Label::new(
                RichText::new(mtu)
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        let addr = iface
            .ipv4_addresses
            .first()
            .map(|s| s.as_str())
            .or_else(|| iface.ipv6_addresses.first().map(|s| s.as_str()))
            .unwrap_or("--");
        ui.add_sized(
            egui::vec2(140.0, height),
            egui::Label::new(
                RichText::new(truncate_str(addr, 18))
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.text),
            ),
        );
        let rx_text = iface
            .rx_bytes_per_sec
            .map(|v| format_throughput(v))
            .unwrap_or_else(|| "--".into());
        let tx_text = iface
            .tx_bytes_per_sec
            .map(|v| format_throughput(v))
            .unwrap_or_else(|| "--".into());
        ui.add_sized(
            egui::vec2(100.0, height),
            egui::Label::new(
                RichText::new(format!("{rx_text} / {tx_text}"))
                    .font(FontId::monospace(9.0))
                    .color(theme.ui.text),
            ),
        );
    });
}

/// Detailed view for a single interface: counters, addresses, MTU.
fn interface_detail(ui: &mut Ui, context: &SectionContext<'_>, iface: &NetworkInterfaceInfo) {
    let theme = context.theme;

    ui.label(
        RichText::new(format!("{} · {}", iface.name, iface.state.label()))
            .font(FontId::proportional(12.0))
            .color(theme.ui.text)
            .strong(),
    );

    Grid::new(format!("net-detail-{}", iface.name))
        .num_columns(2)
        .spacing(egui::vec2(16.0, 4.0))
        .min_col_width(100.0)
        .show(ui, |ui| {
            detail_row(ui, theme, "Type", Some(iface.interface_type.label()));
            detail_row(ui, theme, "State", Some(iface.state.label()));
            detail_row(ui, theme, "MAC", iface.mac_address.as_deref());
            detail_row(
                ui,
                theme,
                "MTU",
                iface.mtu.map(|m| format!("{m}")).as_deref(),
            );
            ui.end_row();

            if !iface.ipv4_addresses.is_empty() {
                detail_row(ui, theme, "IPv4", Some(&iface.ipv4_addresses.join(", ")));
            }
            if !iface.ipv6_addresses.is_empty() {
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
            ui.end_row();

            counter_row(
                ui,
                theme,
                "RX bytes",
                Some(format_bytes(iface.counters.rx_bytes)),
            );
            counter_row(
                ui,
                theme,
                "TX bytes",
                Some(format_bytes(iface.counters.tx_bytes)),
            );
            counter_row(
                ui,
                theme,
                "RX packets",
                Some(format!("{}", iface.counters.rx_packets)),
            );
            counter_row(
                ui,
                theme,
                "TX packets",
                Some(format!("{}", iface.counters.tx_packets)),
            );
            ui.end_row();

            counter_row(
                ui,
                theme,
                "RX errors",
                Some(format!("{}", iface.counters.rx_errors)),
            );
            counter_row(
                ui,
                theme,
                "TX errors",
                Some(format!("{}", iface.counters.tx_errors)),
            );
            counter_row(
                ui,
                theme,
                "RX drops",
                Some(format!("{}", iface.counters.rx_dropped)),
            );
            counter_row(
                ui,
                theme,
                "TX drops",
                Some(format!("{}", iface.counters.tx_dropped)),
            );
            ui.end_row();
        });
}

fn detail_row(ui: &mut Ui, theme: &Theme, label: &str, value: Option<&str>) {
    ui.label(
        RichText::new(label)
            .font(FontId::proportional(10.0))
            .color(theme.ui.secondary_text),
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
                RichText::new("Unavailable")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
        }
    }
}

fn counter_row(ui: &mut Ui, theme: &Theme, label: &str, value: Option<String>) {
    ui.label(
        RichText::new(label)
            .font(FontId::proportional(10.0))
            .color(theme.ui.secondary_text),
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
                RichText::new("Unavailable")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Connections card
// ---------------------------------------------------------------------------

/// Card showing the local connections/socket table with filters and search.
fn connections_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &ConnectionSnapshot) {
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
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(
            RichText::new("Connections")
                .font(FontId::proportional(12.0))
                .color(theme.ui.secondary_text)
                .strong(),
        );
        ui.add_space(2.0);

        if !snapshot.available {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Connection data unavailable")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text),
                );
            });
            return;
        }

        // Summary row.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Total: {}", snapshot.connections.len()))
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("TCP: {}", snapshot.tcp_count))
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("UDP: {}", snapshot.udp_count))
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
            ui.separator();
            summary_item_colored(
                ui,
                theme,
                "LISTEN",
                &snapshot.listen_count.to_string(),
                theme.ui.accent,
            );
            ui.separator();
            summary_item_colored(
                ui,
                theme,
                "ESTABLISHED",
                &snapshot.established_count.to_string(),
                theme.status.success,
            );
        });

        ui.add_space(4.0);

        // Filter buttons.
        let mem_id = ui.id().with("conn-filter-state");
        let search_id = ui.id().with("conn-search-text");

        let active_filter = ui.memory_mut(|m| m.data.get_persisted::<usize>(mem_id).unwrap_or(0));

        let mut new_filter = active_filter;
        ui.horizontal(|ui| {
            for (idx, f) in ConnectionFilter::ALL.iter().enumerate() {
                let is_active = idx == active_filter;
                let label = RichText::new(f.label())
                    .font(FontId::monospace(10.0))
                    .color(if is_active {
                        theme.ui.text
                    } else {
                        theme.ui.secondary_text
                    });
                let btn = egui::Button::new(label).frame(is_active);
                if ui.add(btn).clicked() {
                    new_filter = idx;
                }
            }
        });

        if new_filter != active_filter {
            ui.memory_mut(|m| m.data.insert_persisted(mem_id, new_filter));
        }

        let filter = ConnectionFilter::ALL[new_filter];

        // Search box.
        let mut search_text: String = ui
            .memory_mut(|m| m.data.get_persisted::<String>(search_id))
            .unwrap_or_default();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Search:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            let response = ui.add_sized(
                egui::vec2(200.0, 18.0),
                egui::TextEdit::singleline(&mut search_text)
                    .hint_text("address, port, protocol, state...")
                    .font(FontId::monospace(10.0)),
            );
            if response.changed() {
                ui.memory_mut(|m| m.data.insert_persisted(search_id, search_text.clone()));
            }
        });

        ui.add_space(4.0);

        // Filter connections.
        let indices = filter_connections(snapshot, filter, &search_text);

        if indices.is_empty() {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("No connections match the current filter")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text),
                );
            });
            return;
        }

        // Table header.
        ui.horizontal(|ui| {
            conn_header_cell(ui, theme, "PROTO", 52.0);
            conn_header_cell(ui, theme, "LOCAL ADDR", 140.0);
            conn_header_cell(ui, theme, "PORT", 56.0);
            conn_header_cell(ui, theme, "REMOTE ADDR", 140.0);
            conn_header_cell(ui, theme, "PORT", 56.0);
            conn_header_cell(ui, theme, "STATE", 80.0);
        });
        ui.separator();

        let row_height = 16.0;
        let render_count = indices.len().min(MAX_RENDERED_ROWS);
        let max_rows = 15;
        let visible_rows = render_count.min(max_rows);

        egui::ScrollArea::vertical()
            .id_salt("networking-connections-list")
            .max_height(row_height * visible_rows as f32 + 4.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for &idx in indices.iter().take(render_count) {
                    conn_row(ui, context, &snapshot.connections[idx], row_height);
                }
            });

        if indices.len() > MAX_RENDERED_ROWS {
            ui.label(
                RichText::new(format!(
                    "Showing {MAX_RENDERED_ROWS} of {} connections",
                    indices.len()
                ))
                .font(FontId::proportional(9.0))
                .color(theme.ui.secondary_text),
            );
        }
    });
}

fn conn_header_cell(ui: &mut Ui, theme: &Theme, label: &str, width: f32) {
    ui.add_sized(
        egui::vec2(width, 14.0),
        egui::Label::new(
            RichText::new(label)
                .font(FontId::monospace(9.0))
                .color(theme.ui.secondary_text)
                .strong(),
        ),
    );
}

fn conn_row(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    conn: &crate::section::networking::connections::Connection,
    height: f32,
) {
    use crate::section::networking::connections::{ConnectionState, Protocol};

    let theme = context.theme;
    let state_color = match conn.state {
        ConnectionState::Listen => theme.ui.accent,
        ConnectionState::Established => theme.status.success,
        ConnectionState::TimeWait => theme.ui.secondary_text,
        ConnectionState::CloseWait => theme.status.warning,
        _ => theme.ui.text,
    };

    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(52.0, height),
            egui::Label::new(
                RichText::new(conn.protocol.label())
                    .font(FontId::monospace(10.0))
                    .color(match conn.protocol {
                        Protocol::Tcp => theme.ui.accent,
                        Protocol::Udp => theme.status.warning,
                    }),
            ),
        );
        ui.add_sized(
            egui::vec2(140.0, height),
            egui::Label::new(
                RichText::new(truncate_str(&conn.local_addr, 18))
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.text),
            ),
        );
        ui.add_sized(
            egui::vec2(56.0, height),
            egui::Label::new(
                RichText::new(conn.local_port.to_string())
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.text),
            ),
        );
        ui.add_sized(
            egui::vec2(140.0, height),
            egui::Label::new(
                RichText::new(truncate_str(&conn.remote_addr, 18))
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        ui.add_sized(
            egui::vec2(56.0, height),
            egui::Label::new(
                RichText::new(conn.remote_port.to_string())
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        ui.add_sized(
            egui::vec2(80.0, height),
            egui::Label::new(
                RichText::new(conn.state.label())
                    .font(FontId::monospace(9.0))
                    .color(state_color),
            ),
        );
    });
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Formats bytes per second as a human-readable throughput string.
pub fn format_throughput(bytes_per_sec: f32) -> String {
    const KB: f32 = 1024.0;
    const MB: f32 = KB * 1024.0;
    const GB: f32 = MB * 1024.0;
    if bytes_per_sec >= GB {
        format!("{:.1} GB/s", bytes_per_sec / GB)
    } else if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.1} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// Human-readable byte size.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let value = bytes as f64;
    if value >= TB {
        format!("{:.2} TB", value / TB)
    } else if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

// ---------------------------------------------------------------------------
// Graph rendering
// ---------------------------------------------------------------------------

fn throughput_graph(ui: &mut Ui, context: &SectionContext<'_>, history: &[f32], color: Color32) {
    let theme = context.theme;
    let height = 80.0;
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

    const HISTORY_LEN: usize = 60;
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
            format_throughput(peak),
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
            format_throughput(peak),
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

    #[test]
    fn format_throughput_bytes() {
        assert_eq!(format_throughput(0.0), "0 B/s");
        assert_eq!(format_throughput(512.0), "512 B/s");
        assert_eq!(format_throughput(999.0), "999 B/s");
    }

    #[test]
    fn format_throughput_kilobytes() {
        assert_eq!(format_throughput(1024.0), "1.0 KB/s");
        assert_eq!(format_throughput(1536.0), "1.5 KB/s");
    }

    #[test]
    fn format_throughput_megabytes() {
        assert_eq!(format_throughput(1_048_576.0), "1.0 MB/s");
        assert_eq!(format_throughput(1_572_864.0), "1.5 MB/s");
    }

    #[test]
    fn format_throughput_gigabytes() {
        assert_eq!(format_throughput(1_073_741_824.0), "1.0 GB/s");
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.00 GB");
        assert_eq!(format_bytes(1_099_511_627_776), "1.00 TB");
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let result = truncate_str("hello world", 5);
        assert_eq!(result, "hello");
    }
}
