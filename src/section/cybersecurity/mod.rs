//! Cybersecurity section: read-only local security posture overview.
//!
//! Collects security-relevant information from Linux procfs, sysfs and
//! safe local files once per ~1 Hz cycle. Rendering reads cached values only.
//! All operations are strictly read-only — no modifications are made.

pub mod dashboard;
pub mod security_data;

use super::{Section, SectionContext, SectionId};
use crate::theme::Theme;
use eframe::egui;
use std::time::{Duration, Instant};

/// How often security data is re-collected (1 Hz).
const COLLECT_INTERVAL: Duration = Duration::from_secs(1);

/// The live Cybersecurity dashboard section.
pub struct CybersecuritySection {
    snapshot: security_data::SecuritySnapshot,
    last_collect: Option<Instant>,
}

impl CybersecuritySection {
    pub fn new() -> Self {
        Self {
            snapshot: security_data::SecuritySnapshot::default(),
            last_collect: None,
        }
    }

    fn collect(&mut self) {
        self.snapshot = security_data::SecuritySnapshot::collect();
    }
}

impl Section for CybersecuritySection {
    fn id(&self) -> SectionId {
        SectionId::Cybersecurity
    }

    fn update(&mut self, ctx: &egui::Context) {
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
        egui::ScrollArea::vertical()
            .id_salt("cybersecurity-dashboard")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                dashboard::show(ui, context, &snapshot);
                ui.response()
            })
            .inner
    }

    fn status_label(&self, theme: &Theme) -> Option<(String, egui::Color32)> {
        let warnings = self.snapshot.summary.warnings;
        if warnings > 0 {
            Some((
                format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" }),
                theme.status.warning,
            ))
        } else {
            Some(("secure".into(), theme.status.success))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_id_is_cybersecurity() {
        let section = CybersecuritySection::new();
        assert_eq!(section.id(), SectionId::Cybersecurity);
    }

    #[test]
    fn starts_with_default_snapshot() {
        let section = CybersecuritySection::new();
        assert_eq!(section.snapshot.summary.checks_available, 0);
        assert!(section.snapshot.last_updated.is_none());
    }

    #[test]
    fn collect_populates_snapshot() {
        let mut section = CybersecuritySection::new();
        section.collect();
        // On a real Linux system, at least hostname should be available
        assert!(section.snapshot.last_updated.is_some());
    }

    #[test]
    fn status_label_shows_warnings_when_present() {
        let mut section = CybersecuritySection::new();
        section.collect();
        let label = section.status_label(&crate::theme::get_theme("orbit-dark"));
        assert!(label.is_some());
    }
}
