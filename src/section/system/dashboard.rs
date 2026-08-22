//! Themed dashboard rendering for the System section.
//!
//! Pure presentation: reads the cached [`SystemInfo`] / [`SystemMetrics`]
//! values and paints themed cards, usage bars, per-core rows, rolling
//! graphs and the system information grid. Metrics are collected by the
//! section's update loop, never here.

use super::HISTORY_LEN;
use super::gpu::{GpuBackend, GpuInfo, GpuMonitor};
use super::metrics::{SystemInfo, SystemMetrics};
use super::storage::{DiskIoMetrics, StorageMount, THROUGHPUT_FLOOR_BPS};
use super::thermal::{ThermalMonitor, ThermalZone};
use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::epaint::{PathShape, PathStroke};
use eframe::egui::{
    Align2, Color32, FontId, Frame, Grid, Margin, Pos2, RichText, Shape, Stroke, Ui,
};

pub fn show(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    info: &SystemInfo,
    metrics: &SystemMetrics,
    cpu_history: &[f32],
    ram_history: &[f32],
    gpu: &GpuMonitor,
    gpu_history: &[f32],
    vram_history: &[f32],
    read_history: &[f32],
    write_history: &[f32],
    thermal: &ThermalMonitor,
    thermal_history: &[f32],
    process_monitor: &mut super::process::ProcessMonitor,
    network_monitor: &mut super::network::NetworkMonitor,
) {
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
    ui.columns(2, |columns| {
        cpu_card(&mut columns[0], context, metrics, cpu_history);
        memory_card(&mut columns[1], context, metrics, ram_history);
    });
    ui.columns(2, |columns| {
        gpu_card(&mut columns[0], context, gpu, gpu_history, vram_history);
        thermal_card(&mut columns[1], context, thermal, thermal_history);
    });
    ui.columns(2, |columns| {
        storage_card(&mut columns[0], context, &metrics.storage_mounts);
        disk_activity_card(
            &mut columns[1],
            context,
            metrics.disk_io.as_ref(),
            read_history,
            write_history,
        );
    });
    info_card(ui, context, info, metrics);
    super::network::show_network_card(ui, context, network_monitor);
    super::process::show_process_card(ui, context, process_monitor);
}

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
        ui.spacing_mut().item_spacing.y = 8.0;
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

fn cpu_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    metrics: &SystemMetrics,
    cpu_history: &[f32],
) {
    let theme = context.theme;
    card(ui, context, "CPU", |ui| {
        match metrics.cpu_usage {
            Some(usage) => {
                let color = usage_color(theme, usage / 100.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{usage:.0}%"))
                            .font(FontId::monospace(22.0))
                            .color(theme.ui.text),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("overall").color(theme.ui.secondary_text));
                });
                usage_bar(ui, context, usage / 100.0, color);
                graph(ui, context, cpu_history, color);
            }
            None => {
                ui.label(RichText::new("collecting…").color(theme.ui.secondary_text));
            }
        }
        if !metrics.per_core_usage.is_empty() {
            ui.add_space(2.0);
            ui.columns(2, |columns| {
                for (index, usage) in metrics.per_core_usage.iter().enumerate() {
                    core_row(&mut columns[index % 2], context, index, *usage);
                }
            });
        }
    });
}

fn core_row(ui: &mut Ui, context: &SectionContext<'_>, index: usize, usage: Option<f32>) {
    let theme = context.theme;
    ui.horizontal(|ui| {
        let label = match usage {
            Some(value) => format!("Core {index:>2} {value:>3.0}%"),
            None => format!("Core {index:>2}  --"),
        };
        ui.add_sized(
            egui::vec2(112.0, 14.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        let bar_width = (ui.available_width() - 8.0).max(20.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_width, 8.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, with_alpha(theme.ui.text, 0.08));
        let fraction = usage.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
        if fraction > 0.0 {
            let fill = egui::Rect::from_min_size(
                rect.min,
                egui::vec2((rect.width() * fraction).max(2.0), rect.height()),
            );
            painter.rect_filled(fill, 3.0, usage_color(theme, fraction));
        }
    });
}

fn memory_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    metrics: &SystemMetrics,
    ram_history: &[f32],
) {
    let theme = context.theme;
    card(ui, context, "Memory", |ui| {
        match (
            metrics.memory_used,
            metrics.memory_total,
            metrics.memory_usage,
        ) {
            (Some(used), Some(total), Some(usage)) => {
                let fraction = (usage / 100.0).clamp(0.0, 1.0);
                let color = usage_color(theme, fraction);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{usage:.1}%"))
                            .font(FontId::monospace(22.0))
                            .color(theme.ui.text),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("of {}", format_bytes(total)))
                            .color(theme.ui.secondary_text),
                    );
                });
                usage_bar(ui, context, fraction, color);
                ui.add_space(2.0);
                for (label, value) in [
                    ("used", Some(used)),
                    ("available", metrics.memory_available),
                ] {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            egui::vec2(80.0, 14.0),
                            egui::Label::new(
                                RichText::new(label)
                                    .font(FontId::proportional(11.0))
                                    .color(theme.ui.secondary_text),
                            ),
                        );
                        let text = match value {
                            Some(value) => {
                                format!("{} of {}", format_bytes(value), format_bytes(total))
                            }
                            None => "Unavailable".to_owned(),
                        };
                        ui.label(
                            RichText::new(text)
                                .font(FontId::monospace(11.0))
                                .color(theme.ui.text),
                        );
                    });
                }
                graph(ui, context, ram_history, color);
            }
            _ => {
                ui.label(RichText::new("Unavailable").color(theme.ui.secondary_text));
            }
        }
    });
}

fn gpu_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    monitor: &GpuMonitor,
    gpu_history: &[f32],
    vram_history: &[f32],
) {
    let theme = context.theme;
    let title = if monitor.backend() == GpuBackend::Nvidia {
        format!("GPU · {}", monitor.backend().label())
    } else {
        "GPU".to_owned()
    };
    card(ui, context, &title, |ui| {
        let gpus = monitor.gpus();
        if gpus.is_empty() {
            ui.label(RichText::new("Telemetry unavailable").color(theme.ui.secondary_text));
            return;
        }
        for (index, gpu) in gpus.iter().enumerate() {
            if index == 0 {
                primary_gpu(ui, context, gpu, gpu_history, vram_history);
            } else {
                ui.add_space(2.0);
                ui.separator();
                compact_gpu_row(ui, context, index, gpu);
            }
        }
    });
}

fn primary_gpu(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    gpu: &GpuInfo,
    gpu_history: &[f32],
    vram_history: &[f32],
) {
    let theme = context.theme;
    if let Some(name) = &gpu.name {
        ui.label(
            RichText::new(name)
                .font(FontId::proportional(13.0))
                .color(theme.ui.text)
                .strong(),
        );
    }
    let detail = match (&gpu.vendor, &gpu.driver_version) {
        (Some(vendor), Some(driver)) => format!("{vendor} · Driver {driver}"),
        (Some(vendor), None) => vendor.clone(),
        (None, Some(driver)) => format!("Driver {driver}"),
        (None, None) => String::new(),
    };
    if !detail.is_empty() {
        ui.label(
            RichText::new(detail)
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
        );
    }
    ui.add_space(2.0);

    match gpu.utilization {
        Some(utilization) => {
            let color = usage_color(theme, utilization / 100.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{utilization:.0}%"))
                        .font(FontId::monospace(22.0))
                        .color(theme.ui.text),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("gpu usage").color(theme.ui.secondary_text));
            });
            usage_bar(ui, context, utilization / 100.0, color);
            graph(ui, context, gpu_history, color);
        }
        None => {
            stat_line(ui, context, "GPU Usage", None);
        }
    }

    match (gpu.memory_used, gpu.memory_total, gpu.memory_fraction()) {
        (Some(used), Some(total), Some(fraction)) => {
            let color = usage_color(theme, fraction);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} / {}", format_bytes(used), format_bytes(total)))
                        .font(FontId::monospace(13.0))
                        .color(theme.ui.text),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{:.0}%", fraction * 100.0))
                        .color(theme.ui.secondary_text),
                );
            });
            usage_bar(ui, context, fraction, color);
            graph(ui, context, vram_history, color);
            match gpu.memory_free {
                Some(free) => {
                    ui.label(
                        RichText::new(format!("free {}", format_bytes(free)))
                            .font(FontId::proportional(11.0))
                            .color(theme.ui.secondary_text),
                    );
                }
                None => {
                    stat_line(ui, context, "VRAM free", None);
                }
            }
        }
        _ => {
            stat_line(ui, context, "VRAM", None);
        }
    }

    stat_line_colored(
        ui,
        context,
        "Temperature",
        gpu.temperature.map(|value| format!("{value:.0}°C")),
        gpu.temperature.map_or(theme.ui.secondary_text, |value| {
            temperature_color(theme, value)
        }),
    );
    let power = match (gpu.power_draw, gpu.power_limit) {
        (Some(draw), Some(limit)) => Some(format!("{draw:.0} W / {limit:.0} W")),
        (Some(draw), None) => Some(format!("{draw:.0} W")),
        (None, Some(limit)) => Some(format!("-- / {limit:.0} W")),
        (None, None) => None,
    };
    stat_line(ui, context, "Power", power);
    stat_line(
        ui,
        context,
        "Clock",
        gpu.graphics_clock.map(|value| format!("{value} MHz")),
    );
    stat_line(
        ui,
        context,
        "Mem Clock",
        gpu.memory_clock.map(|value| format!("{value} MHz")),
    );
    stat_line(
        ui,
        context,
        "Fan",
        gpu.fan_speed.map(|value| format!("{value:.0}%")),
    );
}

fn compact_gpu_row(ui: &mut Ui, context: &SectionContext<'_>, index: usize, gpu: &GpuInfo) {
    let theme = context.theme;
    let name = gpu.name.as_deref().unwrap_or("Unknown GPU");
    ui.label(
        RichText::new(format!("GPU {index} · {name}"))
            .font(FontId::proportional(11.0))
            .color(theme.ui.secondary_text),
    );
    match gpu.utilization {
        Some(utilization) => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{utilization:.0}%"))
                        .font(FontId::monospace(11.0))
                        .color(theme.ui.text),
                );
                if let Some(temperature) = gpu.temperature {
                    ui.label(
                        RichText::new(format!("{temperature:.0}°C"))
                            .font(FontId::monospace(11.0))
                            .color(theme.ui.secondary_text),
                    );
                }
            });
            usage_bar(
                ui,
                context,
                utilization / 100.0,
                usage_color(theme, utilization / 100.0),
            );
        }
        None => {
            ui.label(
                RichText::new("Unavailable")
                    .font(FontId::proportional(11.0))
                    .color(theme.status.warning),
            );
        }
    }
}

fn thermal_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    monitor: &ThermalMonitor,
    thermal_history: &[f32],
) {
    let theme = context.theme;
    card(ui, context, "Temperature", |ui| {
        let zones = monitor.zones();
        if zones.is_empty() {
            ui.label(RichText::new("Telemetry unavailable").color(theme.ui.secondary_text));
            return;
        }
        if let Some(max_temp) = monitor.max_temp_celsius() {
            let color = temperature_color(theme, max_temp);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{max_temp:.0}°C"))
                        .font(FontId::monospace(22.0))
                        .color(theme.ui.text),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("peak").color(theme.ui.secondary_text));
            });
            usage_bar(ui, context, (max_temp / 100.0).clamp(0.0, 1.0), color);
            graph(ui, context, thermal_history, color);
        }
        for zone in zones {
            ui.add_space(2.0);
            thermal_zone_row(ui, context, zone);
        }
    });
}

fn thermal_zone_row(ui: &mut Ui, context: &SectionContext<'_>, zone: &ThermalZone) {
    let theme = context.theme;
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(120.0, 16.0),
            egui::Label::new(
                RichText::new(&zone.type_name)
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        match zone.temp_celsius() {
            Some(temp) => {
                let color = temperature_color(theme, temp);
                ui.label(
                    RichText::new(format!("{temp:.0}°C"))
                        .font(FontId::monospace(11.0))
                        .color(color),
                );
            }
            None => {
                ui.label(
                    RichText::new("Unavailable")
                        .font(FontId::proportional(11.0))
                        .color(theme.status.warning),
                );
            }
        }
        if let Some(crit) = zone.critical_temp_celsius() {
            ui.label(
                RichText::new(format!("crit {crit:.0}°C"))
                    .font(FontId::monospace(9.0))
                    .color(theme.ui.secondary_text),
            );
        }
    });
}

fn stat_line(ui: &mut Ui, context: &SectionContext<'_>, label: &str, value: Option<String>) {
    stat_line_colored(ui, context, label, value, context.theme.ui.text);
}

fn stat_line_colored(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    label: &str,
    value: Option<String>,
    value_color: Color32,
) {
    let theme = context.theme;
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(96.0, 16.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        match value {
            Some(value) => {
                ui.label(
                    RichText::new(value)
                        .font(FontId::monospace(11.0))
                        .color(value_color),
                );
            }
            None => {
                ui.label(
                    RichText::new("Unavailable")
                        .font(FontId::proportional(11.0))
                        .color(theme.status.warning),
                );
            }
        }
    });
}

fn storage_card(ui: &mut Ui, context: &SectionContext<'_>, mounts: &[StorageMount]) {
    let theme = context.theme;
    card(ui, context, "Storage", |ui| {
        if mounts.is_empty() {
            ui.label(RichText::new("Telemetry unavailable").color(theme.ui.secondary_text));
            return;
        }
        // Primary mount is `/` if present, otherwise the first entry.
        let primary_idx = mounts
            .iter()
            .position(|m| m.mount_point == "/")
            .unwrap_or(0);
        primary_storage(ui, context, &mounts[primary_idx]);
        for (i, mount) in mounts.iter().enumerate() {
            if i == primary_idx {
                continue;
            }
            ui.add_space(4.0);
            ui.separator();
            compact_storage_row(ui, context, mount);
        }
    });
}

fn primary_storage(ui: &mut Ui, context: &SectionContext<'_>, mount: &StorageMount) {
    let theme = context.theme;
    ui.label(
        RichText::new(&mount.mount_point)
            .font(FontId::proportional(13.0))
            .color(theme.ui.text)
            .strong(),
    );
    match mount.usage_fraction() {
        Some(fraction) => {
            let color = storage_usage_color(theme, fraction);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{:.0}%", fraction * 100.0))
                        .font(FontId::monospace(22.0))
                        .color(theme.ui.text),
                );
                if let Some(total) = mount.total_bytes {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("of {}", format_bytes(total)))
                            .color(theme.ui.secondary_text),
                    );
                }
            });
            usage_bar(ui, context, fraction, color);
            ui.add_space(2.0);
            for (label, value) in [
                ("used", mount.used_bytes),
                ("available", mount.available_bytes),
            ] {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        egui::vec2(80.0, 14.0),
                        egui::Label::new(
                            RichText::new(label)
                                .font(FontId::proportional(11.0))
                                .color(theme.ui.secondary_text),
                        ),
                    );
                    match (value, mount.total_bytes) {
                        (Some(v), Some(total)) => {
                            ui.label(
                                RichText::new(format!(
                                    "{} of {}",
                                    format_bytes(v),
                                    format_bytes(total)
                                ))
                                .font(FontId::monospace(11.0))
                                .color(theme.ui.text),
                            );
                        }
                        _ => {
                            ui.label(
                                RichText::new("Unavailable")
                                    .font(FontId::proportional(11.0))
                                    .color(theme.status.warning),
                            );
                        }
                    }
                });
            }
        }
        None => {
            ui.label(
                RichText::new("Unavailable")
                    .font(FontId::proportional(11.0))
                    .color(theme.status.warning),
            );
        }
    }
    if let Some(device) = &mount.device {
        ui.label(
            RichText::new(format!("Device: {device}"))
                .font(FontId::proportional(11.0))
                .color(theme.ui.secondary_text),
        );
    }
    ui.label(
        RichText::new(format!("Filesystem: {}", mount.filesystem))
            .font(FontId::proportional(11.0))
            .color(theme.ui.secondary_text),
    );
}

fn compact_storage_row(ui: &mut Ui, context: &SectionContext<'_>, mount: &StorageMount) {
    let theme = context.theme;
    let fraction = mount.usage_fraction();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&mount.mount_point)
                .font(FontId::monospace(11.0))
                .color(theme.ui.text),
        );
        match fraction {
            Some(f) => {
                let color = storage_usage_color(theme, f);
                ui.label(
                    RichText::new(format!("{:.0}%", f * 100.0))
                        .font(FontId::monospace(11.0))
                        .color(color),
                );
                let used = mount.used_bytes.map(format_bytes).unwrap_or("--".into());
                let total = mount.total_bytes.map(format_bytes).unwrap_or("--".into());
                ui.label(
                    RichText::new(format!("{used} / {total}"))
                        .font(FontId::monospace(11.0))
                        .color(theme.ui.secondary_text),
                );
            }
            None => {
                ui.label(
                    RichText::new("Unavailable")
                        .font(FontId::proportional(11.0))
                        .color(theme.status.warning),
                );
            }
        }
    });
}

/// Visual band for storage usage: < 80% normal (accent), 80–89% warning,
/// >= 90% high (error). These are UI indicators only, not hardware safety
/// thresholds.
fn storage_usage_color(theme: &Theme, fraction: f32) -> Color32 {
    if fraction >= 0.9 {
        theme.status.error
    } else if fraction >= 0.8 {
        theme.status.warning
    } else {
        theme.ui.accent
    }
}

fn disk_activity_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    disk_io: Option<&DiskIoMetrics>,
    read_history: &[f32],
    write_history: &[f32],
) {
    let theme = context.theme;
    card(ui, context, "Disk Activity", |ui| {
        let (read_rate, write_rate) = match disk_io {
            Some(io) => (io.read_bytes_per_sec, io.write_bytes_per_sec),
            None => (None, None),
        };
        let read_text = match read_rate {
            Some(r) => format_throughput(r),
            None => "--".to_owned(),
        };
        let write_text = match write_rate {
            Some(w) => format_throughput(w),
            None => "--".to_owned(),
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Read")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
            ui.label(
                RichText::new(&read_text)
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
        });
        let read_color = theme.ui.accent;
        throughput_graph(ui, context, read_history, read_color);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Write")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
            ui.label(
                RichText::new(&write_text)
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
        });
        let write_color = theme.status.warning;
        throughput_graph(ui, context, write_history, write_color);
    });
}

/// A small rolling graph that auto-scales to the maximum value in the
/// history window. Unlike [`graph`] (which assumes a 0–100% range),
/// this adapts to any throughput scale.
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
        // Peak label top-left.
        painter.text(
            Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            format_throughput(peak),
            FontId::proportional(9.0),
            theme.ui.secondary_text,
        );
    } else if n == 1 {
        let peak = history[0].max(THROUGHPUT_FLOOR_BPS);
        let x = rect.right();
        let fraction = (history[0] / peak).clamp(0.0, 1.0);
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

fn info_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    info: &SystemInfo,
    metrics: &SystemMetrics,
) {
    card(ui, context, "System", |ui| {
        Grid::new("orbit-system-info")
            .num_columns(2)
            .spacing(egui::vec2(20.0, 6.0))
            .min_col_width(110.0)
            .show(ui, |ui| {
                row(ui, context, "Hostname", info.hostname.as_deref());
                row(ui, context, "OS", info.os_name.as_deref());
                row(ui, context, "Kernel", info.kernel.as_deref());
                row(ui, context, "Architecture", Some(info.architecture));
                match (info.logical_cores, info.physical_cores) {
                    (None, None) => row(ui, context, "Cores", None),
                    _ => {
                        let summary = core_summary(info.logical_cores, info.physical_cores);
                        row(ui, context, "Cores", Some(&summary));
                    }
                }
                row(ui, context, "CPU model", info.cpu_model.as_deref());
                row(
                    ui,
                    context,
                    "Memory",
                    info.total_ram.map(format_bytes).as_deref(),
                );
                row(
                    ui,
                    context,
                    "Uptime",
                    metrics.uptime_secs.map(format_uptime).as_deref(),
                );
                row(ui, context, "ORBIT", Some(info.orbit_version));
            });
    });
}

fn row(ui: &mut Ui, context: &SectionContext<'_>, label: &str, value: Option<&str>) {
    let theme = context.theme;
    ui.label(
        RichText::new(label)
            .font(FontId::proportional(11.0))
            .color(theme.ui.secondary_text),
    );
    match value {
        Some(value) => {
            ui.label(
                RichText::new(value)
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
        }
        None => {
            ui.label(
                RichText::new("Unavailable")
                    .font(FontId::proportional(11.0))
                    .color(theme.status.warning),
            );
        }
    }
    ui.end_row();
}

fn usage_bar(ui: &mut Ui, context: &SectionContext<'_>, fraction: f32, color: Color32) {
    let theme = context.theme;
    let width = ui.available_width();
    if width < 8.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 10.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, with_alpha(theme.ui.text, 0.08));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.0 {
        let fill = egui::Rect::from_min_size(
            rect.min,
            egui::vec2((rect.width() * fraction).max(2.0), rect.height()),
        );
        painter.rect_filled(fill, 4.0, color);
    }
}

fn graph(ui: &mut Ui, context: &SectionContext<'_>, history: &[f32], color: Color32) {
    let theme = context.theme;
    let height = 90.0;
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

    let step = rect.width() / HISTORY_LEN as f32;
    let n = history.len();
    if n >= 1 {
        let points: Vec<Pos2> = history
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let fraction = (value.clamp(0.0, 100.0) / 100.0).clamp(0.0, 1.0);
                let x = rect.right() - (n - 1 - index) as f32 * step;
                let y = rect.bottom() - fraction * rect.height();
                Pos2::new(x, y)
            })
            .collect();
        if n >= 2 {
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
        } else {
            painter.circle_filled(points[0], 2.5, color);
        }
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
        Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        "100%",
        FontId::proportional(9.0),
        theme.ui.secondary_text,
    );
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 2.0),
        Align2::LEFT_BOTTOM,
        "0%",
        FontId::proportional(9.0),
        theme.ui.secondary_text,
    );
    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        Align2::RIGHT_BOTTOM,
        "60s",
        FontId::proportional(9.0),
        theme.ui.secondary_text,
    );
}

/// Bar/graph color: theme accent normally, theme error at >= 90%.
pub fn usage_color(theme: &Theme, fraction: f32) -> Color32 {
    if fraction >= 0.9 {
        theme.status.error
    } else {
        theme.ui.accent
    }
}

/// Informational color band for GPU temperature in °C — a visual cue based
/// on typical NVIDIA operating ranges, not a thermal safety claim:
/// below 70 normal (accent), 70-84 elevated (warning), 85+ high (error).
pub fn temperature_color(theme: &Theme, celsius: f32) -> Color32 {
    if celsius >= 85.0 {
        theme.status.error
    } else if celsius >= 70.0 {
        theme.status.warning
    } else {
        theme.ui.accent
    }
}

/// Human-readable byte size ("1536 B", "1.5 KB", "15.26 GB", ...).
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

/// Human-readable uptime ("59s", "2m 13s", "1h 2m", "1d 3h 4m", ...).
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// "8 cores", "4 cores, 16 threads" or "8 cores" depending on the reported
/// physical/logical counts; "Unavailable" when neither is known.
pub fn core_summary(logical: Option<usize>, physical: Option<usize>) -> String {
    match (logical, physical) {
        (Some(logical), Some(physical)) if logical > physical => {
            format!("{physical} cores, {logical} threads")
        }
        (Some(_), Some(physical)) => format!("{physical} cores"),
        (Some(logical), None) => format!("{logical} cores"),
        (None, Some(physical)) => format!("{physical} cores"),
        (None, None) => "Unavailable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::get_theme;

    #[test]
    fn formats_bytes_in_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(16_000_000 * 1024), "15.26 GB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 * 1024), "3.00 TB");
    }

    #[test]
    fn formats_uptime_in_increasing_units() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(59), "59s");
        assert_eq!(format_uptime(60), "1m 0s");
        assert_eq!(format_uptime(133), "2m 13s");
        assert_eq!(format_uptime(3600), "1h 0m");
        assert_eq!(format_uptime(3725), "1h 2m");
        assert_eq!(format_uptime(86_400 + 3 * 3_600 + 4 * 60 + 5), "1d 3h 4m");
    }

    #[test]
    fn summarizes_core_counts() {
        assert_eq!(core_summary(Some(8), Some(8)), "8 cores");
        assert_eq!(core_summary(Some(16), Some(8)), "8 cores, 16 threads");
        assert_eq!(core_summary(Some(8), None), "8 cores");
        assert_eq!(core_summary(None, Some(8)), "8 cores");
        assert_eq!(core_summary(None, None), "Unavailable");
    }

    #[test]
    fn usage_color_turns_error_at_ninety_percent() {
        let theme = get_theme("orbit-dark");
        assert_eq!(usage_color(&theme, 0.0), theme.ui.accent);
        assert_eq!(usage_color(&theme, 0.89), theme.ui.accent);
        assert_eq!(usage_color(&theme, 0.9), theme.status.error);
        assert_eq!(usage_color(&theme, 1.0), theme.status.error);
    }

    #[test]
    fn temperature_color_bands_normal_elevated_high() {
        let theme = get_theme("orbit-dark");
        assert_eq!(temperature_color(&theme, 40.0), theme.ui.accent);
        assert_eq!(temperature_color(&theme, 69.0), theme.ui.accent);
        assert_eq!(temperature_color(&theme, 70.0), theme.status.warning);
        assert_eq!(temperature_color(&theme, 84.0), theme.status.warning);
        assert_eq!(temperature_color(&theme, 85.0), theme.status.error);
        assert_eq!(temperature_color(&theme, 95.0), theme.status.error);
    }

    #[test]
    fn storage_usage_color_bands() {
        let theme = get_theme("orbit-dark");
        assert_eq!(storage_usage_color(&theme, 0.0), theme.ui.accent);
        assert_eq!(storage_usage_color(&theme, 0.5), theme.ui.accent);
        assert_eq!(storage_usage_color(&theme, 0.79), theme.ui.accent);
        assert_eq!(storage_usage_color(&theme, 0.80), theme.status.warning);
        assert_eq!(storage_usage_color(&theme, 0.85), theme.status.warning);
        assert_eq!(storage_usage_color(&theme, 0.89), theme.status.warning);
        assert_eq!(storage_usage_color(&theme, 0.90), theme.status.error);
        assert_eq!(storage_usage_color(&theme, 1.0), theme.status.error);
    }

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
        assert_eq!(format_throughput(12_582_912.0), "12.0 MB/s");
    }

    #[test]
    fn format_throughput_gigabytes() {
        assert_eq!(format_throughput(1_073_741_824.0), "1.0 GB/s");
    }
}
