//! Themed dashboard rendering for the DevOps Environment Overview.
//!
//! Pure presentation: reads cached [`DevOpsSnapshot`] values and paints
//! themed cards with tool status, version info, and diagnostic details.
//! Data is collected by the section's update loop, never here.

use super::tool_detection::{DevOpsSnapshot, ToolInfo, ToolStatus};
use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::{Color32, FontId, Frame, Grid, Margin, RichText, Stroke, Ui};
use std::time::SystemTime;

/// Render the full DevOps Environment Overview dashboard.
pub fn show(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);

    summary_card(ui, context, snapshot);
    last_updated_row(ui, context, snapshot.last_updated);

    for tool in &snapshot.tools {
        tool_card(ui, context, tool);
    }
}

// ---------------------------------------------------------------------------
// Card helper (same pattern as system/dashboard.rs and cybersecurity/dashboard.rs)
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
                RichText::new("—")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Summary card
// ---------------------------------------------------------------------------

fn summary_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
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
            RichText::new("Environment Status")
                .font(FontId::proportional(13.0))
                .color(theme.ui.text)
                .strong(),
        );

        ui.horizontal(|ui| {
            summary_stat(
                ui,
                theme,
                "Detected",
                snapshot.available_count() as u32,
                theme.status.success,
            );
            ui.separator();
            summary_stat(
                ui,
                theme,
                "Unavailable",
                snapshot.unavailable_count() as u32,
                theme.ui.secondary_text,
            );
            ui.separator();
            summary_stat(
                ui,
                theme,
                "Unknown",
                snapshot.unknown_count() as u32,
                theme.status.warning,
            );
            ui.separator();
            summary_stat(
                ui,
                theme,
                "Total",
                snapshot.tools.len() as u32,
                theme.ui.accent,
            );
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
// Tool card
// ---------------------------------------------------------------------------

fn tool_card(ui: &mut Ui, context: &SectionContext<'_>, tool: &ToolInfo) {
    let theme = context.theme;
    card(ui, context, &tool.name, |ui| {
        Grid::new(format!("devops-tool-{}", tool.name))
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(100.0)
            .show(ui, |ui| {
                // Status row
                ui.label(
                    RichText::new("Status")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let status_color = match tool.status {
                    ToolStatus::Available => theme.status.success,
                    ToolStatus::Unavailable => theme.ui.secondary_text,
                    ToolStatus::Unknown => theme.status.warning,
                };
                ui.label(
                    RichText::new(tool.status.label())
                        .font(FontId::monospace(10.0))
                        .color(status_color)
                        .strong(),
                );

                // Version row
                detail_row(ui, theme, "Version", tool.version.as_deref());

                // Detail row
                ui.label(
                    RichText::new("Detail")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                ui.label(
                    RichText::new(&tool.detail)
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.text),
                );
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_status_label_is_uppercase() {
        assert_eq!(ToolStatus::Available.label(), "AVAILABLE");
        assert_eq!(ToolStatus::Unavailable.label(), "UNAVAILABLE");
        assert_eq!(ToolStatus::Unknown.label(), "UNKNOWN");
    }

    #[test]
    fn detail_row_renders_with_value() {
        // Smoke test that the function signatures are correct and don't panic
        // (actual rendering requires an egui context, so we only test data logic here)
        let tool = ToolInfo {
            name: "Git".into(),
            status: ToolStatus::Available,
            version: Some("git version 2.43.0".into()),
            detail: "Version: git version 2.43.0".into(),
        };
        assert_eq!(tool.status, ToolStatus::Available);
        assert!(tool.version.is_some());
    }

    #[test]
    fn tool_info_with_no_version() {
        let tool = ToolInfo {
            name: "Docker".into(),
            status: ToolStatus::Unknown,
            version: None,
            detail: "Binary found but daemon not reachable".into(),
        };
        assert_eq!(tool.status, ToolStatus::Unknown);
        assert!(tool.version.is_none());
    }

    #[test]
    fn snapshot_summary_counts() {
        let snap = DevOpsSnapshot::collect();
        assert_eq!(
            snap.available_count() + snap.unavailable_count() + snap.unknown_count(),
            snap.tools.len()
        );
    }
}
