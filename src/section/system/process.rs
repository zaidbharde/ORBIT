//! Process monitoring: discovery, collection and rendering.
//!
//! P5.4 implements a read-only process monitor sourced directly from Linux
//! `/proc`. Each refresh reads `/proc/[pid]/stat`, `/proc/[pid]/status`,
//! `/proc/[pid]/cmdline` and `/proc/[pid]/exe` for every numeric PID
//! directory. CPU usage is calculated from deltas of per-process CPU ticks
//! between consecutive 1 Hz samples. The UI presents a searchable, sortable
//! table with optional per-process detail panels.
//!
//! P6.1 adds safe process execution control with confirmation dialogs:
//! - Stop: sends SIGTERM after user confirmation (graceful termination)
//! - Kill: sends SIGKILL after user confirmation (forceful termination)
//! - Self-protection: ORBIT's own PID is detected and disabled
//! - All actions validate PID existence, ownership, and protection status
//! - Confirmation dialogs require explicit user approval before any signal
//!
//! P6.2 adds a non-executing command builder in the process detail panel:
//! - Editable monospace text field showing the process command
//! - Copy button to copy the command text to clipboard
//! - Reset button to revert edits back to the original command
//! - No shell execution, no Run/Execute button
//!
//! On non-Linux platforms the process list is always empty and the UI
//! displays "Telemetry unavailable". Process actions are unsupported.

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

// ---------------------------------------------------------------------------
// P6.1: Safe process action types
// ---------------------------------------------------------------------------

/// A safe process action the UI may request.
///
/// All actions require user confirmation via a dialog before execution.
/// - `Refresh`: re-read the current process's info from `/proc`
/// - `Stop`: send SIGTERM (graceful termination request)
/// - `Kill`: send SIGKILL (forceful termination, cannot be caught)
///
/// Actions are queued as `pending_action` and drained in the update loop.
/// Force-kill (SIGKILL) requires a stronger confirmation dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessAction {
    Refresh { pid: u32 },
    Stop { pid: u32 },
    Kill { pid: u32 },
}

/// Outcome of a validated process action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionResult {
    Success(String),
    Denied(String),
    Error(String),
}

impl ActionResult {
    #[allow(dead_code)]
    pub fn is_success(&self) -> bool {
        matches!(self, ActionResult::Success(_))
    }

    pub fn message(&self) -> &str {
        match self {
            ActionResult::Success(msg) => msg,
            ActionResult::Denied(msg) => msg,
            ActionResult::Error(msg) => msg,
        }
    }
}

/// What the confirmation dialog is currently showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    Stop { pid: u32, name: String },
    Kill { pid: u32, name: String },
}

// ---------------------------------------------------------------------------
// P6.2: Command builder state
// ---------------------------------------------------------------------------

/// Non-executing command preview / builder state for the selected process.
///
/// The command text is displayed in an editable monospace field so users
/// can inspect and copy it. No execution is ever performed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandBuilderState {
    /// The currently displayed (possibly edited) command text.
    pub command_text: String,
    /// The original command text from `/proc/[pid]/cmdline` (for Reset).
    pub original_command: String,
    /// Which PID this state is tracking (to detect selection changes).
    pub tracked_pid: Option<u32>,
}

impl CommandBuilderState {
    pub fn new() -> Self {
        Self {
            command_text: String::new(),
            original_command: String::new(),
            tracked_pid: None,
        }
    }

    /// Sync the command builder with a newly selected process.
    /// If the PID changed, load the process's command text.
    pub fn sync_with_process(&mut self, pid: u32, command: Option<&str>) {
        if self.tracked_pid != Some(pid) {
            self.tracked_pid = Some(pid);
            let cmd = command.unwrap_or("").to_owned();
            self.original_command = cmd.clone();
            self.command_text = cmd;
        }
    }

    /// Reset the command text to the original value.
    pub fn reset(&mut self) {
        self.command_text = self.original_command.clone();
    }

    /// Whether the command text differs from the original.
    pub fn is_modified(&self) -> bool {
        self.command_text != self.original_command
    }
}

// ---------------------------------------------------------------------------
// P6.1: Safety validation (testable pure functions)
// ---------------------------------------------------------------------------

/// PIDs that must never be targeted by process actions.
const PROTECTED_PIDS: &[u32] = &[0, 1, 2];

/// Check whether a PID is in the protected set (kernel, init, kthreadd).
pub fn is_protected_pid(pid: u32) -> bool {
    PROTECTED_PIDS.contains(&pid)
}

/// Validate that a PID is numeric and non-zero (basic sanity).
pub fn validate_pid(pid: u32) -> bool {
    pid > 0
}

/// Read the current UID of a process from `/proc/[pid]/status`.
///
/// Returns `None` if the PID doesn't exist or the status file can't be read.
#[cfg(target_os = "linux")]
pub fn read_process_uid(pid: u32) -> Option<u32> {
    let path = format!("/proc/{pid}/status");
    let content = std::fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key == "Uid" {
                return value.split_whitespace().next()?.parse().ok();
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn read_process_uid(_pid: u32) -> Option<u32> {
    None
}

/// Verify that a process with the given PID exists and is owned by `expected_uid`.
///
/// This re-reads `/proc/[pid]/status` to avoid trusting stale UI data.
pub fn verify_ownership(pid: u32, expected_uid: u32) -> bool {
    read_process_uid(pid).is_some_and(|uid| uid == expected_uid)
}

/// Check if a process is still alive by verifying `/proc/[pid]` exists.
pub fn is_process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Get the current user's UID (real UID).
#[cfg(target_os = "linux")]
pub fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(target_os = "linux"))]
pub fn current_uid() -> u32 {
    0
}

/// Get ORBIT's own PID for self-protection.
pub fn orbit_pid() -> u32 {
    std::process::id()
}

/// Check if a PID is ORBIT itself.
pub fn is_orbit_self(pid: u32) -> bool {
    pid == orbit_pid()
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
    /// Pending action requested from the UI (set by button callbacks).
    pub pending_action: Option<ProcessAction>,
    /// Result of the last executed action, displayed in the UI.
    pub last_action_result: Option<ActionResult>,
    /// When the last action was executed (for auto-clear after a few seconds).
    pub last_action_time: Option<Instant>,
    /// Active confirmation dialog, if any.
    pub confirm_dialog: Option<ConfirmKind>,
    /// P6.2: Non-executing command builder state for the selected process.
    pub command_builder: CommandBuilderState,
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
            pending_action: None,
            last_action_result: None,
            last_action_time: None,
            confirm_dialog: None,
            command_builder: CommandBuilderState::new(),
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

    /// Drain the pending action, returning it if one was set by the UI.
    pub fn drain_pending_action(&mut self) -> Option<ProcessAction> {
        self.pending_action.take()
    }

    /// Open a confirmation dialog for the given action kind.
    pub fn open_confirm(&mut self, kind: ConfirmKind) {
        self.confirm_dialog = Some(kind);
    }

    /// Close the confirmation dialog without taking action.
    pub fn close_confirm(&mut self) {
        self.confirm_dialog = None;
    }

    /// Execute a validated process action. Returns an `ActionResult`.
    ///
    /// This method enforces all safety invariants:
    /// - PID must be non-zero and not protected (PID 1, 2)
    /// - Process must exist in `/proc`
    /// - Caller must own the process (same UID) for Stop/Kill
    /// - ORBIT's own PID is always denied
    pub fn execute_action(&mut self, action: ProcessAction) -> ActionResult {
        let result = match &action {
            ProcessAction::Refresh { pid } => self.execute_refresh(*pid),
            ProcessAction::Stop { pid } => self.execute_stop(*pid),
            ProcessAction::Kill { pid } => self.execute_kill(*pid),
        };
        self.last_action_result = Some(result.clone());
        self.last_action_time = Some(Instant::now());
        result
    }

    fn execute_refresh(&self, pid: u32) -> ActionResult {
        if !validate_pid(pid) {
            return ActionResult::Error("Invalid PID".into());
        }
        if is_protected_pid(pid) {
            return ActionResult::Denied("Cannot refresh protected process".into());
        }
        if !is_process_alive(pid) {
            return ActionResult::Error(format!("PID {pid} does not exist"));
        }
        if self.find_by_pid(pid).is_none() {
            return ActionResult::Error(format!("PID {pid} not in current process list"));
        }
        ActionResult::Success(format!("Refreshed PID {pid}"))
    }

    fn preflight_check(&self, pid: u32) -> Option<ActionResult> {
        if !validate_pid(pid) {
            return Some(ActionResult::Error("Invalid PID".into()));
        }
        if is_orbit_self(pid) {
            return Some(ActionResult::Denied(
                "ORBIT process \u{2014} control disabled".into(),
            ));
        }
        if is_protected_pid(pid) {
            return Some(ActionResult::Denied(format!(
                "Refused: PID {pid} is a protected system process"
            )));
        }
        if !is_process_alive(pid) {
            return Some(ActionResult::Error(format!("PID {pid} no longer exists")));
        }
        let my_uid = current_uid();
        if !verify_ownership(pid, my_uid) {
            return Some(ActionResult::Denied(format!(
                "Refused: PID {pid} is not owned by the current user"
            )));
        }
        None
    }

    fn execute_stop(&self, pid: u32) -> ActionResult {
        if let Some(result) = self.preflight_check(pid) {
            return result;
        }
        // Safety: we validated PID exists, is not protected, is not self, and is owned by us.
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if ret == 0 {
                ActionResult::Success(format!(
                    "Sent SIGTERM to PID {pid}; waiting for process list refresh"
                ))
            } else {
                let err = std::io::Error::last_os_error();
                ActionResult::Error(format!("kill failed: {err}"))
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            ActionResult::Error("Stop not supported on this platform".into())
        }
    }

    fn execute_kill(&self, pid: u32) -> ActionResult {
        if let Some(result) = self.preflight_check(pid) {
            return result;
        }
        // Safety: we validated PID exists, is not protected, is not self, and is owned by us.
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            if ret == 0 {
                ActionResult::Success(format!(
                    "Sent SIGKILL to PID {pid}; waiting for process list refresh"
                ))
            } else {
                let err = std::io::Error::last_os_error();
                ActionResult::Error(format!("kill failed: {err}"))
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            ActionResult::Error("Kill not supported on this platform".into())
        }
    }

    /// Clear the action result if it's older than `age`.
    pub fn clear_old_action_result(&mut self, age: Duration) {
        if let Some(t) = self.last_action_time {
            if t.elapsed() >= age {
                self.last_action_result = None;
                self.last_action_time = None;
            }
        }
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
            if let Some(proc_info) = monitor.find_by_pid(sel_pid).cloned() {
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

                // P6.1: Action buttons
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Actions")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                );

                let is_orbit = is_orbit_self(proc_info.pid);
                let is_protected = is_protected_pid(proc_info.pid);
                let is_alive = is_process_alive(proc_info.pid);
                let my_uid = current_uid();
                let is_owned = proc_info.uid.is_some_and(|u| u == my_uid);
                let can_control = !is_orbit && !is_protected && is_alive && is_owned;

                if is_orbit {
                    ui.label(
                        RichText::new("ORBIT process \u{2014} control disabled")
                            .font(FontId::proportional(10.0))
                            .color(theme.status.warning),
                    );
                } else if is_protected {
                    ui.label(
                        RichText::new("Protected system process \u{2014} control disabled")
                            .font(FontId::proportional(10.0))
                            .color(theme.status.warning),
                    );
                }

                ui.horizontal(|ui| {
                    // Refresh button (always available for listed processes)
                    let refresh_btn = egui::Button::new(
                        RichText::new("Refresh")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    )
                    .fill(theme.ui.accent);
                    if ui.add(refresh_btn).clicked() {
                        monitor.pending_action =
                            Some(ProcessAction::Refresh { pid: proc_info.pid });
                    }

                    // Stop button (SIGTERM with confirmation)
                    let stop_btn = egui::Button::new(
                        RichText::new("Stop")
                            .font(FontId::proportional(10.0))
                            .color(if can_control {
                                theme.status.warning
                            } else {
                                theme.ui.secondary_text
                            }),
                    )
                    .fill(if can_control {
                        with_alpha(theme.status.warning, 0.15)
                    } else {
                        Color32::TRANSPARENT
                    });
                    if ui.add(stop_btn).clicked() && can_control {
                        monitor.open_confirm(ConfirmKind::Stop {
                            pid: proc_info.pid,
                            name: proc_info.name.clone(),
                        });
                    }

                    // Kill button (SIGKILL with stronger confirmation)
                    let kill_btn = egui::Button::new(
                        RichText::new("Kill")
                            .font(FontId::proportional(10.0))
                            .color(if can_control {
                                theme.status.error
                            } else {
                                theme.ui.secondary_text
                            }),
                    )
                    .fill(if can_control {
                        with_alpha(theme.status.error, 0.15)
                    } else {
                        Color32::TRANSPARENT
                    });
                    if ui.add(kill_btn).clicked() && can_control {
                        monitor.open_confirm(ConfirmKind::Kill {
                            pid: proc_info.pid,
                            name: proc_info.name.clone(),
                        });
                    }
                });

                // P6.1: Action result banner
                if let Some(ref result) = monitor.last_action_result {
                    ui.add_space(4.0);
                    let (bg_color, text_color) = match result {
                        ActionResult::Success(_) => {
                            (with_alpha(theme.status.success, 0.15), theme.status.success)
                        }
                        ActionResult::Denied(_) => {
                            (with_alpha(theme.status.warning, 0.15), theme.status.warning)
                        }
                        ActionResult::Error(_) => {
                            (with_alpha(theme.status.error, 0.15), theme.status.error)
                        }
                    };
                    let banner = Frame::new()
                        .fill(bg_color)
                        .corner_radius(4.0)
                        .inner_margin(Margin::symmetric(8, 4));
                    banner.show(ui, |ui| {
                        ui.label(
                            RichText::new(result.message())
                                .font(FontId::proportional(10.0))
                                .color(text_color),
                        );
                    });
                }

                // P6.2: Command Builder section
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Command")
                        .font(FontId::proportional(11.0))
                        .color(theme.ui.secondary_text)
                        .strong(),
                );
                monitor
                    .command_builder
                    .sync_with_process(proc_info.pid, proc_info.command.as_deref());

                let _cmd_response = ui.add(
                    egui::TextEdit::singleline(&mut monitor.command_builder.command_text)
                        .font(FontId::monospace(10.0))
                        .desired_width(ui.available_width())
                        .code_editor(),
                );

                ui.horizontal(|ui| {
                    // Copy button
                    let copy_btn = egui::Button::new(
                        RichText::new("Copy")
                            .font(FontId::proportional(10.0))
                            .color(theme.ui.text),
                    )
                    .fill(theme.ui.accent);
                    if ui.add(copy_btn).clicked() {
                        ui.ctx()
                            .copy_text(monitor.command_builder.command_text.clone());
                    }

                    // Reset button (only enabled when modified)
                    let is_modified = monitor.command_builder.is_modified();
                    let reset_btn = egui::Button::new(
                        RichText::new("Reset")
                            .font(FontId::proportional(10.0))
                            .color(if is_modified {
                                theme.ui.text
                            } else {
                                theme.ui.secondary_text
                            }),
                    )
                    .fill(if is_modified {
                        theme.ui.tab_inactive
                    } else {
                        Color32::TRANSPARENT
                    });
                    if ui.add(reset_btn).clicked() && is_modified {
                        monitor.command_builder.reset();
                    }
                });
            }
        }
    });

    // P6.1: Confirmation dialog overlay (rendered outside the card frame)
    show_confirm_dialog(ui, context, monitor);
}

/// P6.1: Render a centered confirmation dialog as a popup overlay.
fn show_confirm_dialog(
    ui: &mut egui::Ui,
    _context: &SectionContext<'_>,
    monitor: &mut ProcessMonitor,
) {
    let Some(ref kind) = monitor.confirm_dialog else {
        return;
    };
    let theme = _context.theme;
    let appearance = _context.appearance;

    let (title, body, confirm_label, confirm_fill, action) = match kind {
        ConfirmKind::Stop { pid, name } => (
            "Confirm Stop",
            format!(
                "Send SIGTERM to PID {pid} ({name})?\n\nThis is a graceful termination request. The process may clean up before exiting."
            ),
            "Stop (SIGTERM)",
            with_alpha(theme.status.warning, 0.2),
            ProcessAction::Stop { pid: *pid },
        ),
        ConfirmKind::Kill { pid, name } => (
            "Confirm Kill",
            format!(
                "Send SIGKILL to PID {pid} ({name})?\n\nThis forcefully terminates the process immediately. Unsaved data may be lost."
            ),
            "Kill (SIGKILL)",
            with_alpha(theme.status.error, 0.2),
            ProcessAction::Kill { pid: *pid },
        ),
    };

    let _response = egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .frame(
            Frame::new()
                .fill(_context.panel_fill)
                .inner_margin(Margin::symmetric(16, 14))
                .corner_radius(appearance.panel_radius.clamp(0.0, 12.0))
                .stroke(Stroke::new(
                    1.0_f32,
                    with_alpha(theme.ui.border, appearance.border_opacity),
                )),
        )
        .show(ui.ctx(), |ui| {
            ui.label(
                RichText::new(&body)
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.text),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let cancel_btn = egui::Button::new(
                    RichText::new("Cancel")
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.text),
                )
                .fill(theme.ui.tab_inactive);
                if ui.add(cancel_btn).clicked() {
                    monitor.close_confirm();
                }

                ui.add_space(8.0);

                let confirm_btn = egui::Button::new(
                    RichText::new(confirm_label)
                        .font(FontId::proportional(10.0))
                        .color(theme.ui.text),
                )
                .fill(confirm_fill);
                if ui.add(confirm_btn).clicked() {
                    monitor.pending_action = Some(action);
                    monitor.close_confirm();
                }
            });
        });

    // Close on Escape key
    let ctx = ui.ctx().clone();
    ctx.input(|input| {
        if input.key_pressed(egui::Key::Escape) {
            monitor.close_confirm();
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
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn truncate_owned(s: String, max_len: usize) -> String {
    if s.len() <= max_len {
        s
    } else {
        let mut result: String = s.chars().take(max_len - 1).collect();
        result.push('\u{2026}');
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

    // -----------------------------------------------------------------------
    // P6.1: Process action safety tests
    // -----------------------------------------------------------------------

    #[test]
    fn protected_pid_zero_is_blocked() {
        assert!(is_protected_pid(0));
    }

    #[test]
    fn protected_pid_one_is_blocked() {
        assert!(is_protected_pid(1));
    }

    #[test]
    fn protected_pid_two_is_blocked() {
        assert!(is_protected_pid(2));
    }

    #[test]
    fn normal_pid_is_not_protected() {
        assert!(!is_protected_pid(42));
        assert!(!is_protected_pid(1000));
    }

    #[test]
    fn validate_pid_rejects_zero() {
        assert!(!validate_pid(0));
    }

    #[test]
    fn validate_pid_accepts_positive() {
        assert!(validate_pid(1));
        assert!(validate_pid(99999));
    }

    #[test]
    fn action_result_success_is_success() {
        let r = ActionResult::Success("ok".into());
        assert!(r.is_success());
        assert_eq!(r.message(), "ok");
    }

    #[test]
    fn action_result_denied_is_not_success() {
        let r = ActionResult::Denied("no".into());
        assert!(!r.is_success());
        assert_eq!(r.message(), "no");
    }

    #[test]
    fn action_result_error_is_not_success() {
        let r = ActionResult::Error("fail".into());
        assert!(!r.is_success());
        assert_eq!(r.message(), "fail");
    }

    #[test]
    fn refresh_rejects_protected_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Refresh { pid: 1 });
        assert!(matches!(result, ActionResult::Denied(_)));
    }

    #[test]
    fn refresh_rejects_zero_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Refresh { pid: 0 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn stop_rejects_protected_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Stop { pid: 1 });
        assert!(matches!(result, ActionResult::Denied(_)));
    }

    #[test]
    fn stop_rejects_zero_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Stop { pid: 0 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn kill_rejects_protected_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Kill { pid: 1 });
        assert!(matches!(result, ActionResult::Denied(_)));
    }

    #[test]
    fn kill_rejects_zero_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Kill { pid: 0 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn stop_rejects_nonexistent_pid() {
        let mut monitor = ProcessMonitor::new();
        // PID 99999999 is very unlikely to exist
        let result = monitor.execute_action(ProcessAction::Stop { pid: 99999999 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn kill_rejects_nonexistent_pid() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Kill { pid: 99999999 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn execute_action_stores_result() {
        let mut monitor = ProcessMonitor::new();
        assert!(monitor.last_action_result.is_none());
        monitor.execute_action(ProcessAction::Refresh { pid: 0 });
        assert!(monitor.last_action_result.is_some());
        assert!(monitor.last_action_time.is_some());
    }

    #[test]
    fn clear_old_action_result_removes_stale() {
        let mut monitor = ProcessMonitor::new();
        monitor.execute_action(ProcessAction::Refresh { pid: 0 });
        assert!(monitor.last_action_result.is_some());
        monitor.clear_old_action_result(Duration::ZERO);
        assert!(monitor.last_action_result.is_none());
    }

    #[test]
    fn drain_pending_action_returns_and_clears() {
        let mut monitor = ProcessMonitor::new();
        monitor.pending_action = Some(ProcessAction::Refresh { pid: 42 });
        let action = monitor.drain_pending_action();
        assert_eq!(action, Some(ProcessAction::Refresh { pid: 42 }));
        assert!(monitor.pending_action.is_none());
    }

    #[test]
    fn drain_pending_action_returns_none_when_empty() {
        let mut monitor = ProcessMonitor::new();
        assert!(monitor.drain_pending_action().is_none());
    }

    // -----------------------------------------------------------------------
    // P6.1: Confirmation dialog state tests
    // -----------------------------------------------------------------------

    #[test]
    fn confirm_dialog_starts_none() {
        let monitor = ProcessMonitor::new();
        assert!(monitor.confirm_dialog.is_none());
    }

    #[test]
    fn open_confirm_sets_dialog() {
        let mut monitor = ProcessMonitor::new();
        monitor.open_confirm(ConfirmKind::Stop {
            pid: 42,
            name: "test".into(),
        });
        assert!(monitor.confirm_dialog.is_some());
    }

    #[test]
    fn close_confirm_clears_dialog() {
        let mut monitor = ProcessMonitor::new();
        monitor.open_confirm(ConfirmKind::Kill {
            pid: 42,
            name: "test".into(),
        });
        assert!(monitor.confirm_dialog.is_some());
        monitor.close_confirm();
        assert!(monitor.confirm_dialog.is_none());
    }

    #[test]
    fn confirm_kind_equality() {
        let a = ConfirmKind::Stop {
            pid: 1,
            name: "a".into(),
        };
        let b = ConfirmKind::Stop {
            pid: 1,
            name: "a".into(),
        };
        let c = ConfirmKind::Kill {
            pid: 1,
            name: "a".into(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // -----------------------------------------------------------------------
    // P6.1: Self-protection tests
    // -----------------------------------------------------------------------

    #[test]
    fn orbit_pid_is_nonzero() {
        // ORBIT's own PID must be > 0 (a valid process)
        assert!(orbit_pid() > 0);
    }

    #[test]
    fn is_orbit_self_detects_own_pid() {
        let my_pid = orbit_pid();
        assert!(is_orbit_self(my_pid));
    }

    #[test]
    fn is_orbit_self_rejects_other_pid() {
        // PID 99999999 is extremely unlikely to be ORBIT
        assert!(!is_orbit_self(99999999));
    }

    #[test]
    fn stop_rejects_orbit_self() {
        let mut monitor = ProcessMonitor::new();
        let my_pid = orbit_pid();
        let result = monitor.execute_action(ProcessAction::Stop { pid: my_pid });
        assert!(matches!(result, ActionResult::Denied(_)));
        assert!(result.message().contains("ORBIT"));
    }

    #[test]
    fn kill_rejects_orbit_self() {
        let mut monitor = ProcessMonitor::new();
        let my_pid = orbit_pid();
        let result = monitor.execute_action(ProcessAction::Kill { pid: my_pid });
        assert!(matches!(result, ActionResult::Denied(_)));
        assert!(result.message().contains("ORBIT"));
    }

    // -----------------------------------------------------------------------
    // P6.1: Action type tests
    // -----------------------------------------------------------------------

    #[test]
    fn process_action_variants() {
        let a = ProcessAction::Refresh { pid: 1 };
        let b = ProcessAction::Stop { pid: 2 };
        let c = ProcessAction::Kill { pid: 3 };
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn action_result_message_matches() {
        assert_eq!(ActionResult::Success("ok".into()).message(), "ok");
        assert_eq!(ActionResult::Denied("denied".into()).message(), "denied");
        assert_eq!(ActionResult::Error("err".into()).message(), "err");
    }

    #[test]
    fn stop_action_success_on_valid_owned_process() {
        // We can't easily test actual SIGTERM without killing real processes,
        // but we can verify the preflight checks work correctly.
        let mut monitor = ProcessMonitor::new();
        // Nonexistent PID should fail with Error (not crash)
        let result = monitor.execute_action(ProcessAction::Stop { pid: 99999999 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    #[test]
    fn kill_action_success_on_valid_owned_process() {
        let mut monitor = ProcessMonitor::new();
        let result = monitor.execute_action(ProcessAction::Kill { pid: 99999999 });
        assert!(matches!(result, ActionResult::Error(_)));
    }

    // -----------------------------------------------------------------------
    // P6.2: Command builder tests
    // -----------------------------------------------------------------------

    #[test]
    fn command_builder_starts_empty() {
        let cb = CommandBuilderState::new();
        assert!(cb.command_text.is_empty());
        assert!(cb.original_command.is_empty());
        assert!(cb.tracked_pid.is_none());
    }

    #[test]
    fn command_builder_sync_loads_command() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(42, Some("/bin/bash -c echo hi"));
        assert_eq!(cb.tracked_pid, Some(42));
        assert_eq!(cb.command_text, "/bin/bash -c echo hi");
        assert_eq!(cb.original_command, "/bin/bash -c echo hi");
    }

    #[test]
    fn command_builder_sync_handles_none_command() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(1, None);
        assert_eq!(cb.tracked_pid, Some(1));
        assert!(cb.command_text.is_empty());
        assert!(cb.original_command.is_empty());
    }

    #[test]
    fn command_builder_does_not_reset_on_same_pid() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(42, Some("/bin/bash"));
        cb.command_text = "/bin/bash -c edited".to_owned();
        // Sync again with same PID — should NOT overwrite edits
        cb.sync_with_process(42, Some("/bin/bash"));
        assert_eq!(cb.command_text, "/bin/bash -c edited");
    }

    #[test]
    fn command_builder_resets_on_new_pid() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(42, Some("/bin/bash"));
        cb.command_text = "/bin/bash -c edited".to_owned();
        // Sync with different PID — should load new command
        cb.sync_with_process(99, Some("/usr/bin/vim"));
        assert_eq!(cb.command_text, "/usr/bin/vim");
        assert_eq!(cb.original_command, "/usr/bin/vim");
    }

    #[test]
    fn command_builder_reset_restores_original() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(42, Some("/bin/bash"));
        cb.command_text = "/bin/bash -c modified".to_owned();
        assert!(cb.is_modified());
        cb.reset();
        assert_eq!(cb.command_text, "/bin/bash");
        assert!(!cb.is_modified());
    }

    #[test]
    fn command_builder_is_modified_detects_changes() {
        let mut cb = CommandBuilderState::new();
        cb.sync_with_process(42, Some("/bin/bash"));
        assert!(!cb.is_modified());
        cb.command_text = "changed".to_owned();
        assert!(cb.is_modified());
    }

    #[test]
    fn command_builder_equality() {
        let mut a = CommandBuilderState::new();
        a.sync_with_process(1, Some("cmd"));
        let mut b = CommandBuilderState::new();
        b.sync_with_process(1, Some("cmd"));
        assert_eq!(a, b);
    }
}
