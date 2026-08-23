//! Networking section: dedicated network interface monitoring.
//!
//! Reuses the [`NetworkMonitor`] from the System section's network module
//! for data collection, but owns its own instance so the Networking section
//! is fully independent. Collects at ~1 Hz; rendering reads cached values.

pub mod dashboard;

use super::{Section, SectionContext, SectionId};
use crate::section::system::network::NetworkMonitor;
use crate::theme::Theme;
use eframe::egui;
use std::time::{Duration, Instant};

/// How often dynamic metrics are re-read (1 Hz).
const COLLECT_INTERVAL: Duration = Duration::from_secs(1);

/// The live Networking dashboard section.
pub struct NetworkingSection {
    monitor: NetworkMonitor,
    last_collect: Option<Instant>,
}

impl NetworkingSection {
    pub fn new() -> Self {
        Self {
            monitor: NetworkMonitor::new(),
            last_collect: None,
        }
    }
}

impl Section for NetworkingSection {
    fn id(&self) -> SectionId {
        SectionId::Networking
    }

    fn update(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let due = self
            .last_collect
            .map_or(true, |last| now.duration_since(last) >= COLLECT_INTERVAL);
        if due {
            self.monitor.poll();
            self.last_collect = Some(now);
            ctx.request_repaint();
        }
    }

    fn render(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) -> egui::Response {
        let interfaces = self.monitor.interfaces().to_vec();
        let agg_rx: f32 = interfaces.iter().filter_map(|i| i.rx_bytes_per_sec).sum();
        let agg_tx: f32 = interfaces.iter().filter_map(|i| i.tx_bytes_per_sec).sum();
        let total_rx = self.monitor.total_rx;
        let total_tx = self.monitor.total_tx;
        let rx_history = self.monitor.rx_history.clone();
        let tx_history = self.monitor.tx_history.clone();

        egui::ScrollArea::vertical()
            .id_salt("networking-section")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                dashboard::show(
                    ui,
                    context,
                    &interfaces,
                    agg_rx,
                    agg_tx,
                    total_rx,
                    total_tx,
                    &rx_history,
                    &tx_history,
                );
                ui.response()
            })
            .inner
    }

    fn status_label(&self, theme: &Theme) -> Option<(String, egui::Color32)> {
        let up_count = self
            .monitor
            .interfaces()
            .iter()
            .filter(|i| i.state == crate::section::system::network::InterfaceState::Up)
            .count();
        let total = self.monitor.interfaces().len();
        if total == 0 {
            return None;
        }
        Some((
            format!("{up_count}/{total} up"),
            if up_count > 0 {
                theme.status.success
            } else {
                theme.status.warning
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_id_is_networking() {
        let section = NetworkingSection::new();
        assert_eq!(section.id(), SectionId::Networking);
    }

    #[test]
    fn starts_with_no_data() {
        let section = NetworkingSection::new();
        assert!(section.monitor.interfaces().is_empty());
        assert_eq!(section.monitor.total_rx, 0);
        assert_eq!(section.monitor.total_tx, 0);
    }
}
