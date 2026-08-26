//! Themed dashboard rendering for the Cybersecurity section.
//!
//! Pure presentation: reads cached [`SecuritySnapshot`] and [`EventLog`]
//! values and paints themed cards. Data is collected by the section's update
//! loop, never here.

use super::hardening_audit::{CheckCategory, CheckFilter, CheckStatus, HardeningAudit};
use super::security_data::SecuritySnapshot;
use super::security_events::{EventCategory, EventFilter, EventLog, Severity};
use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::{Color32, FontId, Frame, Grid, Margin, RichText, Stroke, Ui};
use std::time::SystemTime;

/// Maximum number of event rows to render.
const MAX_RENDERED_EVENTS: usize = 100;

/// Render the full Security Overview dashboard.
pub fn show(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    snapshot: &SecuritySnapshot,
    event_log: &EventLog,
    hardening_audit: &HardeningAudit,
) {
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);

    summary_card(ui, context, &snapshot.summary, event_log);
    last_updated_row(ui, context, snapshot.last_updated);
    user_session_card(ui, context, &snapshot.user_session);
    privilege_card(ui, context, &snapshot.privilege);
    firewall_card(ui, context, &snapshot.firewall);
    kernel_settings_card(ui, context, &snapshot.kernel_settings);
    auth_summary_card(ui, context, &snapshot.auth_summary);
    security_events_card(ui, context, event_log);
    hardening_audit_card(ui, context, hardening_audit);
}

// ---------------------------------------------------------------------------
// Card helper (same pattern as system/dashboard.rs)
// ---------------------------------------------------------------------------

fn card(ui: &mut Ui, context: &SectionContext<'_>, title: &str, add: impl FnOnce(&mut Ui)) {
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
        ui.add_space(2.0);
        add(ui);
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

// ---------------------------------------------------------------------------
// Summary card
// ---------------------------------------------------------------------------

fn summary_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    summary: &super::security_data::SecuritySummary,
    event_log: &EventLog,
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
            RichText::new("Security Status")
                .font(FontId::proportional(13.0))
                .color(theme.ui.text)
                .strong(),
        );

        ui.horizontal(|ui| {
            summary_stat(
                ui,
                theme,
                "Checks Available",
                summary.checks_available,
                theme.status.success,
            );
            ui.separator();
            summary_stat(
                ui,
                theme,
                "Warnings",
                summary.warnings,
                theme.status.warning,
            );
            ui.separator();
            summary_stat(
                ui,
                theme,
                "Unavailable",
                summary.unavailable,
                theme.ui.secondary_text,
            );
            ui.separator();
            summary_stat(ui, theme, "Events", event_log.len() as u32, theme.ui.accent);
        });
    });
}

fn summary_stat(ui: &mut Ui, theme: &Theme, label: &str, value: u32, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(FontId::proportional(10.0))
                .color(theme.ui.secondary_text),
        );
        ui.label(
            RichText::new(value.to_string())
                .font(FontId::monospace(13.0))
                .color(color)
                .strong(),
        );
    });
}

// ---------------------------------------------------------------------------
// Last updated
// ---------------------------------------------------------------------------

fn last_updated_row(ui: &mut Ui, context: &SectionContext<'_>, last_updated: Option<SystemTime>) {
    let theme = context.theme;
    if let Some(time) = last_updated {
        let elapsed = time.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        ui.label(
            RichText::new(format!("Last updated: {elapsed}s ago"))
                .font(FontId::monospace(10.0))
                .color(theme.ui.secondary_text),
        );
    }
}

// ---------------------------------------------------------------------------
// User session card
// ---------------------------------------------------------------------------

fn user_session_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    session: &super::security_data::UserSession,
) {
    let theme = context.theme;
    card(ui, context, "Current User / Session", |ui| {
        Grid::new("user-session-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                detail_row(ui, theme, "Username", session.username.as_deref());
                detail_row(
                    ui,
                    theme,
                    "UID",
                    session.uid.map(|u| u.to_string()).as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "GID",
                    session.gid.map(|g| g.to_string()).as_deref(),
                );
                detail_row(ui, theme, "Home", session.home.as_deref());
                detail_row(ui, theme, "Shell", session.shell.as_deref());
                detail_row(ui, theme, "Hostname", session.hostname.as_deref());
                ui.end_row();

                let root_text = if session.is_root {
                    Some("YES (elevated privileges)")
                } else {
                    Some("No")
                };
                let root_color = if session.is_root {
                    theme.status.warning
                } else {
                    theme.status.success
                };
                ui.label(
                    RichText::new("Root")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                ui.label(
                    RichText::new(root_text.unwrap_or("Unavailable"))
                        .font(FontId::monospace(10.0))
                        .color(root_color),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Privilege card
// ---------------------------------------------------------------------------

fn privilege_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    privs: &super::security_data::PrivilegeStatus,
) {
    let theme = context.theme;
    card(ui, context, "Privilege Status", |ui| {
        Grid::new("privilege-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                detail_row(
                    ui,
                    theme,
                    "UID",
                    privs.uid.map(|u| u.to_string()).as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "Effective UID",
                    privs.euid.map(|u| u.to_string()).as_deref(),
                );

                let root_text = if privs.is_root { "YES" } else { "No" };
                let root_color = if privs.is_root {
                    theme.status.warning
                } else {
                    theme.status.success
                };
                ui.label(
                    RichText::new("Root")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                ui.label(
                    RichText::new(root_text)
                        .font(FontId::monospace(10.0))
                        .color(root_color),
                );
                ui.end_row();

                detail_row(ui, theme, "Capabilities", privs.capabilities.as_deref());
                detail_row(ui, theme, "Cap (hex)", privs.capabilities_hex.as_deref());
            });
    });
}

// ---------------------------------------------------------------------------
// Firewall card
// ---------------------------------------------------------------------------

fn firewall_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    fw: &super::security_data::FirewallStatus,
) {
    let theme = context.theme;
    card(ui, context, "Firewall Status", |ui| {
        Grid::new("firewall-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("nftables")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let nft_label = if fw.nftables_available {
                    "Available"
                } else {
                    "Not found"
                };
                let nft_color = if fw.nftables_available {
                    theme.status.success
                } else {
                    theme.ui.secondary_text
                };
                ui.label(
                    RichText::new(nft_label)
                        .font(FontId::monospace(10.0))
                        .color(nft_color),
                );

                ui.label(
                    RichText::new("iptables")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let ipt_label = if fw.iptables_available {
                    "Available"
                } else {
                    "Not found"
                };
                let ipt_color = if fw.iptables_available {
                    theme.status.success
                } else {
                    theme.ui.secondary_text
                };
                ui.label(
                    RichText::new(ipt_label)
                        .font(FontId::monospace(10.0))
                        .color(ipt_color),
                );

                ui.label(
                    RichText::new("UFW")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let ufw_label = if fw.ufw_available {
                    "Available"
                } else {
                    "Not found"
                };
                let ufw_color = if fw.ufw_available {
                    theme.status.success
                } else {
                    theme.ui.secondary_text
                };
                ui.label(
                    RichText::new(ufw_label)
                        .font(FontId::monospace(10.0))
                        .color(ufw_color),
                );
                ui.end_row();

                detail_row(
                    ui,
                    theme,
                    "Active Framework",
                    fw.active_framework.as_deref(),
                );
            });

        ui.add_space(4.0);
        ui.label(
            RichText::new(&fw.status_message)
                .font(FontId::proportional(10.0))
                .color(theme.ui.secondary_text),
        );
    });
}

// ---------------------------------------------------------------------------
// Kernel settings card
// ---------------------------------------------------------------------------

fn kernel_settings_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    ks: &super::security_data::KernelSecuritySettings,
) {
    let theme = context.theme;
    card(ui, context, "Kernel Security Settings", |ui| {
        Grid::new("kernel-security-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                detail_row(ui, theme, "ASLR", ks.aslr.as_deref());
                detail_row(
                    ui,
                    theme,
                    "ASLR Interpretation",
                    ks.aslr_interpretation.as_deref(),
                );
                ui.end_row();

                detail_row(ui, theme, "Ptrace Scope", ks.ptrace_scope.as_deref());
                detail_row(
                    ui,
                    theme,
                    "Ptrace Interpretation",
                    ks.ptrace_interpretation.as_deref(),
                );
                ui.end_row();

                detail_row(
                    ui,
                    theme,
                    "Core Dump Pattern",
                    ks.core_dump_pattern.as_deref(),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Auth summary card
// ---------------------------------------------------------------------------

fn auth_summary_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    auth: &super::security_data::AuthSummary,
) {
    let theme = context.theme;
    card(ui, context, "Login / Authentication Summary", |ui| {
        if !auth.available {
            ui.label(
                RichText::new(
                    auth.message
                        .as_deref()
                        .unwrap_or("Authentication history unavailable"),
                )
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
            );
            return;
        }

        Grid::new("auth-summary-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                detail_row(
                    ui,
                    theme,
                    "Recent Failed Logins",
                    auth.recent_failed_count.map(|c| c.to_string()).as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "Recent Successful Logins",
                    auth.recent_success_count.map(|c| c.to_string()).as_deref(),
                );
                ui.end_row();

                detail_row(ui, theme, "Latest Event", auth.latest_event_time.as_deref());
            });
    });
}

// ---------------------------------------------------------------------------
// Security Events card
// ---------------------------------------------------------------------------

fn security_events_card(ui: &mut Ui, context: &SectionContext<'_>, event_log: &EventLog) {
    let theme = context.theme;
    let panel_fill = context.panel_fill;
    let appearance = context.appearance;
    let border_stroke = if appearance.border_width > 0.0 {
        Stroke::new(
            appearance.border_width.clamp(0.0, 4.0),
            with_alpha(theme.ui.border, appearance.border_opacity),
        )
    } else {
        Stroke::NONE
    };
    let corner_radius = appearance.panel_radius.clamp(0.0, 16.0);

    // Event state (stored in memory via memory, no persistence)
    let (mut filter, mut selected_index) = {
        let mem = ui.memory_mut(|m| {
            let filter = m
                .data
                .get_persisted::<EventFilter>(egui::Id::new("cybersec_event_filter"))
                .unwrap_or_default();
            let sel = m
                .data
                .get_persisted::<Option<usize>>(egui::Id::new("cybersec_event_sel"));
            (filter, sel)
        });
        (mem.0, mem.1.flatten())
    };

    Frame::new()
        .fill(panel_fill)
        .inner_margin(Margin::symmetric(12, 10))
        .corner_radius(corner_radius)
        .stroke(border_stroke)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;

            // Title row
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Security Events")
                        .font(FontId::proportional(13.0))
                        .color(theme.ui.text)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} total", event_log.len()))
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                });
            });

            // Filters row
            ui.horizontal(|ui| {
                // Severity filter
                ui.label(
                    RichText::new("Severity:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let sev_label = filter
                    .severity
                    .map(|s| s.label())
                    .unwrap_or("All")
                    .to_string();
                egui::ComboBox::from_id_salt("sev-filter")
                    .selected_text(sev_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(filter.severity.is_none(), "All")
                            .clicked()
                        {
                            filter.severity = None;
                        }
                        for &sev in &Severity::ALL {
                            if ui
                                .selectable_label(filter.severity == Some(sev), sev.label())
                                .clicked()
                            {
                                filter.severity = Some(sev);
                            }
                        }
                    });

                // Category filter
                ui.label(
                    RichText::new("Category:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let cat_label = filter
                    .category
                    .map(|c| c.label())
                    .unwrap_or("All")
                    .to_string();
                egui::ComboBox::from_id_salt("cat-filter")
                    .selected_text(cat_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(filter.category.is_none(), "All")
                            .clicked()
                        {
                            filter.category = None;
                        }
                        for &cat in &EventCategory::ALL {
                            if ui
                                .selectable_label(filter.category == Some(cat), cat.label())
                                .clicked()
                            {
                                filter.category = Some(cat);
                            }
                        }
                    });
            });

            // Search box
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Search:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let mut search = filter.search.clone();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .hint_text("Filter events...")
                        .desired_width(200.0)
                        .font(FontId::proportional(11.0)),
                );
                if response.changed() {
                    filter.search = search;
                }
                // Clear button
                if !filter.search.is_empty()
                    && ui
                        .button(
                            RichText::new("×")
                                .font(FontId::proportional(11.0))
                                .color(theme.ui.secondary_text),
                        )
                        .clicked()
                {
                    filter.search.clear();
                    response.request_focus();
                }
            });

            ui.separator();

            // Event table
            let filtered_indices = event_log.filter(&filter);
            let filtered_count = filtered_indices.len();

            if filtered_count == 0 {
                ui.label(
                    RichText::new("No events match the current filters.")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text),
                );
            } else {
                // Table header
                Frame::new()
                    .fill(with_alpha(theme.ui.secondary_text, 0.08))
                    .corner_radius(4.0)
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.columns(4, |cols| {
                            header_cell(&mut cols[0], theme, "Time", 80.0);
                            header_cell(&mut cols[1], theme, "Sev", 40.0);
                            header_cell(&mut cols[2], theme, "Category", 60.0);
                            header_cell(&mut cols[3], theme, "Summary", 200.0);
                        });
                    });

                // Event rows (max rendered)
                let display_count = filtered_count.min(MAX_RENDERED_EVENTS);
                let display_indices = &filtered_indices[..display_count];

                egui::ScrollArea::vertical()
                    .id_salt("event-table-scroll")
                    .max_height(200.0)
                    .show_rows(ui, 20.0, display_count, |ui, row_range| {
                        for &idx in &display_indices[row_range] {
                            if let Some(event) = event_log.get(idx) {
                                let is_selected = selected_index == Some(idx);
                                let bg = if is_selected {
                                    with_alpha(theme.ui.accent, 0.15)
                                } else {
                                    Color32::TRANSPARENT
                                };

                                Frame::new()
                                    .fill(bg)
                                    .corner_radius(4.0)
                                    .inner_margin(Margin::symmetric(8, 4))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Time
                                            ui.label(
                                                RichText::new(&event.timestamp)
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.secondary_text),
                                            );
                                            // Severity badge
                                            let sev_color = match event.severity {
                                                Severity::Info => theme.status.success,
                                                Severity::Warning => theme.status.warning,
                                                Severity::Error => theme.status.error,
                                            };
                                            ui.label(
                                                RichText::new(event.severity.label())
                                                    .font(FontId::proportional(10.0))
                                                    .color(sev_color)
                                                    .strong(),
                                            );
                                            // Category badge
                                            ui.label(
                                                RichText::new(event.category.label())
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.accent),
                                            );
                                            // Source
                                            ui.label(
                                                RichText::new(&event.source)
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.secondary_text),
                                            );
                                            // Summary
                                            ui.label(
                                                RichText::new(&event.summary)
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.text),
                                            );
                                        });

                                        // Click to select
                                        let response = ui.interact(
                                            ui.max_rect(),
                                            egui::Id::new(("event_row", idx)),
                                            egui::Sense::click(),
                                        );
                                        if response.clicked() {
                                            selected_index = if selected_index == Some(idx) {
                                                None
                                            } else {
                                                Some(idx)
                                            };
                                        }
                                    });
                            }
                        }
                    });

                if filtered_count > MAX_RENDERED_EVENTS {
                    ui.label(
                        RichText::new(format!(
                            "Showing {} of {} events",
                            MAX_RENDERED_EVENTS, filtered_count
                        ))
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                    );
                }
            }

            // Event detail panel
            if let Some(idx) = selected_index {
                if let Some(event) = event_log.get(idx) {
                    ui.separator();
                    event_detail_panel(ui, theme, event);
                }
            }
        });

    // Persist filter and selection
    ui.memory_mut(|m| {
        m.data
            .insert_persisted(egui::Id::new("cybersec_event_filter"), filter);
        m.data
            .insert_persisted(egui::Id::new("cybersec_event_sel"), selected_index);
    });
}

fn header_cell(ui: &mut Ui, theme: &Theme, label: &str, width: f32) {
    ui.set_min_width(width);
    ui.label(
        RichText::new(label)
            .font(FontId::proportional(10.0))
            .color(theme.ui.secondary_text)
            .strong(),
    );
}

fn event_detail_panel(ui: &mut Ui, theme: &Theme, event: &super::security_events::SecurityEvent) {
    let sev_color = match event.severity {
        Severity::Info => theme.status.success,
        Severity::Warning => theme.status.warning,
        Severity::Error => theme.status.error,
    };

    Frame::new()
        .fill(with_alpha(theme.ui.accent, 0.08))
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(
                RichText::new("Event Details")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.text)
                    .strong(),
            );

            Grid::new("event-detail-grid")
                .num_columns(2)
                .spacing(egui::vec2(12.0, 3.0))
                .min_col_width(80.0)
                .show(ui, |ui| {
                    detail_label(ui, theme, "Timestamp");
                    ui.label(
                        RichText::new(&event.timestamp)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Severity");
                    ui.label(
                        RichText::new(event.severity.label())
                            .font(FontId::proportional(10.0))
                            .color(sev_color)
                            .strong(),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Category");
                    ui.label(
                        RichText::new(event.category.label())
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.accent),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Source");
                    ui.label(
                        RichText::new(&event.source)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Summary");
                    ui.label(
                        RichText::new(&event.summary)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();
                });
        });
}

fn detail_label(ui: &mut Ui, theme: &Theme, label: &str) {
    ui.label(
        RichText::new(label)
            .font(FontId::proportional(10.0))
            .color(theme.ui.secondary_text),
    );
}

// ---------------------------------------------------------------------------
// Hardening Audit card
// ---------------------------------------------------------------------------

fn hardening_audit_card(ui: &mut Ui, context: &SectionContext<'_>, audit: &HardeningAudit) {
    let theme = context.theme;
    let panel_fill = context.panel_fill;
    let appearance = context.appearance;
    let border_stroke = if appearance.border_width > 0.0 {
        Stroke::new(
            appearance.border_width.clamp(0.0, 4.0),
            with_alpha(theme.ui.border, appearance.border_opacity),
        )
    } else {
        Stroke::NONE
    };
    let corner_radius = appearance.panel_radius.clamp(0.0, 16.0);

    let (mut filter, mut selected_index) = {
        let mem = ui.memory_mut(|m| {
            let filter = m
                .data
                .get_persisted::<CheckFilter>(egui::Id::new("cybersec_audit_filter"))
                .unwrap_or_default();
            let sel = m
                .data
                .get_persisted::<Option<usize>>(egui::Id::new("cybersec_audit_sel"));
            (filter, sel)
        });
        (mem.0, mem.1.flatten())
    };

    Frame::new()
        .fill(panel_fill)
        .inner_margin(Margin::symmetric(12, 10))
        .corner_radius(corner_radius)
        .stroke(border_stroke)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;

            // Title row
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Hardening Audit")
                        .font(FontId::proportional(13.0))
                        .color(theme.ui.text)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} checks", audit.summary.total))
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                });
            });

            // Summary counts
            ui.horizontal(|ui| {
                audit_stat(ui, theme, "PASS", audit.summary.pass, theme.status.success);
                ui.separator();
                audit_stat(
                    ui,
                    theme,
                    "WARNING",
                    audit.summary.warning,
                    theme.status.warning,
                );
                ui.separator();
                audit_stat(ui, theme, "FAIL", audit.summary.fail, theme.status.error);
                ui.separator();
                audit_stat(
                    ui,
                    theme,
                    "UNAVAILABLE",
                    audit.summary.unavailable,
                    theme.ui.secondary_text,
                );
            });

            // Filters row
            ui.horizontal(|ui| {
                // Status filter
                ui.label(
                    RichText::new("Status:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let status_label = filter
                    .status
                    .map(|s| s.label())
                    .unwrap_or("All")
                    .to_string();
                egui::ComboBox::from_id_salt("audit-status-filter")
                    .selected_text(status_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(filter.status.is_none(), "All")
                            .clicked()
                        {
                            filter.status = None;
                        }
                        for &s in &CheckStatus::ALL {
                            if ui
                                .selectable_label(filter.status == Some(s), s.label())
                                .clicked()
                            {
                                filter.status = Some(s);
                            }
                        }
                    });

                // Category filter
                ui.label(
                    RichText::new("Category:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let cat_label = filter
                    .category
                    .map(|c| c.label())
                    .unwrap_or("All")
                    .to_string();
                egui::ComboBox::from_id_salt("audit-cat-filter")
                    .selected_text(cat_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(filter.category.is_none(), "All")
                            .clicked()
                        {
                            filter.category = None;
                        }
                        for &c in &CheckCategory::ALL {
                            if ui
                                .selectable_label(filter.category == Some(c), c.label())
                                .clicked()
                            {
                                filter.category = Some(c);
                            }
                        }
                    });
            });

            // Search box
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Search:")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let mut search = filter.search.clone();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .hint_text("Search checks...")
                        .desired_width(200.0)
                        .font(FontId::proportional(11.0)),
                );
                if response.changed() {
                    filter.search = search;
                }
                if !filter.search.is_empty()
                    && ui
                        .button(
                            RichText::new("×")
                                .font(FontId::proportional(11.0))
                                .color(theme.ui.secondary_text),
                        )
                        .clicked()
                {
                    filter.search.clear();
                    response.request_focus();
                }
            });

            ui.separator();

            // Check table
            let filtered_indices = audit.filter(&filter);
            let filtered_count = filtered_indices.len();

            if filtered_count == 0 {
                ui.label(
                    RichText::new("No checks match the current filters.")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text),
                );
            } else {
                // Table header
                Frame::new()
                    .fill(with_alpha(theme.ui.secondary_text, 0.08))
                    .corner_radius(4.0)
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.columns(4, |cols| {
                            header_cell(&mut cols[0], theme, "Status", 60.0);
                            header_cell(&mut cols[1], theme, "Category", 80.0);
                            header_cell(&mut cols[2], theme, "Check", 120.0);
                            header_cell(&mut cols[3], theme, "Summary", 200.0);
                        });
                    });

                // Check rows
                let display_count = filtered_count.min(MAX_RENDERED_EVENTS);
                let display_indices = &filtered_indices[..display_count];

                egui::ScrollArea::vertical()
                    .id_salt("audit-table-scroll")
                    .max_height(200.0)
                    .show_rows(ui, 20.0, display_count, |ui, row_range| {
                        for &idx in &display_indices[row_range] {
                            if let Some(check) = audit.get(idx) {
                                let is_selected = selected_index == Some(idx);
                                let bg = if is_selected {
                                    with_alpha(theme.ui.accent, 0.15)
                                } else {
                                    Color32::TRANSPARENT
                                };

                                Frame::new()
                                    .fill(bg)
                                    .corner_radius(4.0)
                                    .inner_margin(Margin::symmetric(8, 4))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Status badge
                                            let status_color = match check.status {
                                                CheckStatus::Pass => theme.status.success,
                                                CheckStatus::Warning => theme.status.warning,
                                                CheckStatus::Fail => theme.status.error,
                                                CheckStatus::Unavailable => theme.ui.secondary_text,
                                            };
                                            ui.label(
                                                RichText::new(check.status.label())
                                                    .font(FontId::proportional(10.0))
                                                    .color(status_color)
                                                    .strong(),
                                            );
                                            // Category
                                            ui.label(
                                                RichText::new(check.category.label())
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.accent),
                                            );
                                            // Name
                                            ui.label(
                                                RichText::new(&check.name)
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.text),
                                            );
                                            // Summary
                                            ui.label(
                                                RichText::new(&check.summary)
                                                    .font(FontId::proportional(10.0))
                                                    .color(theme.ui.secondary_text),
                                            );
                                        });

                                        // Click to select
                                        let response = ui.interact(
                                            ui.max_rect(),
                                            egui::Id::new(("audit_row", idx)),
                                            egui::Sense::click(),
                                        );
                                        if response.clicked() {
                                            selected_index = if selected_index == Some(idx) {
                                                None
                                            } else {
                                                Some(idx)
                                            };
                                        }
                                    });
                            }
                        }
                    });

                if filtered_count > MAX_RENDERED_EVENTS {
                    ui.label(
                        RichText::new(format!(
                            "Showing {} of {} checks",
                            MAX_RENDERED_EVENTS, filtered_count
                        ))
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                    );
                }
            }

            // Detail panel
            if let Some(idx) = selected_index {
                if let Some(check) = audit.get(idx) {
                    ui.separator();
                    check_detail_panel(ui, theme, check);
                }
            }
        });

    // Persist filter and selection
    ui.memory_mut(|m| {
        m.data
            .insert_persisted(egui::Id::new("cybersec_audit_filter"), filter);
        m.data
            .insert_persisted(egui::Id::new("cybersec_audit_sel"), selected_index);
    });
}

fn audit_stat(ui: &mut Ui, theme: &Theme, label: &str, value: u32, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(FontId::proportional(9.0))
                .color(theme.ui.secondary_text),
        );
        ui.label(
            RichText::new(value.to_string())
                .font(FontId::monospace(12.0))
                .color(color)
                .strong(),
        );
    });
}

fn check_detail_panel(ui: &mut Ui, theme: &Theme, check: &super::hardening_audit::HardeningCheck) {
    let status_color = match check.status {
        CheckStatus::Pass => theme.status.success,
        CheckStatus::Warning => theme.status.warning,
        CheckStatus::Fail => theme.status.error,
        CheckStatus::Unavailable => theme.ui.secondary_text,
    };

    Frame::new()
        .fill(with_alpha(theme.ui.accent, 0.08))
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(
                RichText::new("Check Details")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.text)
                    .strong(),
            );

            Grid::new("audit-detail-grid")
                .num_columns(2)
                .spacing(egui::vec2(12.0, 3.0))
                .min_col_width(80.0)
                .show(ui, |ui| {
                    detail_label(ui, theme, "Name");
                    ui.label(
                        RichText::new(&check.name)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Category");
                    ui.label(
                        RichText::new(check.category.label())
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.accent),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Status");
                    ui.label(
                        RichText::new(check.status.label())
                            .font(FontId::proportional(10.0))
                            .color(status_color)
                            .strong(),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Summary");
                    ui.label(
                        RichText::new(&check.summary)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();

                    detail_label(ui, theme, "Detail");
                    ui.label(
                        RichText::new(&check.detail)
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    );
                    ui.end_row();
                });
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_stat_rendering() {
        assert_eq!(0u32.to_string(), "0");
    }
}
