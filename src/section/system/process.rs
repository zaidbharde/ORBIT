//! Process monitoring: discovery, collection and rendering.
//!
//! P5.4 implements a read-only process monitor sourced directly from Linux
//! `/proc`. Each refresh reads `/proc/[pid]/stat`, `/proc/[pid]/status`,
//! `/proc/[pid]/cmdline` and `/proc/[pid]/exe` for every numeric PID
//! directory. CPU usage is calculated from deltas of per-process CPU ticks
//! between consecutive 1 Hz samples. The UI presents a searchable, sortable
//! table with optional per-process detail panels.
//!
//! On non-Linux platforms the process list is always empty and the UI
//! displays "Telemetry unavailable". All queries are strictly read-only: no
//! signals are sent, no process state is modified.

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// A single process snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub memory_percent: Option<f32>,
    pub state: ProcessState,
    pub uid: Option<u32>,
    pub username: Option<String>,
    pub command: Option<String>,
    pub executable: Option<String>,
    cpu_ticks: u64,
    start_ticks: u64,
}

/// Human-readable process state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep,
    Stopped,
    Zombie,
    Unknown,
}

impl ProcessState {
    pub fn from_linux_char(ch: char) -> Self {
        match ch {
            'R' => ProcessState::Running,
            'S' => ProcessState::Sleeping,
            'D' => ProcessState::DiskSleep,
            'T' | 't' => ProcessState::Stopped,
            'Z' => ProcessState::Zombie,
            _ => ProcessState::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProcessState::Running => "Running",
            ProcessState::Sleeping => "Sleeping",
            ProcessState::DiskSleep => "Disk Sleep",
            ProcessState::Stopped => "Stopped",
            ProcessState::Zombie => "Zombie",
            ProcessState::Unknown => "Unknown",
        }
    }
}

/// Sort key for the process table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessSortKey {
    Cpu,
    Memory,
    Pid,
    Name,
}

impl ProcessSortKey {
    pub fn label(self) -> &'static str {
        match self {
            ProcessSortKey::Cpu => "CPU",
            ProcessSortKey::Memory => "Memory",
            ProcessSortKey::Pid => "PID",
            ProcessSortKey::Name => "Name",
        }
    }
}

/// Cached process list with UI interaction state.
pub struct ProcessMonitor {
    processes: Vec<ProcessInfo>,
    prev_ticks: HashMap<(u32, u64), u64>,
    prev_total_ticks: Option<u64>,
    num_cores: f32,
    total_ram: u64,
    last_collect: Option<Instant>,
    pub sort_key: ProcessSortKey,
    pub sort_desc: bool,
    pub search: String,
    pub selected_pid: Option<u32>,
    pub total_count: usize,
    pub running_count: usize,
    pub sleeping_count: usize,
    pub stopped_count: usize,
    pub zombie_count: usize,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
            prev_ticks: HashMap::new(),
            prev_total_ticks: None,
            num_cores: num_cpus(),
            total_ram: 0,
            last_collect: None,
            sort_key: ProcessSortKey::Cpu,
            sort_desc: true,
            search: String::new(),
            selected_pid: None,
            total_count: 0,
            running_count: 0,
            sleeping_count: 0,
            stopped_count: 0,
            zombie_count: 0,
        }
    }

    pub fn poll(&mut self, total_ram: u64) {
        self.total_ram = total_ram;
        let now = Instant::now();
        let (mut new_processes, total_ticks) = imp::collect_processes();
        let elapsed = self
            .last_collect
            .map_or(Duration::ZERO, |t| now.duration_since(t));
        self.last_collect = Some(now);

        let mut new_prev: HashMap<(u32, u64), u64> = HashMap::new();
        for proc in &mut new_processes {
            let key = (proc.pid, proc.start_ticks);
            let current_ticks = proc.cpu_ticks;
            if let Some(&prev_ticks) = self.prev_ticks.get(&key) {
                if let (Some(prev_total), Some(curr_total)) = (self.prev_total_ticks, total_ticks) {
                    let proc_delta = current_ticks.saturating_sub(prev_ticks);
                    let sys_delta = curr_total.saturating_sub(prev_total);
                    if sys_delta > 0 && elapsed.as_secs_f32() > 0.0 {
                        let usage = proc_delta as f32 / sys_delta as f32 * self.num_cores * 100.0;
                        proc.cpu_usage = Some(usage.clamp(0.0, 100.0 * self.num_cores));
                    } else {
                        proc.cpu_usage = Some(0.0);
                    }
                }
            }
            if self.total_ram > 0 {
                if let Some(rss) = proc.memory_bytes {
                    proc.memory_percent = Some(rss as f32 / self.total_ram as f32);
                }
            }
            new_prev.insert(key, current_ticks);
        }

        self.prev_ticks = new_prev;
        self.prev_total_ticks = total_ticks;
        self.update_counts(&new_processes);
        self.processes = new_processes;
    }

    fn update_counts(&mut self, procs: &[ProcessInfo]) {
        self.total_count = procs.len();
        self.running_count = 0;
        self.sleeping_count = 0;
        self.stopped_count = 0;
        self.zombie_count = 0;
        for p in procs {
            match p.state {
                ProcessState::Running => self.running_count += 1,
                ProcessState::Sleeping | ProcessState::DiskSleep => self.sleeping_count += 1,
                ProcessState::Stopped => self.stopped_count += 1,
                ProcessState::Zombie => self.zombie_count += 1,
                ProcessState::Unknown => {}
            }
        }
    }

    pub fn sorted_filtered(&self) -> Vec<&ProcessInfo> {
        let search_lower = self.search.to_lowercase();
        let mut list: Vec<&ProcessInfo> = self
            .processes
            .iter()
            .filter(|p| {
                if search_lower.is_empty() {
                    return true;
                }
                p.name.to_lowercase().contains(&search_lower)
                    || p.command
                        .as_deref()
                        .is_some_and(|c| c.to_lowercase().contains(&search_lower))
                    || p.executable
                        .as_deref()
                        .is_some_and(|e| e.to_lowercase().contains(&search_lower))
            })
            .collect();
        list.sort_by(|a, b| {
            let ord = match self.sort_key {
                ProcessSortKey::Cpu => {
                    let a_val = a.cpu_usage.unwrap_or(0.0);
                    let b_val = b.cpu_usage.unwrap_or(0.0);
                    a_val
                        .partial_cmp(&b_val)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
                ProcessSortKey::Memory => {
                    let a_val = a.memory_bytes.unwrap_or(0);
                    let b_val = b.memory_bytes.unwrap_or(0);
                    a_val.cmp(&b_val)
                }
                ProcessSortKey::Pid => a.pid.cmp(&b.pid),
                ProcessSortKey::Name => a.name.cmp(&b.name),
            };
            if self.sort_desc { ord.reverse() } else { ord }
        });
        list
    }

    pub fn find_by_pid(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.iter().find(|p| p.pid == pid)
    }
}

fn num_cpus() -> f32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as f32)
        .unwrap_or(1.0)
}

// ---------------------------------------------------------------------------
// /proc parsing (pure, testable)
// ---------------------------------------------------------------------------

pub fn parse_stat_total_ticks(content: &str) -> Option<u64> {
    let line = content.lines().next()?;
    let mut fields = line.split_whitespace().skip(1);
    let mut total: u64 = 0;
    for _ in 0..8 {
        total += fields.next()?.parse::<u64>().ok()?;
    }
    Some(total)
}

pub fn parse_proc_stat(content: &str) -> Option<(u32, String, char, u64, u64, u64)> {
    let open = content.find('(')?;
    let close = content.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid = content[..open].trim().parse::<u32>().ok()?;
    let name = content[open + 1..close].to_owned();
    let rest = content[close + 2..].split_whitespace();
    let fields: Vec<&str> = rest.collect();
    if fields.len() < 22 {
        return None;
    }
    let state = fields[0].chars().next()?;
    let utime = fields[11].parse::<u64>().ok()?;
    let stime = fields[12].parse::<u64>().ok()?;
    let starttime = fields[19].parse::<u64>().ok()?;
    Some((pid, name, state, utime, stime, starttime))
}

pub fn parse_proc_status(
    content: &str,
) -> (Option<String>, Option<char>, Option<u32>, Option<u64>) {
    let mut name = None;
    let mut state = None;
    let mut uid = None;
    let mut rss = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Name" => name = Some(value.to_owned()),
            "State" => state = value.chars().next(),
            "Uid" => {
                uid = value.split_whitespace().next().and_then(|s| s.parse().ok());
            }
            "VmRSS" => {
                let kb: u64 = value
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                rss = Some(kb * 1024);
            }
            _ => {}
        }
    }
    (name, state, uid, rss)
}

pub fn parse_proc_cmdline(content: &[u8]) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(content);
    let result: String = text
        .chars()
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect();
    let trimmed = result.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Platform implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::{
        ProcessInfo, ProcessState, parse_proc_cmdline, parse_proc_stat, parse_proc_status,
    };

    fn read_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn read_file_bytes(path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    pub fn collect_processes() -> (Vec<ProcessInfo>, Option<u64>) {
        let total_ticks = read_file("/proc/stat").and_then(|c| super::parse_stat_total_ticks(&c));

        let mut processes = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return (processes, total_ticks);
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid_str) = name.to_str() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            let dir = format!("/proc/{pid}");
            let stat_path = format!("{dir}/stat");
            let status_path = format!("{dir}/status");
            let Some(stat_content) = read_file(&stat_path) else {
                continue;
            };
            let Some((_, stat_name, stat_state, utime, stime, start_ticks)) =
                parse_proc_stat(&stat_content)
            else {
                continue;
            };
            let (status_name, status_state, uid, rss) = read_file(&status_path)
                .map(|c| parse_proc_status(&c))
                .unwrap_or((None, None, None, None));
            let name = status_name.or(Some(stat_name)).unwrap_or_default();
            let state = status_state
                .or(Some(stat_state))
                .map(ProcessState::from_linux_char)
                .unwrap_or(ProcessState::Unknown);
            let cmdline =
                read_file_bytes(&format!("{dir}/cmdline")).and_then(|b| parse_proc_cmdline(&b));
            let exe = std::fs::read_link(format!("{dir}/exe"))
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok());
            let cpu_ticks = utime + stime;
            processes.push(ProcessInfo {
                pid,
                name,
                cpu_usage: None,
                memory_bytes: rss,
                memory_percent: None,
                state,
                uid,
                username: uid.and_then(resolve_username),
                command: cmdline,
                executable: exe,
                cpu_ticks,
                start_ticks,
            });
        }
        (processes, total_ticks)
    }

    fn resolve_username(uid: u32) -> Option<String> {
        let content = std::fs::read_to_string("/etc/passwd").ok()?;
        for line in content.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 && fields[2].parse::<u32>().ok() == Some(uid) {
                return Some(fields[0].to_owned());
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::ProcessInfo;

    pub fn collect_processes() -> (Vec<ProcessInfo>, Option<u64>) {
        (Vec::new(), None)
    }
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

use crate::glass::with_alpha;
use crate::section::SectionContext;
use crate::theme::Theme;
use eframe::egui;
use eframe::egui::{Color32, FontId, Frame, Margin, RichText, Stroke, Ui};

pub fn show_process_card(ui: &mut Ui, context: &SectionContext<'_>, monitor: &mut ProcessMonitor) {
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
            RichText::new("Processes")
                .font(FontId::proportional(12.0))
                .color(theme.ui.secondary_text)
                .strong(),
        );
        ui.add_space(2.0);

        if monitor.total_count == 0 {
            ui.label(
                RichText::new("Telemetry unavailable")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text),
            );
            return;
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} processes", monitor.total_count))
                    .font(FontId::monospace(11.0))
                    .color(theme.ui.text),
            );
            if monitor.running_count > 0 {
                ui.label(
                    RichText::new(format!("Running: {}", monitor.running_count))
                        .font(FontId::proportional(10.0))
                        .color(theme.status.success),
                );
            }
            if monitor.stopped_count > 0 {
                ui.label(
                    RichText::new(format!("Stopped: {}", monitor.stopped_count))
                        .font(FontId::proportional(10.0))
                        .color(theme.status.warning),
                );
            }
            if monitor.zombie_count > 0 {
                ui.label(
                    RichText::new(format!("Zombie: {}", monitor.zombie_count))
                        .font(FontId::proportional(10.0))
                        .color(theme.status.error),
                );
            }
        });

        ui.add_space(4.0);

        // Search box.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Search:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add(
                egui::TextEdit::singleline(&mut monitor.search)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(10.0)),
            );
        });

        // Sort selector.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Sort:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            for key in [
                ProcessSortKey::Cpu,
                ProcessSortKey::Memory,
                ProcessSortKey::Pid,
                ProcessSortKey::Name,
            ] {
                let is_active = monitor.sort_key == key;
                let label = if is_active {
                    if monitor.sort_desc {
                        format!("{} ▼", key.label())
                    } else {
                        format!("{} ▲", key.label())
                    }
                } else {
                    key.label().to_owned()
                };
                let btn =
                    egui::Button::new(RichText::new(&label).font(FontId::monospace(10.0)).color(
                        if is_active {
                            theme.ui.accent
                        } else {
                            theme.ui.secondary_text
                        },
                    ))
                    .fill(Color32::TRANSPARENT);
                if ui.add(btn).clicked() {
                    if monitor.sort_key == key {
                        monitor.sort_desc = !monitor.sort_desc;
                    } else {
                        monitor.sort_key = key;
                        monitor.sort_desc = true;
                    }
                }
            }
        });

        ui.add_space(4.0);

        // Column headers.
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2(52.0, 14.0),
                egui::Label::new(
                    RichText::new("PID")
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.secondary_text),
                ),
            );
            ui.add_sized(
                egui::vec2(120.0, 14.0),
                egui::Label::new(
                    RichText::new("NAME")
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.secondary_text),
                ),
            );
            ui.add_sized(
                egui::vec2(50.0, 14.0),
                egui::Label::new(
                    RichText::new("CPU")
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.secondary_text),
                ),
            );
            ui.add_sized(
                egui::vec2(70.0, 14.0),
                egui::Label::new(
                    RichText::new("MEMORY")
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.secondary_text),
                ),
            );
            ui.label(
                RichText::new("STATE")
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            );
        });

        ui.separator();

        // Compute sorted list now (after any sort/search mutations above).
        let sorted = monitor.sorted_filtered();
        let sorted_pids: Vec<u32> = sorted.iter().map(|p| p.pid).collect();
        let row_height = 16.0;
        let max_visible_rows = 20;
        let total_rows = sorted_pids.len();
        let visible_rows = total_rows.min(max_visible_rows);

        egui::ScrollArea::vertical()
            .id_salt("process-list")
            .max_height(row_height * visible_rows as f32 + 8.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for &pid in &sorted_pids {
                    let proc_info = match monitor.find_by_pid(pid) {
                        Some(p) => p.clone(),
                        None => continue,
                    };
                    let is_selected = monitor.selected_pid == Some(proc_info.pid);
                    let response = ui
                        .horizontal(|ui| {
                            if is_selected {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), row_height),
                                    egui::Sense::hover(),
                                );
                                ui.painter_at(rect).rect_filled(
                                    rect,
                                    2.0,
                                    with_alpha(theme.ui.accent, 0.12),
                                );
                            }
                            ui.add_sized(
                                egui::vec2(52.0, row_height),
                                egui::Label::new(
                                    RichText::new(format!("{}", proc_info.pid))
                                        .font(FontId::monospace(10.0))
                                        .color(theme.ui.text),
                                ),
                            );
                            ui.add_sized(
                                egui::vec2(120.0, row_height),
                                egui::Label::new(
                                    RichText::new(truncate(&proc_info.name, 16))
                                        .font(FontId::monospace(10.0))
                                        .color(theme.ui.text),
                                ),
                            );
                            let cpu_text = proc_info
                                .cpu_usage
                                .map(|u| format!("{u:>5.1}%"))
                                .unwrap_or_else(|| "  -- ".into());
                            ui.add_sized(
                                egui::vec2(50.0, row_height),
                                egui::Label::new(
                                    RichText::new(cpu_text)
                                        .font(FontId::monospace(10.0))
                                        .color(cpu_color(theme, proc_info.cpu_usage)),
                                ),
                            );
                            let mem_text = proc_info
                                .memory_bytes
                                .map(|b| {
                                    let s = crate::section::system::dashboard::format_bytes(b);
                                    truncate_owned(s, 8)
                                })
                                .unwrap_or_else(|| "--".into());
                            ui.add_sized(
                                egui::vec2(70.0, row_height),
                                egui::Label::new(
                                    RichText::new(mem_text)
                                        .font(FontId::monospace(10.0))
                                        .color(theme.ui.text),
                                ),
                            );
                            ui.label(
                                RichText::new(proc_info.state.label())
                                    .font(FontId::monospace(10.0))
                                    .color(state_color(theme, proc_info.state)),
                            );
                        })
                        .response;

                    if response.interact(egui::Sense::click()).clicked() {
                        if monitor.selected_pid == Some(proc_info.pid) {
                            monitor.selected_pid = None;
                        } else {
                            monitor.selected_pid = Some(proc_info.pid);
                        }
                    }
                }
            });

        // Detail panel for selected process.
        if let Some(sel_pid) = monitor.selected_pid {
            if let Some(proc_info) = monitor.find_by_pid(sel_pid) {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Process Details")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                );
                detail_row(ui, theme, "PID", Some(format!("{}", proc_info.pid)));
                detail_row(ui, theme, "Name", Some(proc_info.name.clone()));
                detail_row(ui, theme, "State", Some(proc_info.state.label().to_owned()));
                detail_row(
                    ui,
                    theme,
                    "User",
                    proc_info
                        .username
                        .clone()
                        .or_else(|| proc_info.uid.map(|u| format!("UID {u}"))),
                );
                detail_row(
                    ui,
                    theme,
                    "CPU",
                    proc_info.cpu_usage.map(|u| format!("{u:.1}%")),
                );
                detail_row(
                    ui,
                    theme,
                    "Memory",
                    proc_info
                        .memory_bytes
                        .map(crate::section::system::dashboard::format_bytes),
                );
                detail_row(ui, theme, "Executable", proc_info.executable.clone());
                detail_row(ui, theme, "Command", proc_info.command.clone());
            }
        }
    });
}

fn detail_row(ui: &mut Ui, theme: &Theme, label: &str, value: Option<String>) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(80.0, 14.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            ),
        );
        match value {
            Some(v) => {
                ui.label(
                    RichText::new(truncate(&v, 60))
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.text),
                );
            }
            None => {
                ui.label(
                    RichText::new("Unavailable")
                        .font(FontId::proportional(10.0))
                        .color(theme.status.warning),
                );
            }
        }
    });
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len { s } else { &s[..max_len] }
}

fn truncate_owned(s: String, max_len: usize) -> String {
    if s.len() <= max_len {
        s
    } else {
        let mut result: String = s.chars().take(max_len - 1).collect();
        result.push('…');
        result
    }
}

fn cpu_color(theme: &Theme, usage: Option<f32>) -> Color32 {
    match usage {
        Some(u) if u >= 50.0 => theme.status.error,
        Some(u) if u >= 20.0 => theme.status.warning,
        _ => theme.ui.text,
    }
}

fn state_color(theme: &Theme, state: ProcessState) -> Color32 {
    match state {
        ProcessState::Running => theme.status.success,
        ProcessState::Zombie => theme.status.error,
        ProcessState::Stopped => theme.status.warning,
        _ => theme.ui.secondary_text,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_total_cpu_ticks() {
        let content = "cpu  331689 648 80457 2566257 1294 0 1308 0 0 0\n";
        assert_eq!(parse_stat_total_ticks(content), Some(2981653));
    }

    #[test]
    fn rejects_empty_stat() {
        assert!(parse_stat_total_ticks("").is_none());
    }

    #[test]
    fn rejects_stat_with_missing_fields() {
        assert!(parse_stat_total_ticks("cpu  1 2 3\n").is_none());
    }

    #[test]
    fn parses_a_real_proc_stat() {
        let content = "1 (systemd) S 0 1 1 0 -1 4194560 100105 759945 38 1802 267 190 1601 1258 20 0 1 0 7 27488256 4218 18446744073709551615 1 1 0 0 0 0 671173123 4096 1260 0 0 0 17 10 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let (pid, name, state, utime, stime, start_ticks) = parse_proc_stat(content).unwrap();
        assert_eq!(pid, 1);
        assert_eq!(name, "systemd");
        assert_eq!(state, 'S');
        assert_eq!(utime, 267);
        assert_eq!(stime, 190);
        assert_eq!(start_ticks, 7);
    }

    #[test]
    fn parses_process_name_with_spaces() {
        let content = "12345 (My Process) R 0 12345 12345 0 -1 4194304 100 200 0 0 500 100 0 0 20 0 1 0 1000 100000 50 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let (pid, name, state, utime, stime, start_ticks) = parse_proc_stat(content).unwrap();
        assert_eq!(pid, 12345);
        assert_eq!(name, "My Process");
        assert_eq!(state, 'R');
        assert_eq!(utime, 500);
        assert_eq!(stime, 100);
        assert_eq!(start_ticks, 1000);
    }

    #[test]
    fn rejects_proc_stat_with_too_few_fields() {
        assert!(parse_proc_stat("1 (test) S 0").is_none());
    }

    #[test]
    fn rejects_proc_stat_with_invalid_pid() {
        assert!(
            parse_proc_stat(
                "abc (test) S 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23"
            )
            .is_none()
        );
    }

    #[test]
    fn parses_status_fields() {
        let content = "Name:\tsystemd\nState:\tS (sleeping)\nTgid:\t1\nPid:\t1\nPPid:\t0\nTracerPid:\t0\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nVmPeak:\t26880 kB\nVmSize:\t26844 kB\nVmRSS:\t100105 kB\n";
        let (name, state, uid, rss) = parse_proc_status(content);
        assert_eq!(name.as_deref(), Some("systemd"));
        assert_eq!(state, Some('S'));
        assert_eq!(uid, Some(0));
        assert_eq!(rss, Some(100105 * 1024));
    }

    #[test]
    fn parses_user_process_status() {
        let content = "Name:\tbash\nState:\tS (sleeping)\nPid:\t22275\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t3956 kB\n";
        let (name, state, uid, rss) = parse_proc_status(content);
        assert_eq!(name.as_deref(), Some("bash"));
        assert_eq!(state, Some('S'));
        assert_eq!(uid, Some(1000));
        assert_eq!(rss, Some(3956 * 1024));
    }

    #[test]
    fn handles_missing_fields_in_status() {
        let content = "Name:\ttest\n";
        let (name, state, uid, rss) = parse_proc_status(content);
        assert_eq!(name.as_deref(), Some("test"));
        assert_eq!(state, None);
        assert_eq!(uid, None);
        assert_eq!(rss, None);
    }

    #[test]
    fn parses_cmdline_with_nul_separators() {
        let input = b"/bin/bash\0-c\0echo hello\0";
        assert_eq!(
            parse_proc_cmdline(input).as_deref(),
            Some("/bin/bash -c echo hello")
        );
    }

    #[test]
    fn empty_cmdline_is_none() {
        assert!(parse_proc_cmdline(b"").is_none());
    }

    #[test]
    fn whitespace_only_cmdline_is_none() {
        assert!(parse_proc_cmdline(b"\0\0\0").is_none());
    }

    #[test]
    fn state_from_char() {
        assert_eq!(ProcessState::from_linux_char('R'), ProcessState::Running);
        assert_eq!(ProcessState::from_linux_char('S'), ProcessState::Sleeping);
        assert_eq!(ProcessState::from_linux_char('D'), ProcessState::DiskSleep);
        assert_eq!(ProcessState::from_linux_char('T'), ProcessState::Stopped);
        assert_eq!(ProcessState::from_linux_char('t'), ProcessState::Stopped);
        assert_eq!(ProcessState::from_linux_char('Z'), ProcessState::Zombie);
        assert_eq!(ProcessState::from_linux_char('X'), ProcessState::Unknown);
    }

    #[test]
    fn state_labels() {
        assert_eq!(ProcessState::Running.label(), "Running");
        assert_eq!(ProcessState::Sleeping.label(), "Sleeping");
        assert_eq!(ProcessState::DiskSleep.label(), "Disk Sleep");
        assert_eq!(ProcessState::Stopped.label(), "Stopped");
        assert_eq!(ProcessState::Zombie.label(), "Zombie");
        assert_eq!(ProcessState::Unknown.label(), "Unknown");
    }

    #[test]
    fn monitor_starts_empty() {
        let monitor = ProcessMonitor::new();
        assert_eq!(monitor.total_count, 0);
        assert!(monitor.sorted_filtered().is_empty());
    }

    #[test]
    fn sort_key_labels() {
        assert_eq!(ProcessSortKey::Cpu.label(), "CPU");
        assert_eq!(ProcessSortKey::Memory.label(), "Memory");
        assert_eq!(ProcessSortKey::Pid.label(), "PID");
        assert_eq!(ProcessSortKey::Name.label(), "Name");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn truncate_owned_adds_ellipsis() {
        assert_eq!(truncate_owned("hello world".into(), 5), "hell\u{2026}");
    }

    #[test]
    fn cpu_delta_zero_elapsed() {
        let mut monitor = ProcessMonitor::new();
        monitor.poll(0);
        for p in &monitor.processes {
            assert!(p.cpu_usage.is_none());
        }
    }

    #[test]
    fn cpu_delta_counter_reset() {
        let prev_ticks = 1000u64;
        let curr_ticks = 500u64;
        let delta = curr_ticks.saturating_sub(prev_ticks);
        assert_eq!(delta, 0);
    }

    #[test]
    fn memory_percent_calculation() {
        let mut monitor = ProcessMonitor::new();
        monitor.total_ram = 16_000_000_000;
        monitor.processes.push(ProcessInfo {
            pid: 999,
            name: "test".into(),
            cpu_usage: None,
            memory_bytes: Some(1_000_000_000),
            memory_percent: None,
            state: ProcessState::Sleeping,
            uid: None,
            username: None,
            command: None,
            executable: None,
            cpu_ticks: 0,
            start_ticks: 0,
        });
        for p in &mut monitor.processes {
            if let Some(rss) = p.memory_bytes {
                if monitor.total_ram > 0 {
                    p.memory_percent = Some(rss as f32 / monitor.total_ram as f32);
                }
            }
        }
        let pct = monitor.processes[0].memory_percent.unwrap();
        assert!((pct - 0.0625).abs() < 0.001);
    }
}
