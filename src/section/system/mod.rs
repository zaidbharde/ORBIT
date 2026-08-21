//! System section: static system information plus live CPU, RAM and GPU
//! monitoring at ~1 Hz. CPU/RAM come from Linux `/proc` (no external
//! dependencies); GPU telemetry comes from the read-only NVIDIA backend in
//! [`gpu`]. Metrics are collected in `update()` — the dashboard render
//! path only reads cached values.

pub mod dashboard;
pub mod gpu;
pub mod metrics;
pub mod storage;

use super::{Section, SectionContext, SectionId};
use crate::theme::Theme;
use eframe::egui;
use gpu::GpuMonitor;
use metrics::{
    CpuTicks, SystemInfo, SystemMetrics, collect_system_info, cpu_usage_delta, read_cpu_ticks,
    read_memory_kib, read_uptime_secs,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use storage::DiskIoMonitor;

/// How often dynamic metrics are re-read (1 Hz, well below the 60 fps frame
/// rate, so the update loop stays lightweight).
const COLLECT_INTERVAL: Duration = Duration::from_secs(1);

/// How many samples the rolling history graphs keep (one per second).
pub const HISTORY_LEN: usize = 60;

/// The live System dashboard.
pub struct SystemSection {
    info: SystemInfo,
    metrics: SystemMetrics,
    gpu: GpuMonitor,
    disk_io: DiskIoMonitor,
    prev_cpu: Option<CpuTicks>,
    cpu_history: VecDeque<f32>,
    ram_history: VecDeque<f32>,
    gpu_history: VecDeque<f32>,
    vram_history: VecDeque<f32>,
    read_history: VecDeque<f32>,
    write_history: VecDeque<f32>,
    last_collect: Option<Instant>,
}

impl SystemSection {
    pub fn new() -> Self {
        Self {
            info: collect_system_info(),
            metrics: SystemMetrics::default(),
            gpu: GpuMonitor::new(),
            disk_io: DiskIoMonitor::new(),
            prev_cpu: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            ram_history: VecDeque::with_capacity(HISTORY_LEN),
            gpu_history: VecDeque::with_capacity(HISTORY_LEN),
            vram_history: VecDeque::with_capacity(HISTORY_LEN),
            read_history: VecDeque::with_capacity(HISTORY_LEN),
            write_history: VecDeque::with_capacity(HISTORY_LEN),
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

        self.gpu.poll();
        if let Some(primary) = self.gpu.primary() {
            if let Some(utilization) = primary.utilization {
                push_history(&mut self.gpu_history, utilization);
            }
            if let Some(fraction) = primary.memory_fraction() {
                push_history(&mut self.vram_history, fraction * 100.0);
            }
        }

        self.metrics.storage_mounts = storage::collect_storage_mounts();

        self.disk_io.poll();
        if let Some(io_metrics) = self.disk_io.metrics() {
            self.metrics.disk_io = Some(io_metrics.clone());
            if let Some(read) = io_metrics.read_bytes_per_sec {
                push_history(&mut self.read_history, read);
            }
            if let Some(write) = io_metrics.write_bytes_per_sec {
                push_history(&mut self.write_history, write);
            }
        } else {
            self.metrics.disk_io = None;
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
        let gpu_history = self.gpu_history.make_contiguous();
        let vram_history = self.vram_history.make_contiguous();
        let read_history = self.read_history.make_contiguous();
        let write_history = self.write_history.make_contiguous();
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
                    &self.gpu,
                    gpu_history,
                    vram_history,
                    read_history,
                    write_history,
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
        assert!(section.gpu_history.len() <= HISTORY_LEN);
        assert!(section.vram_history.len() <= HISTORY_LEN);
        assert!(section.read_history.len() <= HISTORY_LEN);
        assert!(section.write_history.len() <= HISTORY_LEN);
    }
}
