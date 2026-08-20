//! System section: static system information plus live CPU and RAM
//! monitoring at ~1 Hz, all sourced from Linux `/proc` (no external
//! dependencies). Metrics are collected in `update()` — the dashboard
//! render path only reads cached values.

pub mod dashboard;
pub mod metrics;

use super::{Section, SectionContext, SectionId};
use crate::theme::Theme;
use eframe::egui;
use metrics::{
    CpuTicks, SystemInfo, SystemMetrics, collect_system_info, cpu_usage_delta, read_cpu_ticks,
    read_memory_kib, read_uptime_secs,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often dynamic metrics are re-read (1 Hz, well below the 60 fps frame
/// rate, so the update loop stays lightweight).
const COLLECT_INTERVAL: Duration = Duration::from_secs(1);

/// How many samples the rolling history graphs keep (one per second).
pub const HISTORY_LEN: usize = 60;

/// The live System dashboard.
pub struct SystemSection {
    info: SystemInfo,
    metrics: SystemMetrics,
    prev_cpu: Option<CpuTicks>,
    cpu_history: VecDeque<f32>,
    ram_history: VecDeque<f32>,
    last_collect: Option<Instant>,
}

impl SystemSection {
    pub fn new() -> Self {
        Self {
            info: collect_system_info(),
            metrics: SystemMetrics::default(),
            prev_cpu: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            ram_history: VecDeque::with_capacity(HISTORY_LEN),
            last_collect: None,
        }
    }

    fn collect(&mut self) {
        if let Some(ticks) = read_cpu_ticks() {
            if let Some(prev) = &self.prev_cpu {
                if let Some((overall, per_core)) = cpu_usage_delta(prev, &ticks) {
                    self.metrics.cpu_usage = Some(overall);
                    self.metrics.per_core_usage = per_core.into_iter().map(Some).collect();
                    push_history(&mut self.cpu_history, overall);
                }
            }
            self.prev_cpu = Some(ticks);
        }

        if let Some((total_kib, available_kib)) = read_memory_kib() {
            let total = total_kib * 1024;
            let available = available_kib * 1024;
            let used = total.saturating_sub(available);
            let usage = if total > 0 {
                used as f32 / total as f32 * 100.0
            } else {
                0.0
            };
            self.metrics.memory_total = Some(total);
            self.metrics.memory_used = Some(used);
            self.metrics.memory_available = Some(available);
            self.metrics.memory_usage = Some(usage);
            push_history(&mut self.ram_history, usage);
        }

        self.metrics.uptime_secs = read_uptime_secs();
    }
}

fn push_history(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

impl Section for SystemSection {
    fn id(&self) -> SectionId {
        SectionId::System
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
        let cpu_history = self.cpu_history.make_contiguous();
        let ram_history = self.ram_history.make_contiguous();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                dashboard::show(
                    ui,
                    context,
                    &self.info,
                    &self.metrics,
                    cpu_history,
                    ram_history,
                );
                ui.response()
            })
            .inner
    }

    fn status_label(&self, theme: &Theme) -> Option<(String, egui::Color32)> {
        self.metrics
            .cpu_usage
            .map(|usage| (format!("cpu {usage:.0}%"), theme.ui.accent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_capped_at_history_len() {
        let mut history = VecDeque::new();
        for index in 0..(HISTORY_LEN + 10) {
            push_history(&mut history, index as f32);
        }
        assert_eq!(history.len(), HISTORY_LEN);
        assert_eq!(*history.front().unwrap(), 10.0);
        assert_eq!(*history.back().unwrap(), (HISTORY_LEN + 9) as f32);
    }

    #[test]
    fn collecting_twice_produces_deltas_without_crash() {
        let mut section = SystemSection::new();
        section.collect();
        section.collect();
        assert!(section.cpu_history.len() <= HISTORY_LEN);
        assert!(section.ram_history.len() <= HISTORY_LEN);
    }
}
