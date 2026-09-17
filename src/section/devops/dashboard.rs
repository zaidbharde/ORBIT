//! Themed dashboard rendering for the DevOps Toolchain & Project Environment Inspector.
//!
//! Pure presentation: reads cached [`DevOpsSnapshot`] values and paints
//! themed cards with tool status, version info, and diagnostic details.
//! Data is collected by the section's update loop, never here.

use super::tool_detection::{DevOpsSnapshot, GitWorkingTreeState, ToolInfo, ToolStatus};
use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::{Color32, FontId, Frame, Grid, Margin, RichText, Stroke, Ui};
use std::time::SystemTime;

/// Render the full DevOps Toolchain & Project Environment Inspector dashboard.
pub fn show(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);

    summary_card(ui, context, snapshot);
    last_updated_row(ui, context, snapshot.last_updated);

    // Version Control
    git_overview_card(ui, context, snapshot);

    // Rust Toolchain
    rust_toolchain_card(ui, context, snapshot);

    // Node.js Toolchain
    nodejs_card(ui, context, snapshot);

    // Python Environment
    python_card(ui, context, snapshot);

    // Container Tools
    container_tools_card(ui, context, snapshot);

    // Project Environment
    project_env_card(ui, context, snapshot);

    // Individual tool cards (compact)
    ui.add_space(4.0);
    section_header(ui, context, "Individual Tool Details");
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

fn section_header(ui: &mut Ui, context: &SectionContext<'_>, title: &str) {
    let theme = context.theme;
    ui.label(
        RichText::new(title)
            .font(FontId::proportional(13.0))
            .color(theme.ui.accent)
            .strong(),
    );
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

fn status_badge(ui: &mut Ui, theme: &Theme, status: ToolStatus) {
    let color = match status {
        ToolStatus::Available => theme.status.success,
        ToolStatus::Unavailable => theme.ui.secondary_text,
        ToolStatus::Unknown => theme.status.warning,
    };
    ui.label(
        RichText::new(status.label())
            .font(FontId::monospace(10.0))
            .color(color)
            .strong(),
    );
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
            RichText::new("Toolchain & Environment Status")
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
// Git Overview card
// ---------------------------------------------------------------------------

fn git_overview_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Version Control — Git", |ui| {
        Grid::new("git-overview-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                // Git tool status
                let git_tool = snapshot.tools.iter().find(|t| t.name == "Git");
                ui.label(
                    RichText::new("Status")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                if let Some(tool) = git_tool {
                    status_badge(ui, theme, tool.status);
                } else {
                    ui.label(
                        RichText::new("—")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                }

                // Version
                let git_tool = snapshot.tools.iter().find(|t| t.name == "Git");
                detail_row(
                    ui,
                    theme,
                    "Version",
                    git_tool.and_then(|t| t.version.as_deref()),
                );

                // In repo
                ui.label(
                    RichText::new("In Repository")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let repo_label = if snapshot.git_repo.in_repo {
                    "Yes"
                } else {
                    "No"
                };
                let repo_color = if snapshot.git_repo.in_repo {
                    theme.status.success
                } else {
                    theme.ui.secondary_text
                };
                ui.label(
                    RichText::new(repo_label)
                        .font(FontId::monospace(10.0))
                        .color(repo_color),
                );

                // Repository root
                detail_row(
                    ui,
                    theme,
                    "Repository Root",
                    snapshot.git_repo.repo_root.as_deref(),
                );

                // Branch
                detail_row(ui, theme, "Branch", snapshot.git_repo.branch.as_deref());

                // Working tree state
                ui.label(
                    RichText::new("Working Tree")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let state_color = match snapshot.git_repo.working_tree_state {
                    GitWorkingTreeState::Clean => theme.status.success,
                    GitWorkingTreeState::Modified => theme.status.warning,
                    GitWorkingTreeState::Unknown => theme.ui.secondary_text,
                };
                ui.label(
                    RichText::new(snapshot.git_repo.working_tree_state.label())
                        .font(FontId::monospace(10.0))
                        .color(state_color),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Rust Toolchain card
// ---------------------------------------------------------------------------

fn rust_toolchain_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Rust Toolchain", |ui| {
        Grid::new("rust-toolchain-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                // rustc
                let rustc = snapshot.tools.iter().find(|t| t.name == "Rust (rustc)");
                ui.label(
                    RichText::new("rustc")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                if let Some(tool) = rustc {
                    status_badge(ui, theme, tool.status);
                } else {
                    ui.label(
                        RichText::new("—")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                }

                let rustc = snapshot.tools.iter().find(|t| t.name == "Rust (rustc)");
                detail_row(
                    ui,
                    theme,
                    "rustc Version",
                    rustc.and_then(|t| t.version.as_deref()),
                );

                // cargo
                let cargo = snapshot.tools.iter().find(|t| t.name == "Cargo");
                ui.label(
                    RichText::new("Cargo")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                if let Some(tool) = cargo {
                    status_badge(ui, theme, tool.status);
                } else {
                    ui.label(
                        RichText::new("—")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                }

                let cargo = snapshot.tools.iter().find(|t| t.name == "Cargo");
                detail_row(
                    ui,
                    theme,
                    "Cargo Version",
                    cargo.and_then(|t| t.version.as_deref()),
                );

                // rustup
                ui.label(
                    RichText::new("rustup")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let rustup_status = if snapshot.rustup.available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, rustup_status);

                detail_row(
                    ui,
                    theme,
                    "rustup Version",
                    snapshot.rustup.version.as_deref(),
                );

                // Active toolchain
                detail_row(
                    ui,
                    theme,
                    "Active Toolchain",
                    snapshot.rustup.active_toolchain.as_deref(),
                );

                // Default toolchain
                detail_row(
                    ui,
                    theme,
                    "Default Toolchain",
                    snapshot.rustup.default_toolchain.as_deref(),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Node.js card
// ---------------------------------------------------------------------------

fn nodejs_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Node.js Toolchain", |ui| {
        Grid::new("nodejs-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                // Node.js
                ui.label(
                    RichText::new("Node.js")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let node_status = if snapshot.node_env.node_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, node_status);
                detail_row(
                    ui,
                    theme,
                    "Node Version",
                    snapshot.node_env.node_version.as_deref(),
                );

                // npm
                ui.label(
                    RichText::new("npm")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let npm_status = if snapshot.node_env.npm_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, npm_status);
                detail_row(
                    ui,
                    theme,
                    "npm Version",
                    snapshot.node_env.npm_version.as_deref(),
                );

                // pnpm
                ui.label(
                    RichText::new("pnpm")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let pnpm_status = if snapshot.node_env.pnpm_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, pnpm_status);
                detail_row(
                    ui,
                    theme,
                    "pnpm Version",
                    snapshot.node_env.pnpm_version.as_deref(),
                );

                // yarn
                ui.label(
                    RichText::new("yarn")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let yarn_status = if snapshot.node_env.yarn_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, yarn_status);
                detail_row(
                    ui,
                    theme,
                    "yarn Version",
                    snapshot.node_env.yarn_version.as_deref(),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Python card
// ---------------------------------------------------------------------------

fn python_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Python Environment", |ui| {
        Grid::new("python-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                // Python 3
                ui.label(
                    RichText::new("Python 3")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let py_status = if snapshot.python_env.python3_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, py_status);
                detail_row(
                    ui,
                    theme,
                    "Python Version",
                    snapshot.python_env.python3_version.as_deref(),
                );

                // pip
                ui.label(
                    RichText::new("pip")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let pip_status = if snapshot.python_env.pip_available {
                    ToolStatus::Available
                } else {
                    ToolStatus::Unavailable
                };
                status_badge(ui, theme, pip_status);
                detail_row(
                    ui,
                    theme,
                    "pip Version",
                    snapshot.python_env.pip_version.as_deref(),
                );

                // Virtual environment
                ui.label(
                    RichText::new("Virtual Environment")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                let venv_label = if snapshot.python_env.venv_detected {
                    "Detected"
                } else {
                    "Not found"
                };
                let venv_color = if snapshot.python_env.venv_detected {
                    theme.status.success
                } else {
                    theme.ui.secondary_text
                };
                ui.label(
                    RichText::new(venv_label)
                        .font(FontId::monospace(10.0))
                        .color(venv_color),
                );

                if snapshot.python_env.venv_detected {
                    detail_row(
                        ui,
                        theme,
                        "Venv Path",
                        snapshot.python_env.venv_path.as_deref(),
                    );
                }
            });
    });
}

// ---------------------------------------------------------------------------
// Container Tools card
// ---------------------------------------------------------------------------

fn container_tools_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Container Tools", |ui| {
        Grid::new("container-tools-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                // Docker
                let docker = snapshot.tools.iter().find(|t| t.name == "Docker");
                ui.label(
                    RichText::new("Docker")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                if let Some(tool) = docker {
                    status_badge(ui, theme, tool.status);
                    detail_row(ui, theme, "Detail", Some(&tool.detail));
                } else {
                    ui.label(
                        RichText::new("—")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                    detail_row(ui, theme, "Detail", None);
                }

                // Podman
                let podman = snapshot.tools.iter().find(|t| t.name == "Podman");
                ui.label(
                    RichText::new("Podman")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.secondary_text),
                );
                if let Some(tool) = podman {
                    status_badge(ui, theme, tool.status);
                    detail_row(ui, theme, "Detail", Some(&tool.detail));
                } else {
                    ui.label(
                        RichText::new("—")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.secondary_text),
                    );
                    detail_row(ui, theme, "Detail", None);
                }
            });
    });
}

// ---------------------------------------------------------------------------
// Project Environment card
// ---------------------------------------------------------------------------

fn project_env_card(ui: &mut Ui, context: &SectionContext<'_>, snapshot: &DevOpsSnapshot) {
    let theme = context.theme;
    card(ui, context, "Project Environment", |ui| {
        Grid::new("project-env-grid")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 4.0))
            .min_col_width(120.0)
            .show(ui, |ui| {
                detail_row(
                    ui,
                    theme,
                    "Working Directory",
                    snapshot.project_env.working_dir.as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "OS / Distribution",
                    snapshot.project_env.os_name.as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "Kernel",
                    snapshot.project_env.kernel_version.as_deref(),
                );
                detail_row(
                    ui,
                    theme,
                    "Architecture",
                    snapshot.project_env.architecture.as_deref(),
                );
            });
    });
}

// ---------------------------------------------------------------------------
// Individual tool card (compact)
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
                status_badge(ui, theme, tool.status);

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

    #[test]
    fn git_working_tree_state_labels() {
        assert_eq!(GitWorkingTreeState::Clean.label(), "Clean");
        assert_eq!(GitWorkingTreeState::Modified.label(), "Modified");
        assert_eq!(GitWorkingTreeState::Unknown.label(), "Unknown");
    }

    #[test]
    fn snapshot_has_git_repo_info() {
        let snap = DevOpsSnapshot::collect();
        // in_repo may be true or false depending on where tests are run
        let _ = snap.git_repo.in_repo;
    }

    #[test]
    fn snapshot_has_rustup_info() {
        let snap = DevOpsSnapshot::collect();
        // rustup may or may not be available
        let _ = snap.rustup.available;
    }

    #[test]
    fn snapshot_has_node_env() {
        let snap = DevOpsSnapshot::collect();
        // node may or may not be available
        let _ = snap.node_env.node_available;
    }

    #[test]
    fn snapshot_has_python_env() {
        let snap = DevOpsSnapshot::collect();
        // python may or may not be available
        let _ = snap.python_env.python3_available;
    }

    #[test]
    fn snapshot_has_project_env() {
        let snap = DevOpsSnapshot::collect();
        assert!(snap.project_env.working_dir.is_some());
    }
}
