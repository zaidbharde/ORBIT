//! DevOps section: read-only environment overview for development and
//! operations toolchain detection.
//!
//! Checks for Git, Rust toolchain, Node.js, Python and container tools by probing
//! PATH and running fixed, minimal version commands. Also inspects the current
//! project environment (OS, kernel, architecture, working directory).
//! All operations are strictly read-only — no packages are installed, no
//! repositories modified, no containers created or deleted.

pub mod dashboard;
pub mod tool_detection;

use super::{Section, SectionContext, SectionId};
use crate::glass::with_alpha;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::{FontId, RichText};
use std::time::{Duration, Instant};

/// How often tool data is re-collected (1 Hz).
const COLLECT_INTERVAL: Duration = Duration::from_secs(1);

/// The live DevOps Toolchain & Project Environment Inspector section.
pub struct DevOpsSection {
    snapshot: tool_detection::DevOpsSnapshot,
    last_collect: Option<Instant>,
    refresh_pending: bool,
}

impl DevOpsSection {
    pub fn new() -> Self {
        Self {
            snapshot: tool_detection::DevOpsSnapshot::default(),
            last_collect: None,
            refresh_pending: false,
        }
    }

    fn collect(&mut self) {
        self.snapshot = tool_detection::DevOpsSnapshot::collect();
    }

    /// Force an immediate refresh, bypassing the interval timer.
    pub fn refresh(&mut self) {
        self.collect();
        self.last_collect = Some(Instant::now());
    }
}

impl Section for DevOpsSection {
    fn id(&self) -> SectionId {
        SectionId::DevOps
    }

    fn update(&mut self, ctx: &egui::Context) {
        if self.refresh_pending {
            self.refresh_pending = false;
            self.collect();
            self.last_collect = Some(Instant::now());
            ctx.request_repaint();
            return;
        }

        let now = Instant::now();
        let due = self
            .last_collect
            .map_or(true, |last| now.duration_since(last) >= COLLECT_INTERVAL);
        if due {
            self.collect();
            self.last_collect = Some(now);
            ctx.request_repaint();
        }
    }

    fn render(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) -> egui::Response {
        let snapshot = self.snapshot.clone();
        let theme = context.theme;
        let mut refresh_clicked = false;

        let response = egui::ScrollArea::vertical()
            .id_salt("devops-dashboard")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Refresh button row
                ui.horizontal(|ui| {
                    let btn = ui.add(
                        egui::Button::new(
                            RichText::new("Refresh")
                                .font(FontId::proportional(11.0))
                                .color(theme.ui.text),
                        )
                        .fill(with_alpha(theme.ui.accent, 0.15))
                        .stroke(egui::Stroke::new(1.0_f32, with_alpha(theme.ui.accent, 0.4)))
                        .corner_radius(4.0),
                    );
                    if btn.clicked() {
                        refresh_clicked = true;
                    }

                    // Show last updated info inline
                    if let Some(time) = snapshot.last_updated {
                        let elapsed = time.elapsed().map(|d| d.as_secs()).unwrap_or(0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("Updated {elapsed}s ago"))
                                    .font(FontId::monospace(10.0))
                                    .color(theme.ui.secondary_text),
                            );
                        });
                    }
                });

                ui.add_space(4.0);

                dashboard::show(ui, context, &snapshot);
                ui.response()
            })
            .inner;

        if refresh_clicked {
            self.refresh_pending = true;
        }

        response
    }

    fn status_label(&self, theme: &Theme) -> Option<(String, egui::Color32)> {
        let available = self.snapshot.available_count();
        let total = self.snapshot.tools.len();
        if total == 0 {
            return Some(("scanning…".into(), theme.ui.secondary_text));
        }
        Some((
            format!("{available}/{total} detected"),
            if available == total {
                theme.status.success
            } else if available > 0 {
                theme.status.warning
            } else {
                theme.status.error
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_id_is_devops() {
        let section = DevOpsSection::new();
        assert_eq!(section.id(), SectionId::DevOps);
    }

    #[test]
    fn starts_with_empty_snapshot() {
        let section = DevOpsSection::new();
        assert!(section.snapshot.tools.is_empty());
        assert!(section.snapshot.last_updated.is_none());
    }

    #[test]
    fn collect_populates_snapshot() {
        let mut section = DevOpsSection::new();
        section.collect();
        // 9 tools: git, rustc, cargo, node, npm, python3, pip, docker, podman
        assert_eq!(section.snapshot.tools.len(), 9);
        assert!(section.snapshot.last_updated.is_some());
    }

    #[test]
    fn refresh_updates_last_collect() {
        let mut section = DevOpsSection::new();
        assert!(section.last_collect.is_none());
        section.refresh();
        assert!(section.last_collect.is_some());
    }

    #[test]
    fn status_label_shows_counts() {
        let mut section = DevOpsSection::new();
        section.collect();
        let theme = crate::theme::get_theme("orbit-dark");
        let label = section.status_label(&theme);
        assert!(label.is_some());
        let (text, _color) = label.unwrap();
        assert!(text.contains("/"));
    }

    #[test]
    fn status_label_before_collect_shows_scanning() {
        let section = DevOpsSection::new();
        let theme = crate::theme::get_theme("orbit-dark");
        let label = section.status_label(&theme);
        assert!(label.is_some());
        let (text, _color) = label.unwrap();
        assert!(text.contains("scanning"));
    }

    #[test]
    fn collect_is_idempotent() {
        let mut section = DevOpsSection::new();
        section.collect();
        let count1 = section.snapshot.tools.len();
        section.collect();
        let count2 = section.snapshot.tools.len();
        assert_eq!(count1, count2);
    }

    #[test]
    fn collect_populates_project_env() {
        let mut section = DevOpsSection::new();
        section.collect();
        assert!(section.snapshot.project_env.working_dir.is_some());
        assert!(section.snapshot.project_env.architecture.is_some());
    }

    #[test]
    fn collect_populates_git_repo() {
        let mut section = DevOpsSection::new();
        section.collect();
        // May or may not be in a git repo
        let _ = section.snapshot.git_repo.in_repo;
    }
}
